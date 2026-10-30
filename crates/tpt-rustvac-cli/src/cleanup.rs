//! `prune`, `toolchains`, `caches` and `sweep`: removal of regenerable Rust scratch.
//! Everything here shows the plan first and asks before it removes anything.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use clap::Subcommand;
use serde::Serialize;
use tabled::settings::Style;
use tabled::{Table, Tabled};
use tpt_rustvac_core::caches::{self, CacheKind, CleanSpec, Tier};
use tpt_rustvac_core::config::{cargo_home, rustup_home, Config, RemoveMode, RemovePolicy};
use tpt_rustvac_core::filter::Filter;
use tpt_rustvac_core::model::{Arena, Node};
use tpt_rustvac_core::prune::{self, PruneSpec};
use tpt_rustvac_core::safety::{choose_removal, Removal};
use tpt_rustvac_core::sweep::{self, SweepInputs};
use tpt_rustvac_core::target::RustcInfo;
use tpt_rustvac_core::toolchains;
use tpt_rustvac_core::{disk, wsl};

use crate::format::{age_label, human_bytes};
use crate::{confirm, print_json, CmdResult};

#[derive(Subcommand)]
pub enum ToolchainsCommand {
    /// Uninstall a toolchain with `rustup toolchain uninstall`. The default toolchain is refused.
    Rm {
        name: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum CachesCommand {
    /// Remove regenerable cache entries. With no kind flags, every regenerable kind is cleared.
    Clean {
        /// Leftovers in rustup's tmp folder.
        #[arg(long)]
        rustup_tmp: bool,
        /// Rustup's download folder and update hashes.
        #[arg(long)]
        rustup_downloads: bool,
        /// Cargo's downloaded crates and unpacked sources.
        #[arg(long)]
        cargo_registry: bool,
        /// Cargo's git checkouts. The git database is kept.
        #[arg(long)]
        cargo_git: bool,
        /// Only entries whose newest file is at least this old (for example 30d).
        #[arg(long)]
        older_than: Option<String>,
        /// auto, trash or permanent. Overrides remove_mode in the config.
        #[arg(long, value_parser = parse_remove)]
        remove: Option<RemoveMode>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
    },
}

pub fn parse_remove(s: &str) -> Result<RemoveMode, String> {
    s.parse()
}

/// The installed compilers, for the stale-toolchain check. An empty list is an error here,
/// because it would make every target look stale.
pub fn installed_compilers() -> Result<Vec<RustcInfo>, String> {
    let home = rustup_home().ok_or("cannot find the rustup home (set RUSTUP_HOME)")?;
    let found = toolchains::installed_rustc_versions(&home);
    if found.is_empty() {
        return Err(format!(
            "no installed toolchains found in {}, so stale targets cannot be told apart",
            home.display()
        ));
    }
    Ok(found)
}

fn policy_text(policy: &RemovePolicy) -> String {
    match policy.mode {
        RemoveMode::Trash => "everything goes to the trash".into(),
        RemoveMode::Permanent => "everything is deleted permanently".into(),
        RemoveMode::Auto => format!(
            "up to {} goes to the trash, larger items are deleted permanently",
            human_bytes(policy.trash_max)
        ),
    }
}

fn action_word(size: u64, policy: &RemovePolicy) -> &'static str {
    match choose_removal(size, policy) {
        Removal::Trashed => "trash",
        Removal::Deleted => "delete",
    }
}

fn print_skipped(skipped: &[(PathBuf, String)]) {
    if skipped.is_empty() {
        return;
    }
    println!("left alone ({}):", skipped.len());
    for (path, reason) in skipped.iter().take(20) {
        println!("  {}: {reason}", path.display());
    }
    if skipped.len() > 20 {
        println!("  ...and {} more", skipped.len() - 20);
    }
}

/// Runs a removal and prints one line per path. Returns the bytes freed and the failure count.
fn report_removals(
    results: Vec<(PathBuf, tpt_rustvac_core::Result<Removal>)>,
    sizes: &dyn Fn(&Path) -> u64,
) -> (u64, usize) {
    let (mut freed, mut failed) = (0u64, 0usize);
    for (path, result) in results {
        match result {
            Ok(how) => {
                freed += sizes(&path);
                let how = match how {
                    Removal::Trashed => "trashed",
                    Removal::Deleted => "deleted",
                };
                println!("ok    {how} {}", path.display());
            }
            Err(e) => {
                failed += 1;
                println!("FAIL  {}: {e}", path.display());
            }
        }
    }
    (freed, failed)
}

fn finish(freed: u64, failed: usize, total: usize) -> CmdResult {
    println!("freed {}", human_bytes(freed));
    if failed > 0 {
        println!("{failed} of {total} failed; those were left in place");
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}

// ---- prune ----

pub struct PruneArgs {
    pub incremental: bool,
    pub profile: Option<String>,
    pub stale_deps: bool,
    pub remove: Option<RemoveMode>,
    pub dry_run: bool,
    pub yes: bool,
}

pub fn prune(
    config: &Config,
    args: PruneArgs,
    candidates: Vec<(&Node, &Node)>,
    mut skipped: Vec<(PathBuf, String)>,
) -> CmdResult {
    let chosen = usize::from(args.incremental)
        + usize::from(args.profile.is_some())
        + usize::from(args.stale_deps);
    if chosen != 1 {
        return Err("pass exactly one of --incremental, --profile <name> or --stale-deps".into());
    }
    let spec = if args.incremental {
        Some(PruneSpec::Incremental)
    } else {
        args.profile.map(PruneSpec::Profile)
    };
    let policy = config
        .removal_policy(args.remove)
        .map_err(|e| e.to_string())?;
    let now = SystemTime::now();

    let mut items = Vec::new();
    for (project, t) in &candidates {
        match &spec {
            Some(spec) => items.extend(
                prune::plan(&t.path, spec)
                    .into_iter()
                    .filter(|i| i.size > 0),
            ),
            None => match prune::live_crate_dirs(&project.path, &t.path) {
                Ok(live) => items.extend(
                    prune::plan_stale_deps(&t.path, &live)
                        .into_iter()
                        .filter(|i| i.size > 0),
                ),
                Err(reason) => skipped.push((t.path.clone(), reason)),
            },
        }
    }
    if items.is_empty() {
        println!("nothing to prune");
        print_skipped(&skipped);
        return Ok(ExitCode::SUCCESS);
    }
    items.sort_by_key(|i| std::cmp::Reverse(i.size));
    let total: u64 = items.iter().map(|i| i.size).sum();
    println!(
        "{} items in {} targets, {} to reclaim ({}):",
        items.len(),
        candidates.len(),
        human_bytes(total),
        policy_text(&policy)
    );
    for i in &items {
        println!(
            "  {:>10}  {:>6}  {:<6}  {}",
            human_bytes(i.size),
            age_label(i.newest, now),
            action_word(i.size, &policy),
            i.path.display()
        );
    }
    print_skipped(&skipped);
    if args.dry_run {
        println!("dry run: nothing changed");
        return Ok(ExitCode::SUCCESS);
    }
    if !args.yes && !confirm(&format!("Remove {} items?", items.len()))? {
        println!("cancelled");
        return Ok(ExitCode::SUCCESS);
    }
    let results = items
        .iter()
        .map(|i| {
            (
                i.path.clone(),
                tpt_rustvac_core::safety::remove_group_with_policy(&i.members, i.size, &policy),
            )
        })
        .collect();
    let size_of = |p: &Path| items.iter().find(|i| i.path == p).map_or(0, |i| i.size);
    let (freed, failed) = report_removals(results, &size_of);
    finish(freed, failed, items.len())
}

// ---- toolchains ----

#[derive(Tabled)]
struct ToolchainRow {
    #[tabled(rename = "Toolchain")]
    name: String,
    #[tabled(rename = "Size")]
    size: String,
    #[tabled(rename = "Installed")]
    installed: String,
    #[tabled(rename = "Last changed")]
    last_used: String,
    #[tabled(rename = "Marker")]
    marker: String,
}

#[derive(Serialize)]
struct ToolchainJson {
    name: String,
    path: String,
    size_bytes: u64,
    installed_unix: Option<u64>,
    last_used_unix: Option<u64>,
    default: Option<bool>,
}

pub fn toolchains_command(command: Option<ToolchainsCommand>, json: bool) -> CmdResult {
    let home = rustup_home().ok_or("cannot find the rustup home (set RUSTUP_HOME)")?;
    match command {
        None => list_toolchains(&home, json),
        Some(ToolchainsCommand::Rm { name, yes }) => {
            let found = toolchains::installed(&home);
            let Some(tc) = found.iter().find(|t| t.name == name) else {
                return Err(format!("no toolchain named '{name}' in {}", home.display()));
            };
            let size = caches::discover(Some(&home), None)
                .iter()
                .find(|i| i.path == tc.path)
                .map_or(0, |i| i.size);
            println!("{name}: {} at {}", human_bytes(size), tc.path.display());
            if !yes && !confirm(&format!("Uninstall {name} with rustup?"))? {
                println!("cancelled");
                return Ok(ExitCode::SUCCESS);
            }
            let output = toolchains::uninstall(&name).map_err(|e| e.to_string())?;
            print!("{output}");
            println!("freed about {}", human_bytes(size));
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn list_toolchains(home: &Path, json: bool) -> CmdResult {
    let items = caches::discover(Some(home), None);
    // The default marker comes from rustup. Without rustup the table has no markers.
    let listing = toolchains::list_with_rustup().ok();
    let marker_of = |name: &str| -> Option<(bool, bool)> {
        listing
            .as_ref()
            .map(|l| l.iter().find(|x| x.name == name))
            .map(|x| x.map_or((false, false), |x| (x.default, x.active)))
    };
    let installed_unix = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|m| m.created().or_else(|_| m.modified()))
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
    };
    let toolchain_items: Vec<_> = items
        .iter()
        .filter(|i| i.kind == CacheKind::Toolchain)
        .collect();

    if json {
        let rows: Vec<ToolchainJson> = toolchain_items
            .iter()
            .map(|i| ToolchainJson {
                name: i.name.clone(),
                path: i.path.display().to_string(),
                size_bytes: i.size,
                installed_unix: installed_unix(&i.path),
                last_used_unix: toolchains::last_used(&i.path)
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()),
                default: marker_of(&i.name).map(|m| m.0),
            })
            .collect();
        print_json(&rows)?;
        return Ok(ExitCode::SUCCESS);
    }
    if toolchain_items.is_empty() {
        println!("no toolchains in {}", home.join("toolchains").display());
        return Ok(ExitCode::SUCCESS);
    }
    let now = SystemTime::now();
    let rows: Vec<ToolchainRow> = toolchain_items
        .iter()
        .map(|i| ToolchainRow {
            name: i.name.clone(),
            size: human_bytes(i.size),
            installed: age_label(
                installed_unix(&i.path).map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s)),
                now,
            ),
            last_used: age_label(toolchains::last_used(&i.path), now),
            marker: match marker_of(&i.name) {
                Some((true, _)) => "default".into(),
                Some((_, true)) => "active".into(),
                Some(_) => String::new(),
                None => "?".into(),
            },
        })
        .collect();
    let total: u64 = toolchain_items.iter().map(|i| i.size).sum();
    let mut table = Table::new(rows);
    table.with(Style::rounded());
    println!("{table}");
    println!(
        "{} toolchains, {} total. Remove one with `rustvac toolchains rm <name>`.",
        toolchain_items.len(),
        human_bytes(total)
    );
    if listing.is_none() {
        println!("note: rustup was not found on PATH, so default markers are unknown and removal is unavailable");
    }
    Ok(ExitCode::SUCCESS)
}

// ---- caches ----

#[derive(Tabled)]
struct CacheRow {
    #[tabled(rename = "Kind")]
    kind: String,
    #[tabled(rename = "Tier")]
    tier: String,
    #[tabled(rename = "Size")]
    size: String,
    #[tabled(rename = "Files")]
    files: u64,
    #[tabled(rename = "Age")]
    age: String,
    #[tabled(rename = "Path")]
    path: String,
}

#[derive(Serialize)]
struct CacheJson {
    kind: CacheKind,
    tier: Tier,
    name: String,
    path: String,
    size_bytes: u64,
    files: u64,
    newest_unix: Option<u64>,
}

pub fn caches_command(command: Option<CachesCommand>, json: bool, config: &Config) -> CmdResult {
    let rustup = rustup_home();
    let cargo = cargo_home();
    let items = caches::discover(rustup.as_deref(), cargo.as_deref());
    let now = SystemTime::now();
    match command {
        None => {
            if json {
                let rows: Vec<CacheJson> = items
                    .iter()
                    .map(|i| CacheJson {
                        kind: i.kind,
                        tier: i.tier(),
                        name: i.name.clone(),
                        path: i.path.display().to_string(),
                        size_bytes: i.size,
                        files: i.files,
                        newest_unix: i
                            .newest
                            .and_then(|n| n.duration_since(SystemTime::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs()),
                    })
                    .collect();
                print_json(&rows)?;
                return Ok(ExitCode::SUCCESS);
            }
            if items.is_empty() {
                println!("no rustup or cargo folders found");
                return Ok(ExitCode::SUCCESS);
            }
            let tier_name = |t: Tier| match t {
                Tier::Regenerable => "regenerable",
                Tier::Managed => "managed by rustup",
                Tier::Manual => "manual",
            };
            let rows: Vec<CacheRow> = items
                .iter()
                .map(|i| CacheRow {
                    kind: if i.name.is_empty() {
                        i.kind.label().to_string()
                    } else {
                        format!("{} {}", i.kind.label(), i.name)
                    },
                    tier: tier_name(i.tier()).into(),
                    size: human_bytes(i.size),
                    files: i.files,
                    age: age_label(i.newest, now),
                    path: i.path.display().to_string(),
                })
                .collect();
            let mut table = Table::new(rows);
            table.with(Style::rounded());
            println!("{table}");
            let regenerable: u64 = items
                .iter()
                .filter(|i| i.tier() == Tier::Regenerable)
                .map(|i| i.size)
                .sum();
            println!(
                "{} regenerable. Clear it with `rustvac caches clean`. Toolchains: `rustvac toolchains`.",
                human_bytes(regenerable)
            );
            Ok(ExitCode::SUCCESS)
        }
        Some(CachesCommand::Clean {
            rustup_tmp,
            rustup_downloads,
            cargo_registry,
            cargo_git,
            older_than,
            remove,
            dry_run,
            yes,
        }) => {
            let any = rustup_tmp || rustup_downloads || cargo_registry || cargo_git;
            let mut kinds = Vec::new();
            if rustup_tmp || !any {
                kinds.push(CacheKind::RustupTmp);
            }
            if rustup_downloads || !any {
                kinds.push(CacheKind::RustupDownloads);
                kinds.push(CacheKind::RustupUpdateHashes);
            }
            if cargo_registry || !any {
                kinds.push(CacheKind::CargoRegistryCache);
                kinds.push(CacheKind::CargoRegistrySrc);
            }
            if cargo_git || !any {
                kinds.push(CacheKind::CargoGitCheckouts);
            }
            let older = older_than
                .as_deref()
                .map(crate::format::parse_duration)
                .transpose()?;
            let policy = config.removal_policy(remove).map_err(|e| e.to_string())?;
            let spec = CleanSpec {
                kinds,
                older_than: older,
                busy_grace: Duration::from_secs(config.busy_grace_seconds),
            };
            let plan = caches::plan_clean(&items, &spec, now);
            if plan.entries.is_empty() {
                println!("nothing to clean");
                print_skipped(&plan.skipped);
                return Ok(ExitCode::SUCCESS);
            }
            println!(
                "{} entries, {} to reclaim ({}):",
                plan.entries.len(),
                human_bytes(plan.total_size()),
                policy_text(&policy)
            );
            for e in &plan.entries {
                println!(
                    "  {:>10}  {:>6}  {:<6}  {}",
                    human_bytes(e.size),
                    age_label(e.newest, now),
                    action_word(e.size, &policy),
                    e.path.display()
                );
            }
            print_skipped(&plan.skipped);
            if dry_run {
                println!("dry run: nothing changed");
                return Ok(ExitCode::SUCCESS);
            }
            if !yes && !confirm(&format!("Remove {} entries?", plan.entries.len()))? {
                println!("cancelled");
                return Ok(ExitCode::SUCCESS);
            }
            let results = caches::execute(&plan, &policy);
            let size_of = |p: &Path| {
                plan.entries
                    .iter()
                    .find(|e| e.path == p)
                    .map_or(0, |e| e.size)
            };
            let (freed, failed) = report_removals(results, &size_of);
            finish(freed, failed, plan.entries.len())
        }
    }
}

// ---- sweep ----

pub fn build_sweep_plan(
    arena: &Arena,
    filter: &Filter,
    config: &Config,
) -> Result<sweep::SweepPlan, String> {
    // A missing or empty rustup home only turns stale detection off. The other items still count.
    let installed = installed_compilers().unwrap_or_default();
    let rustup = rustup_home();
    let cargo = cargo_home();
    let keep = config.keep_paths();
    Ok(sweep::plan(&SweepInputs {
        arena,
        keep: &keep,
        filter,
        installed: &installed,
        rustup_home: rustup.as_deref(),
        cargo_home: cargo.as_deref(),
        busy_grace: Duration::from_secs(config.busy_grace_seconds),
        now: SystemTime::now(),
    }))
}

pub fn sweep_command(
    arena: &Arena,
    filter: &Filter,
    config: &Config,
    remove: Option<RemoveMode>,
    dry_run: bool,
    yes: bool,
) -> CmdResult {
    if installed_compilers().is_err() {
        eprintln!(
            "note: no installed toolchains found, so targets from old compilers are not detected"
        );
    }
    let policy = config.removal_policy(remove).map_err(|e| e.to_string())?;
    let plan = build_sweep_plan(arena, filter, config)?;
    if plan.items.is_empty() {
        println!("nothing to sweep");
        print_skipped(&plan.skipped);
        return Ok(ExitCode::SUCCESS);
    }
    let now = SystemTime::now();
    let mut items: Vec<_> = plan.items.iter().collect();
    items.sort_by_key(|i| std::cmp::Reverse(i.size));
    println!("{} items ({}):", plan.items.len(), policy_text(&policy));
    for i in items.iter().take(30) {
        println!(
            "  {:>10}  {:>6}  {:<6}  {}",
            human_bytes(i.size),
            age_label(i.newest, now),
            action_word(i.size, &policy),
            i.path.display()
        );
    }
    if items.len() > 30 {
        println!("  ...and {} more", items.len() - 30);
    }
    println!("by kind:");
    for (kind, bytes, count) in plan.by_kind() {
        println!("  {:>10}  {count:>4}  {}", human_bytes(bytes), kind.label());
    }
    print_skipped(&plan.skipped);
    println!("reclaimable: {}", human_bytes(plan.total_size()));
    if dry_run {
        println!("dry run: nothing changed");
        return Ok(ExitCode::SUCCESS);
    }
    if !yes && !confirm(&format!("Remove {} items?", plan.items.len()))? {
        println!("cancelled");
        return Ok(ExitCode::SUCCESS);
    }
    let results = sweep::execute(&plan, &policy);
    let size_of = |p: &Path| {
        plan.items
            .iter()
            .find(|i| i.path == p)
            .map_or(0, |i| i.size)
    };
    let (freed, failed) = report_removals(results, &size_of);
    finish(freed, failed, plan.items.len())
}

// ---- disk ----

pub fn disk_command(json: bool) -> CmdResult {
    let items = caches::discover(rustup_home().as_deref(), cargo_home().as_deref());
    let space =
        tpt_rustvac_core::config::home_dir().and_then(|h| tpt_rustvac_core::check::disk_space(&h));
    let report = disk::build(&items, wsl::discover_here(), space);
    let steps = disk::next_steps(&report);
    if json {
        #[derive(Serialize)]
        struct WslOut<'a> {
            #[serde(flatten)]
            disk: &'a wsl::WslDisk,
            compact_steps: String,
        }
        #[derive(Serialize)]
        struct Out<'a> {
            #[serde(flatten)]
            report: &'a disk::DiskReport,
            next_steps: &'a disk::Steps,
            wsl_steps: Vec<WslOut<'a>>,
        }
        let wsl_steps = report
            .wsl
            .iter()
            .map(|d| WslOut {
                disk: d,
                compact_steps: wsl::compact_hint(d),
            })
            .collect();
        print_json(&Out {
            report: &report,
            next_steps: &steps,
            wsl_steps,
        })?;
        return Ok(ExitCode::SUCCESS);
    }
    print!("{}", render_disk(&report, &steps, SystemTime::now()));
    Ok(ExitCode::SUCCESS)
}

pub fn render_disk(report: &disk::DiskReport, steps: &disk::Steps, now: SystemTime) -> String {
    let mut out = String::new();
    match (report.free_bytes, report.total_bytes) {
        (Some(f), Some(t)) => out.push_str(&format!(
            "free space on the home drive: {} of {}\n\n",
            human_bytes(f),
            human_bytes(t)
        )),
        _ => out.push_str("free space on the home drive: unknown\n\n"),
    }
    let step = |out: &mut String, s: &Option<String>| {
        if let Some(s) = s {
            out.push_str(&format!("  next: {s}\n"));
        }
        out.push('\n');
    };

    out.push_str(&format!(
        "Rustup scratch: {}\n",
        human_bytes(report.rustup_bytes())
    ));
    for i in &report.rustup {
        out.push_str(&format!("  {:>10}  {}\n", human_bytes(i.bytes), i.label));
    }
    step(&mut out, &steps.rustup);

    out.push_str(&format!(
        "Toolchains: {}\n",
        human_bytes(report.toolchain_bytes())
    ));
    for t in &report.toolchains {
        let changed = age_label(
            t.last_used_unix
                .map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s)),
            now,
        );
        out.push_str(&format!(
            "  {:>10}  {}  (changed {changed} ago)\n",
            human_bytes(t.bytes),
            t.name
        ));
    }
    step(&mut out, &steps.toolchains);

    out.push_str(&format!("Cargo: {}\n", human_bytes(report.cargo_bytes())));
    for i in &report.cargo {
        let tier = match i.tier {
            Tier::Regenerable => "",
            _ => "  (not part of a sweep)",
        };
        out.push_str(&format!(
            "  {:>10}  {}{tier}\n",
            human_bytes(i.bytes),
            i.label
        ));
    }
    step(&mut out, &steps.cargo);

    if report.wsl.is_empty() {
        out.push_str("WSL and Docker disks: none found\n");
    } else {
        out.push_str(&format!(
            "WSL and Docker disks: {} (report only)\n",
            human_bytes(report.wsl_bytes())
        ));
        for d in &report.wsl {
            out.push_str(&format!(
                "  {:>10}  {}  {}\n",
                human_bytes(d.size_bytes),
                d.name,
                d.path.display()
            ));
        }
        step(&mut out, &steps.wsl);
        for d in &report.wsl {
            out.push_str(&format!("To compact {}:\n", d.name));
            for line in wsl::compact_hint(d).lines() {
                out.push_str(&format!("  {line}\n"));
            }
        }
    }
    out
}
