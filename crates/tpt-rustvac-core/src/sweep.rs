//! One removal plan across everything regenerable: targets built by a compiler that is no
//! longer installed, `incremental/` folders in the remaining targets, and the rustup and cargo
//! caches. `rustvac sweep` runs it and `rustvac check` only reports it, so the check never
//! recommends something a sweep would refuse.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::caches::{self, CacheKind, CleanSpec};
use crate::config::RemovePolicy;
use crate::error::Result;
use crate::filter::Filter;
use crate::model::{Arena, Kind, State};
use crate::prune::{self, PruneSpec};
use crate::safety::{is_kept, remove_group_with_policy, sync_managed, Removal};
use crate::target::{built_with, is_stale, RustcInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepKind {
    StaleTarget,
    Incremental,
    StaleDeps,
    RustupTmp,
    RustupDownloads,
    RustupUpdateHashes,
    CargoRegistryCache,
    CargoRegistrySrc,
    CargoGitCheckouts,
}

impl SweepKind {
    pub fn label(self) -> &'static str {
        match self {
            SweepKind::StaleTarget => "targets from an uninstalled compiler",
            SweepKind::Incremental => "incremental build caches",
            SweepKind::StaleDeps => "build output for old dependency versions",
            SweepKind::RustupTmp => "rustup tmp",
            SweepKind::RustupDownloads => "rustup downloads",
            SweepKind::RustupUpdateHashes => "rustup update hashes",
            SweepKind::CargoRegistryCache => "cargo registry cache",
            SweepKind::CargoRegistrySrc => "cargo registry src",
            SweepKind::CargoGitCheckouts => "cargo git checkouts",
        }
    }
}

impl From<CacheKind> for Option<SweepKind> {
    fn from(k: CacheKind) -> Self {
        match k {
            CacheKind::RustupTmp => Some(SweepKind::RustupTmp),
            CacheKind::RustupDownloads => Some(SweepKind::RustupDownloads),
            CacheKind::RustupUpdateHashes => Some(SweepKind::RustupUpdateHashes),
            CacheKind::CargoRegistryCache => Some(SweepKind::CargoRegistryCache),
            CacheKind::CargoRegistrySrc => Some(SweepKind::CargoRegistrySrc),
            CacheKind::CargoGitCheckouts => Some(SweepKind::CargoGitCheckouts),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SweepItem {
    pub kind: SweepKind,
    pub path: PathBuf,
    /// Everything removed for this item. Most items are one folder, but a stale dependency
    /// build is several files.
    pub members: Vec<PathBuf>,
    pub size: u64,
    pub newest: Option<SystemTime>,
}

#[derive(Debug, Clone, Default)]
pub struct SweepPlan {
    pub items: Vec<SweepItem>,
    /// Selected but left alone: busy, kept, or sync-managed, with the reason.
    pub skipped: Vec<(PathBuf, String)>,
}

impl SweepPlan {
    pub fn total_size(&self) -> u64 {
        self.items.iter().map(|i| i.size).sum()
    }

    /// Bytes and item count per kind, in the order the kinds are declared.
    pub fn by_kind(&self) -> Vec<(SweepKind, u64, usize)> {
        use SweepKind::*;
        [
            StaleTarget,
            Incremental,
            StaleDeps,
            RustupTmp,
            RustupDownloads,
            RustupUpdateHashes,
            CargoRegistryCache,
            CargoRegistrySrc,
            CargoGitCheckouts,
        ]
        .into_iter()
        .filter_map(|k| {
            let of_kind: Vec<_> = self.items.iter().filter(|i| i.kind == k).collect();
            (!of_kind.is_empty()).then(|| (k, of_kind.iter().map(|i| i.size).sum(), of_kind.len()))
        })
        .collect()
    }
}

pub struct SweepInputs<'a> {
    /// Result of a Rust-targets scan.
    pub arena: &'a Arena,
    pub keep: &'a [PathBuf],
    /// Age rule for targets and their incremental folders.
    pub filter: &'a Filter,
    /// Compilers that are installed now. Empty disables stale-target detection, because an
    /// empty list would make every target look stale.
    pub installed: &'a [RustcInfo],
    pub rustup_home: Option<&'a Path>,
    pub cargo_home: Option<&'a Path>,
    pub busy_grace: Duration,
    pub now: SystemTime,
}

/// Builds the plan. Reads the disk to size things but changes nothing.
pub fn plan(inputs: &SweepInputs) -> SweepPlan {
    let mut plan = SweepPlan::default();
    for node in inputs.arena.iter().filter(|n| n.kind == Kind::Target) {
        match &node.state {
            State::Done => {}
            State::Busy => {
                plan.skipped
                    .push((node.path.clone(), "busy: modified recently".into()));
                continue;
            }
            _ => continue,
        }
        if is_kept(&node.path, inputs.keep) {
            plan.skipped
                .push((node.path.clone(), "kept by the config".into()));
            continue;
        }
        if !inputs.filter.accepts(node, inputs.now) {
            continue;
        }
        if let Some(reason) = sync_managed(&node.path) {
            plan.skipped.push((node.path.clone(), reason));
            continue;
        }
        let built = built_with(&node.path);
        if !inputs.installed.is_empty() && is_stale(built.as_ref(), inputs.installed) {
            plan.items.push(SweepItem {
                kind: SweepKind::StaleTarget,
                path: node.path.clone(),
                members: vec![node.path.clone()],
                size: node.size,
                newest: node.newest,
            });
            // The whole target goes, so its incremental folders are not listed again.
            continue;
        }
        for item in prune::plan(&node.path, &PruneSpec::Incremental) {
            if item.size == 0 {
                continue;
            }
            plan.items.push(SweepItem {
                kind: SweepKind::Incremental,
                members: vec![item.path.clone()],
                path: item.path,
                size: item.size,
                newest: item.newest,
            });
        }
        // Build output for dependency versions the project's lockfile no longer names. Needs
        // the project's Cargo.lock, so a target without a usable one is reported and skipped.
        let project = node
            .parent
            .and_then(|p| inputs.arena.get(p))
            .map(|p| p.path.clone());
        let live = project
            .ok_or_else(|| "no project folder".to_string())
            .and_then(|p| prune::live_crate_dirs(&p, &node.path));
        match live {
            Ok(live) => {
                for item in prune::plan_stale_deps(&node.path, &live) {
                    if item.size == 0 {
                        continue;
                    }
                    plan.items.push(SweepItem {
                        kind: SweepKind::StaleDeps,
                        path: item.path,
                        members: item.members,
                        size: item.size,
                        newest: item.newest,
                    });
                }
            }
            Err(reason) => plan.skipped.push((
                node.path.clone(),
                format!("old dependency builds not checked: {reason}"),
            )),
        }
    }

    let items = caches::discover(inputs.rustup_home, inputs.cargo_home);
    let spec = CleanSpec {
        kinds: vec![
            CacheKind::RustupTmp,
            CacheKind::RustupDownloads,
            CacheKind::RustupUpdateHashes,
            CacheKind::CargoRegistryCache,
            CacheKind::CargoRegistrySrc,
            CacheKind::CargoGitCheckouts,
        ],
        older_than: None,
        busy_grace: inputs.busy_grace,
    };
    let clean = caches::plan_clean(&items, &spec, inputs.now);
    for e in clean.entries {
        if let Some(kind) = Option::<SweepKind>::from(e.kind) {
            plan.items.push(SweepItem {
                kind,
                members: vec![e.path.clone()],
                path: e.path,
                size: e.size,
                newest: e.newest,
            });
        }
    }
    plan.skipped.extend(clean.skipped);
    plan
}

/// Removes every planned item under the policy. One failure does not stop the rest.
pub fn execute(plan: &SweepPlan, policy: &RemovePolicy) -> Vec<(PathBuf, Result<Removal>)> {
    plan.items
        .iter()
        .map(|i| {
            (
                i.path.clone(),
                remove_group_with_policy(&i.members, i.size, policy),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    fn info_json(release: &str) -> String {
        format!(r#"{{"outputs":{{"1":{{"stdout":"release: {release}\n"}}}}}}"#,)
    }

    /// Two projects: `old` was built by 1.80.0 (not installed), `new` by 1.99.1 (installed).
    fn arena(root: &Path) -> Arena {
        let mut arena = Arena::new();
        let r = arena.push(None, "root".into(), root.to_path_buf(), Kind::Root, false);
        for (name, release) in [("old", "1.80.0"), ("new", "1.99.1")] {
            let project = root.join(name);
            let target = project.join("target");
            write(&target.join("debug/deps/x"), 1000);
            write(&target.join("debug/incremental/a/f"), 500);
            fs::write(target.join(".rustc_info.json"), info_json(release)).unwrap();
            let p = arena.push(Some(r), name.into(), project, Kind::Project, false);
            let t = arena.push(Some(p), "target".into(), target, Kind::Target, true);
            let node = arena.get_mut(t).unwrap();
            node.size = 1500;
            node.state = State::Done;
        }
        arena
    }

    fn installed() -> Vec<RustcInfo> {
        vec![crate::target::parse_rustc_vv("release: 1.99.1\n").unwrap()]
    }

    fn inputs<'a>(
        arena: &'a Arena,
        installed: &'a [RustcInfo],
        keep: &'a [PathBuf],
        filter: &'a Filter,
    ) -> SweepInputs<'a> {
        SweepInputs {
            arena,
            keep,
            filter,
            installed,
            rustup_home: None,
            cargo_home: None,
            busy_grace: Duration::from_secs(60),
            now: SystemTime::now(),
        }
    }

    #[test]
    fn stale_target_goes_whole_and_current_target_loses_incremental() {
        let dir = tempfile::tempdir().unwrap();
        let arena = arena(dir.path());
        let installed = installed();
        let plan = plan(&inputs(&arena, &installed, &[], &Filter::default()));
        let stale: Vec<_> = plan
            .items
            .iter()
            .filter(|i| i.kind == SweepKind::StaleTarget)
            .collect();
        assert_eq!(stale.len(), 1);
        assert!(stale[0].path.ends_with("old/target") || stale[0].path.ends_with("old\\target"));
        let inc: Vec<_> = plan
            .items
            .iter()
            .filter(|i| i.kind == SweepKind::Incremental)
            .collect();
        assert_eq!(inc.len(), 1);
        assert!(inc[0].path.starts_with(dir.path().join("new")));
        assert_eq!(plan.items.len(), 2);
    }

    #[test]
    fn no_installed_compilers_means_nothing_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let arena = arena(dir.path());
        let plan = plan(&inputs(&arena, &[], &[], &Filter::default()));
        assert!(plan.items.iter().all(|i| i.kind == SweepKind::Incremental));
        assert_eq!(plan.items.len(), 2);
    }

    #[test]
    fn kept_and_busy_targets_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let mut arena = arena(dir.path());
        let keep = vec![dir.path().join("old")];
        // Mark "new" busy.
        let busy_id = arena
            .iter()
            .find(|n| n.kind == Kind::Target && n.path.starts_with(dir.path().join("new")))
            .unwrap()
            .id;
        arena.get_mut(busy_id).unwrap().state = State::Busy;
        let installed = installed();
        let plan = plan(&inputs(&arena, &installed, &keep, &Filter::default()));
        assert!(plan.items.is_empty());
        assert_eq!(plan.skipped.len(), 2);
    }

    #[test]
    fn stale_dependency_builds_are_listed_for_targets_that_are_not_stale() {
        let dir = tempfile::tempdir().unwrap();
        let arena = arena(dir.path());
        let installed = installed();
        // `new` has a lockfile that names serde 1.0.2. Its build still has serde 1.0.1.
        fs::write(
            dir.path().join("new/Cargo.lock"),
            "version = 4\n[[package]]\nname = \"serde\"\nversion = \"1.0.2\"\n",
        )
        .unwrap();
        let deps = dir.path().join("new/target/debug/deps");
        fs::create_dir_all(dir.path().join("new/target/debug/.fingerprint/serde-aaaa")).unwrap();
        fs::write(
            deps.join("serde-aaaa.d"),
            "x : /home/me/.cargo/registry/src/index.crates.io-6f17/serde-1.0.1/src/lib.rs\n",
        )
        .unwrap();
        write(&deps.join("libserde-aaaa.rlib"), 800);
        let plan = plan(&inputs(&arena, &installed, &[], &Filter::default()));
        let stale: Vec<_> = plan
            .items
            .iter()
            .filter(|i| i.kind == SweepKind::StaleDeps)
            .collect();
        assert_eq!(stale.len(), 1);
        assert!(stale[0].path.ends_with("serde-aaaa.d"));
        assert_eq!(stale[0].members.len(), 3);
    }

    #[test]
    fn execute_deletes_the_planned_paths() {
        let dir = tempfile::tempdir().unwrap();
        let arena = arena(dir.path());
        let installed = installed();
        let plan = plan(&inputs(&arena, &installed, &[], &Filter::default()));
        let policy = RemovePolicy {
            mode: crate::config::RemoveMode::Permanent,
            trash_max: 0,
        };
        for (_, r) in execute(&plan, &policy) {
            r.unwrap();
        }
        assert!(!dir.path().join("old/target").exists());
        assert!(!dir.path().join("new/target/debug/incremental").exists());
        assert!(dir.path().join("new/target/debug/deps/x").exists());
    }
}
