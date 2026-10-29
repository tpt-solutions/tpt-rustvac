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
    /// How regenerable caches are removed: `auto`, `trash` or `permanent`.
    pub remove_mode: RemoveMode,
    /// Under `auto`, items up to this size go to the trash and larger ones are deleted. Like `100MB`.
    pub trash_max_size: String,
    /// How often the scheduled check runs: `off`, `daily` or `weekly`. `rustvac status`
    /// reports when the check is overdue. Registering the task is `rustvac schedule set`.
    pub check_every: String,
    /// Warn when free space on the home drive drops below this, like `50GB`.
    pub warn_below: Option<String>,
}

/// How a regenerable cache is removed. `target/` compress is unaffected: it always archives,
/// verifies and then trashes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RemoveMode {
    /// Small items to the trash, large ones deleted. Trashing a large cache on the same drive frees nothing.
    #[default]
    Auto,
    Trash,
    Permanent,
}

impl std::str::FromStr for RemoveMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(RemoveMode::Auto),
            "trash" => Ok(RemoveMode::Trash),
            "permanent" => Ok(RemoveMode::Permanent),
            _ => Err(format!(
                "unknown remove mode '{s}' (auto, trash or permanent)"
            )),
        }
    }
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
            remove_mode: RemoveMode::Auto,
            trash_max_size: "256MB".into(),
            check_every: "off".into(),
            warn_below: None,
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

    /// Writes the config as TOML, replacing the file in one step. Comments in an existing
    /// file are not kept. The config is validated first, so a bad one is never written.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let text = toml::to_string_pretty(self)
            .map_err(|e| Error::Config(format!("could not encode the config: {e}")))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| Error::io(&tmp, e))?;
        std::fs::rename(&tmp, path).map_err(|e| Error::io(path, e))
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
        parse_size(&self.trash_max_size)
            .map_err(|e| Error::Config(format!("trash_max_size: {e}")))?;
        self.check_interval()?;
        if let Some(w) = &self.warn_below {
            parse_size(w).map_err(|e| Error::Config(format!("warn_below: {e}")))?;
        }
        Ok(())
    }

    /// `check_every` as an interval, or `None` for `off`.
    pub fn check_interval(&self) -> Result<Option<crate::schedule::Every>> {
        match self.check_every.to_ascii_lowercase().as_str() {
            "off" => Ok(None),
            other => other.parse().map(Some).map_err(|_| {
                Error::Config(format!(
                    "check_every: '{other}' is not off, daily or weekly"
                ))
            }),
        }
    }

    /// `warn_below` in bytes.
    pub fn warn_below_bytes(&self) -> Result<Option<u64>> {
        self.warn_below
            .as_deref()
            .map(|w| parse_size(w).map_err(|e| Error::Config(format!("warn_below: {e}"))))
            .transpose()
    }

    /// The removal policy for regenerable caches, with `override_mode` taking precedence
    /// over the configured mode (for a `--remove` flag).
    pub fn removal_policy(&self, override_mode: Option<RemoveMode>) -> Result<RemovePolicy> {
        Ok(RemovePolicy {
            mode: override_mode.unwrap_or(self.remove_mode),
            trash_max: parse_size(&self.trash_max_size)
                .map_err(|e| Error::Config(format!("trash_max_size: {e}")))?,
        })
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

/// Removal mode plus the size threshold that `auto` uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemovePolicy {
    pub mode: RemoveMode,
    pub trash_max: u64,
}

/// Parses sizes like `100MB`, `5GiB` or `2048`. Units are powers of 1024 for both
/// the `B` and `iB` forms, so `MB` and `MiB` mean the same thing here.
pub fn parse_size(input: &str) -> std::result::Result<u64, String> {
    let s = input.trim();
    let split = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let value: f64 = num.parse().map_err(|_| format!("invalid size '{input}'"))?;
    const KIB: u64 = 1024;
    let multiplier = match unit
        .trim()
        .to_ascii_lowercase()
        .trim_end_matches("ib")
        .trim_end_matches('b')
    {
        "" => 1,
        "k" => KIB,
        "m" => KIB.pow(2),
        "g" => KIB.pow(3),
        "t" => KIB.pow(4),
        _ => return Err(format!("unknown size unit in '{input}'")),
    };
    Ok((value * multiplier as f64) as u64)
}

/// `RUSTUP_HOME`, else `~/.rustup`.
pub fn rustup_home() -> Option<PathBuf> {
    env_dir("RUSTUP_HOME").or_else(|| home_dir().map(|h| h.join(".rustup")))
}

/// `CARGO_HOME`, else `~/.cargo`.
pub fn cargo_home() -> Option<PathBuf> {
    env_dir("CARGO_HOME").or_else(|| home_dir().map(|h| h.join(".cargo")))
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        let cfg = Config::default();
        cfg.validate().unwrap();
        let policy = cfg.removal_policy(None).unwrap();
        assert_eq!(policy.mode, RemoveMode::Auto);
        assert_eq!(policy.trash_max, 256 * 1024 * 1024);
    }

    #[test]
    fn parses_new_fields() {
        let cfg: Config =
            toml::from_str("remove_mode = \"permanent\"\ntrash_max_size = \"1GiB\"").unwrap();
        cfg.validate().unwrap();
        let policy = cfg.removal_policy(Some(RemoveMode::Trash)).unwrap();
        assert_eq!(policy.mode, RemoveMode::Trash);
        assert_eq!(policy.trash_max, 1024 * 1024 * 1024);
    }

    #[test]
    fn rejects_bad_values() {
        assert!(toml::from_str::<Config>("remove_mode = \"shred\"").is_err());
        let cfg: Config = toml::from_str("trash_max_size = \"lots\"").unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn save_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        let mut cfg = Config {
            warn_below: Some("1GB".into()),
            remove_mode: RemoveMode::Trash,
            ..Config::default()
        };
        cfg.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), cfg);
        // Without a warning size the field is simply absent, and loads back as None.
        cfg.warn_below = None;
        cfg.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), cfg);
    }

    #[test]
    fn save_refuses_an_invalid_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = Config {
            trash_max_size: "lots".into(),
            ..Config::default()
        };
        assert!(cfg.save(&path).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn check_fields() {
        let cfg: Config =
            toml::from_str("check_every = \"weekly\"\nwarn_below = \"50GB\"").unwrap();
        cfg.validate().unwrap();
        assert_eq!(
            cfg.check_interval().unwrap(),
            Some(crate::schedule::Every::Weekly)
        );
        assert_eq!(
            cfg.warn_below_bytes().unwrap(),
            Some(50 * 1024 * 1024 * 1024)
        );
        assert_eq!(Config::default().check_interval().unwrap(), None);
        let bad: Config = toml::from_str("check_every = \"hourly\"").unwrap();
        assert!(bad.validate().is_err());
        let bad: Config = toml::from_str("warn_below = \"lots\"").unwrap();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("100MB").unwrap(), 100 * 1024 * 1024);
        assert_eq!(parse_size("5GiB").unwrap(), 5 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("2048").unwrap(), 2048);
        assert!(parse_size("12XB").is_err());
    }
}
