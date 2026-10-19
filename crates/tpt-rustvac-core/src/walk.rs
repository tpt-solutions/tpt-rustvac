//! Parallel size measurement for a directory tree.
//!
//! Each thread keeps local per-directory totals and merges them into shared
//! state when it finishes, so the hot loop takes no locks. The result is one
//! aggregate per directory, not one record per file, so memory stays bounded
//! by directory count.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use ignore::{WalkBuilder, WalkState};

use crate::identity::hardlink_identity;

/// Aggregate for a directory: its own files plus, after rollup, everything below it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Agg {
    pub size: u64,
    pub files: u64,
    pub newest: Option<SystemTime>,
}

impl Agg {
    pub fn add(&mut self, other: &Agg) {
        self.size += other.size;
        self.files += other.files;
        self.newest = max_time(self.newest, other.newest);
    }
}

fn max_time(a: Option<SystemTime>, b: Option<SystemTime>) -> Option<SystemTime> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

#[derive(Debug, Clone)]
pub struct WalkError {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct Measured {
    /// Every directory under (and including) the root, keyed by path, with totals after rollup.
    pub dirs: BTreeMap<PathBuf, Agg>,
    pub errors: Vec<WalkError>,
    pub cancelled: bool,
}

impl Measured {
    pub fn total(&self, root: &Path) -> Agg {
        self.dirs.get(root).copied().unwrap_or_default()
    }
}

pub struct WalkOpts {
    pub exclude: Vec<glob::Pattern>,
    pub cross_device: bool,
    pub follow_links: bool,
    pub cancel: Arc<AtomicBool>,
}

pub fn is_excluded(patterns: &[glob::Pattern], path: &Path) -> bool {
    let opts = glob::MatchOptions {
        case_sensitive: !cfg!(windows),
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    patterns.iter().any(|p| p.matches_path_with(path, opts))
}

#[derive(Default)]
struct Shared {
    dirs: Mutex<HashMap<PathBuf, Agg>>,
    identities: Mutex<HashSet<(u64, u64)>>,
    errors: Mutex<Vec<WalkError>>,
}

/// Per-thread buffer. Merged into `Shared` when dropped.
struct Local<'a> {
    map: HashMap<PathBuf, Agg>,
    shared: &'a Shared,
}

impl Drop for Local<'_> {
    fn drop(&mut self) {
        let mut dirs = self.shared.dirs.lock().unwrap_or_else(|e| e.into_inner());
        for (path, agg) in self.map.drain() {
            dirs.entry(path).or_default().add(&agg);
        }
    }
}

/// Walks `root` in parallel and returns per-directory totals. Hardlinked files
/// count once. Unreadable entries are recorded and the walk continues.
pub fn measure_tree(root: &Path, opts: &WalkOpts) -> Measured {
    let shared = Shared::default();
    let patterns = Arc::new(opts.exclude.clone());

    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(false)
        .hidden(false)
        .follow_links(opts.follow_links)
        .same_file_system(!opts.cross_device)
        .filter_entry(move |e| !is_excluded(&patterns, e.path()));

    let walker = builder.build_parallel();
    let cancel = opts.cancel.clone();
    let shared_ref = &shared;
    walker.run(|| {
        let mut local = Local {
            map: HashMap::new(),
            shared: shared_ref,
        };
        let cancel = cancel.clone();
        Box::new(move |result| {
            if cancel.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }
            visit(&mut local, result);
            WalkState::Continue
        })
    });

    let cancelled = opts.cancel.load(Ordering::Relaxed);
    let mut dirs: BTreeMap<PathBuf, Agg> = shared
        .dirs
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .into_iter()
        .collect();
    rollup(&mut dirs);

    Measured {
        dirs,
        errors: shared
            .errors
            .into_inner()
            .unwrap_or_else(|e| e.into_inner()),
        cancelled,
    }
}

fn visit(local: &mut Local<'_>, result: Result<ignore::DirEntry, ignore::Error>) {
    let entry = match result {
        Ok(e) => e,
        Err(e) => {
            record_error(
                local.shared,
                error_path(&e).unwrap_or_default(),
                e.to_string(),
            );
            return;
        }
    };
    let Some(ft) = entry.file_type() else { return };
    let path = entry.path();

    if ft.is_dir() {
        local.map.entry(path.to_path_buf()).or_default();
        return;
    }
    if !ft.is_file() {
        // Sockets, FIFOs, device nodes and symlinks (we do not follow them) are skipped.
        return;
    }
    let Some(parent) = path.parent() else { return };

    let md = match entry.metadata() {
        Ok(md) => md,
        Err(e) => {
            record_error(local.shared, path.to_path_buf(), e.to_string());
            return;
        }
    };

    let is_duplicate = match hardlink_identity(path, &md) {
        Some(id) => {
            let mut seen = local
                .shared
                .identities
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            !seen.insert(id)
        }
        None => false,
    };

    let agg = local.map.entry(parent.to_path_buf()).or_default();
    if !is_duplicate {
        agg.size += md.len();
        agg.files += 1;
    }
    agg.newest = max_time(agg.newest, md.modified().ok());
}

fn record_error(shared: &Shared, path: PathBuf, message: String) {
    shared
        .errors
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(WalkError { path, message });
}

fn error_path(e: &ignore::Error) -> Option<PathBuf> {
    match e {
        ignore::Error::WithPath { path, .. } => Some(path.clone()),
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            error_path(err)
        }
        ignore::Error::Partial(errs) => errs.iter().find_map(error_path),
        _ => None,
    }
}

/// Adds each directory's totals into its parent. Children sort after their parent
/// in a `BTreeMap<PathBuf, _>`, so walking the keys in reverse finishes every
/// child before it is added upward.
fn rollup(dirs: &mut BTreeMap<PathBuf, Agg>) {
    let keys: Vec<PathBuf> = dirs.keys().cloned().collect();
    for key in keys.iter().rev() {
        let Some(parent) = key.parent() else { continue };
        let child = dirs[key];
        if let Some(p) = dirs.get_mut(parent) {
            p.add(&child);
        }
    }
}

/// A directory counts as busy if anything inside changed within `grace`.
pub fn is_busy(newest: Option<SystemTime>, now: SystemTime, grace: Duration) -> bool {
    match newest {
        Some(t) => now.duration_since(t).map(|age| age < grace).unwrap_or(true),
        None => false,
    }
}
