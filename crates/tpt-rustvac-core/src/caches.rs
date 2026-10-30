//! Rustup and Cargo scratch folders: what they hold, and how to clear the regenerable part.
//!
//! Tiers:
//! - Regenerable: rustup `tmp`, `downloads` and `update-hashes`, cargo `registry/cache`, `registry/src` and
//!   `git/checkouts`. Removing them only costs a re-download or re-extract.
//! - Managed: `toolchains/<name>`. Removed only through rustup, never by deleting folders.
//! - Manual: `registry/index` and `git/db`. They need the network to rebuild, so they are
//!   listed but never part of a default sweep.
//!
//! Callers pass the rustup and cargo homes explicitly, so tests never touch the real ones.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::config::RemovePolicy;
use crate::error::Result;
use crate::identity::disk_bytes;
use crate::safety::{remove_with_policy, sync_managed, Removal};
use crate::walk::{is_busy, measure_tree, Agg, WalkOpts};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Regenerable,
    Managed,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheKind {
    RustupTmp,
    RustupDownloads,
    RustupUpdateHashes,
    CargoRegistryCache,
    CargoRegistrySrc,
    CargoGitCheckouts,
    Toolchain,
    CargoRegistryIndex,
    CargoGitDb,
}

impl CacheKind {
    pub fn tier(self) -> Tier {
        match self {
            CacheKind::Toolchain => Tier::Managed,
            CacheKind::CargoRegistryIndex | CacheKind::CargoGitDb => Tier::Manual,
            _ => Tier::Regenerable,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CacheKind::RustupTmp => "rustup tmp",
            CacheKind::RustupDownloads => "rustup downloads",
            CacheKind::RustupUpdateHashes => "rustup update hashes",
            CacheKind::CargoRegistryCache => "cargo registry cache",
            CacheKind::CargoRegistrySrc => "cargo registry src",
            CacheKind::CargoGitCheckouts => "cargo git checkouts",
            CacheKind::Toolchain => "toolchain",
            CacheKind::CargoRegistryIndex => "cargo registry index",
            CacheKind::CargoGitDb => "cargo git db",
        }
    }
}

/// One direct child of a cache folder. This is the unit that gets removed, so an
/// install in progress (a recently modified entry) can be skipped on its own.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub path: PathBuf,
    pub size: u64,
    pub files: u64,
    pub newest: Option<SystemTime>,
}

#[derive(Debug, Clone)]
pub struct CacheItem {
    pub kind: CacheKind,
    /// The folder for cache kinds, or `toolchains/<name>` for a toolchain.
    pub path: PathBuf,
    /// Toolchain name, otherwise empty.
    pub name: String,
    pub size: u64,
    pub files: u64,
    pub newest: Option<SystemTime>,
    /// Direct children of the folder. Empty for toolchains.
    pub entries: Vec<CacheEntry>,
}

impl CacheItem {
    pub fn tier(&self) -> Tier {
        self.kind.tier()
    }
}

/// Finds and sizes every cache item. A home that is `None` or missing yields nothing for it.
pub fn discover(rustup_home: Option<&Path>, cargo_home: Option<&Path>) -> Vec<CacheItem> {
    let mut items = Vec::new();
    if let Some(rustup) = rustup_home {
        items.extend(folder(CacheKind::RustupTmp, &rustup.join("tmp")));
        items.extend(folder(
            CacheKind::RustupDownloads,
            &rustup.join("downloads"),
        ));
        items.extend(folder(
            CacheKind::RustupUpdateHashes,
            &rustup.join("update-hashes"),
        ));
        for tc in crate::toolchains::installed(rustup) {
            let agg = measure_dir(&tc.path);
            items.push(CacheItem {
                kind: CacheKind::Toolchain,
                path: tc.path,
                name: tc.name,
                size: agg.size,
                files: agg.files,
                newest: agg.newest,
                entries: Vec::new(),
            });
        }
    }
    if let Some(cargo) = cargo_home {
        let reg = cargo.join("registry");
        let git = cargo.join("git");
        items.extend(folder(CacheKind::CargoRegistryCache, &reg.join("cache")));
        items.extend(folder(CacheKind::CargoRegistrySrc, &reg.join("src")));
        items.extend(folder(CacheKind::CargoGitCheckouts, &git.join("checkouts")));
        items.extend(folder(CacheKind::CargoRegistryIndex, &reg.join("index")));
        items.extend(folder(CacheKind::CargoGitDb, &git.join("db")));
    }
    items
}

fn folder(kind: CacheKind, path: &Path) -> Option<CacheItem> {
    let read = std::fs::read_dir(path).ok()?;
    let mut entries: Vec<CacheEntry> = read
        .filter_map(|e| e.ok())
        .map(|e| {
            let agg = measure_entry(&e.path());
            CacheEntry {
                path: e.path(),
                size: agg.size,
                files: agg.files,
                newest: agg.newest,
            }
        })
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut total = Agg::default();
    for e in &entries {
        total.add(&Agg {
            size: e.size,
            files: e.files,
            newest: e.newest,
        });
    }
    Some(CacheItem {
        kind,
        path: path.to_path_buf(),
        name: String::new(),
        size: total.size,
        files: total.files,
        newest: total.newest,
        entries,
    })
}

/// A file or a folder tree, sized on disk.
fn measure_entry(path: &Path) -> Agg {
    match std::fs::symlink_metadata(path) {
        Ok(md) if md.is_dir() => measure_dir(path),
        Ok(md) => Agg {
            size: disk_bytes(path, &md),
            files: 1,
            newest: md.modified().ok(),
        },
        Err(_) => Agg::default(),
    }
}

fn measure_dir(path: &Path) -> Agg {
    let opts = WalkOpts {
        exclude: Vec::new(),
        cross_device: false,
        follow_links: false,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let mut agg = measure_tree(path, &opts).total(path);
    // The folder's own mtime counts: a fresh empty folder is still recent activity.
    if let Ok(t) = std::fs::metadata(path).and_then(|m| m.modified()) {
        agg.newest = Some(agg.newest.map_or(t, |n| n.max(t)));
    }
    agg
}

/// Which kinds to clear, and how old an entry must be.
#[derive(Debug, Clone, Default)]
pub struct CleanSpec {
    pub kinds: Vec<CacheKind>,
    /// Only entries whose newest file is at least this old. Unknown age never matches.
    pub older_than: Option<Duration>,
    pub busy_grace: Duration,
}

#[derive(Debug, Clone)]
pub struct PlanEntry {
    pub kind: CacheKind,
    pub path: PathBuf,
    pub size: u64,
    pub files: u64,
    pub newest: Option<SystemTime>,
}

#[derive(Debug, Clone, Default)]
pub struct CleanPlan {
    pub entries: Vec<PlanEntry>,
    /// Entries that were selected but left alone, with the reason.
    pub skipped: Vec<(PathBuf, String)>,
}

impl CleanPlan {
    pub fn total_size(&self) -> u64 {
        self.entries.iter().map(|e| e.size).sum()
    }
}

/// Builds the removal plan. Only regenerable kinds can be planned: a toolchain or a manual
/// kind in `spec.kinds` is ignored here, because those have their own commands.
pub fn plan_clean(items: &[CacheItem], spec: &CleanSpec, now: SystemTime) -> CleanPlan {
    let mut plan = CleanPlan::default();
    for item in items {
        if item.tier() != Tier::Regenerable || !spec.kinds.contains(&item.kind) {
            continue;
        }
        for entry in &item.entries {
            if is_busy(entry.newest, now, spec.busy_grace) {
                plan.skipped.push((
                    entry.path.clone(),
                    format!(
                        "modified within the last {}s, so it may be in use",
                        spec.busy_grace.as_secs()
                    ),
                ));
                continue;
            }
            if let Some(older) = spec.older_than {
                let old_enough = entry
                    .newest
                    .and_then(|t| now.duration_since(t).ok())
                    .is_some_and(|age| age >= older);
                if !old_enough {
                    continue;
                }
            }
            if let Some(reason) = sync_managed(&entry.path) {
                plan.skipped.push((entry.path.clone(), reason));
                continue;
            }
            plan.entries.push(PlanEntry {
                kind: item.kind,
                path: entry.path.clone(),
                size: entry.size,
                files: entry.files,
                newest: entry.newest,
            });
        }
    }
    plan
}

/// Removes each planned entry under the policy. One failure does not stop the rest.
pub fn execute(plan: &CleanPlan, policy: &RemovePolicy) -> Vec<(PathBuf, Result<Removal>)> {
    plan.entries
        .iter()
        .map(|e| (e.path.clone(), remove_with_policy(&e.path, e.size, policy)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RemoveMode;
    use std::fs;

    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    fn homes() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let rustup = dir.path().join("rustup");
        let cargo = dir.path().join("cargo");
        write(&rustup.join("tmp/partial-install/a"), 1000);
        write(&rustup.join("downloads/x.partial"), 500);
        write(&rustup.join("update-hashes/stable-x"), 64);
        write(&rustup.join("toolchains/stable-x/bin/rustc"), 100);
        write(&cargo.join("registry/cache/index-1/foo.crate"), 2000);
        write(&cargo.join("registry/src/index-1/foo-1.0/lib.rs"), 3000);
        write(&cargo.join("registry/index/index-1/.cache/f"), 400);
        write(&cargo.join("git/checkouts/repo-1/abc/f"), 700);
        write(&cargo.join("git/db/repo-1/pack"), 800);
        (dir, rustup, cargo)
    }

    fn all_regenerable() -> Vec<CacheKind> {
        vec![
            CacheKind::RustupTmp,
            CacheKind::RustupDownloads,
            CacheKind::RustupUpdateHashes,
            CacheKind::CargoRegistryCache,
            CacheKind::CargoRegistrySrc,
            CacheKind::CargoGitCheckouts,
        ]
    }

    #[test]
    fn discovers_every_kind() {
        let (_dir, rustup, cargo) = homes();
        let items = discover(Some(&rustup), Some(&cargo));
        let has = |k: CacheKind| items.iter().any(|i| i.kind == k && i.size > 0);
        assert!(has(CacheKind::RustupTmp));
        assert!(has(CacheKind::Toolchain));
        assert!(has(CacheKind::CargoGitDb));
        assert!(has(CacheKind::CargoRegistryIndex));
        let tc = items
            .iter()
            .find(|i| i.kind == CacheKind::Toolchain)
            .unwrap();
        assert_eq!(tc.name, "stable-x");
        assert!(discover(None, None).is_empty());
    }

    #[test]
    fn plan_takes_regenerable_only() {
        let (_dir, rustup, cargo) = homes();
        let items = discover(Some(&rustup), Some(&cargo));
        let mut kinds = all_regenerable();
        // Asking for managed and manual kinds does nothing.
        kinds.extend([
            CacheKind::Toolchain,
            CacheKind::CargoGitDb,
            CacheKind::CargoRegistryIndex,
        ]);
        let spec = CleanSpec {
            kinds,
            older_than: None,
            busy_grace: Duration::from_secs(60),
        };
        // Everything was just written, so look an hour ahead to get past the busy check.
        let later = SystemTime::now() + Duration::from_secs(3600);
        let plan = plan_clean(&items, &spec, later);
        assert!(plan.skipped.is_empty());
        let kinds: Vec<_> = plan.entries.iter().map(|e| e.kind).collect();
        assert_eq!(kinds.len(), 6);
        assert!(!kinds.contains(&CacheKind::Toolchain));
        assert!(!kinds.contains(&CacheKind::CargoGitDb));
        assert!(plan.total_size() > 0);
    }

    #[test]
    fn recent_entries_are_skipped_as_busy() {
        let (_dir, rustup, cargo) = homes();
        let items = discover(Some(&rustup), Some(&cargo));
        let spec = CleanSpec {
            kinds: all_regenerable(),
            older_than: None,
            busy_grace: Duration::from_secs(60),
        };
        let plan = plan_clean(&items, &spec, SystemTime::now());
        assert!(plan.entries.is_empty());
        assert_eq!(plan.skipped.len(), 6);
    }

    #[test]
    fn older_than_leaves_young_entries_alone() {
        let (_dir, rustup, cargo) = homes();
        let items = discover(Some(&rustup), Some(&cargo));
        let spec = CleanSpec {
            kinds: all_regenerable(),
            older_than: Some(Duration::from_secs(7200)),
            busy_grace: Duration::from_secs(60),
        };
        let later = SystemTime::now() + Duration::from_secs(3600);
        let plan = plan_clean(&items, &spec, later);
        assert!(plan.entries.is_empty());
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn execute_removes_only_the_planned_entries() {
        let (_dir, rustup, cargo) = homes();
        let items = discover(Some(&rustup), Some(&cargo));
        let spec = CleanSpec {
            kinds: vec![CacheKind::RustupTmp],
            older_than: None,
            busy_grace: Duration::from_secs(60),
        };
        let later = SystemTime::now() + Duration::from_secs(3600);
        let plan = plan_clean(&items, &spec, later);
        let policy = RemovePolicy {
            mode: RemoveMode::Permanent,
            trash_max: 0,
        };
        for (_, result) in execute(&plan, &policy) {
            assert_eq!(result.unwrap(), Removal::Deleted);
        }
        // The folder stays, its contents go, and other kinds are untouched.
        assert!(rustup.join("tmp").is_dir());
        assert!(!rustup.join("tmp/partial-install").exists());
        assert!(rustup.join("downloads/x.partial").exists());
        assert!(rustup.join("toolchains/stable-x").exists());
    }
}
