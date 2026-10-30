//! Invariants that must hold for every input, not only the hand-picked fixtures in the other
//! tests. Small input spaces are enumerated in full. Sizes, names and tree shapes are generated
//! with proptest. Nothing here touches the real rustup or cargo homes.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use proptest::prelude::*;
use tpt_rustvac_core::archive;
use tpt_rustvac_core::caches::{self, CacheEntry, CacheItem, CacheKind, CleanSpec, Tier};
use tpt_rustvac_core::check::{self, CheckInput};
use tpt_rustvac_core::config::{RemoveMode, RemovePolicy};
use tpt_rustvac_core::disk;
use tpt_rustvac_core::filter::Filter;
use tpt_rustvac_core::model::{Arena, Kind, State};
use tpt_rustvac_core::safety::{choose_removal, Removal};
use tpt_rustvac_core::scan::{self, ScanEvent};
use tpt_rustvac_core::sweep::{self, SweepInputs, SweepKind, SweepPlan};
use tpt_rustvac_core::target::{parse_rustc_vv, RustcInfo};
use tpt_rustvac_core::ScanRequest;

const ALL_KINDS: [CacheKind; 9] = [
    CacheKind::RustupTmp,
    CacheKind::RustupDownloads,
    CacheKind::RustupUpdateHashes,
    CacheKind::CargoRegistryCache,
    CacheKind::CargoRegistrySrc,
    CacheKind::CargoGitCheckouts,
    CacheKind::Toolchain,
    CacheKind::CargoRegistryIndex,
    CacheKind::CargoGitDb,
];

fn installed() -> Vec<RustcInfo> {
    vec![parse_rustc_vv("release: 1.99.1\ncommit-hash: aaa\n").unwrap()]
}

fn cache_item(kind: CacheKind, size: u64, entries: Vec<CacheEntry>) -> CacheItem {
    CacheItem {
        kind,
        path: PathBuf::from(format!("/cache/{kind:?}")),
        name: String::new(),
        size,
        files: entries.len() as u64,
        newest: None,
        entries,
    }
}

fn scan_projects(projects: &Path) -> Arena {
    let mut req = ScanRequest::rust_targets(vec![projects.to_path_buf()]);
    // The fixtures were written a moment ago, so the busy guard is off for the scan.
    req.busy_grace = Duration::ZERO;
    let handle = scan::start(req);
    let mut arena = Arena::new();
    for event in handle.events.iter() {
        arena.apply(&event);
        if let ScanEvent::Finished { .. } = event {
            break;
        }
    }
    handle.join();
    arena
}

// --- Cache planner -----------------------------------------------------------------------

/// Every kind, every age case, and both age rules. A planned entry must be regenerable, must
/// not be busy, and with an age rule must have a known age that is old enough.
#[test]
fn cache_planner_is_exact_over_kind_age_and_busy() {
    let now = SystemTime::now();
    let day = Duration::from_secs(86_400);
    let busy_age = Duration::from_secs(10);
    // `None` is an unknown age. 10 seconds is inside the busy grace. 10 days is old enough.
    let ages: [Option<Duration>; 3] = [None, Some(busy_age), Some(10 * day)];
    let age_rules = [None, Some(day)];
    for kind in ALL_KINDS {
        for age in ages {
            for older_than in age_rules {
                let entry = CacheEntry {
                    path: PathBuf::from(format!("/cache/{kind:?}/entry")),
                    size: 10,
                    files: 1,
                    newest: age.map(|a| now - a),
                };
                let items = [cache_item(kind, 10, vec![entry])];
                let spec = CleanSpec {
                    kinds: ALL_KINDS.to_vec(),
                    older_than,
                    busy_grace: Duration::from_secs(60),
                };
                let planned = caches::plan_clean(&items, &spec, now).entries.len() == 1;

                let busy = age == Some(busy_age);
                let old_enough = age.is_some_and(|a| a >= day);
                let expected = kind.tier() == Tier::Regenerable
                    && !busy
                    && (older_than.is_none() || old_enough);
                assert_eq!(
                    planned, expected,
                    "kind {kind:?}, age {age:?}, older_than {older_than:?}"
                );
            }
        }
    }
}

// --- Sweep planner -----------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]

    /// Each project is `(stale, kept, busy)`. A stale target was built by a compiler that is not
    /// installed. For every shape: nothing kept or busy is planned, only known roots are touched,
    /// the total is the sum of the items, and every stale target that is neither kept nor busy
    /// is planned as a whole.
    #[test]
    fn sweep_plan_respects_roots_keep_busy_and_stale(
        shape in prop::collection::vec((any::<bool>(), any::<bool>(), any::<bool>()), 1..=4),
    ) {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        for (i, (stale, _, _)) in shape.iter().enumerate() {
            let project = projects.join(format!("p{i}"));
            support::write_file(&project.join("Cargo.toml"), 40);
            let target = project.join("target");
            support::write_file(&target.join("debug/deps/lib.rlib"), 4000);
            support::write_incremental(&target, "debug", 1500);
            let (release, commit) = if *stale { ("1.80.0", "bbb") } else { ("1.99.1", "aaa") };
            support::write_rustc_info(&target, release, commit);
        }
        let homes = support::build_homes(&dir.path().join("homes"));

        let mut arena = scan_projects(&projects);
        let mut kept = Vec::new();
        let targets: Vec<(tpt_rustvac_core::NodeId, PathBuf)> = arena
            .iter()
            .filter(|n| n.kind == Kind::Target)
            .map(|n| (n.id, n.path.clone()))
            .collect();
        for (id, path) in targets {
            let i = (0..shape.len())
                .find(|i| path.starts_with(projects.join(format!("p{i}"))))
                .unwrap();
            let (_, keep, busy) = shape[i];
            if busy {
                arena.get_mut(id).unwrap().state = State::Busy;
            }
            if keep {
                kept.push(path);
            }
        }

        let installed = installed();
        let plan: SweepPlan = sweep::plan(&SweepInputs {
            arena: &arena,
            keep: &kept,
            filter: &Filter::default(),
            installed: &installed,
            rustup_home: Some(&homes.rustup),
            cargo_home: Some(&homes.cargo),
            busy_grace: Duration::ZERO,
            now: SystemTime::now(),
        });

        let roots = [projects.clone(), homes.rustup.clone(), homes.cargo.clone()];
        for item in &plan.items {
            prop_assert!(
                roots.iter().any(|r| item.path.starts_with(r)),
                "{:?} is outside the scan roots and cache homes", item.path
            );
            for member in &item.members {
                prop_assert!(roots.iter().any(|r| member.starts_with(r)));
            }
        }
        prop_assert_eq!(
            plan.total_size(),
            plan.items.iter().map(|i| i.size).sum::<u64>()
        );

        for (i, (stale, keep, busy)) in shape.iter().enumerate() {
            let target = projects.join(format!("p{i}")).join("target");
            if *keep || *busy {
                prop_assert!(
                    plan.items.iter().all(|it| !it.path.starts_with(&target)),
                    "a kept or busy target was planned: {:?}", target
                );
                prop_assert!(
                    plan.skipped.iter().any(|(p, _)| p.starts_with(&target)),
                    "a kept or busy target was not reported as skipped: {:?}", target
                );
            } else if *stale {
                prop_assert!(
                    plan.items
                        .iter()
                        .any(|it| it.kind == SweepKind::StaleTarget && it.path.starts_with(&target)),
                    "a stale target was not planned: {:?}", target
                );
            }
        }
    }
}

// --- Removal policy ----------------------------------------------------------------------

proptest! {
    /// Under `auto`, once an item is deleted permanently, every larger item is too. The fixed
    /// modes ignore size. The threshold itself is trashed, one byte more is deleted.
    #[test]
    fn auto_removal_is_monotonic_in_size(
        a in any::<u64>(),
        b in any::<u64>(),
        trash_max in any::<u64>(),
    ) {
        let (small, large) = if a <= b { (a, b) } else { (b, a) };
        let auto = RemovePolicy { mode: RemoveMode::Auto, trash_max };
        if choose_removal(small, &auto) == Removal::Deleted {
            prop_assert_eq!(choose_removal(large, &auto), Removal::Deleted);
        }
        prop_assert_eq!(choose_removal(trash_max, &auto), Removal::Trashed);
        if let Some(over) = trash_max.checked_add(1) {
            prop_assert_eq!(choose_removal(over, &auto), Removal::Deleted);
        }

        for mode in [RemoveMode::Trash, RemoveMode::Permanent] {
            let policy = RemovePolicy { mode, trash_max };
            prop_assert_eq!(choose_removal(small, &policy), choose_removal(large, &policy));
        }
    }
}

// --- Disk report and alert thresholds ----------------------------------------------------

proptest! {
    /// `regenerable_bytes` is the sum of regenerable rustup and cargo items. A toolchain is
    /// never counted, even though it is not managed by a tier.
    #[test]
    fn disk_report_regenerable_bytes_is_the_sum_of_regenerable_items(
        items in prop::collection::vec((0..ALL_KINDS.len(), 0u64..1_000_000), 0..20),
    ) {
        let items: Vec<CacheItem> = items
            .into_iter()
            .map(|(k, size)| cache_item(ALL_KINDS[k], size, Vec::new()))
            .collect();
        let report = disk::build(&items, Vec::new(), None);
        let expected: u64 = items
            .iter()
            .filter(|i| i.tier() == Tier::Regenerable && i.kind != CacheKind::Toolchain)
            .map(|i| i.size)
            .sum();
        prop_assert_eq!(report.regenerable_bytes(), expected);
    }

    /// More free space never raises the low-space alert.
    #[test]
    fn low_space_alert_is_monotonic_in_free_space(
        x in any::<u64>(),
        y in any::<u64>(),
        warn in any::<u64>(),
    ) {
        let (less, more) = if x <= y { (x, y) } else { (y, x) };
        let low = |free: u64| {
            let plan = SweepPlan::default();
            check::build(&CheckInput {
                plan: &plan,
                drive: Path::new("/"),
                space: Some((free, u64::MAX)),
                compressible_bytes: 0,
                wsl: &[],
                warn_below: Some(warn),
                now: SystemTime::UNIX_EPOCH,
                top_n: 1,
            })
            .low_space
        };
        if low(more) {
            prop_assert!(low(less));
        }
    }
}

// --- Archive ----------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(8))]

    /// Pack, verify and restore give back the same manifest and the same bytes for every file.
    #[test]
    fn pack_verify_restore_round_trip(sizes in prop::collection::vec(0usize..20_000, 1..=5)) {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let target = project.join("target");
        for (i, size) in sizes.iter().enumerate() {
            let path = target.join("debug/deps").join(format!("f{i}.bin"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let bytes: Vec<u8> = (0..*size).map(|b| (b as u8).wrapping_mul(31).wrapping_add(i as u8)).collect();
            fs::write(&path, bytes).unwrap();
        }

        let archive_dir = dir.path().join("archive");
        let report = archive::pack(&target, &project, &archive_dir).unwrap();
        prop_assert_eq!(report.manifest.files, sizes.len() as u64);
        prop_assert!(archive::verify(&report.archive).unwrap() == report.manifest);

        let into = dir.path().join("restored");
        let restored = archive::restore(&report.archive, &into).unwrap();
        prop_assert_eq!(&restored, &into.join("target"));
        for i in 0..sizes.len() {
            let rel = Path::new("debug/deps").join(format!("f{i}.bin"));
            prop_assert_eq!(
                fs::read(target.join(&rel)).unwrap(),
                fs::read(restored.join(&rel)).unwrap()
            );
        }
    }
}

/// A damaged archive must not verify. `pack` runs this check before it returns, and
/// `compress_and_vacuum` only trashes the original after `pack` returns.
#[test]
fn a_truncated_archive_fails_verification() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    let target = project.join("target");
    fs::create_dir_all(target.join("debug")).unwrap();
    fs::write(target.join("debug/big.bin"), vec![7u8; 50_000]).unwrap();

    let report = archive::pack(&target, &project, &dir.path().join("archive")).unwrap();
    let bytes = fs::read(&report.archive).unwrap();
    fs::write(&report.archive, &bytes[..bytes.len() / 2]).unwrap();
    assert!(archive::verify(&report.archive).is_err());
}
