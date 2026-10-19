//! GUI state and logic. Nothing in here touches FLTK, so it can be read and reasoned
//! about on its own. The window code lives in `main.rs` and `tree.rs`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime};

use tpt_rustvac_core::archive;
use tpt_rustvac_core::config::{default_config_path, Config};
use tpt_rustvac_core::filter::Filter;
use tpt_rustvac_core::model::{Arena, Kind, Node, NodeId, State};
use tpt_rustvac_core::safety::{self, is_kept};
use tpt_rustvac_core::scan::{self, ScanEvent, ScanHandle, ScanRequest};

/// Scan events handled per UI tick, so a huge scan cannot stall the window.
pub const EVENTS_PER_TICK: usize = 20_000;
/// Folders shallower than this start expanded.
const DEFAULT_OPEN_DEPTH: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    RustTargets,
    AnyFolder,
    Mixed,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::RustTargets, Mode::AnyFolder, Mode::Mixed];

    pub fn index(self) -> i32 {
        Mode::ALL.iter().position(|m| *m == self).unwrap_or(0) as i32
    }

    pub fn from_index(i: i32) -> Mode {
        Mode::ALL
            .get(i.max(0) as usize)
            .copied()
            .unwrap_or(Mode::RustTargets)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Compress,
    Trash,
    DeletePermanently,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub id: NodeId,
    pub path: PathBuf,
    pub bytes: u64,
}

/// One visible line of the tree, in display order.
#[derive(Debug, Clone, Copy)]
pub struct Row {
    pub id: NodeId,
    pub depth: usize,
    pub has_children: bool,
    pub expanded: bool,
}

enum WorkMsg {
    Item {
        path: PathBuf,
        result: Result<String, String>,
    },
    Done,
}

struct Work {
    rx: Receiver<WorkMsg>,
    total: usize,
    done: usize,
    succeeded: usize,
    failures: Vec<String>,
}

pub struct Model {
    pub config: Config,
    pub config_error: Option<String>,
    pub mode: Mode,
    pub root: PathBuf,

    pub arena: Arena,
    scan: Option<ScanHandle>,
    pub scanning: bool,
    pub progress: (usize, usize),
    pub failures: Vec<(PathBuf, String)>,
    pub last_scan: Option<SystemTime>,

    pub visible: Vec<bool>,
    pub view_dirty: bool,
    pub rows: Vec<Row>,
    /// Folders whose open/closed state the user has flipped from the default.
    toggled: HashSet<NodeId>,

    pub selected: Option<NodeId>,
    pub checked: HashSet<NodeId>,
    pub query: String,
    pub min_age_days: u64,
    pub min_size_mb: u64,
    pub hide_archived: bool,
    pub sort_desc: bool,

    work: Option<Work>,
    pub status: String,
}

impl Model {
    pub fn new() -> Self {
        let (config, config_error) = match default_config_path() {
            Some(path) => match Config::load(&path) {
                Ok(c) => (c, None),
                Err(e) => (Config::default(), Some(e.to_string())),
            },
            None => (Config::default(), None),
        };
        let root = config
            .root_paths()
            .into_iter()
            .next()
            .unwrap_or_else(|| PathBuf::from("."));
        Model {
            config,
            config_error,
            mode: Mode::RustTargets,
            root,
            arena: Arena::new(),
            scan: None,
            scanning: false,
            progress: (0, 0),
            failures: Vec::new(),
            last_scan: None,
            visible: Vec::new(),
            view_dirty: true,
            rows: Vec::new(),
            toggled: HashSet::new(),
            selected: None,
            checked: HashSet::new(),
            query: String::new(),
            min_age_days: 0,
            min_size_mb: 0,
            hide_archived: false,
            sort_desc: true,
            work: None,
            status: String::new(),
        }
    }

    pub fn vacuum(&self) -> PathBuf {
        self.config.archive_dir_path()
    }

    fn grace(&self) -> Duration {
        Duration::from_secs(self.config.busy_grace_seconds)
    }

    fn filter(&self) -> Filter {
        Filter {
            older_than: (self.min_age_days > 0)
                .then(|| Duration::from_secs(self.min_age_days * 86_400)),
            min_size: (self.min_size_mb > 0).then(|| self.min_size_mb * 1024 * 1024),
        }
    }

    pub fn is_busy_working(&self) -> bool {
        self.work.is_some()
    }

    /// Cancels any running scan and starts a fresh one for the current mode and root.
    pub fn start_scan(&mut self) {
        if let Some(old) = self.scan.take() {
            old.cancel();
        }
        self.arena = Arena::new();
        self.checked.clear();
        self.selected = None;
        self.failures.clear();
        self.toggled.clear();
        self.progress = (0, 0);
        self.last_scan = None;
        self.view_dirty = true;

        let mut req = match self.mode {
            Mode::RustTargets => ScanRequest::rust_targets(vec![self.root.clone()]),
            Mode::AnyFolder | Mode::Mixed => ScanRequest::general(self.root.clone()),
        };
        req.exclude = self.config.exclude_patterns().unwrap_or_default();
        req.busy_grace = self.grace();
        self.scan = Some(scan::start(req));
        self.scanning = true;
    }

    /// Applies up to `EVENTS_PER_TICK` scan events. Returns true if the tree changed.
    pub fn poll_scan(&mut self) -> bool {
        let Some(handle) = &self.scan else {
            return false;
        };
        let events: Vec<ScanEvent> = handle.events.try_iter().take(EVENTS_PER_TICK).collect();
        if events.is_empty() {
            return false;
        }
        let finished = events
            .iter()
            .any(|e| matches!(e, ScanEvent::Finished { .. }));
        for event in &events {
            self.arena.apply(event);
            match event {
                ScanEvent::Progress { done, total } => self.progress = (*done, *total),
                ScanEvent::Failed { path, message } => {
                    self.failures.push((path.clone(), message.clone()))
                }
                ScanEvent::Discovered {
                    id,
                    kind: Kind::Directory,
                    name,
                    path,
                    ..
                } if self.mode == Mode::Mixed
                    && name == "target"
                    && path
                        .parent()
                        .is_some_and(|p| p.join("Cargo.toml").is_file()) =>
                {
                    if let Some(node) = self.arena.get_mut(*id) {
                        node.is_target = true;
                    }
                }
                _ => {}
            }
        }
        self.view_dirty = true;
        if finished {
            self.scanning = false;
            self.last_scan = Some(SystemTime::now());
            self.scan = None;
            self.apply_archive_overlay();
        }
        true
    }

    /// Marks targets whose project already has an archive. The row is struck through
    /// and badged, but stays actionable.
    fn apply_archive_overlay(&mut self) {
        let archived: HashSet<String> = archive::list(&self.vacuum())
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.slug)
            .collect();
        if archived.is_empty() {
            return;
        }
        let updates: Vec<NodeId> = self
            .arena
            .iter()
            .filter(|n| is_rust_target(n) && n.state == State::Done)
            .filter(|n| {
                n.path
                    .parent()
                    .map(|p| archived.contains(&archive::slug_for(p)))
                    .unwrap_or(false)
            })
            .map(|n| n.id)
            .collect();
        for id in updates {
            if let Some(node) = self.arena.get_mut(id) {
                node.state = State::Archived;
            }
        }
        self.view_dirty = true;
    }

    /// Recomputes which nodes match the filters, then the flat list of visible rows.
    pub fn rebuild_view(&mut self) {
        let now = SystemTime::now();
        let q = self.query.trim().to_lowercase();
        let filter = self.filter();
        let n = self.arena.len();
        let mut vis = vec![false; n];
        // Parents always have smaller ids than their children, so a reverse pass sees children first.
        for idx in (0..n).rev() {
            let Some(node) = self.arena.get(NodeId(idx)) else {
                continue;
            };
            let self_ok = node.kind != Kind::Root && self.passes(node, &q, &filter, now);
            let child_ok = node
                .children
                .iter()
                .any(|c| vis.get(c.0).copied().unwrap_or(false));
            vis[idx] = self_ok || child_ok;
        }
        self.visible = vis;

        let mut rows = Vec::new();
        if let Some(root) = self.arena.root() {
            let root_id = root.id;
            self.push_rows(root_id, 0, &mut rows);
        }
        self.rows = rows;
        self.view_dirty = false;
    }

    fn push_rows(&self, id: NodeId, depth: usize, rows: &mut Vec<Row>) {
        if !self.visible.get(id.0).copied().unwrap_or(false) {
            return;
        }
        let Some(node) = self.arena.get(id) else {
            return;
        };
        let mut kids: Vec<&Node> = node
            .children
            .iter()
            .filter(|c| self.visible.get(c.0).copied().unwrap_or(false))
            .filter_map(|c| self.arena.get(*c))
            .collect();
        kids.sort_by(|a, b| {
            let ord = a.size.cmp(&b.size).then_with(|| a.name.cmp(&b.name));
            if self.sort_desc {
                ord.reverse()
            } else {
                ord
            }
        });
        let expanded = self.is_expanded(id, depth);
        rows.push(Row {
            id,
            depth,
            has_children: !kids.is_empty(),
            expanded,
        });
        if expanded {
            for kid in kids {
                self.push_rows(kid.id, depth + 1, rows);
            }
        }
    }

    pub fn is_expanded(&self, id: NodeId, depth: usize) -> bool {
        (depth < DEFAULT_OPEN_DEPTH) ^ self.toggled.contains(&id)
    }

    pub fn toggle_expanded(&mut self, id: NodeId) {
        if !self.toggled.remove(&id) {
            self.toggled.insert(id);
        }
        self.view_dirty = true;
    }

    fn passes(&self, node: &Node, query: &str, filter: &Filter, now: SystemTime) -> bool {
        if self.hide_archived && node.state == State::Archived {
            return false;
        }
        if !query.is_empty() {
            let parent_name = node
                .path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_lowercase());
            let hit = node.name.to_lowercase().contains(query)
                || parent_name.map(|p| p.contains(query)).unwrap_or(false);
            if !hit {
                return false;
            }
        }
        filter.accepts(node, now)
    }

    /// Targets the user can act on right now: visible, not busy, not kept.
    pub fn actionable_targets(&self) -> Vec<NodeId> {
        let keep = self.config.keep_paths();
        self.arena
            .iter()
            .filter(|n| is_rust_target(n))
            .filter(|n| self.visible.get(n.id.0).copied().unwrap_or(false))
            .filter(|n| matches!(n.state, State::Done | State::Archived))
            .filter(|n| !is_kept(&n.path, &keep))
            .map(|n| n.id)
            .collect()
    }

    pub fn reclaimable_bytes(&self) -> u64 {
        let targets: HashSet<NodeId> = self.actionable_targets().into_iter().collect();
        self.arena
            .iter()
            .filter(|n| targets.contains(&n.id))
            .map(|n| n.size)
            .sum()
    }

    pub fn target_count(&self) -> usize {
        self.arena.iter().filter(|n| is_rust_target(n)).count()
    }

    /// Checks each id against the safety rules. Returns the items that may proceed,
    /// and a message listing anything refused.
    pub fn vet(&self, kind: ActionKind, ids: &[NodeId]) -> (Vec<Item>, Option<String>) {
        let keep = self.config.keep_paths();
        let mut items = Vec::new();
        let mut refused = Vec::new();
        for id in ids {
            let Some(node) = self.arena.get(*id) else {
                continue;
            };
            let reason = if node.kind == Kind::Root {
                Some("it is the scan root")
            } else if is_kept(&node.path, &keep) {
                Some("it is in your keep list")
            } else if kind == ActionKind::Compress && !is_rust_target(node) {
                Some("only target folders can be compressed")
            } else if kind == ActionKind::Compress && node.state == State::Busy {
                Some("it was modified recently")
            } else {
                None
            };
            match reason {
                Some(r) => refused.push(format!("{}: {r}", node.path.display())),
                None => items.push(Item {
                    id: *id,
                    path: node.path.clone(),
                    bytes: node.size,
                }),
            }
        }
        let note = (!refused.is_empty())
            .then(|| format!("Skipped {}: {}", refused.len(), refused.join("; ")));
        (items, note)
    }

    /// Runs an action on a worker thread so the window keeps responding.
    pub fn run_work(&mut self, kind: ActionKind, items: Vec<Item>) {
        let (tx, rx) = mpsc::channel();
        let total = items.len();
        let vacuum = self.vacuum();
        let grace = self.grace();
        let projects: Vec<Option<PathBuf>> = items
            .iter()
            .map(|i| {
                self.arena
                    .get(i.id)
                    .and_then(|n| n.path.parent().map(Path::to_path_buf))
            })
            .collect();
        std::thread::Builder::new()
            .name("rustvac-work".into())
            .spawn(move || {
                for (item, project) in items.into_iter().zip(projects) {
                    let result = match kind {
                        ActionKind::Compress => {
                            let project = project.unwrap_or_else(|| item.path.clone());
                            archive::compress_and_vacuum(&item.path, &project, &vacuum, grace)
                                .map(|r| format!("archived to {}", r.archive.display()))
                                .map_err(|e| e.to_string())
                        }
                        ActionKind::Trash => safety::move_to_trash(&item.path)
                            .map(|_| "moved to trash".to_string())
                            .map_err(|e| e.to_string()),
                        ActionKind::DeletePermanently => safety::delete_permanently(&item.path)
                            .map(|_| "deleted".to_string())
                            .map_err(|e| e.to_string()),
                    };
                    if tx
                        .send(WorkMsg::Item {
                            path: item.path,
                            result,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                let _ = tx.send(WorkMsg::Done);
            })
            .expect("failed to spawn work thread");
        self.work = Some(Work {
            rx,
            total,
            done: 0,
            succeeded: 0,
            failures: Vec::new(),
        });
        self.status = format!("Working on {total} item(s)…");
    }

    /// Returns true when the work has finished and the caller should rescan.
    pub fn poll_work(&mut self) -> bool {
        let Some(work) = &mut self.work else {
            return false;
        };
        let mut finished = false;
        while let Ok(msg) = work.rx.try_recv() {
            match msg {
                WorkMsg::Done => finished = true,
                WorkMsg::Item { path, result } => {
                    work.done += 1;
                    match result {
                        Ok(_) => work.succeeded += 1,
                        Err(e) => work.failures.push(format!("{}: {e}", path.display())),
                    }
                }
            }
        }
        if finished {
            let total = work.total;
            let succeeded = work.succeeded;
            let failures = std::mem::take(&mut work.failures);
            self.work = None;
            self.status = if failures.is_empty() {
                format!("Done: {succeeded} of {total} succeeded. Rescanning…")
            } else {
                format!(
                    "{succeeded} of {total} succeeded. Failed: {}",
                    failures.join("; ")
                )
            };
            true
        } else {
            self.status = format!("Working… {}/{}", work.done, work.total);
            false
        }
    }

    pub fn last_scan_text(&self) -> String {
        match self.last_scan {
            Some(t) => {
                let local: chrono::DateTime<chrono::Local> = t.into();
                local.format("%H:%M:%S").to_string()
            }
            None => "—".into(),
        }
    }
}

pub fn is_rust_target(node: &Node) -> bool {
    node.kind == Kind::Target || node.is_target
}

pub fn human(bytes: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", units[unit])
    }
}
