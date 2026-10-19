//! `rustvac`: find Rust target folders, measure any folder, and vacuum what you don't need.

mod format;

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use tabled::settings::Style;
use tabled::{Table, Tabled};
use tpt_rustvac_core::archive;
use tpt_rustvac_core::config::{default_config_path, expand_tilde, Config};
use tpt_rustvac_core::filter::Filter;
use tpt_rustvac_core::model::{Arena, Kind, NodeId, State};
use tpt_rustvac_core::safety::{check_scan_root, is_kept};
use tpt_rustvac_core::scan::{self, ScanEvent, ScanRequest};

use format::{age_days, age_label, human_bytes, parse_duration, parse_size};

#[derive(Parser)]
#[command(
    name = "rustvac",
    version,
    about = "Find your Rust target folders. Measure any folder. Vacuum the rest."
)]
struct Cli {
    /// Config file. Defaults to %APPDATA%\rustvac\config.toml on Windows, ~/.config/rustvac/config.toml elsewhere.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Find Rust target/ folders and report their size and age.
    Scan {
        /// Directory to scan. Repeatable. Defaults to the config's roots.
        #[arg(long = "path")]
        paths: Vec<PathBuf>,
        /// Only show targets whose newest file is at least this old (for example 2d, 12h).
        #[arg(long)]
        older_than: Option<String>,
        #[arg(long)]
        json: bool,
        /// Allow scanning a filesystem root or your home directory.
        #[arg(long)]
        force: bool,
    },
    /// Human table of Rust targets, largest first.
    List {
        #[arg(long = "path")]
        paths: Vec<PathBuf>,
        #[arg(long)]
        force: bool,
    },
    /// Measure any folder: sizes of its subfolders, largest first.
    Measure {
        path: PathBuf,
        /// How many levels of subfolders to print.
        #[arg(long, default_value_t = 3)]
        depth: usize,
        /// How many children to print per folder.
        #[arg(long, default_value_t = 20)]
        top: usize,
        /// Print everything, with no depth or count limit.
        #[arg(long)]
        full: bool,
        /// Hide folders smaller than this, for example 100MB.
        #[arg(long)]
        min_size: Option<String>,
        /// Follow mounted drives and other filesystems below the path.
        #[arg(long)]
        cross_device: bool,
        /// Glob pattern to skip. Repeatable.
        #[arg(long = "exclude")]
        excludes: Vec<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        force: bool,
    },
    /// Archive old target/ folders into the vacuum, then move the originals to the trash.
    Compress {
        #[arg(long)]
        older_than: Option<String>,
        /// Show the plan and change nothing. This is the default when --yes is not given.
        #[arg(long)]
        dry_run: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// archive: .tar.zst into the vacuum (default). transparent: Windows compact.exe in place.
        #[arg(long, value_enum, default_value = "archive")]
        mode: Mode,
        #[arg(long = "path")]
        paths: Vec<PathBuf>,
        #[arg(long)]
        force: bool,
    },
    /// Restore an archive back into its project's target/ folder.
    Restore {
        /// Project slug, as shown by `rustvac archive list`.
        slug: String,
        /// Project folder to restore into. Defaults to the project recorded in the archive.
        #[arg(long)]
        into: Option<PathBuf>,
    },
    /// Manage archives in the vacuum.
    Archive {
        #[command(subcommand)]
        command: ArchiveCommand,
    },
    /// Launch the GUI.
    Gui,
}

#[derive(Subcommand)]
enum ArchiveCommand {
    /// List archives in the vacuum.
    List,
    /// Delete every archive for a slug. This cannot be undone.
    Rm {
        slug: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Mode {
    Archive,
    Transparent,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

type CmdResult = Result<ExitCode, String>;

fn run(cli: Cli) -> CmdResult {
    let config_path = cli.config.clone().or_else(default_config_path);
    let config = match &config_path {
        Some(p) => Config::load(p).map_err(|e| e.to_string())?,
        None => Config::default(),
    };
    config.validate().map_err(|e| e.to_string())?;

    match cli.command {
        Command::Scan {
            paths,
            older_than,
            json,
            force,
        } => {
            let filter = build_filter(older_than.as_deref(), None)?;
            let roots = resolve_roots(paths, &config, force)?;
            let arena = scan_targets(&roots, &config)?;
            print_targets(&arena, &filter, &config, json)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::List { paths, force } => {
            let roots = resolve_roots(paths, &config, force)?;
            let arena = scan_targets(&roots, &config)?;
            print_targets(&arena, &Filter::default(), &config, false)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Measure {
            path,
            depth,
            top,
            full,
            min_size,
            cross_device,
            excludes,
            json,
            force,
        } => {
            check_scan_root(&path, force).map_err(|e| e.to_string())?;
            let min = min_size.as_deref().map(parse_size).transpose()?;
            let mut req = ScanRequest::general(path.clone());
            req.cross_device = cross_device;
            req.exclude = config.exclude_patterns().map_err(|e| e.to_string())?;
            for pattern in &excludes {
                req.exclude.push(
                    glob::Pattern::new(pattern)
                        .map_err(|e| format!("bad --exclude '{pattern}': {e}"))?,
                );
            }
            let arena = run_scan(req)?;
            let limits = if full {
                Limits::unlimited()
            } else {
                Limits {
                    depth,
                    top,
                    min_size: min,
                }
            };
            if json {
                print_json(&measure_json(&arena))?;
            } else {
                print_tree(&arena, &limits);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Compress {
            older_than,
            dry_run,
            yes,
            mode,
            paths,
            force,
        } => {
            let filter = build_filter(older_than.as_deref(), Some(config.older_than_days))?;
            let roots = resolve_roots(paths, &config, force)?;
            let arena = scan_targets(&roots, &config)?;
            compress(&arena, &filter, &config, mode, dry_run, yes)
        }
        Command::Restore { slug, into } => {
            let vacuum = config.archive_dir_path();
            let entry = archive::latest_for_slug(&vacuum, &slug).map_err(|e| e.to_string())?;
            let manifest = archive::verify(&entry.path).map_err(|e| e.to_string())?;
            let destination = into.unwrap_or_else(|| manifest.project.clone());
            let restored =
                archive::restore(&entry.path, &destination).map_err(|e| e.to_string())?;
            println!(
                "restored {} -> {}",
                entry.path.display(),
                restored.display()
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Archive { command } => archive_command(command, &config),
        Command::Gui => launch_gui(),
    }
}

fn build_filter(older_than: Option<&str>, default_days: Option<u64>) -> Result<Filter, String> {
    let older = match older_than {
        Some(s) => Some(parse_duration(s)?),
        None => default_days.map(|d| Duration::from_secs(d * 86_400)),
    };
    Ok(Filter {
        older_than: older,
        min_size: None,
    })
}

fn resolve_roots(
    paths: Vec<PathBuf>,
    config: &Config,
    force: bool,
) -> Result<Vec<PathBuf>, String> {
    let roots = if paths.is_empty() {
        config.root_paths()
    } else {
        paths
            .into_iter()
            .map(|p| expand_tilde(&p.to_string_lossy()))
            .collect()
    };
    if roots.is_empty() {
        return Err("no roots: pass --path or set roots in the config".into());
    }
    for root in &roots {
        check_scan_root(root, force).map_err(|e| e.to_string())?;
    }
    Ok(roots)
}

fn scan_targets(roots: &[PathBuf], config: &Config) -> Result<Arena, String> {
    let mut req = ScanRequest::rust_targets(roots.to_vec());
    req.exclude = config.exclude_patterns().map_err(|e| e.to_string())?;
    req.busy_grace = Duration::from_secs(config.busy_grace_seconds);
    run_scan(req)
}

/// Runs a scan to completion and reports progress on stderr. Stdout stays clean for piping.
fn run_scan(req: ScanRequest) -> Result<Arena, String> {
    let handle = scan::start(req);
    let mut arena = Arena::new();
    let mut failures: Vec<(PathBuf, String)> = Vec::new();
    let interactive = io::IsTerminal::is_terminal(&io::stderr());
    for event in handle.events.iter() {
        arena.apply(&event);
        match &event {
            ScanEvent::Progress { done, total } if interactive => {
                eprint!("\rsizing targets {done}/{total}");
            }
            ScanEvent::Failed { path, message } => failures.push((path.clone(), message.clone())),
            ScanEvent::Finished { elapsed, cancelled } => {
                if interactive {
                    eprint!("\r{:<40}\r", "");
                }
                if *cancelled {
                    eprintln!("scan cancelled");
                }
                eprintln!("scanned in {:.2}s", elapsed.as_secs_f64());
                break;
            }
            _ => {}
        }
    }
    handle.join();
    if !failures.is_empty() {
        eprintln!("warning: {} paths could not be read:", failures.len());
        for (path, message) in failures.iter().take(20) {
            eprintln!("  {}: {message}", path.display());
        }
        if failures.len() > 20 {
            eprintln!("  ...and {} more", failures.len() - 20);
        }
    }
    Ok(arena)
}

#[derive(Tabled)]
struct TargetRow {
    #[tabled(rename = "Project")]
    project: String,
    #[tabled(rename = "Size")]
    size: String,
    #[tabled(rename = "Files")]
    files: u64,
    #[tabled(rename = "Age")]
    age: String,
    #[tabled(rename = "State")]
    state: String,
    #[tabled(rename = "Path")]
    path: String,
}

#[derive(Serialize)]
struct TargetJson {
    project: String,
    path: String,
    size_bytes: u64,
    files: u64,
    age_days: Option<u64>,
    newest_unix: Option<u64>,
    state: String,
    kept: bool,
}

/// Targets as (project node, target node), largest first.
fn sorted_targets<'a>(
    arena: &'a Arena,
    filter: &Filter,
    now: SystemTime,
) -> Vec<(&'a tpt_rustvac_core::Node, &'a tpt_rustvac_core::Node)> {
    let mut out: Vec<_> = arena
        .iter()
        .filter(|n| n.kind == Kind::Target)
        .filter(|n| filter.accepts(n, now))
        .map(|t| {
            let project = t.parent.and_then(|p| arena.get(p)).unwrap_or(t);
            (project, t)
        })
        .collect();
    out.sort_by_key(|(_, t)| std::cmp::Reverse(t.size));
    out
}

fn state_label(state: &State, kept: bool) -> String {
    let base = match state {
        State::Pending => "pending",
        State::Scanning => "scanning",
        State::Done => "ok",
        State::Failed(_) => "failed",
        State::Busy => "busy",
        State::Archived => "archived",
    };
    if kept {
        format!("{base}, kept")
    } else {
        base.to_string()
    }
}

fn print_targets(arena: &Arena, filter: &Filter, config: &Config, json: bool) -> CmdResult {
    let now = SystemTime::now();
    let keep = config.keep_paths();
    let targets = sorted_targets(arena, filter, now);

    if json {
        let rows: Vec<TargetJson> = targets
            .iter()
            .map(|(project, t)| {
                let kept = is_kept(&t.path, &keep);
                TargetJson {
                    project: project.name.clone(),
                    path: t.path.display().to_string(),
                    size_bytes: t.size,
                    files: t.files,
                    age_days: age_days(t.newest, now),
                    newest_unix: t
                        .newest
                        .and_then(|n| n.duration_since(SystemTime::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs()),
                    state: state_label(&t.state, false),
                    kept,
                }
            })
            .collect();
        print_json(&rows)?;
        return Ok(ExitCode::SUCCESS);
    }

    if targets.is_empty() {
        println!("no Rust target folders found");
        return Ok(ExitCode::SUCCESS);
    }
    let total: u64 = targets.iter().map(|(_, t)| t.size).sum();
    let rows: Vec<TargetRow> = targets
        .iter()
        .map(|(project, t)| TargetRow {
            project: project.name.clone(),
            size: human_bytes(t.size),
            files: t.files,
            age: age_label(t.newest, now),
            state: state_label(&t.state, is_kept(&t.path, &keep)),
            path: t.path.display().to_string(),
        })
        .collect();
    let mut table = Table::new(rows);
    table.with(Style::rounded());
    println!("{table}");
    println!("{} targets, {} total", targets.len(), human_bytes(total));
    Ok(ExitCode::SUCCESS)
}

fn compress(
    arena: &Arena,
    filter: &Filter,
    config: &Config,
    mode: Mode,
    dry_run: bool,
    yes: bool,
) -> CmdResult {
    let now = SystemTime::now();
    let keep = config.keep_paths();
    let candidates: Vec<_> = sorted_targets(arena, filter, now)
        .into_iter()
        .filter(|(_, t)| t.state == State::Done)
        .filter(|(_, t)| !is_kept(&t.path, &keep))
        .collect();

    if candidates.is_empty() {
        println!("nothing to compress");
        return Ok(ExitCode::SUCCESS);
    }
    let total: u64 = candidates.iter().map(|(_, t)| t.size).sum();
    println!(
        "{} targets, {} to reclaim:",
        candidates.len(),
        human_bytes(total)
    );
    for (_, t) in &candidates {
        println!(
            "  {:>10}  {:>6}  {}",
            human_bytes(t.size),
            age_label(t.newest, now),
            t.path.display()
        );
    }

    if dry_run || !yes {
        if dry_run {
            println!("dry run: nothing changed");
            return Ok(ExitCode::SUCCESS);
        }
        if !confirm(&format!(
            "Compress {} targets and move the originals to the trash?",
            candidates.len()
        ))? {
            println!("cancelled");
            return Ok(ExitCode::SUCCESS);
        }
    }

    let vacuum = config.archive_dir_path();
    let grace = Duration::from_secs(config.busy_grace_seconds);
    let mut failed = 0usize;
    for (project, t) in &candidates {
        let project_path = project.path.clone();
        let result = match mode {
            Mode::Archive => archive::compress_and_vacuum(&t.path, &project_path, &vacuum, grace)
                .map(|r| {
                    format!(
                        "archived to {} ({} -> {})",
                        r.archive.display(),
                        human_bytes(r.manifest.bytes),
                        human_bytes(r.compressed_bytes)
                    )
                })
                .map_err(|e| e.to_string()),
            Mode::Transparent => archive::compact_transparent(&t.path)
                .map(|_| "compacted in place".to_string())
                .map_err(|e| e.to_string()),
        };
        match result {
            Ok(msg) => println!("ok    {}: {msg}", t.path.display()),
            Err(e) => {
                failed += 1;
                println!("FAIL  {}: {e}", t.path.display());
            }
        }
    }
    if failed > 0 {
        println!(
            "{failed} of {} failed; their originals were left in place",
            candidates.len()
        );
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}

fn archive_command(command: ArchiveCommand, config: &Config) -> CmdResult {
    let vacuum = config.archive_dir_path();
    match command {
        ArchiveCommand::List => {
            let entries = archive::list(&vacuum).map_err(|e| e.to_string())?;
            if entries.is_empty() {
                println!("no archives in {}", vacuum.display());
                return Ok(ExitCode::SUCCESS);
            }
            #[derive(Tabled)]
            struct Row {
                #[tabled(rename = "Slug")]
                slug: String,
                #[tabled(rename = "Date")]
                date: String,
                #[tabled(rename = "Size")]
                size: String,
            }
            let rows: Vec<Row> = entries
                .iter()
                .map(|e| Row {
                    slug: e.slug.clone(),
                    date: e.date.clone(),
                    size: human_bytes(e.bytes_on_disk),
                })
                .collect();
            let mut table = Table::new(rows);
            table.with(Style::rounded());
            println!("{table}");
            Ok(ExitCode::SUCCESS)
        }
        ArchiveCommand::Rm { slug, yes } => {
            if !yes
                && !confirm(&format!(
                    "Delete all archives for '{slug}'? This cannot be undone."
                ))?
            {
                println!("cancelled");
                return Ok(ExitCode::SUCCESS);
            }
            let removed = archive::remove_slug(&vacuum, &slug).map_err(|e| e.to_string())?;
            for path in removed {
                println!("removed {}", path.display());
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn launch_gui() -> CmdResult {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let gui = exe.with_file_name(format!("rustvac-gui{}", std::env::consts::EXE_SUFFIX));
    if !gui.exists() {
        return Err(format!(
            "GUI not found at {} (build it with `cargo build -p tpt-rustvac-gui`)",
            gui.display()
        ));
    }
    std::process::Command::new(&gui)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(ExitCode::SUCCESS)
}

/// Asks a yes/no question. Anything other than an explicit "y" is no. Used so a
/// closed or non-interactive stdin cannot approve a destructive action.
fn confirm(question: &str) -> Result<bool, String> {
    print!("{question} [y/N] ");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

struct Limits {
    depth: usize,
    top: usize,
    min_size: Option<u64>,
}

impl Limits {
    fn unlimited() -> Self {
        Limits {
            depth: usize::MAX,
            top: usize::MAX,
            min_size: None,
        }
    }
}

fn print_tree(arena: &Arena, limits: &Limits) {
    let Some(root) = arena.root() else { return };
    println!(
        "{}  {}  {} files",
        root.path.display(),
        human_bytes(root.size),
        root.files
    );
    print_children(arena, root.id, "", 1, limits);
}

fn print_children(arena: &Arena, parent: NodeId, prefix: &str, level: usize, limits: &Limits) {
    let Some(node) = arena.get(parent) else {
        return;
    };
    if level > limits.depth {
        return;
    }
    let mut children: Vec<_> = node
        .children
        .iter()
        .filter_map(|id| arena.get(*id))
        .filter(|c| limits.min_size.map(|m| c.size >= m).unwrap_or(true))
        .collect();
    children.sort_by(|a, b| b.size.cmp(&a.size).then(a.name.cmp(&b.name)));
    let hidden = children.len().saturating_sub(limits.top);
    let shown: Vec<_> = children.into_iter().take(limits.top).collect();
    let count = shown.len();
    for (i, child) in shown.iter().enumerate() {
        let last = i + 1 == count && hidden == 0;
        let branch = if last { "└─ " } else { "├─ " };
        println!(
            "{prefix}{branch}{:<36} {:>10}  {:>8} files",
            child.name,
            human_bytes(child.size),
            child.files
        );
        let next_prefix = format!("{prefix}{}", if last { "   " } else { "│  " });
        print_children(arena, child.id, &next_prefix, level + 1, limits);
    }
    if hidden > 0 {
        println!("{prefix}└─ … {hidden} more");
    }
}

#[derive(Serialize)]
struct MeasureNodeJson {
    name: String,
    path: String,
    size_bytes: u64,
    files: u64,
    children: Vec<MeasureNodeJson>,
}

fn measure_json(arena: &Arena) -> Option<MeasureNodeJson> {
    let root = arena.root()?;
    Some(build_json(arena, root.id))
}

fn build_json(arena: &Arena, id: NodeId) -> MeasureNodeJson {
    let node = arena.get(id).expect("ids in the arena are valid");
    let mut children: Vec<_> = node.children.iter().filter_map(|c| arena.get(*c)).collect();
    children.sort_by_key(|c| std::cmp::Reverse(c.size));
    MeasureNodeJson {
        name: node.name.clone(),
        path: node.path.display().to_string(),
        size_bytes: node.size,
        files: node.files,
        children: children
            .into_iter()
            .map(|c| build_json(arena, c.id))
            .collect(),
    }
}

fn print_json<T: Serialize + ?Sized>(value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    println!("{text}");
    Ok(())
}
