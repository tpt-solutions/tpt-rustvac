//! The window's remembered session: the last mode and folder, recent folders and the filters.
//! It lives in `ui_state.json` beside `config.toml`, so the config's comments are never
//! rewritten. Reading and writing never block the window: a missing or broken file gives the
//! defaults, and a failed write is ignored.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// How many recent folders are kept.
pub const MAX_RECENT: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiState {
    /// `rust`, `any` or `mixed`. `None` when nothing was saved.
    pub mode: Option<String>,
    /// The folder last scanned, if it still exists.
    pub root: Option<PathBuf>,
    /// Most recent first, without duplicates, and only folders that exist.
    pub recent_roots: Vec<PathBuf>,
    pub min_age_days: u64,
    pub min_size_mb: u64,
    pub hide_archived: bool,
    pub sort_desc: bool,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            mode: None,
            root: None,
            recent_roots: Vec::new(),
            min_age_days: 0,
            min_size_mb: 0,
            hide_archived: false,
            sort_desc: true,
        }
    }
}

impl UiState {
    /// Reads the saved session. Anything missing or unreadable gives the defaults.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|value| Self::from_json(&value))
            .unwrap_or_default()
    }

    /// Writes the session, creating the folder if needed. A failed write is ignored.
    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(&self.to_json()) {
            let _ = std::fs::write(path, text);
        }
    }

    /// The folder to scan at startup: the saved one if it still exists.
    pub fn usable_root(&self) -> Option<&PathBuf> {
        self.root.as_ref().filter(|r| r.is_dir())
    }

    /// Puts `root` at the front of the recent list. Duplicates are removed and the list is
    /// capped at [`MAX_RECENT`].
    pub fn remember_root(&mut self, root: &Path) {
        self.recent_roots.retain(|r| r != root);
        self.recent_roots.insert(0, root.to_path_buf());
        self.recent_roots.truncate(MAX_RECENT);
    }

    fn from_json(value: &Value) -> Self {
        let mut recent: Vec<PathBuf> = Vec::new();
        for path in value["recentRoots"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
        {
            if !recent.contains(&path) {
                recent.push(path);
            }
        }
        recent.truncate(MAX_RECENT);
        UiState {
            mode: value["mode"].as_str().map(String::from),
            root: value["root"].as_str().map(PathBuf::from),
            recent_roots: recent,
            min_age_days: value["minAgeDays"].as_u64().unwrap_or(0),
            min_size_mb: value["minSizeMb"].as_u64().unwrap_or(0),
            hide_archived: value["hideArchived"].as_bool().unwrap_or(false),
            sort_desc: value["sortDesc"].as_bool().unwrap_or(true),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "mode": self.mode,
            "root": self.root.as_ref().map(|p| p.display().to_string()),
            "recentRoots": self
                .recent_roots
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
            "minAgeDays": self.min_age_days,
            "minSizeMb": self.min_size_mb,
            "hideArchived": self.hide_archived,
            "sortDesc": self.sort_desc,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_session_loads_back_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("code");
        std::fs::create_dir(&folder).unwrap();
        let file = dir.path().join("cfg").join("ui_state.json");
        let state = UiState {
            mode: Some("mixed".into()),
            root: Some(folder.clone()),
            recent_roots: vec![folder.clone()],
            min_age_days: 30,
            min_size_mb: 100,
            hide_archived: true,
            sort_desc: false,
        };
        state.save(&file);
        assert_eq!(UiState::load(&file), state);
    }

    #[test]
    fn a_missing_or_broken_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            UiState::load(&dir.path().join("none.json")),
            UiState::default()
        );
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{ not json").unwrap();
        assert_eq!(UiState::load(&bad), UiState::default());
    }

    #[test]
    fn a_folder_that_no_longer_exists_is_not_used() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone");
        let state = UiState {
            root: Some(gone.clone()),
            recent_roots: vec![gone],
            ..UiState::default()
        };
        let file = dir.path().join("ui_state.json");
        state.save(&file);
        let loaded = UiState::load(&file);
        assert!(loaded.usable_root().is_none());
        assert!(loaded.recent_roots.is_empty());
    }

    #[test]
    fn recent_folders_are_deduplicated_and_capped() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = UiState::default();
        for i in 0..(MAX_RECENT + 3) {
            let folder = dir.path().join(format!("f{i}"));
            std::fs::create_dir(&folder).unwrap();
            state.remember_root(&folder);
        }
        assert_eq!(state.recent_roots.len(), MAX_RECENT);
        let newest = dir.path().join(format!("f{}", MAX_RECENT + 2));
        assert_eq!(state.recent_roots[0], newest);

        // Picking an existing folder again moves it to the front without a duplicate.
        let again = state.recent_roots[4].clone();
        state.remember_root(&again);
        assert_eq!(state.recent_roots.len(), MAX_RECENT);
        assert_eq!(state.recent_roots[0], again);
        assert_eq!(
            state.recent_roots.iter().filter(|r| **r == again).count(),
            1
        );

        // Duplicates in the file are dropped when it is read.
        let file = dir.path().join("ui_state.json");
        let mut doubled = state.clone();
        doubled.recent_roots = vec![again.clone(), again.clone()];
        doubled.save(&file);
        assert_eq!(UiState::load(&file).recent_roots, vec![again]);
    }
}
