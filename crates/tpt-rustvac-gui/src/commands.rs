//! The commands the page calls through `window.tpt.invoke`. The page polls `state.get` to
//! drive the scan and the work threads, so the app never pushes events and the page never
//! misses one. Each command names a capability; the window's grants in `main.rs` allow them.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde_json::{json, Value};
use tpt_rustvac_core::model::{Kind, Node, NodeId, State};
use tpt_shell::{App, CommandError, Target};

use crate::model::{self, ActionKind, Item, Mode, Model};

/// The app state. Commands run on the UI thread; work threads only send messages.
pub type Shared = Arc<Mutex<Model>>;

/// Rows sent to the page per poll. A larger tree is trimmed, and the page says so.
const MAX_ROWS: usize = 5_000;
/// Failure messages sent to the page per poll.
const MAX_FAILURES: usize = 50;

/// Registers every command the page may call.
pub fn register(app: App, model: &Shared) -> App {
    app.command(
        "dialog.pick_folder",
        "fs.read",
        no_target,
        dialog_pick_folder,
    )
    .command("state.get", "fs.read", no_target, handler(model, state_get))
    .command(
        "scan.start",
        "fs.read",
        root_target,
        handler(model, scan_start),
    )
    .command("view.set", "fs.read", no_target, handler(model, view_set))
    .command(
        "node.expand",
        "fs.read",
        no_target,
        handler(model, node_expand),
    )
    .command(
        "node.select",
        "fs.read",
        no_target,
        handler(model, node_select),
    )
    .command(
        "node.check",
        "fs.read",
        no_target,
        handler(model, node_check),
    )
    .command(
        "node.check_all",
        "fs.read",
        no_target,
        handler(model, node_check_all),
    )
    .command(
        "action.prepare",
        "fs.read",
        no_target,
        handler(model, action_prepare),
    )
    .command(
        "action.run",
        "fs.write",
        no_target,
        handler(model, action_run),
    )
    .command(
        "path.reveal",
        "fs.read",
        path_target,
        handler(model, path_reveal),
    )
}

/// Wraps a command body so it runs with the state locked.
fn handler(
    model: &Shared,
    body: fn(&mut Model, &Value) -> Result<Value, CommandError>,
) -> impl Fn(&Value) -> Result<Value, CommandError> + 'static {
    let model = model.clone();
    move |args| {
        let mut m = model
            .lock()
            .map_err(|_| CommandError::new("RUSTVAC_STATE", "the app state is unavailable"))?;
        body(&mut m, args)
    }
}

fn no_target(_: &Value) -> Target {
    Target::None
}

fn root_target(args: &Value) -> Target {
    Target::Path(args["root"].as_str().unwrap_or_default().to_string())
}

fn path_target(args: &Value) -> Target {
    Target::Path(args["path"].as_str().unwrap_or_default().to_string())
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, CommandError> {
    args[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CommandError::new("RUSTVAC_BAD_ARGS", &format!("missing {key}")))
}

fn node_id(m: &Model, args: &Value) -> Result<NodeId, CommandError> {
    let raw = args["id"]
        .as_u64()
        .ok_or_else(|| CommandError::new("RUSTVAC_BAD_ARGS", "missing id"))?;
    let id = NodeId(raw as usize);
    m.arena.get(id).map(|_| id).ok_or_else(|| {
        CommandError::new("RUSTVAC_UNKNOWN_NODE", "that row is no longer in the scan")
    })
}

fn node_ids(args: &Value) -> Vec<NodeId> {
    args["ids"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_u64)
                .map(|id| NodeId(id as usize))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_mode(name: &str) -> Result<Mode, CommandError> {
    match name {
        "rust" => Ok(Mode::RustTargets),
        "any" => Ok(Mode::AnyFolder),
        "mixed" => Ok(Mode::Mixed),
        _ => Err(CommandError::new(
            "RUSTVAC_BAD_ARGS",
            "mode must be rust, any or mixed",
        )),
    }
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::RustTargets => "rust",
        Mode::AnyFolder => "any",
        Mode::Mixed => "mixed",
    }
}

fn parse_kind(name: &str) -> Result<ActionKind, CommandError> {
    match name {
        "compress" => Ok(ActionKind::Compress),
        "trash" => Ok(ActionKind::Trash),
        _ => Err(CommandError::new(
            "RUSTVAC_BAD_ARGS",
            "action must be compress or trash",
        )),
    }
}

/// Advances the scan and the work threads, then returns the state the page draws.
fn state_get(m: &mut Model, _: &Value) -> Result<Value, CommandError> {
    m.poll_scan();
    if m.poll_work() {
        m.start_scan();
    }
    if m.view_dirty {
        m.rebuild_view();
    }
    Ok(snapshot(m))
}

fn snapshot(m: &Model) -> Value {
    let now = SystemTime::now();
    let rows: Vec<Value> = m
        .rows
        .iter()
        .take(MAX_ROWS)
        .filter_map(|row| {
            let node = m.arena.get(row.id)?;
            let mut value = node_json(node, now);
            value["id"] = json!(row.id.0);
            value["depth"] = json!(row.depth);
            value["hasChildren"] = json!(row.has_children);
            value["expanded"] = json!(row.expanded);
            value["checked"] = json!(m.checked.contains(&row.id));
            value["selected"] = json!(m.selected == Some(row.id));
            Some(value)
        })
        .collect();
    let mut checked: Vec<usize> = m.checked.iter().map(|id| id.0).collect();
    checked.sort_unstable();
    let roots: Vec<String> = m
        .config
        .root_paths()
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    let selected = m.selected.and_then(|id| {
        let mut value = node_json(m.arena.get(id)?, now);
        value["id"] = json!(id.0);
        Some(value)
    });
    let failures: Vec<String> = m
        .failures
        .iter()
        .take(MAX_FAILURES)
        .map(|(path, message)| format!("{}: {message}", path.display()))
        .collect();

    json!({
        "mode": mode_name(m.mode),
        "root": m.root.display().to_string(),
        "rootIsProject": m.root.join("Cargo.toml").is_file(),
        "query": m.query,
        "minAgeDays": m.min_age_days,
        "minSizeMb": m.min_size_mb,
        "hideArchived": m.hide_archived,
        "sortDesc": m.sort_desc,
        "scanning": m.scanning,
        "progress": { "done": m.progress.0, "total": m.progress.1 },
        "working": m.is_busy_working(),
        "status": m.status,
        "lastScan": m.last_scan_millis(),
        "targets": m.target_count(),
        "reclaimable": m.reclaimable_bytes(),
        "failures": failures,
        "failureCount": m.failures.len(),
        "configError": m.config_error,
        "rows": rows,
        "rowsTruncated": m.rows.len() > MAX_ROWS,
        "checked": checked,
        "roots": roots,
        "selected": selected,
    })
}

fn node_json(node: &Node, now: SystemTime) -> Value {
    json!({
        "name": node.name,
        "path": node.path.display().to_string(),
        "kind": kind_label(node.kind),
        "size": node.size,
        "files": node.files,
        "ageDays": age_days(node, now),
        "state": state_label(&node.state),
        "target": model::is_rust_target(node),
    })
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Root => "root",
        Kind::Project => "project",
        Kind::Target => "target",
        Kind::Directory => "directory",
    }
}

fn state_label(state: &State) -> &'static str {
    match state {
        State::Pending => "pending",
        State::Scanning => "scanning",
        State::Done => "done",
        State::Failed(_) => "failed",
        State::Busy => "busy",
        State::Archived => "archived",
    }
}

/// Whole days since the newest file was written, or `null` when the mtime is unknown.
fn age_days(node: &Node, now: SystemTime) -> Value {
    match node.newest.and_then(|t| now.duration_since(t).ok()) {
        Some(age) => json!(age.as_secs() / 86_400),
        None => Value::Null,
    }
}

fn scan_start(m: &mut Model, args: &Value) -> Result<Value, CommandError> {
    let mode = parse_mode(str_arg(args, "mode")?)?;
    let root = PathBuf::from(str_arg(args, "root")?);
    if !root.is_dir() {
        return Err(CommandError::new(
            "RUSTVAC_NOT_A_FOLDER",
            "that path is not a folder",
        ));
    }
    m.mode = mode;
    m.root = root;
    m.start_scan();
    Ok(json!({}))
}

fn view_set(m: &mut Model, args: &Value) -> Result<Value, CommandError> {
    if let Some(query) = args["query"].as_str() {
        m.query = query.to_string();
    }
    if let Some(days) = args["minAgeDays"].as_u64() {
        m.min_age_days = days;
    }
    if let Some(mb) = args["minSizeMb"].as_u64() {
        m.min_size_mb = mb;
    }
    if let Some(hide) = args["hideArchived"].as_bool() {
        m.hide_archived = hide;
    }
    if let Some(desc) = args["sortDesc"].as_bool() {
        m.sort_desc = desc;
    }
    m.view_dirty = true;
    Ok(json!({}))
}

fn node_expand(m: &mut Model, args: &Value) -> Result<Value, CommandError> {
    let id = node_id(m, args)?;
    m.toggle_expanded(id);
    Ok(json!({}))
}

fn node_select(m: &mut Model, args: &Value) -> Result<Value, CommandError> {
    m.selected = if args["id"].is_null() {
        None
    } else {
        Some(node_id(m, args)?)
    };
    Ok(json!({}))
}

fn node_check(m: &mut Model, args: &Value) -> Result<Value, CommandError> {
    let id = node_id(m, args)?;
    if args["on"].as_bool().unwrap_or(true) {
        m.checked.insert(id);
    } else {
        m.checked.remove(&id);
    }
    Ok(json!({}))
}

/// Checks every visible target that can be acted on, or clears all checks.
fn node_check_all(m: &mut Model, args: &Value) -> Result<Value, CommandError> {
    if args["on"].as_bool().unwrap_or(true) {
        m.checked = m.actionable_targets().into_iter().collect();
    } else {
        m.checked.clear();
    }
    Ok(json!({}))
}

/// Lists what an action would do, for the confirmation dialog. Changes nothing.
fn action_prepare(m: &mut Model, args: &Value) -> Result<Value, CommandError> {
    let kind = parse_kind(str_arg(args, "action")?)?;
    let (items, note) = m.vet(kind, &node_ids(args));
    let bytes: u64 = items.iter().map(|item| item.bytes).sum();
    Ok(json!({
        "items": items.iter().map(item_json).collect::<Vec<Value>>(),
        "bytes": bytes,
        "note": note,
    }))
}

/// Runs an action on the checked targets, or the one selected, after the dialog confirmed it.
fn action_run(m: &mut Model, args: &Value) -> Result<Value, CommandError> {
    let kind = parse_kind(str_arg(args, "action")?)?;
    if m.is_busy_working() {
        return Err(CommandError::new(
            "RUSTVAC_BUSY",
            "an action is already running",
        ));
    }
    let (items, note): (Vec<Item>, Option<String>) = m.vet(kind, &node_ids(args));
    if items.is_empty() {
        let reason = note.unwrap_or_else(|| "nothing is selected".into());
        return Err(CommandError::new("RUSTVAC_NOTHING_TO_DO", &reason));
    }
    let started = items.len();
    m.run_work(kind, items);
    Ok(json!({ "started": started, "note": note }))
}

fn item_json(item: &Item) -> Value {
    json!({ "path": item.path.display().to_string(), "bytes": item.bytes })
}

/// Asks for a folder with the native dialog, starting in `start` when that is a folder.
/// Returns the chosen path, or `null` when the user cancels.
fn dialog_pick_folder(args: &Value) -> Result<Value, CommandError> {
    let start = args["start"]
        .as_str()
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir());
    Ok(json!(pick_folder(start)?.map(|p| p.display().to_string())))
}

#[cfg(windows)]
fn pick_folder(start: Option<PathBuf>) -> Result<Option<PathBuf>, CommandError> {
    let mut dialog = rfd::FileDialog::new().set_title("Choose a folder to scan");
    if let Some(dir) = start {
        dialog = dialog.set_directory(dir);
    }
    Ok(dialog.pick_folder())
}

#[cfg(not(windows))]
fn pick_folder(_: Option<PathBuf>) -> Result<Option<PathBuf>, CommandError> {
    Err(CommandError::new(
        "RUSTVAC_UNSUPPORTED",
        "the folder picker is only built for Windows so far",
    ))
}

/// Opens Explorer with the item selected. Only paths that are in the scan are accepted.
fn path_reveal(m: &mut Model, args: &Value) -> Result<Value, CommandError> {
    let path = PathBuf::from(str_arg(args, "path")?);
    if !m.arena.iter().any(|node| node.path == path) {
        return Err(CommandError::new(
            "RUSTVAC_UNKNOWN_PATH",
            "only paths in the current scan can be shown",
        ));
    }
    reveal(&path)?;
    Ok(json!({}))
}

#[cfg(windows)]
fn reveal(path: &std::path::Path) -> Result<(), CommandError> {
    std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn()
        .map(|_| ())
        .map_err(|e| CommandError::new("RUSTVAC_REVEAL", &e.to_string()))
}

#[cfg(not(windows))]
fn reveal(_: &std::path::Path) -> Result<(), CommandError> {
    Err(CommandError::new(
        "RUSTVAC_UNSUPPORTED",
        "showing a folder is only built for Windows so far",
    ))
}
