//! Sweep, caches and check against fixture trees. The rustup and cargo homes are tempdirs,
//! passed explicitly, so these tests never touch the developer's real ones.

mod support;

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use tpt_rustvac_core::caches::{self, CacheKind, CleanSpec};
use tpt_rustvac_core::check::{self, CheckInput};
use tpt_rustvac_core::config::{RemoveMode, RemovePolicy};
use tpt_rustvac_core::filter::Filter;
use tpt_rustvac_core::model::{Arena, Kind, State};
use tpt_rustvac_core::sweep::{self, SweepInputs, SweepKind};
use tpt_rustvac_core::target::{parse_rustc_vv, RustcInfo};
use tpt_rustvac_core::{scan, ScanRequest};

fn run_scan(root: PathBuf) -> Arena {
    let mut req = ScanRequest::rust_targets(vec![root]);
    // The fixtures were written a moment ago, so turn the busy guard off for the scan.
    req.busy_grace = Duration::ZERO;
    let handle = scan::start(req);
    let mut arena = Arena::new();
    for event in handle.events.iter() {
        arena.apply(&event);
        if let scan::ScanEvent::Finished { .. } = event {
            break;
        }
    }
    handle.join();
    arena
}

fn installed() -> Vec<RustcInfo> {
    vec![parse_rustc_vv("release: 1.99.1\ncommit-hash: aaa\n").unwrap()]
}

struct World {
    _dir: tempfile::TempDir,
    projects: PathBuf,
    homes: support::Homes,
}

/// `old` was built by 1.80.0 (not installed). `current` by 1.99.1 (installed) and has
/// incremental caches. `kept` is stale too, but the config keeps it.
fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let projects = dir.path().join("projects");
    for (name, release, commit) in [
        ("old", "1.80.0", "bbb"),
        ("current", "1.99.1", "aaa"),
        ("kept", "1.80.0", "bbb"),
    ] {
        let project = projects.join(name);
        support::write_file(&project.join("Cargo.toml"), 40);
        let target = project.join("target");
        support::write_file(&target.join("debug/deps/lib.rlib"), 4000);
        support::write_incremental(&target, "debug", 1500);
        support::write_rustc_info(&target, release, commit);
    }
    let homes = support::build_homes(&dir.path().join("homes"));
    World {
        _dir: dir,
        projects,
        homes,
    }
}

fn plan_for(w: &World, arena: &Arena, keep: &[PathBuf], filter: &Filter) -> sweep::SweepPlan {
    let installed = installed();
    sweep::plan(&SweepInputs {
        arena,
        keep,
        filter,
        installed: &installed,
        rustup_home: Some(&w.homes.rustup),
        cargo_home: Some(&w.homes.cargo),
        busy_grace: Duration::ZERO,
        now: SystemTime::now(),
    })
}

fn of_kind(plan: &sweep::SweepPlan, kind: SweepKind) -> Vec<&sweep::SweepItem> {
    plan.items.iter().filter(|i| i.kind == kind).collect()
}

#[test]
fn sweep_plans_every_regenerable_kind() {
    let w = world();
    let arena = run_scan(w.projects.clone());
    let keep = vec![w.projects.join("kept")];
    let plan = plan_for(&w, &arena, &keep, &Filter::default());

    // The stale target goes whole. The kept one is skipped. The current one gives up incremental only.
    let stale = of_kind(&plan, SweepKind::StaleTarget);
    assert_eq!(stale.len(), 1);
    assert!(stale[0].path.starts_with(w.projects.join("old")));
    let node = arena
        .iter()
        .find(|n| n.kind == Kind::Target && n.path == stale[0].path)
        .unwrap();
    assert_eq!(stale[0].size, node.size);

    let inc = of_kind(&plan, SweepKind::Incremental);
    assert_eq!(inc.len(), 1);
    assert_eq!(
        inc[0].size,
        support::disk_of(
            &w.projects
                .join("current/target/debug/incremental/crate-abc/dep-graph.bin")
        )
    );

    assert_eq!(of_kind(&plan, SweepKind::RustupTmp).len(), 1);
    assert_eq!(of_kind(&plan, SweepKind::RustupDownloads).len(), 1);
    assert_eq!(of_kind(&plan, SweepKind::CargoRegistryCache).len(), 1);
    assert_eq!(of_kind(&plan, SweepKind::CargoRegistrySrc).len(), 1);
    assert_eq!(of_kind(&plan, SweepKind::CargoGitCheckouts).len(), 1);

    // Nothing from the manual or managed tiers, and nothing from the kept project.
    assert_eq!(plan.items.len(), 7);
    assert!(plan
        .items
        .iter()
        .all(|i| !i.path.starts_with(w.projects.join("kept"))));
    assert!(plan
        .items
        .iter()
        .all(|i| !i.path.starts_with(w.homes.cargo.join("git/db"))));
    assert!(plan
        .items
        .iter()
        .all(|i| !i.path.starts_with(w.homes.rustup.join("toolchains"))));
    assert!(plan
        .skipped
        .iter()
        .any(|(p, why)| p.starts_with(w.projects.join("kept")) && why.contains("kept")));
}

#[test]
fn busy_targets_and_caches_are_skipped() {
    let w = world();
    let mut arena = run_scan(w.projects.clone());
    let ids: Vec<_> = arena
        .iter()
        .filter(|n| n.kind == Kind::Target)
        .map(|n| n.id)
        .collect();
    for id in ids {
        arena.get_mut(id).unwrap().state = State::Busy;
    }
    let installed = installed();
    // A real grace period: the cache entries were written a moment ago, so they count as busy.
    let plan = sweep::plan(&SweepInputs {
        arena: &arena,
        keep: &[],
        filter: &Filter::default(),
        installed: &installed,
        rustup_home: Some(&w.homes.rustup),
        cargo_home: Some(&w.homes.cargo),
        busy_grace: Duration::from_secs(3600),
        now: SystemTime::now(),
    });
    assert!(plan.items.is_empty());
    assert_eq!(plan.skipped.len(), 3 + 5);
}

#[test]
fn sweep_execute_removes_only_what_it_listed() {
    let w = world();
    let arena = run_scan(w.projects.clone());
    let plan = plan_for(&w, &arena, &[], &Filter::default());
    let policy = RemovePolicy {
        mode: RemoveMode::Permanent,
        trash_max: 0,
    };
    for (path, result) in sweep::execute(&plan, &policy) {
        result.unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
    assert!(!w.projects.join("old/target").exists());
    assert!(!w.projects.join("kept/target").exists());
    // Sources and the build output itself stay.
    assert!(w.projects.join("old/Cargo.toml").exists());
    assert!(w
        .projects
        .join("current/target/debug/deps/lib.rlib")
        .exists());
    assert!(!w.projects.join("current/target/debug/incremental").exists());
    // Toolchains and the manual tier are untouched.
    assert!(w.homes.rustup.join("toolchains/stable-fake").exists());
    assert!(w.homes.cargo.join("git/db/repo-1/pack").exists());
    assert!(w
        .homes
        .cargo
        .join("registry/index/index-1/.cache/f")
        .exists());
    // The cache folders stay, empty.
    assert!(w.homes.rustup.join("tmp").is_dir());
    assert!(!w.homes.rustup.join("tmp/partial-install").exists());
}

#[test]
fn clean_sizes_match_what_discover_reports() {
    let w = world();
    let items = caches::discover(Some(&w.homes.rustup), Some(&w.homes.cargo));
    let tmp = items
        .iter()
        .find(|i| i.kind == CacheKind::RustupTmp)
        .unwrap();
    assert_eq!(
        tmp.size,
        support::disk_of(&w.homes.rustup.join("tmp/partial-install/a.bin"))
    );
    let spec = CleanSpec {
        kinds: vec![CacheKind::RustupTmp],
        older_than: None,
        busy_grace: Duration::ZERO,
    };
    let plan = caches::plan_clean(&items, &spec, SystemTime::now());
    assert_eq!(plan.total_size(), tmp.size);
}

#[test]
fn check_reports_the_same_total_as_sweep() {
    let w = world();
    let arena = run_scan(w.projects.clone());
    let plan = plan_for(&w, &arena, &[], &Filter::default());
    let rec = check::build(&CheckInput {
        plan: &plan,
        drive: &w.projects,
        // Injected, so the test does not depend on the real drive.
        space: Some((10_000, 100_000)),
        compressible_bytes: 0,
        warn_below: Some(50_000),
        now: SystemTime::now(),
        top_n: 3,
    });
    assert_eq!(rec.reclaimable_bytes, plan.total_size());
    assert_eq!(rec.free_bytes, Some(10_000));
    assert!(rec.low_space);
    assert_eq!(rec.top.len(), 3);
    assert!(rec.top.windows(2).all(|p| p[0].bytes >= p[1].bytes));

    let out = w._dir.path().join("report");
    check::save(&out, &rec, "text").unwrap();
    assert_eq!(check::load(&out).unwrap(), rec);
    assert_eq!(
        fs::read_to_string(out.join(check::TEXT_NAME)).unwrap(),
        "text"
    );
}

#[test]
fn check_never_lists_what_sweep_refuses() {
    let w = world();
    let arena = run_scan(w.projects.clone());
    let keep = vec![w.projects.join("old"), w.projects.join("kept")];
    let plan = plan_for(&w, &arena, &keep, &Filter::default());
    let rec = check::build(&CheckInput {
        plan: &plan,
        drive: &w.projects,
        space: None,
        compressible_bytes: 0,
        warn_below: None,
        now: SystemTime::now(),
        top_n: 50,
    });
    let refused: Vec<String> = keep.iter().map(|k| k.display().to_string()).collect();
    assert!(rec
        .top
        .iter()
        .all(|t| !refused.iter().any(|r| t.path.starts_with(r.as_str()))));
    assert!(rec.by_kind.iter().all(|k| k.kind != SweepKind::StaleTarget));
}

/// Runs a fake `rustc` script, so this only works where scripts are executable.
#[cfg(unix)]
#[test]
fn installed_versions_come_from_each_toolchains_rustc() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("toolchains/stable-fake/bin");
    fs::create_dir_all(&bin).unwrap();
    let rustc = bin.join("rustc");
    fs::write(
        &rustc,
        "#!/bin/sh\nprintf 'rustc 1.99.1 (abc 2026-01-01)\\nrelease: 1.99.1\\ncommit-hash: aaa\\n'\n",
    )
    .unwrap();
    fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755)).unwrap();
    // A half-installed toolchain with no rustc is left out.
    fs::create_dir_all(dir.path().join("toolchains/broken/bin")).unwrap();

    let found = tpt_rustvac_core::toolchains::installed_rustc_versions(dir.path());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].release, "1.99.1");
    assert_eq!(found[0].commit.as_deref(), Some("aaa"));
}
