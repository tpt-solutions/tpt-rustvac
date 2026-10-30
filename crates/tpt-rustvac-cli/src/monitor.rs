//! `check`, `schedule` and `status`: the scheduled drive check and what it recommends.
//! The check is read-only apart from its report files. It never removes anything.

#[cfg(not(windows))]
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::process::Stdio;
use std::process::{Command, ExitCode};
use std::time::{Duration, SystemTime};

use clap::Subcommand;
use tpt_rustvac_core::check::{self, CheckInput, Recommendation};
use tpt_rustvac_core::config::{home_dir, parse_size, Config};
use tpt_rustvac_core::filter::Filter;
use tpt_rustvac_core::model::{Arena, Kind, State};
use tpt_rustvac_core::safety::is_kept;
use tpt_rustvac_core::schedule::{self, Every};
use tpt_rustvac_core::sweep::SweepKind;

use crate::cleanup::build_sweep_plan;
use crate::format::{age_label, human_bytes};
use crate::{confirm, print_json, CmdResult};

#[derive(Subcommand)]
pub enum ScheduleCommand {
    /// Register a task that runs `rustvac check` on a schedule.
    Set {
        /// daily or weekly (weekly runs on Monday).
        #[arg(long)]
        every: Every,
        /// Time of day, HH:MM, 24-hour.
        #[arg(long, default_value = "09:00")]
        at: String,
        /// Warn when free space on the home drive is under this size, for example 50GB.
        #[arg(long)]
        warn_below: Option<String>,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Show whether the task is registered, and when it runs next.
    Status,
    /// Remove the task.
    Remove {
        #[arg(long)]
        yes: bool,
    },
}

fn report_home() -> Result<PathBuf, String> {
    home_dir()
        .map(|h| check::report_dir(&h))
        .ok_or_else(|| "cannot find your home directory".to_string())
}

// ---- check ----

pub struct CheckArgs {
    pub json: bool,
    pub warn_below: Option<String>,
    pub notify: bool,
}

pub fn check(arena: &Arena, filter: &Filter, config: &Config, args: CheckArgs) -> CmdResult {
    let now = SystemTime::now();
    let plan = build_sweep_plan(arena, filter, config)?;
    let warn_below = match &args.warn_below {
        Some(w) => Some(parse_size(w)?),
        None => config.warn_below_bytes().map_err(|e| e.to_string())?,
    };

    // Old targets that `compress` could archive. A target the sweep already lists whole is
    // not counted twice.
    let keep = config.keep_paths();
    let compressible: u64 = arena
        .iter()
        .filter(|n| n.kind == Kind::Target && n.state == State::Done)
        .filter(|n| !is_kept(&n.path, &keep) && filter.accepts(n, now))
        .filter(|n| {
            !plan
                .items
                .iter()
                .any(|i| i.kind == SweepKind::StaleTarget && i.path == n.path)
        })
        .map(|n| n.size)
        .sum();

    let home = home_dir().ok_or("cannot find your home directory")?;
    let wsl_disks = tpt_rustvac_core::wsl::discover_here();
    let rec = check::build(&CheckInput {
        plan: &plan,
        drive: &home,
        space: check::disk_space(&home),
        compressible_bytes: compressible,
        wsl: &wsl_disks,
        warn_below,
        now,
        top_n: 10,
    });
    let text = render_report(&rec);
    let dir = report_home()?;
    check::save(&dir, &rec, &text).map_err(|e| e.to_string())?;

    if args.json {
        print_json(&rec)?;
    } else {
        print!("{text}");
        println!("saved to {}", dir.display());
    }
    if rec.low_space && args.notify {
        if let Err(e) = notify(&notice_text(&rec)) {
            eprintln!("could not show a notification: {e}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn kind_label(kind: SweepKind) -> &'static str {
    kind.label()
}

pub fn render_report(rec: &Recommendation) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "rustvac check, {}\n",
        check::local_time(rec.created_unix)
    ));
    match (rec.free_bytes, rec.total_bytes) {
        (Some(f), Some(t)) => out.push_str(&format!(
            "free space on {}: {} of {}\n",
            rec.drive,
            human_bytes(f),
            human_bytes(t)
        )),
        _ => out.push_str(&format!("free space on {}: unknown\n", rec.drive)),
    }
    if rec.low_space {
        if let Some(w) = rec.warn_below {
            out.push_str(&format!(
                "WARNING: free space is under {}\n",
                human_bytes(w)
            ));
        }
    }
    out.push_str(&format!(
        "reclaimable by `rustvac sweep`: {}\n",
        human_bytes(rec.reclaimable_bytes)
    ));
    for k in &rec.by_kind {
        out.push_str(&format!(
            "  {:>10}  {:>4}  {}\n",
            human_bytes(k.bytes),
            k.count,
            kind_label(k.kind)
        ));
    }
    if !rec.top.is_empty() {
        out.push_str("largest items:\n");
        for t in &rec.top {
            out.push_str(&format!("  {:>10}  {}\n", human_bytes(t.bytes), t.path));
        }
    }
    if rec.compressible_bytes > 0 {
        out.push_str(&format!(
            "old targets that `rustvac compress` could archive: {}\n",
            human_bytes(rec.compressible_bytes)
        ));
    }
    if let Some(d) = tpt_rustvac_core::disk::largest_wsl(&rec.wsl) {
        let total: u64 = rec.wsl.iter().map(|d| d.size_bytes).sum();
        out.push_str(&format!(
            "WSL and Docker disks: {} in {} (largest: {}, {}). Report only; see `rustvac disk` for the compact steps.\n",
            human_bytes(total),
            rec.wsl.len(),
            d.name,
            human_bytes(d.size_bytes)
        ));
    }
    out
}

fn notice_text(rec: &Recommendation) -> String {
    format!(
        "Free space is {}. rustvac found {} you can reclaim. Run: rustvac sweep",
        rec.free_bytes.map(human_bytes).unwrap_or_default(),
        human_bytes(rec.reclaimable_bytes)
    )
}

/// A Windows toast through the PowerShell runtime. Other platforms only get the report file.
#[cfg(windows)]
fn notify(body: &str) -> Result<(), String> {
    let script = format!(
        "$null = [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]; \
         $xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
         $t = $xml.GetElementsByTagName('text'); \
         $t.Item(0).AppendChild($xml.CreateTextNode('rustvac: low disk space')) | Out-Null; \
         $t.Item(1).AppendChild($xml.CreateTextNode('{}')) | Out-Null; \
         $toast = [Windows.UI.Notifications.ToastNotification]::new($xml); \
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe').Show($toast)",
        body.replace('\'', "''")
    );
    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &script,
        ])
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("powershell exited with {status}"))
    }
}

#[cfg(not(windows))]
fn notify(_body: &str) -> Result<(), String> {
    Err("notifications are Windows-only; see the saved report".into())
}

// ---- status ----

/// The one-line summary for `status` and the top of `list`. `None` when no check has run.
pub fn status_line(config: &Config, now: SystemTime) -> Option<String> {
    let rec = check::load(&check::report_dir(&home_dir()?))?;
    let checked = SystemTime::UNIX_EPOCH + Duration::from_secs(rec.created_unix);
    let mut line = format!("last check: {} ago", age_label(Some(checked), now));
    if let (Some(f), Some(t)) = (rec.free_bytes, rec.total_bytes) {
        line.push_str(&format!(", free {} of {}", human_bytes(f), human_bytes(t)));
    }
    line.push_str(&format!(
        ", reclaimable {}",
        human_bytes(rec.reclaimable_bytes)
    ));
    if rec.reclaimable_bytes > 0 {
        line.push_str(", run `rustvac sweep`");
    }
    if rec.low_space {
        line.insert_str(0, "LOW SPACE: ");
    }
    if let Ok(Some(every)) = config.check_interval() {
        if rec.age(now) > every.period() * 2 {
            line.push_str(&format!(
                " (overdue: check_every is {}, see `rustvac schedule status`)",
                every.as_str()
            ));
        }
    }
    Some(line)
}

pub fn status(config: &Config) -> CmdResult {
    match status_line(config, SystemTime::now()) {
        Some(line) => println!("{line}"),
        None => println!("no check has run yet. Run `rustvac check`."),
    }
    Ok(ExitCode::SUCCESS)
}

// ---- schedule ----

pub fn schedule_command(
    command: ScheduleCommand,
    config: &Config,
    config_path: Option<&Path>,
) -> CmdResult {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    match command {
        ScheduleCommand::Set {
            every,
            at,
            warn_below,
            yes,
        } => {
            let at = schedule::parse_time(&at)?;
            let warn_below = warn_below.or_else(|| config.warn_below.clone());
            if let Some(w) = &warn_below {
                parse_size(w)?;
            }
            // Only an explicit --config is baked into the task. The default path needs no flag.
            let (command_text, registered) =
                register_plan(&exe, config_path, every, at, warn_below.as_deref());
            println!(
                "This registers a {} check at {:02}:{:02}:",
                every.as_str(),
                at.0,
                at.1
            );
            println!("  {command_text}");
            println!("The check only reads and writes a report. It never removes anything.");
            if !yes && !confirm("Register it?")? {
                println!("cancelled");
                return Ok(ExitCode::SUCCESS);
            }
            registered()?;
            println!("registered. See it with `rustvac schedule status`.");
            Ok(ExitCode::SUCCESS)
        }
        ScheduleCommand::Status => schedule_status(),
        ScheduleCommand::Remove { yes } => {
            if !yes && !confirm("Remove the scheduled check?")? {
                println!("cancelled");
                return Ok(ExitCode::SUCCESS);
            }
            remove_schedule()?;
            println!("removed");
            Ok(ExitCode::SUCCESS)
        }
    }
}

type Registration = Box<dyn FnOnce() -> Result<(), String>>;

#[cfg(windows)]
fn register_plan(
    exe: &Path,
    config: Option<&Path>,
    every: Every,
    at: (u8, u8),
    warn_below: Option<&str>,
) -> (String, Registration) {
    let args = schedule::schtasks_create_args(exe, config, every, at, warn_below);
    let shown = format!(
        "schtasks {}",
        args.iter()
            .map(|a| {
                if a.contains(' ') {
                    format!("\"{}\"", a.replace('"', "\\\""))
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    );
    (shown, Box::new(move || run_schtasks(&args).map(|_| ())))
}

#[cfg(not(windows))]
fn register_plan(
    exe: &Path,
    config: Option<&Path>,
    every: Every,
    at: (u8, u8),
    warn_below: Option<&str>,
) -> (String, Registration) {
    let line = schedule::crontab_line(exe, config, every, at, warn_below);
    (
        format!("crontab: {line}"),
        Box::new(move || {
            let existing = read_crontab()?;
            write_crontab(&schedule::crontab_with(&existing, &line))
        }),
    )
}

#[cfg(windows)]
fn run_schtasks(args: &[String]) -> Result<String, String> {
    let out = Command::new("schtasks")
        .args(args)
        .output()
        .map_err(|e| format!("could not run schtasks: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(text.trim().to_string())
    }
}

#[cfg(windows)]
fn schedule_status() -> CmdResult {
    match run_schtasks(&schedule::schtasks_query_args()) {
        Ok(text) => {
            println!("registered: {}", schedule::TASK_NAME);
            for line in text.lines().filter(|l| {
                let l = l.trim_start();
                [
                    "Next Run Time",
                    "Status",
                    "Task To Run",
                    "Schedule Type",
                    "Start Time",
                    "Last Run Time",
                    "Last Result",
                ]
                .iter()
                .any(|k| l.starts_with(k))
            }) {
                println!("  {}", line.trim());
            }
        }
        Err(_) => println!("not registered. Use `rustvac schedule set --every daily`."),
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(windows)]
fn remove_schedule() -> Result<(), String> {
    run_schtasks(&schedule::schtasks_delete_args()).map(|_| ())
}

#[cfg(not(windows))]
fn read_crontab() -> Result<String, String> {
    let out = Command::new("crontab")
        .arg("-l")
        .output()
        .map_err(|e| format!("could not run crontab: {e}"))?;
    // No crontab yet exits non-zero with nothing on stdout.
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(not(windows))]
fn write_crontab(text: &str) -> Result<(), String> {
    let mut child = Command::new("crontab")
        .arg("-")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run crontab: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("no stdin for crontab")?
        .write_all(text.as_bytes())
        .map_err(|e| e.to_string())?;
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("crontab exited with {status}"))
    }
}

#[cfg(not(windows))]
fn schedule_status() -> CmdResult {
    let existing = read_crontab()?;
    let lines = schedule::crontab_find(&existing);
    if lines.is_empty() {
        println!("not registered. Use `rustvac schedule set --every daily`.");
    } else {
        println!("registered in your crontab:");
        for l in lines {
            println!("  {l}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(not(windows))]
fn remove_schedule() -> Result<(), String> {
    let existing = read_crontab()?;
    if schedule::crontab_find(&existing).is_empty() {
        return Err("no rustvac line in your crontab".into());
    }
    write_crontab(&schedule::crontab_without(&existing))
}
