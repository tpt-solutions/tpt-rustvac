//! `config.toml` loading. Location: `%APPDATA%\rustvac\config.toml` on Windows,
//! `~/.config/rustvac/config.toml` elsewhere. A missing file means defaults.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub roots: Vec<String>,
    pub older_than_days: u64,
    pub archive_dir: String,
    pub format: String,
    pub exclude: Vec<String>,
    pub keep: Vec<String>,
    pub busy_grace_seconds: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            roots: vec!["~/Code".into()],
            older_than_days: 2,
            archive_dir: "~/.rustvac/archive".into(),
            format: "tar.zst".into(),
            exclude: vec!["**/vendor/**".into(), "**/node_modules/**".into()],
            keep: Vec::new(),
            busy_grace_seconds: 60,
        }
    }
}

impl Config {
    /// Parses and validates a config file. A missing file yields defaults.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(Error::io(path, e)),
        };
        let cfg: Config =
            toml::from_str(&text).map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        if self.format != "tar.zst" {
            return Err(Error::Config(format!(
                "unsupported format '{}' (only \"tar.zst\")",
                self.format
            )));
        }
        if self.busy_grace_seconds == 0 {
            return Err(Error::Config(
                "busy_grace_seconds must be greater than 0".into(),
            ));
        }
        Ok(())
    }

    pub fn archive_dir_path(&self) -> PathBuf {
        expand_tilde(&self.archive_dir)
    }

    pub fn root_paths(&self) -> Vec<PathBuf> {
        self.roots.iter().map(|r| expand_tilde(r)).collect()
    }

    pub fn keep_paths(&self) -> Vec<PathBuf> {
        self.keep.iter().map(|k| expand_tilde(k)).collect()
    }

    pub fn exclude_patterns(&self) -> Result<Vec<glob::Pattern>> {
        self.exclude
            .iter()
            .map(|p| {
                glob::Pattern::new(p)
                    .map_err(|e| Error::Config(format!("bad exclude pattern '{p}': {e}")))
            })
            .collect()
    }
}

/// The current user's home directory, read from the environment. This replaces the
/// `dirs` crate, which pulls in an MPL-2.0 dependency that the licence policy rejects.
pub fn home_dir() -> Option<PathBuf> {
    let vars: &[&str] = if cfg!(windows) {
        &["USERPROFILE", "HOME"]
    } else {
        &["HOME"]
    };
    vars.iter()
        .filter_map(std::env::var_os)
        .find(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The config file location for the current platform.
pub fn default_config_path() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA")
            .filter(|v| !v.is_empty())
            .map(|d| PathBuf::from(d).join("rustvac").join("config.toml"))
    } else {
        home_dir().map(|h| h.join(".config").join("rustvac").join("config.toml"))
    }
}

/// Expands a leading `~` or `~/` to the home directory. Other paths are returned as-is.
pub fn expand_tilde(s: &str) -> PathBuf {
    if s == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(s));
    }
    if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}
