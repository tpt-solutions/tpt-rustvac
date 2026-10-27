mod support;

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use tpt_rustvac_core::archive::{self, MANIFEST};
use tpt_rustvac_core::config::Config;
use tpt_rustvac_core::model::{Arena, Kind, NodeId, State};
use tpt_rustvac_core::safety::{check_scan_root, long_path};
use tpt_rustvac_core::target::{resolve_target_dir, TargetEnv};
use tpt_rustvac_core::walk::is_busy;
use tpt_rustvac_core::{scan, ScanRequest};

/// Runs a scan to completion and returns the arena it produced.
fn run_scan(req: ScanRequest) -> Arena {
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

fn env_none() -> TargetEnv {
    TargetEnv {
        cargo_target_dir: None,
        home: None,
    }
}

fn targets_of(arena: &Arena) -> Vec<(String, u64, u64)> {
    let mut out: Vec<(String, u64, u64)> = arena
        .iter()
        .filter(|n| n.kind == Kind::Target)
        .map(|n| {
            (
                n.path
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                n.size,
                n.files,
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn rust_scan_finds_projects_and_sizes_targets() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = support::build_rust_fixture(dir.path());

    let mut req = ScanRequest::rust_targets(vec![fixture.root.clone()]);
    req.target_env = env_none();
    let arena = run_scan(req);

    let root = arena.root().unwrap();
    assert_eq!(root.size, fixture.total_bytes, "root total");
    assert_eq!(root.files, fixture.total_files, "root file count");

    let mut expected: Vec<(String, u64, u64)> = fixture
        .projects
        .iter()
        .map(|(p, _, bytes, files)| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                *bytes,
                *files,
            )
        })
        .collect();
    expected.sort();
    assert_eq!(targets_of(&arena), expected);

    // Notes is not a project and must not appear anywhere in the tree.
    assert!(arena.iter().all(|n| !n.path.ends_with("notes")));
    // The member crate is not reported, because its target does not exist.
    assert!(arena.iter().all(|n| !n.path.ends_with("member")));
}

#[test]
fn rust_scan_projects_roll_up_targets() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = support::build_rust_fixture(dir.path());
    let mut req = ScanRequest::rust_targets(vec![fixture.root.clone()]);
    req.target_env = env_none();
    // Fixtures were written just now, so use zero grace to see the real state.
    req.busy_grace = Duration::ZERO;
    let arena = run_scan(req);

    let api = arena
        .iter()
        .find(|n| n.kind == Kind::Project && n.name == "api-server")
        .unwrap();
    let api_fixture = fixture
        .projects
        .iter()
        .find(|p| p.0.ends_with("api-server"))
        .unwrap()
        .2;
    assert_eq!(api.size, api_fixture);
    assert_eq!(api.files, 2);
    assert_eq!(api.state, State::Done);
    assert_eq!(api.children.len(), 1);

    // One level under target/ is built: debug/ and release/ with their own sizes.
    let target = arena.get(api.children[0]).unwrap();
    let mut kids: Vec<(String, u64)> = target
        .children
        .iter()
        .map(|c| {
            (
                arena.get(*c).unwrap().name.clone(),
                arena.get(*c).unwrap().size,
            )
        })
        .collect();
    kids.sort();
    let api_target = fixture.root.join("api-server/target");
    assert_eq!(
        kids,
        vec![
            (
                "debug".to_string(),
                support::disk_of(&api_target.join("debug/deps/lib.rlib"))
            ),
            (
                "release".to_string(),
                support::disk_of(&api_target.join("release/app"))
            ),
        ]
    );
}

#[test]
fn freshly_written_targets_are_busy() {
    let dir = tempfile::tempdir().unwrap();
    support::build_rust_fixture(dir.path());
    let mut req = ScanRequest::rust_targets(vec![dir.path().to_path_buf()]);
    req.target_env = env_none();
    let arena = run_scan(req);
    assert!(arena
        .iter()
        .filter(|n| n.kind == Kind::Target)
        .all(|n| n.state == State::Busy));
}

#[test]
fn rust_scan_resolves_env_target_dir_once() {
    let dir = tempfile::tempdir().unwrap();
    let shared = dir.path().join("shared-target");
    let shared_bytes = support::write_file(&shared.join("a.bin"), 123);
    support::write_file(&dir.path().join("p1/Cargo.toml"), 10);
    support::write_file(&dir.path().join("p2/Cargo.toml"), 10);

    let mut req = ScanRequest::rust_targets(vec![dir.path().to_path_buf()]);
    req.target_env = TargetEnv {
        cargo_target_dir: Some(shared.clone()),
        home: None,
    };
    let arena = run_scan(req);

    // Both projects point at the same target, so it is counted once.
    let targets: Vec<_> = arena.iter().filter(|n| n.kind == Kind::Target).collect();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].size, shared_bytes);
}

#[test]
fn general_scan_rolls_up_directories() {
    let dir = tempfile::tempdir().unwrap();
    let file1 = support::write_file(&dir.path().join("a/b/file1"), 100);
    let file2 = support::write_file(&dir.path().join("a/file2"), 50);
    let file3 = support::write_file(&dir.path().join("c/file3"), 10);

    let arena = run_scan(ScanRequest::general(dir.path().to_path_buf()));
    let by_name = |name: &str| arena.iter().find(|n| n.name == name).unwrap();

    assert_eq!(arena.root().unwrap().size, file1 + file2 + file3);
    assert_eq!(arena.root().unwrap().files, 3);
    assert_eq!(by_name("a").size, file1 + file2);
    assert_eq!(by_name("b").size, file1);
    assert_eq!(by_name("c").size, file3);
    assert_eq!(by_name("b").kind, Kind::Directory);
    // Parent links must agree with the children lists.
    let a = by_name("a");
    let b = by_name("b");
    assert_eq!(b.parent, Some(a.id));
    assert!(a.children.contains(&b.id));
}

#[cfg(not(windows))]
#[test]
fn general_scan_counts_hardlinks_once() {
    hardlink_case();
}

#[cfg(windows)]
#[test]
fn general_scan_counts_hardlinks_once() {
    hardlink_case();
}

fn hardlink_case() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.bin");
    let bytes = support::write_file(&original, 1000);
    if fs::hard_link(&original, dir.path().join("link.bin")).is_err() {
        eprintln!("filesystem does not support hard links; skipping");
        return;
    }
    let arena = run_scan(ScanRequest::general(dir.path().to_path_buf()));
    let root = arena.root().unwrap();
    assert_eq!(root.size, bytes, "hardlinked file must count once");
    assert_eq!(root.files, 1);
}

#[test]
fn scan_can_be_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    support::build_rust_fixture(dir.path());
    let mut req = ScanRequest::rust_targets(vec![dir.path().to_path_buf()]);
    req.target_env = env_none();
    let handle = scan::start(req);
    handle.cancel();
    let mut finished = None;
    for event in handle.events.iter() {
        if let scan::ScanEvent::Finished { cancelled, .. } = event {
            finished = Some(cancelled);
            break;
        }
    }
    handle.join();
    assert_eq!(finished, Some(true));
}

#[test]
fn target_resolution_order() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    support::write_file(&project.join("Cargo.toml"), 1);

    // Default.
    assert_eq!(
        resolve_target_dir(&project, &env_none()),
        project.join("target")
    );

    // Project config, relative to the project.
    fs::create_dir_all(project.join(".cargo")).unwrap();
    fs::write(
        project.join(".cargo/config.toml"),
        "[build]\ntarget-dir = \"out\"\n",
    )
    .unwrap();
    assert_eq!(
        resolve_target_dir(&project, &env_none()),
        project.join("out")
    );

    // Environment beats config.
    let env = TargetEnv {
        cargo_target_dir: Some(dir.path().join("env-target")),
        home: None,
    };
    assert_eq!(
        resolve_target_dir(&project, &env),
        dir.path().join("env-target")
    );
}

#[test]
fn guardrail_refuses_roots_and_home_without_force() {
    assert!(check_scan_root(Path::new(if cfg!(windows) { r"C:\" } else { "/" }), false).is_err());
    assert!(check_scan_root(Path::new(if cfg!(windows) { r"C:\" } else { "/" }), true).is_ok());
    if let Some(home) = tpt_rustvac_core::config::home_dir() {
        assert!(check_scan_root(&home, false).is_err());
    }
    let dir = tempfile::tempdir().unwrap();
    assert!(check_scan_root(dir.path(), false).is_ok());
}

#[test]
fn config_defaults_and_validation() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    assert_eq!(Config::load(&missing).unwrap(), Config::default());

    let bad = dir.path().join("bad.toml");
    fs::write(&bad, "format = \"zip\"\n").unwrap();
    assert!(Config::load(&bad).is_err());

    let good = dir.path().join("good.toml");
    fs::write(&good, "older_than_days = 7\nkeep = [\"~/Code/keep-me\"]\n").unwrap();
    let cfg = Config::load(&good).unwrap();
    assert_eq!(cfg.older_than_days, 7);
    assert_eq!(cfg.busy_grace_seconds, 60);
}

#[test]
fn busy_detection_uses_grace_period() {
    let now = SystemTime::now();
    let grace = Duration::from_secs(60);
    assert!(is_busy(Some(now), now, grace));
    assert!(!is_busy(Some(now - Duration::from_secs(600)), now, grace));
    assert!(!is_busy(None, now, grace), "unknown age never blocks");
}

#[test]
fn archive_round_trip_restores_identical_tree() {
    let work = tempfile::tempdir().unwrap();
    let project = work.path().join("proj");
    let target = project.join("target");
    support::write_file(&project.join("Cargo.toml"), 10);
    support::write_file(&target.join("debug/deps/big.rlib"), 300_000);
    support::write_file(&target.join("debug/small"), 7);
    support::write_file(&target.join("release/app"), 0);
    let vacuum = work.path().join("vacuum");

    let report = archive::pack(&target, &project, &vacuum).unwrap();
    assert_eq!(report.manifest.files, 3);
    assert_eq!(report.manifest.bytes, 300_007);
    assert!(report.archive.exists());
    assert!(
        report.compressed_bytes < 300_007,
        "zstd should shrink repetitive data"
    );

    let listed = archive::list(&vacuum).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].slug, archive::slug_for(&project));

    let restore_into = work.path().join("restored-project");
    let restored = archive::restore(&report.archive, &restore_into).unwrap();
    assert_eq!(restored, restore_into.join("target"));
    assert_eq!(
        fs::read(restored.join("debug/deps/big.rlib"))
            .unwrap()
            .len(),
        300_000
    );
    assert_eq!(fs::read(restored.join("debug/small")).unwrap(), b"xxxxxxx");
    assert!(restored.join("release/app").exists());
    assert!(
        !restored.join(MANIFEST).exists(),
        "manifest must not be extracted"
    );

    // Restoring over an existing target is refused.
    assert!(archive::restore(&report.archive, &restore_into).is_err());
}

#[test]
fn verify_detects_truncated_archive() {
    let work = tempfile::tempdir().unwrap();
    let project = work.path().join("proj");
    let target = project.join("target");
    support::write_file(&target.join("data.bin"), 200_000);
    let vacuum = work.path().join("vacuum");
    let report = archive::pack(&target, &project, &vacuum).unwrap();

    let bytes = fs::read(&report.archive).unwrap();
    fs::write(&report.archive, &bytes[..bytes.len() / 2]).unwrap();
    assert!(archive::verify(&report.archive).is_err());
}

#[test]
fn pack_refuses_archive_inside_target() {
    let work = tempfile::tempdir().unwrap();
    let project = work.path().join("proj");
    let target = project.join("target");
    support::write_file(&target.join("x"), 1);
    assert!(archive::pack(&target, &project, &target.join("vacuum")).is_err());
}

#[test]
fn slug_parse_round_trip_and_list_ignores_strangers() {
    let work = tempfile::tempdir().unwrap();
    let vacuum = work.path().join("vacuum");
    support::write_file(&vacuum.join("notes.txt"), 1);
    support::write_file(&vacuum.join("api-deadbeef-20260101.tar.zst"), 1);
    support::write_file(&vacuum.join("api-deadbeef-20260301.tar.zst"), 1);
    let listed = archive::list(&vacuum).unwrap();
    assert_eq!(listed.len(), 2);
    let latest = archive::latest_for_slug(&vacuum, "api-deadbeef").unwrap();
    assert_eq!(latest.date, "20260301");
}

#[test]
fn long_path_prefixes_on_windows_only() {
    let p = Path::new("relative/x");
    let out = long_path(p);
    if cfg!(windows) {
        assert!(out.to_string_lossy().starts_with(r"\\?\"));
    } else {
        assert_eq!(out, p);
    }
}

#[test]
fn node_ids_match_arena_index() {
    let mut arena = Arena::new();
    let root = arena.push(None, "r".into(), "r".into(), Kind::Root, false);
    let child = arena.push(Some(root), "c".into(), "r/c".into(), Kind::Directory, false);
    assert_eq!(root, NodeId(0));
    assert_eq!(child, NodeId(1));
    assert_eq!(arena.get(root).unwrap().children, vec![child]);
}
