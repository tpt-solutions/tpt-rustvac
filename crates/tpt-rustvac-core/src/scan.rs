//! Scan engine. Runs on a background thread and streams [`ScanEvent`]s over an
//! `mpsc` channel. The UI polls the channel each frame and never blocks on the walk.
//!
//! Rust-target mode discovers projects serially with pruning, then sizes each
//! target in parallel. General mode measures one tree in parallel.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use ignore::WalkBuilder;
use rayon::prelude::*;

use crate::model::{Arena, Kind, NodeId, State};
use crate::target::{resolve_target_dir, TargetEnv};
use crate::walk::{is_busy, is_excluded, measure_tree, measure_tree_serial, Agg, WalkOpts};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanMode {
    /// Find Rust `target/` folders only. Uses `ScanRequest::roots`.
    RustTargets,
    /// Measure any folder the user points at.
    General(PathBuf),
}

#[derive(Debug, Clone)]
pub struct ScanRequest {
    pub mode: ScanMode,
    pub roots: Vec<PathBuf>,
    pub older_than: Option<Duration>,
    pub min_size: Option<u64>,
    pub follow_links: bool,
    pub cross_device: bool,
    pub exclude: Vec<glob::Pattern>,
    pub busy_grace: Duration,
    pub target_env: TargetEnv,
}

impl ScanRequest {
    pub fn rust_targets(roots: Vec<PathBuf>) -> Self {
        ScanRequest {
            mode: ScanMode::RustTargets,
            roots,
            older_than: None,
            min_size: None,
            follow_links: false,
            cross_device: false,
            exclude: Vec::new(),
            busy_grace: Duration::from_secs(60),
            target_env: TargetEnv::from_process(),
        }
    }

    pub fn general(path: PathBuf) -> Self {
        ScanRequest {
            mode: ScanMode::General(path.clone()),
            roots: vec![path],
            ..Self::rust_targets(Vec::new())
        }
    }
}

#[derive(Debug, Clone)]
pub enum ScanEvent {
    Discovered {
        id: NodeId,
        parent: Option<NodeId>,
        name: String,
        path: PathBuf,
        kind: Kind,
        is_target: bool,
    },
    NodeDone {
        id: NodeId,
        size: u64,
        files: u64,
        newest: Option<SystemTime>,
        state: State,
    },
    Progress {
        done: usize,
        total: usize,
    },
    Failed {
        path: PathBuf,
        message: String,
    },
    Finished {
        elapsed: Duration,
        cancelled: bool,
    },
}

impl Arena {
    /// Applies one event to the arena. `Progress`, `Failed` and `Finished` are for the
    /// consumer's own bookkeeping and leave the arena unchanged.
    pub fn apply(&mut self, event: &ScanEvent) {
        match event {
            ScanEvent::Discovered {
                parent,
                name,
                path,
                kind,
                is_target,
                ..
            } => {
                self.push(*parent, name.clone(), path.clone(), *kind, *is_target);
            }
            ScanEvent::NodeDone {
                id,
                size,
                files,
                newest,
                state,
            } => {
                if let Some(node) = self.get_mut(*id) {
                    node.size = *size;
                    node.files = *files;
                    node.newest = *newest;
                    node.state = state.clone();
                }
            }
            ScanEvent::Progress { .. } | ScanEvent::Failed { .. } | ScanEvent::Finished { .. } => {}
        }
    }
}

pub struct ScanHandle {
    pub events: Receiver<ScanEvent>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ScanHandle {
    /// Asks the scan to stop at the next directory boundary.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Blocks until the background thread exits.
    pub fn join(mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Starts a scan on a background thread.
pub fn start(req: ScanRequest) -> ScanHandle {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let thread_cancel = cancel.clone();
    let thread = std::thread::Builder::new()
        .name("rustvac-scan".into())
        .spawn(move || {
            let emitter = Emitter {
                tx,
                cancel: thread_cancel,
            };
            let started = Instant::now();
            match &req.mode {
                ScanMode::RustTargets => run_rust_targets(&req, &emitter),
                ScanMode::General(path) => run_general(&req, path, &emitter),
            }
            emitter.send(ScanEvent::Finished {
                elapsed: started.elapsed(),
                cancelled: emitter.is_cancelled(),
            });
        })
        .expect("failed to spawn scan thread");
    ScanHandle {
        events: rx,
        cancel,
        thread: Some(thread),
    }
}

struct Emitter {
    tx: Sender<ScanEvent>,
    cancel: Arc<AtomicBool>,
}

impl Emitter {
    fn send(&self, event: ScanEvent) {
        // If the receiver is gone nobody is listening, so stop the scan.
        if self.tx.send(event).is_err() {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn failed(&self, path: PathBuf, message: String) {
        self.send(ScanEvent::Failed { path, message });
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// A discovered target waiting to be sized.
struct PendingTarget {
    project_id: NodeId,
    target_id: NodeId,
    target_path: PathBuf,
}

/// A project's totals while its targets are still being sized.
#[derive(Default)]
struct Rollup {
    agg: Agg,
    busy: bool,
    /// Targets of this project not yet sized.
    left: usize,
}

#[derive(Default)]
struct Discovery {
    pending: Vec<Found>,
    project_dirs: Vec<PathBuf>,
    targets: HashSet<PathBuf>,
}

struct Found {
    project: PathBuf,
    target: PathBuf,
}

/// Decides whether the walker should enter `entry`, and records any Cargo project it sees.
fn discover_filter(
    entry: &ignore::DirEntry,
    found: &Mutex<Discovery>,
    exclude: &[glob::Pattern],
    env: &TargetEnv,
) -> bool {
    let path = entry.path();
    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
    if !is_dir {
        return true;
    }
    if is_excluded(exclude, path) {
        return false;
    }
    let mut d = found.lock().unwrap_or_else(|e| e.into_inner());
    // Never descend into a project we already found, or into a known target.
    if d.targets.contains(path)
        || d.project_dirs
            .iter()
            .any(|p| p != path && path.starts_with(p))
    {
        return false;
    }
    if path.join("Cargo.toml").is_file() {
        let target = resolve_target_dir(path, env);
        if target.is_dir() && !d.targets.contains(&target) {
            d.targets.insert(target.clone());
            d.project_dirs.push(path.to_path_buf());
            d.pending.push(Found {
                project: path.to_path_buf(),
                target,
            });
            return false;
        }
    }
    true
}

fn run_rust_targets(req: &ScanRequest, emit: &Emitter) {
    let root_path = req.roots.first().cloned().unwrap_or_default();
    let root_name = if req.roots.len() > 1 {
        format!("{} roots", req.roots.len())
    } else {
        display_name(&root_path)
    };
    let root_id = NodeId(0);
    emit.send(ScanEvent::Discovered {
        id: root_id,
        parent: None,
        name: root_name,
        path: root_path,
        kind: Kind::Root,
        is_target: false,
    });

    // Phase 1: discovery. Serial, but pruned, so it never walks inside a project.
    let found = Arc::new(Mutex::new(Discovery::default()));
    let exclude = Arc::new(req.exclude.clone());
    let env = Arc::new(req.target_env.clone());
    let mut next_id = 1usize;
    let mut targets: Vec<PendingTarget> = Vec::new();

    for root in &req.roots {
        if emit.is_cancelled() {
            break;
        }
        let mut builder = WalkBuilder::new(root);
        builder
            .standard_filters(false)
            .hidden(false)
            .follow_links(req.follow_links)
            .same_file_system(!req.cross_device);
        let f = found.clone();
        let ex = exclude.clone();
        let en = env.clone();
        builder.filter_entry(move |e| discover_filter(e, &f, &ex, &en));

        // The walk runs on every core. `discover_filter` records projects under the lock, and
        // the events go out after the walk, sorted by project path, so ids stay in order.
        builder.build_parallel().run(|| {
            Box::new(|result: Result<ignore::DirEntry, ignore::Error>| {
                if emit.is_cancelled() {
                    return ignore::WalkState::Quit;
                }
                if let Err(e) = result {
                    emit.failed(root.clone(), e.to_string());
                }
                ignore::WalkState::Continue
            })
        });
        flush_discoveries(&found, emit, &mut next_id, &mut targets);
    }

    if emit.is_cancelled() {
        return;
    }

    // Phase 2: size each target in parallel. Each result streams out as it finishes.
    let total = targets.len();
    let done = AtomicUsize::new(0);
    let now = SystemTime::now();
    // Share the cancel flag so a cancel request also stops the walkers mid-tree.
    let opts = WalkOpts {
        exclude: req.exclude.clone(),
        cross_device: req.cross_device,
        follow_links: req.follow_links,
        cancel: emit.cancel.clone(),
    };

    // Each target streams its own children, and the project's rollup goes out once its last
    // target is sized, so the tree fills in during the scan. Child ids are handed out under the
    // same lock as their `Discovered` event, so the consumer's arena index equals the id.
    let next_child_id = Mutex::new(next_id);
    let mut open_projects: HashMap<NodeId, Rollup> = HashMap::new();
    for target in &targets {
        open_projects.entry(target.project_id).or_default().left += 1;
    }
    let projects = Mutex::new(open_projects);
    let root_agg = Mutex::new(Agg::default());

    targets.par_iter().for_each(|pending| {
        let PendingTarget {
            project_id,
            target_id,
            target_path,
        } = pending;
        if emit.is_cancelled() {
            return;
        }
        let measured = measure_tree_serial(target_path, &opts);
        let agg = measured.total(target_path);
        let busy = is_busy(agg.newest, now, req.busy_grace);
        let root_error = measured
            .errors
            .iter()
            .find(|e| e.path == *target_path)
            .cloned();
        for err in &measured.errors {
            emit.failed(err.path.clone(), err.message.clone());
        }
        let state = match (&root_error, busy) {
            (Some(e), _) if agg.files == 0 => State::Failed(e.message.clone()),
            (_, true) => State::Busy,
            _ => State::Done,
        };
        emit.send(ScanEvent::NodeDone {
            id: *target_id,
            size: agg.size,
            files: agg.files,
            newest: agg.newest,
            state,
        });
        let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
        emit.send(ScanEvent::Progress {
            done: finished,
            total,
        });

        // One level below the target (debug/, release/, ...). Deeper levels are not built.
        {
            let mut next = next_child_id.lock().unwrap_or_else(|e| e.into_inner());
            for (path, child) in measured.dirs.iter().filter(|(p, _)| {
                p.parent() == Some(target_path.as_path()) && p.as_path() != target_path.as_path()
            }) {
                let id = NodeId(*next);
                *next += 1;
                emit.send(ScanEvent::Discovered {
                    id,
                    parent: Some(*target_id),
                    name: display_name(path),
                    path: path.clone(),
                    kind: Kind::Directory,
                    is_target: false,
                });
                emit.send(ScanEvent::NodeDone {
                    id,
                    size: child.size,
                    files: child.files,
                    newest: child.newest,
                    state: done_state(child, now, req.busy_grace),
                });
            }
        }

        root_agg.lock().unwrap_or_else(|e| e.into_inner()).add(&agg);
        let mut open = projects.lock().unwrap_or_else(|e| e.into_inner());
        let rollup = open.entry(*project_id).or_default();
        rollup.agg.add(&agg);
        rollup.busy |= busy;
        rollup.left -= 1;
        if rollup.left == 0 {
            emit.send(ScanEvent::NodeDone {
                id: *project_id,
                size: rollup.agg.size,
                files: rollup.agg.files,
                newest: rollup.agg.newest,
                state: if rollup.busy {
                    State::Busy
                } else {
                    State::Done
                },
            });
        }
    });

    if emit.is_cancelled() {
        return;
    }

    let root = root_agg.into_inner().unwrap_or_else(|e| e.into_inner());
    emit.send(ScanEvent::NodeDone {
        id: root_id,
        size: root.size,
        files: root.files,
        newest: root.newest,
        state: State::Done,
    });
}

/// Drains discoveries made by the walker and emits Discovered events for them.
fn flush_discoveries(
    found: &Mutex<Discovery>,
    emit: &Emitter,
    next_id: &mut usize,
    targets: &mut Vec<PendingTarget>,
) {
    let mut pending = std::mem::take(&mut found.lock().unwrap_or_else(|e| e.into_inner()).pending);
    // The parallel walk finds projects in no fixed order. Sorting keeps the ids the same each run.
    pending.sort_by(|a, b| a.project.cmp(&b.project));
    for item in pending {
        let project_id = NodeId(*next_id);
        *next_id += 1;
        emit.send(ScanEvent::Discovered {
            id: project_id,
            parent: Some(NodeId(0)),
            name: display_name(&item.project),
            path: item.project.clone(),
            kind: Kind::Project,
            is_target: false,
        });
        let target_id = NodeId(*next_id);
        *next_id += 1;
        emit.send(ScanEvent::Discovered {
            id: target_id,
            parent: Some(project_id),
            name: "target".into(),
            path: item.target.clone(),
            kind: Kind::Target,
            is_target: true,
        });
        targets.push(PendingTarget {
            project_id,
            target_id,
            target_path: item.target,
        });
    }
}

fn run_general(req: &ScanRequest, root: &Path, emit: &Emitter) {
    let root_id = NodeId(0);
    emit.send(ScanEvent::Discovered {
        id: root_id,
        parent: None,
        name: display_name(root),
        path: root.to_path_buf(),
        kind: Kind::Root,
        is_target: false,
    });

    let opts = WalkOpts {
        exclude: req.exclude.clone(),
        cross_device: req.cross_device,
        follow_links: req.follow_links,
        cancel: emit.cancel.clone(),
    };
    let measured = measure_tree(root, &opts);
    for err in &measured.errors {
        emit.failed(err.path.clone(), err.message.clone());
    }
    if emit.is_cancelled() {
        return;
    }

    let now = SystemTime::now();
    let mut ids: HashMap<PathBuf, NodeId> = HashMap::new();
    ids.insert(root.to_path_buf(), root_id);
    let mut next_id = 1usize;

    // BTreeMap order puts every parent before its children, so ids are assigned top-down.
    for (path, agg) in &measured.dirs {
        if path == root {
            continue;
        }
        let parent = path
            .parent()
            .and_then(|p| ids.get(p).copied())
            .unwrap_or(root_id);
        let id = NodeId(next_id);
        next_id += 1;
        ids.insert(path.clone(), id);
        emit.send(ScanEvent::Discovered {
            id,
            parent: Some(parent),
            name: display_name(path),
            path: path.clone(),
            kind: Kind::Directory,
            is_target: false,
        });
        emit.send(ScanEvent::NodeDone {
            id,
            size: agg.size,
            files: agg.files,
            newest: agg.newest,
            state: done_state(agg, now, req.busy_grace),
        });
    }

    let root_agg = measured.total(root);
    emit.send(ScanEvent::NodeDone {
        id: root_id,
        size: root_agg.size,
        files: root_agg.files,
        newest: root_agg.newest,
        state: done_state(&root_agg, now, req.busy_grace),
    });
}

fn done_state(agg: &Agg, now: SystemTime, grace: Duration) -> State {
    if is_busy(agg.newest, now, grace) {
        State::Busy
    } else {
        State::Done
    }
}
