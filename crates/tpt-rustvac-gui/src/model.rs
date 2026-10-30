//! GUI state and logic, independent of the window. The page reads it through the commands in
//! `commands.rs`.

use std::cmp::Ordering;
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

use crate::ui_state::UiState;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Compress,
    Trash,
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
        id: NodeId,
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

/// What the disk-health panel shows: the read-only report and its next steps.
pub struct DiskView {
    pub report: tpt_rustvac_core::disk::DiskReport,
    pub steps: tpt_rustvac_core::disk::Steps,
}

pub struct Model {
    pub config: Config,
    pub config_error: Option<String>,
    pub mode: Mode,
    pub root: PathBuf,

    pub arena: Arena,
    scan: Option<ScanHandle>,
    pub scanning: bool,
    /// A background refresh is filling `pending_arena`. The visible tree stays on screen
    /// until it finishes and is swapped in.
    pub refreshing: bool,
    pending_arena: Option<Arena>,
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
    /// The user's choice of a flat, targets-only table. It is on by default in Rust Targets
    /// mode. `flat_targets` also needs a mode that is not Any Folder.
    pub targets_only: bool,

    work: Option<Work>,
    pub status: String,

    pub disk: Option<DiskView>,
    disk_rx: Option<Receiver<DiskView>>,

    /// The remembered session, as last saved or loaded.
    pub ui: UiState,
    ui_path: Option<PathBuf>,
}

/// The session file, beside `config.toml`.
const UI_STATE_FILE: &str = "ui_state.json";

fn mode_key(mode: Mode) -> &'static str {
    match mode {
        Mode::RustTargets => "rust",
        Mode::AnyFolder => "any",
        Mode::Mixed => "mixed",
    }
}

fn mode_from_key(key: &str) -> Option<Mode> {
    match key {
        "rust" => Some(Mode::RustTargets),
        "any" => Some(Mode::AnyFolder),
        "mixed" => Some(Mode::Mixed),
        _ => None,
    }
}

impl Model {
    /// Records the folder and mode of a scan that was just started, and saves the session.
    pub fn note_scan_started(&mut self) {
        self.ui.remember_root(&self.root);
        self.save_session();
    }

    /// Saves the current mode, folder and filters. Writes only when something changed, and a
    /// failed write is ignored, so the window never waits on it.
    pub fn save_session(&mut self) {
        let Some(path) = self.ui_path.clone() else {
            return;
        };
        let next = UiState {
            mode: Some(mode_key(self.mode).to_string()),
            root: Some(self.root.clone()),
            recent_roots: self.ui.recent_roots.clone(),
            min_age_days: self.min_age_days,
            min_size_mb: self.min_size_mb,
            hide_archived: self.hide_archived,
            sort_desc: self.sort_desc,
        };
        if next != self.ui {
            next.save(&path);
            self.ui = next;
        }
    }

    /// Free and total bytes on the home drive, which is the drive the alert watches.
    pub fn disk_space(&self) -> Option<(u64, u64)> {
        tpt_rustvac_core::check::disk_space(&tpt_rustvac_core::config::home_dir()?)
    }

    /// Starts the disk report on a background thread. Measuring the cargo registry can take
    /// seconds, and the window must not wait for it. A second call while one runs does nothing.
    pub fn refresh_disk(&mut self) {
        if self.disk_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.disk_rx = Some(rx);
        std::thread::spawn(move || {
            use tpt_rustvac_core::config::{cargo_home, rustup_home};
            let items = tpt_rustvac_core::caches::discover(
                rustup_home().as_deref(),
                cargo_home().as_deref(),
            );
            let space = tpt_rustvac_core::config::home_dir()
                .and_then(|h| tpt_rustvac_core::check::disk_space(&h));
            let report = tpt_rustvac_core::disk::build(
                &items,
                tpt_rustvac_core::wsl::discover_here(),
                space,
            );
            let steps = tpt_rustvac_core::disk::next_steps(&report);
            let _ = tx.send(DiskView { report, steps });
        });
    }

    /// Picks up a finished disk report. True while one is still running.
    pub fn poll_disk(&mut self) -> bool {
        let Some(rx) = &self.disk_rx else {
            return false;
        };
        match rx.try_recv() {
            Ok(view) => {
                self.disk = Some(view);
                self.disk_rx = None;
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.disk_rx = None;
                false
            }
        }
    }

    pub fn disk_loading(&self) -> bool {
        self.disk_rx.is_some()
    }

    /// True when `path` is a WSL or Docker disk in the last report.
    pub fn is_known_wsl_disk(&self, path: &Path) -> bool {
        self.disk
            .as_ref()
            .is_some_and(|d| d.report.wsl.iter().any(|w| w.path == path))
    }

    pub fn new() -> Self {
        let (config, config_error) = match default_config_path() {
            Some(path) => match Config::load(&path) {
                Ok(c) => (c, None),
                Err(e) => (Config::default(), Some(e.to_string())),
            },
            None => (Config::default(), None),
        };
        // The remembered session wins. The config's first root is the fallback when the
        // saved folder is gone.
        let ui_path =
            default_config_path().and_then(|p| p.parent().map(|dir| dir.join(UI_STATE_FILE)));
        let ui = ui_path.as_deref().map(UiState::load).unwrap_or_default();
        let root = match ui.usable_root() {
            Some(saved) => native_separators(saved.clone()),
            None => native_separators(
                config
                    .root_paths()
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| PathBuf::from(".")),
            ),
        };
        let mode = ui
            .mode
            .as_deref()
            .and_then(mode_from_key)
            .unwrap_or(Mode::RustTargets);
        Model {
            config,
            config_error,
            mode,
            root,
            arena: Arena::new(),
            scan: None,
            scanning: false,
            refreshing: false,
            pending_arena: None,
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
            min_age_days: ui.min_age_days,
            min_size_mb: ui.min_size_mb,
            hide_archived: ui.hide_archived,
            sort_desc: ui.sort_desc,
            targets_only: mode == Mode::RustTargets,
            ui,
            ui_path,
            work: None,
            status: String::new(),
            disk: None,
            disk_rx: None,
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
        self.cancel_scan();
        self.arena = Arena::new();
        self.checked.clear();
        self.selected = None;
        self.failures.clear();
        self.toggled.clear();
        self.progress = (0, 0);
        self.last_scan = None;
        self.view_dirty = true;

        self.scan = Some(scan::start(self.scan_request()));
        self.scanning = true;
    }

    /// Scans the current mode and root again into a separate arena. The tree on screen,
    /// with its checks, selection and open folders, stays until the new scan is done. Then
    /// it is swapped in and the marks are matched by path. Does nothing while a scan runs.
    pub fn refresh_scan(&mut self) {
        if self.scanning || self.refreshing {
            return;
        }
        self.cancel_scan();
        self.pending_arena = Some(Arena::new());
        self.failures.clear();
        self.progress = (0, 0);
        self.scan = Some(scan::start(self.scan_request()));
        self.refreshing = true;
    }

    fn cancel_scan(&mut self) {
        if let Some(old) = self.scan.take() {
            old.cancel();
        }
        self.scanning = false;
        self.refreshing = false;
        self.pending_arena = None;
    }

    fn scan_request(&self) -> ScanRequest {
        let mut req = match self.mode {
            Mode::RustTargets => ScanRequest::rust_targets(vec![self.root.clone()]),
            Mode::AnyFolder | Mode::Mixed => ScanRequest::general(self.root.clone()),
        };
        req.exclude = self.config.exclude_patterns().unwrap_or_default();
        req.busy_grace = self.grace();
        req
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
        // A background refresh fills its own arena, and the visible tree is left alone.
        let background = self.refreshing;
        for event in &events {
            let arena = if background {
                self.pending_arena.get_or_insert_with(Arena::new)
            } else {
                &mut self.arena
            };
            arena.apply(event);
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
                    if let Some(node) = arena.get_mut(*id) {
                        node.is_target = true;
                    }
                }
                _ => {}
            }
        }
        self.view_dirty = true;
        if finished {
            self.scan = None;
            self.last_scan = Some(SystemTime::now());
            if background {
                self.finish_refresh();
            } else {
                self.scanning = false;
                self.apply_archive_overlay();
            }
        }
        true
    }

    /// Swaps in the refreshed arena. Checks, the selection and open folders are matched by
    /// path, because ids change between scans. Anything gone from disk drops out.
    fn finish_refresh(&mut self) {
        let Some(fresh) = self.pending_arena.take() else {
            self.refreshing = false;
            return;
        };
        let by_path = |ids: &HashSet<NodeId>, old: &Arena| -> HashSet<PathBuf> {
            ids.iter()
                .filter_map(|id| old.get(*id))
                .map(|n| n.path.clone())
                .collect()
        };
        let checked_paths = by_path(&self.checked, &self.arena);
        let toggled_paths = by_path(&self.toggled, &self.arena);
        let selected_path = self
            .selected
            .and_then(|id| self.arena.get(id))
            .map(|n| n.path.clone());

        let ids_for = |paths: &HashSet<PathBuf>| -> HashSet<NodeId> {
            fresh
                .iter()
                .filter(|n| paths.contains(&n.path))
                .map(|n| n.id)
                .collect()
        };
        self.checked = ids_for(&checked_paths);
        self.toggled = ids_for(&toggled_paths);
        self.selected =
            selected_path.and_then(|path| fresh.iter().find(|n| n.path == path).map(|n| n.id));
        self.arena = fresh;
        self.refreshing = false;
        self.apply_archive_overlay();
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
            let self_ok =
                node.kind != Kind::Root && !node.removed && self.passes(node, &q, &filter, now);
            let child_ok = node
                .children
                .iter()
                .any(|c| vis.get(c.0).copied().unwrap_or(false));
            vis[idx] = self_ok || child_ok;
        }
        self.visible = vis;

        let mut rows = Vec::new();
        if self.flat_targets() {
            rows = target_rows(&self.arena, &self.visible, self.sort_desc);
        } else if let Some(root) = self.arena.root() {
            let root_id = root.id;
            self.push_rows(root_id, 0, &mut rows);
        }
        self.rows = rows;
        self.view_dirty = false;
    }

    /// True when the page shows the flat targets table instead of the tree. Any Folder has
    /// no targets to list, so it always shows the tree.
    pub fn flat_targets(&self) -> bool {
        self.targets_only && self.mode != Mode::AnyFolder
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
        kids.sort_by(|a, b| size_order(a, b, self.sort_desc));
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
            let reason: Option<String> = if node.kind == Kind::Root {
                Some("it is the scan root".into())
            } else if is_kept(&node.path, &keep) {
                Some("it is in your keep list".into())
            } else if kind == ActionKind::Compress && !is_rust_target(node) {
                Some("only target folders can be compressed".into())
            } else if kind == ActionKind::Compress && node.state == State::Busy {
                Some("it was modified recently".into())
            } else {
                safety::sync_managed(&node.path)
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
                    };
                    if tx
                        .send(WorkMsg::Item {
                            id: item.id,
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

    /// Applies finished actions to the tree in place. A target that was compressed or
    /// trashed leaves the tree, with its size taken off its project and the root. A failed
    /// target stays in the tree and stays checked. Nothing is rescanned.
    pub fn poll_work(&mut self) {
        let Some(work) = &mut self.work else {
            return;
        };
        let mut finished = false;
        let mut removed = Vec::new();
        while let Ok(msg) = work.rx.try_recv() {
            match msg {
                WorkMsg::Done => finished = true,
                WorkMsg::Item { id, path, result } => {
                    work.done += 1;
                    match result {
                        Ok(_) => {
                            work.succeeded += 1;
                            removed.push(id);
                        }
                        Err(e) => work.failures.push(format!("{}: {e}", path.display())),
                    }
                }
            }
        }
        for id in removed {
            self.arena.remove_subtree(id);
            self.checked.remove(&id);
            if self.selected == Some(id) {
                self.selected = None;
            }
            self.view_dirty = true;
        }
        if finished {
            let total = work.total;
            let succeeded = work.succeeded;
            let failures = std::mem::take(&mut work.failures);
            self.work = None;
            self.status = if failures.is_empty() {
                format!("Done: {succeeded} of {total} succeeded.")
            } else {
                format!(
                    "{succeeded} of {total} succeeded. Failed: {}",
                    failures.join("; ")
                )
            };
        } else {
            self.status = format!("Working… {}/{}", work.done, work.total);
        }
    }

    /// When the last scan finished, as milliseconds since the Unix epoch. The page formats it.
    pub fn last_scan_millis(&self) -> Option<u64> {
        self.last_scan
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
    }
}

/// The config may write Windows paths with `/`. Scan paths use `\` there, so every row
/// shows one style.
fn native_separators(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(path.to_string_lossy().replace('/', "\\"))
    }
    #[cfg(not(windows))]
    {
        path
    }
}

pub fn is_rust_target(node: &Node) -> bool {
    node.kind == Kind::Target || node.is_target
}

/// The order of tree siblings and table rows: by size, then by name. Descending when
/// `sort_desc` is set.
fn size_order(a: &Node, b: &Node, sort_desc: bool) -> Ordering {
    let ord = a.size.cmp(&b.size).then_with(|| a.name.cmp(&b.name));
    if sort_desc {
        ord.reverse()
    } else {
        ord
    }
}

/// The visible targets as a flat list, in the same order as the tree. Rows have no depth
/// and no children, because the table draws no tree.
pub fn target_rows(arena: &Arena, visible: &[bool], sort_desc: bool) -> Vec<Row> {
    let mut targets: Vec<&Node> = arena
        .iter()
        .filter(|n| is_rust_target(n) && visible.get(n.id.0).copied().unwrap_or(false))
        .collect();
    targets.sort_by(|a, b| size_order(a, b, sort_desc));
    targets
        .into_iter()
        .map(|n| Row {
            id: n.id,
            depth: 0,
            has_children: false,
            expanded: false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scan root with two projects, each holding a target folder and a plain directory.
    /// Returns the arena and the target ids, small project first.
    fn two_projects() -> (Arena, [NodeId; 2]) {
        let mut arena = Arena::new();
        let root = arena.push(None, "root".into(), PathBuf::from("/r"), Kind::Root, false);
        let mut targets = Vec::new();
        for (name, size) in [("small", 10u64), ("big", 500)] {
            let project = arena.push(
                Some(root),
                name.into(),
                PathBuf::from(format!("/r/{name}")),
                Kind::Project,
                false,
            );
            let target = arena.push(
                Some(project),
                "target".into(),
                PathBuf::from(format!("/r/{name}/target")),
                Kind::Target,
                true,
            );
            arena.get_mut(target).unwrap().size = size;
            arena.get_mut(project).unwrap().size = size + 1;
            arena.push(
                Some(project),
                "src".into(),
                PathBuf::from(format!("/r/{name}/src")),
                Kind::Directory,
                false,
            );
            targets.push(target);
        }
        (arena, [targets[0], targets[1]])
    }

    #[test]
    fn targets_only_lists_visible_targets_largest_first() {
        let (arena, [small, big]) = two_projects();
        let visible = vec![true; arena.len()];
        let rows = target_rows(&arena, &visible, true);
        let ids: Vec<NodeId> = rows.iter().map(|r| r.id).collect();
        assert_eq!(ids, [big, small]);
        assert!(rows.iter().all(|r| r.depth == 0 && !r.has_children));

        let ascending: Vec<NodeId> = target_rows(&arena, &visible, false)
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ascending, [small, big]);
    }

    #[test]
    fn targets_only_respects_the_filter_result() {
        let (arena, [small, big]) = two_projects();
        // What rebuild_view produces when the size filter hides the 10-byte target.
        let mut visible = vec![true; arena.len()];
        visible[small.0] = false;
        let ids: Vec<NodeId> = target_rows(&arena, &visible, true)
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, [big]);
    }
}
