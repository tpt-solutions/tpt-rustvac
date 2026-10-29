//! Partial cleanup inside a `target/` folder: the `incremental/` caches, a whole build
//! profile, or the build output of dependency versions the lockfile no longer uses. Everything
//! here is regenerable by building again.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::SystemTime;

use serde::Deserialize;

use crate::identity::disk_bytes;
use crate::safety::absolute;
use crate::walk::{measure_tree, WalkOpts};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PruneSpec {
    /// Every `incremental/` folder: `<profile>/incremental` and `<triple>/<profile>/incremental`.
    Incremental,
    /// A whole profile folder, such as `debug`: `<profile>` and `<triple>/<profile>`.
    Profile(String),
}

#[derive(Debug, Clone)]
pub struct PruneItem {
    /// The `target/` folder this belongs to.
    pub target: PathBuf,
    /// The folder shown for this item. For a stale dependency unit it is the unit's `.d` file.
    pub path: PathBuf,
    /// Everything removed for this item, in order. For a folder it is the folder itself.
    pub members: Vec<PathBuf>,
    pub size: u64,
    pub files: u64,
    pub newest: Option<SystemTime>,
}

/// The folders `spec` selects inside one target. Callers skip busy and kept targets first.
pub fn plan(target: &Path, spec: &PruneSpec) -> Vec<PruneItem> {
    paths_for(target, spec)
        .into_iter()
        .map(|path| {
            let agg = measure(&path);
            PruneItem {
                target: target.to_path_buf(),
                members: vec![path.clone()],
                path,
                size: agg.size,
                files: agg.files,
                newest: agg.newest,
            }
        })
        .collect()
}

/// Folder paths only, with no sizing. Split out so the layout rules are testable on their own.
pub fn paths_for(target: &Path, spec: &PruneSpec) -> Vec<PathBuf> {
    let leaf = match spec {
        PruneSpec::Incremental => "incremental",
        PruneSpec::Profile(p) => p.as_str(),
    };
    let mut out = Vec::new();
    for level1 in subdirs(target) {
        match spec {
            // <profile>/incremental
            PruneSpec::Incremental => {
                let direct = level1.join(leaf);
                if direct.is_dir() {
                    out.push(direct);
                }
            }
            // <profile>
            PruneSpec::Profile(_) => {
                if level1.file_name().is_some_and(|n| n == leaf) {
                    out.push(level1.clone());
                }
            }
        }
        // <triple>/<profile>[/incremental]
        for level2 in subdirs(&level1) {
            match spec {
                PruneSpec::Incremental => {
                    let nested = level2.join(leaf);
                    if nested.is_dir() {
                        out.push(nested);
                    }
                }
                PruneSpec::Profile(_) => {
                    if level2.file_name().is_some_and(|n| n == leaf)
                        && level2.join(".fingerprint").is_dir()
                    {
                        out.push(level2);
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The crate folders the project's `Cargo.lock` names, as `name-version`, which is how cargo
/// lays out its registry source cache. Refuses when the target is not inside the project: a
/// target shared through `CARGO_TARGET_DIR` can hold units that other projects still use, and
/// this project's lockfile cannot say which.
pub fn live_crate_dirs(project: &Path, target: &Path) -> Result<HashSet<String>, String> {
    let abs_project = absolute(project);
    if absolute(target).parent() != Some(abs_project.as_path()) {
        return Err(
            "the target folder is outside the project, so other projects may share it".into(),
        );
    }
    let lock = project.join("Cargo.lock");
    let text = fs::read_to_string(&lock)
        .map_err(|_| format!("no readable Cargo.lock in {}", project.display()))?;
    lock_dirs(&text).map_err(|e| format!("cannot read {}: {e}", lock.display()))
}

/// The `name-version` folders of every package in a `Cargo.lock`.
pub fn lock_dirs(lock_text: &str) -> Result<HashSet<String>, String> {
    #[derive(Deserialize)]
    struct Lockfile {
        #[serde(default)]
        package: Vec<LockedPackage>,
    }
    #[derive(Deserialize)]
    struct LockedPackage {
        name: String,
        version: String,
    }
    let lock: Lockfile = toml::from_str(lock_text).map_err(|e| e.to_string())?;
    Ok(lock
        .package
        .into_iter()
        .map(|p| format!("{}-{}", p.name, p.version))
        .collect())
}

/// The registry crate folders named in one `.d` file, such as `serde-1.0.229`. Every source
/// file a unit was built from is listed there, so a unit whose folder is not in the lockfile is
/// build output for a version the project no longer uses. Accepts Windows and Unix separators
/// and the `\ ` escape for spaces.
pub fn registry_crate_dirs(dep_info: &str) -> Vec<String> {
    let text = dep_info.replace("\\ ", " ");
    let parts: Vec<&str> = text.split(['/', '\\']).collect();
    parts
        .windows(4)
        .filter(|w| w[0] == "registry" && w[1] == "src" && w[2].starts_with("index"))
        .map(|w| w[3].to_string())
        .collect()
}

/// Build output of crates whose version the lockfile no longer names. Each unit is one `.d`
/// file in a `deps/` folder, with its sibling artifacts and its `.fingerprint/` folder. `live`
/// comes from `live_crate_dirs`. A unit with no registry source paths is never stale, and an
/// unreadable `.d` file is left alone.
pub fn plan_stale_deps(target: &Path, live: &HashSet<String>) -> Vec<PruneItem> {
    let mut out = Vec::new();
    for profile in profile_dirs(target) {
        let deps = profile.join("deps");
        let Ok(entries) = fs::read_dir(&deps) else {
            continue;
        };
        let names: Vec<(String, PathBuf)> = entries
            .filter_map(|e| e.ok())
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .collect();
        for (name, dep_info) in names.iter().filter(|(n, _)| n.ends_with(".d")) {
            let Some(stem) = name.strip_suffix(".d") else {
                continue;
            };
            let Ok(text) = fs::read_to_string(dep_info) else {
                continue;
            };
            if registry_crate_dirs(&text).iter().all(|d| live.contains(d)) {
                continue;
            }
            let mut members: Vec<PathBuf> = names
                .iter()
                .filter(|(n, _)| belongs_to(n, stem))
                .map(|(_, p)| p.clone())
                .collect();
            let fingerprint = profile.join(".fingerprint").join(stem);
            if fingerprint.is_dir() {
                members.push(fingerprint);
            }

            let (mut size, mut files, mut newest) = (0u64, 0u64, None::<SystemTime>);
            for member in &members {
                if member.is_dir() {
                    let agg = measure(member);
                    size += agg.size;
                    files += agg.files;
                    newest = newest.max(agg.newest);
                } else if let Ok(md) = fs::metadata(member) {
                    size += disk_bytes(member, &md);
                    files += 1;
                    newest = newest.max(md.modified().ok());
                }
            }
            out.push(PruneItem {
                target: target.to_path_buf(),
                path: dep_info.clone(),
                members,
                size,
                files,
                newest,
            });
        }
    }
    out
}

/// Profile folders in a target: `<profile>` and `<triple>/<profile>`. Cargo gives each one a
/// `.fingerprint` folder, which is how they are told apart from `doc/` and other folders.
fn profile_dirs(target: &Path) -> Vec<PathBuf> {
    let is_profile = |dir: &Path| dir.join(".fingerprint").is_dir();
    let mut out = Vec::new();
    for level1 in subdirs(target) {
        if is_profile(&level1) {
            out.push(level1.clone());
        }
        for level2 in subdirs(&level1) {
            if is_profile(&level2) {
                out.push(level2);
            }
        }
    }
    out.sort();
    out
}

/// Whether a file in `deps/` belongs to the unit `stem`: `<stem>.d`, `<stem>.rlib` and so on,
/// or the `lib<stem>` form that library outputs use. A longer hash that merely starts with
/// `stem` does not match.
fn belongs_to(file_name: &str, stem: &str) -> bool {
    let tail = file_name.strip_prefix(stem).or_else(|| {
        file_name
            .strip_prefix("lib")
            .and_then(|rest| rest.strip_prefix(stem))
    });
    tail.is_some_and(|t| t.is_empty() || t.starts_with('.'))
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default()
}

fn measure(path: &Path) -> crate::walk::Agg {
    let opts = WalkOpts {
        exclude: Vec::new(),
        cross_device: false,
        follow_links: false,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    measure_tree(path, &opts).total(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(root: &Path) {
        for d in [
            "debug/incremental/a",
            "debug/deps",
            "release/deps",
            "x86_64-unknown-linux-gnu/debug/incremental/b",
            "x86_64-unknown-linux-gnu/debug/.fingerprint",
            "doc/deps/incremental-notes",
        ] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
    }

    #[test]
    fn finds_incremental_dirs_at_both_depths() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        let found = paths_for(dir.path(), &PruneSpec::Incremental);
        assert_eq!(
            found,
            vec![
                dir.path().join("debug/incremental"),
                dir.path()
                    .join("x86_64-unknown-linux-gnu/debug/incremental"),
            ]
        );
    }

    #[test]
    fn finds_profile_dirs_at_both_depths() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        let found = paths_for(dir.path(), &PruneSpec::Profile("debug".into()));
        assert_eq!(
            found,
            vec![
                dir.path().join("debug"),
                dir.path().join("x86_64-unknown-linux-gnu/debug"),
            ]
        );
    }

    #[test]
    fn plan_sizes_what_it_finds() {
        let dir = tempfile::tempdir().unwrap();
        tree(dir.path());
        fs::write(dir.path().join("debug/incremental/a/f"), vec![0u8; 5000]).unwrap();
        let items = plan(dir.path(), &PruneSpec::Incremental);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].files, 1);
        assert!(items[0].size > 0);
        assert_eq!(items[1].files, 0);
        assert_eq!(items[0].members, vec![items[0].path.clone()]);
    }

    #[test]
    fn lock_dirs_reads_name_and_version_and_ignores_the_rest() {
        let lock = r#"
version = 4

[[package]]
name = "serde"
version = "1.0.229"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "abc"

[[package]]
name = "tpt-app"
version = "0.1.0"
"#;
        let dirs = lock_dirs(lock).unwrap();
        assert_eq!(dirs.len(), 2);
        assert!(dirs.contains("serde-1.0.229"));
        assert!(dirs.contains("tpt-app-0.1.0"));
    }

    #[test]
    fn lock_dirs_with_no_packages_is_empty() {
        assert!(lock_dirs("version = 4\n").unwrap().is_empty());
    }

    #[test]
    fn registry_dirs_from_windows_and_unix_dep_info() {
        let win = r"C:\Work\app\target\debug\deps\app-1a.d: C:\Users\me\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\serde-1.0.229\src\lib.rs C:\Users\me\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\serde-1.0.229\src\de\mod.rs";
        assert_eq!(
            registry_crate_dirs(win),
            vec!["serde-1.0.229", "serde-1.0.229"]
        );

        let unix = "/home/me/app/target/debug/deps/x.d: /home/me/.cargo/registry/src/index.crates.io-6f17/serde-1.0.1/src/lib.rs";
        assert_eq!(registry_crate_dirs(unix), vec!["serde-1.0.1"]);
    }

    #[test]
    fn registry_dirs_handle_escaped_spaces() {
        let text = r"/home/me/My\ Code/target/debug/deps/x.d: /home/me/.cargo/registry/src/index.crates.io-6f17/serde-1.0.1/src/lib.rs";
        assert_eq!(registry_crate_dirs(text), vec!["serde-1.0.1"]);
    }

    #[test]
    fn path_and_git_sources_are_not_registry_dirs() {
        let text = r"C:\Work\app\src\lib.rs C:\Users\me\.cargo\git\checkouts\repo-1\abc\src\lib.rs";
        assert!(registry_crate_dirs(text).is_empty());
    }

    #[test]
    fn belongs_to_matches_unit_outputs_only() {
        assert!(belongs_to("serde-aaaa.d", "serde-aaaa"));
        assert!(belongs_to("libserde-aaaa.rlib", "serde-aaaa"));
        assert!(belongs_to("libserde-aaaa.rmeta", "serde-aaaa"));
        assert!(belongs_to("serde-aaaa", "serde-aaaa"));
        // The library of a crate named `libfoo` is `liblibfoo-*.rlib`.
        assert!(belongs_to("liblibfoo-aaaa.rlib", "libfoo-aaaa"));
        assert!(!belongs_to("serde-aaaa1.d", "serde-aaaa"));
        assert!(!belongs_to("serde-bbbb.d", "serde-aaaa"));
    }

    /// `deps/` with a stale serde, a live serde, and a workspace crate with no registry source.
    fn stale_tree(root: &Path) -> HashSet<String> {
        let registry = "/home/me/.cargo/registry/src/index.crates.io-6f17";
        let deps = root.join("debug/deps");
        fs::create_dir_all(&deps).unwrap();
        fs::create_dir_all(root.join("debug/.fingerprint/serde-aaaa")).unwrap();
        fs::write(
            deps.join("serde-aaaa.d"),
            format!(
                "{} : {registry}/serde-1.0.1/src/lib.rs\n",
                deps.join("serde-aaaa.d").display()
            ),
        )
        .unwrap();
        fs::write(deps.join("libserde-aaaa.rlib"), vec![0u8; 3000]).unwrap();
        fs::write(deps.join("serde-aaaa.rmeta"), vec![0u8; 500]).unwrap();
        fs::write(root.join("debug/.fingerprint/serde-aaaa/json"), b"{}").unwrap();
        fs::write(
            deps.join("serde-bbbb.d"),
            format!("x : {registry}/serde-1.0.2/src/lib.rs\n"),
        )
        .unwrap();
        fs::write(deps.join("libserde-bbbb.rlib"), vec![0u8; 3000]).unwrap();
        fs::write(deps.join("app-cccc.d"), "x : /home/me/app/src/main.rs\n").unwrap();
        HashSet::from(["serde-1.0.2".to_string(), "app-0.1.0".to_string()])
    }

    #[test]
    fn stale_units_are_found_with_all_their_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let live = stale_tree(dir.path());
        let items = plan_stale_deps(dir.path(), &live);
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert!(item.path.ends_with("serde-aaaa.d"));
        let names: HashSet<String> = item
            .members
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            HashSet::from([
                "serde-aaaa.d".to_string(),
                "libserde-aaaa.rlib".to_string(),
                "serde-aaaa.rmeta".to_string(),
                "serde-aaaa".to_string(),
            ])
        );
        assert_eq!(item.files, 4);
        assert!(item.size >= 3500);
    }

    #[test]
    fn live_and_workspace_units_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let live = stale_tree(dir.path());
        let items = plan_stale_deps(dir.path(), &live);
        assert!(items.iter().all(|i| !i.path.ends_with("serde-bbbb.d")));
        assert!(items.iter().all(|i| !i.path.ends_with("app-cccc.d")));
    }

    #[test]
    fn live_crate_dirs_refuses_a_target_outside_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("app");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("Cargo.lock"), "version = 4\n").unwrap();
        let shared = dir.path().join("shared-target");
        let err = live_crate_dirs(&project, &shared).unwrap_err();
        assert!(err.contains("outside the project"));
        assert!(live_crate_dirs(&project, &project.join("target")).is_ok());
    }

    #[test]
    fn live_crate_dirs_refuses_a_missing_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("app");
        fs::create_dir_all(&project).unwrap();
        let err = live_crate_dirs(&project, &project.join("target")).unwrap_err();
        assert!(err.contains("Cargo.lock"));
    }
}
