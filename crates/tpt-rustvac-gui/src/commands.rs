//! The commands the page calls through `window.tpt.invoke`. The page polls `state.get` to
//! drive the scan and the work threads, so the app never pushes events and the page never
//! misses one. Each command names a capability; the window's grants in `main.rs` allow them.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde_json::{json, Value};
use tpt_rustvac_core::config::{default_config_path, Config, RemoveMode};
use tpt_rustvac_core::model::{Kind, Node, NodeId, State};
use tpt_shell::{App, CommandError, Target};

use crate::model::{self, ActionKind, CheckRule, Item, Mode, Model};

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
        "node.check_range",
        "fs.read",
        no_target,
        handler(model, node_check_range),
    )
    .command(
        "node.check_where",
        "fs.read",
        no_target,
        handler(model, node_check_where),
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
        "settings.set",
        "fs.write",
        no_target,
        handler(model, settings_set),
    )
    .command(
        "disk.refresh",
        "fs.read",
        no_target,
        handler(model, disk_refresh),
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
    m.poll_disk();
    m.poll_work();
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
    let (actionable, checked_actionable, checked_bytes) = m.check_summary();
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
        "targetsOnly": m.targets_only,
        "flatTargets": m.flat_targets(),
        "scanning": m.scanning,
        "refreshing": m.refreshing,
        "recentRoots": m.ui.recent_roots.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "progress": { "done": m.progress.0, "total": m.progress.1 },
        "working": m.is_busy_working(),
        "status": m.status,
        "lastScan": m.last_scan_millis(),
        "targets": m.target_count(),
        "reclaimable": m.reclaimable_bytes(),
        "actionableCount": actionable,
        "checkedActionable": checked_actionable,
        "checkedBytes": checked_bytes,
        "failures": failures,
        "failureCount": m.failures.len(),
        "configError": m.config_error,
        "settings": settings_json(&m.config),
        "space": space_json(m),
        "disk": disk_json(m),
        "rows": rows,
        "rowsTruncated": m.rows.len() > MAX_ROWS,
        "checked": checked,
        "roots": roots,
        "selected": selected,
    })
}

/// The disk-health panel's data. `report` is null until the first refresh finishes.
fn disk_json(m: &Model) -> Value {
    use tpt_rustvac_core::wsl;
    let report = m.disk.as_ref().map(|d| {
        let mut value = serde_json::to_value(&d.report).unwrap_or(Value::Null);
        value["nextSteps"] = serde_json::to_value(&d.steps).unwrap_or(Value::Null);
        value["wslCompact"] = Value::Array(
            d.report
                .wsl
                .iter()
                .map(|w| json!({ "path": w.path.display().to_string(), "steps": wsl::compact_hint(w) }))
                .collect(),
        );
        value
    });
    json!({ "loading": m.disk_loading(), "report": report })
}

fn disk_refresh(m: &mut Model, _: &Value) -> Result<Value, CommandError> {
    m.refresh_disk();
    Ok(json!({}))
}

fn settings_json(c: &Config) -> Value {
    json!({
        "warnBelow": c.warn_below,
        "checkEvery": c.check_every,
        "refreshInBackground": c.refresh_in_background,
        "removeMode": remove_mode_name(c.remove_mode),
        "trashMaxSize": c.trash_max_size,
        "olderThanDays": c.older_than_days,
        "busyGraceSeconds": c.busy_grace_seconds,
        "configPath": default_config_path().map(|p| p.display().to_string()),
    })
}

fn remove_mode_name(mode: RemoveMode) -> &'static str {
    match mode {
        RemoveMode::Auto => "auto",
        RemoveMode::Trash => "trash",
        RemoveMode::Permanent => "permanent",
    }
}

/// Free space on the home drive, and whether it is under the alert size.
fn space_json(m: &Model) -> Value {
    let space = m.disk_space();
    let warn = m.config.warn_below_bytes().ok().flatten();
    json!({
        "free": space.map(|s| s.0),
        "total": space.map(|s| s.1),
        "warnBelow": warn,
        "low": matches!((space, warn), (Some((free, _)), Some(w)) if free < w),
    })
}

/// Applies the settings the page sends and saves them. The whole change is validated before
/// anything is written, so a bad value leaves the config as it was.
fn settings_set(m: &mut Model, args: &Value) -> Result<Value, CommandError> {
    let bad = |message: String| CommandError::new("RUSTVAC_BAD_ARGS", &message);
    let mut next = m.config.clone();

    if let Some(v) = args.get("warnBelow") {
        // Null or an empty string turns the alert off.
        next.warn_below = v
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
    }
    if let Some(v) = args["checkEvery"].as_str() {
        next.check_every = v.to_string();
    }
    if let Some(v) = args["refreshInBackground"].as_bool() {
        next.refresh_in_background = v;
    }
    if let Some(v) = args["removeMode"].as_str() {
        next.remove_mode = v.parse().map_err(bad)?;
    }
    if let Some(v) = args["trashMaxSize"].as_str() {
        next.trash_max_size = v.trim().to_string();
    }
    if let Some(v) = args.get("olderThanDays").filter(|v| !v.is_null()) {
        next.older_than_days = v
            .as_u64()
            .ok_or_else(|| bad("olderThanDays must be a whole number".into()))?;
    }
    if let Some(v) = args.get("busyGraceSeconds").filter(|v| !v.is_null()) {
        next.busy_grace_seconds = v
            .as_u64()
            .ok_or_else(|| bad("busyGraceSeconds must be a whole number".into()))?;
    }

    let path = default_config_path().ok_or_else(|| {
        CommandError::new("RUSTVAC_SETTINGS", "no config location on this system")
    })?;
    next.save(&path)
        .map_err(|e| CommandError::new("RUSTVAC_SETTINGS", &e.to_string()))?;
    m.config = next;
    m.config_error = None;
    Ok(Value::Null)
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
    // Rescan with the setting on keeps the tree, its checks and its selection. Anything else
    // starts from an empty tree.
    let same_place = m.mode == mode && m.root == root;
    let refresh =
        same_place && m.config.refresh_in_background && !m.arena.is_empty() && !m.scanning;
    m.mode = mode;
    m.root = root;
    // The flat table is the default for Rust targets only.
    m.targets_only = mode == Mode::RustTargets;
    if refresh {
        m.refresh_scan();
    } else {
        m.start_scan();
    }
    m.note_scan_started();
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
    if let Some(on) = args["targetsOnly"].as_bool() {
        m.targets_only = on;
    }
    m.view_dirty = true;
    m.save_session();
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

/// Checks the actionable targets between two visible rows, inclusive, in either order. Shift-click
/// uses this. Rows that cannot be acted on are not checked.
fn node_check_range(m: &mut Model, args: &Value) -> Result<Value, CommandError> {
    let rows: Vec<NodeId> = m.rows.iter().map(|row| row.id).collect();
    let position =
        |key: &str| -> Result<usize, CommandError> {
            let id =
                NodeId(args[key].as_u64().ok_or_else(|| {
                    CommandError::new("RUSTVAC_BAD_ARGS", &format!("missing {key}"))
                })? as usize);
            rows.iter().position(|row| *row == id).ok_or_else(|| {
                CommandError::new("RUSTVAC_UNKNOWN_NODE", "that row is not in the table")
            })
        };
    let (from, to) = (position("from")?, position("to")?);
    let actionable: std::collections::HashSet<NodeId> =
        m.actionable_targets().into_iter().collect();
    for id in &rows[from.min(to)..=from.max(to)] {
        if actionable.contains(id) {
            m.checked.insert(*id);
        }
    }
    Ok(json!({}))
}

/// Replaces the checks with the actionable targets that match a quick-check preset.
fn node_check_where(m: &mut Model, args: &Value) -> Result<Value, CommandError> {
    let rule = CheckRule {
        older_than_days: args["olderThanDays"].as_u64(),
        larger_than_bytes: args["largerThanMb"].as_u64().map(|mb| mb * 1024 * 1024),
        not_archived: args["notArchived"].as_bool().unwrap_or(false),
    };
    m.checked = m
        .checkable_matching(&rule, SystemTime::now())
        .into_iter()
        .collect();
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
    if !m.arena.iter().any(|node| node.path == path) && !m.is_known_wsl_disk(&path) {
        return Err(CommandError::new(
            "RUSTVAC_UNKNOWN_PATH",
            "only paths in the current scan or the disk report can be shown",
        ));
    }
    reveal(&path)?;
    Ok(json!({}))
}

#[cfg(windows)]
fn reveal(path: &std::path::Path) -> Result<(), CommandError> {
    use std::os::windows::process::CommandExt;

    // Explorer needs the path quoted after /select,. A normal arg() would quote the whole
    // thing when the path has spaces, and Explorer then opens Documents instead.
    std::process::Command::new("explorer")
        .raw_arg(format!("/select,\"{}\"", path.display()))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Points the config location at a tempdir, so the real config is never touched.
    fn scratch_config() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let var = if cfg!(windows) { "APPDATA" } else { "HOME" };
        std::env::set_var(var, dir.path());
        dir
    }

    #[test]
    fn settings_are_validated_saved_and_reported() {
        let dir = scratch_config();
        let mut m = Model::new();

        settings_set(
            &mut m,
            &json!({ "warnBelow": "1GB", "removeMode": "trash", "olderThanDays": 5 }),
        )
        .unwrap();
        assert_eq!(m.config.warn_below.as_deref(), Some("1GB"));
        let saved = default_config_path().unwrap();
        assert!(saved.starts_with(dir.path()));
        assert_eq!(Config::load(&saved).unwrap(), m.config);
        let reported = settings_json(&m.config);
        assert_eq!(reported["warnBelow"], "1GB");
        assert_eq!(reported["removeMode"], "trash");
        assert_eq!(reported["olderThanDays"], 5);

        // A bad value changes nothing, in memory or on disk.
        let before = m.config.clone();
        assert!(settings_set(&mut m, &json!({ "warnBelow": "lots" })).is_err());
        assert!(settings_set(&mut m, &json!({ "removeMode": "shred" })).is_err());
        assert!(settings_set(&mut m, &json!({ "busyGraceSeconds": 0 })).is_err());
        assert_eq!(m.config, before);
        assert_eq!(Config::load(&saved).unwrap(), before);

        // An empty size turns the alert off.
        settings_set(&mut m, &json!({ "warnBelow": "" })).unwrap();
        assert_eq!(m.config.warn_below, None);
    }
}
