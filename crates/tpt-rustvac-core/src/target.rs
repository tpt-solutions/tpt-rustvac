//! Resolves where a Cargo project's `target/` directory actually lives.
//!
//! Order: `CARGO_TARGET_DIR`, then `build.target-dir` in `<project>/.cargo/config.toml`,
//! then `~/.cargo/config.toml`, then `<project>/target`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Inputs that come from the process environment. Split out so tests can supply them.
#[derive(Debug, Clone, Default)]
pub struct TargetEnv {
    pub cargo_target_dir: Option<PathBuf>,
    pub home: Option<PathBuf>,
}

impl TargetEnv {
    pub fn from_process() -> Self {
        TargetEnv {
            cargo_target_dir: std::env::var_os("CARGO_TARGET_DIR")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            home: crate::config::home_dir(),
        }
    }
}

#[derive(Deserialize, Default)]
struct CargoConfig {
    #[serde(default)]
    build: BuildSection,
}

#[derive(Deserialize, Default)]
struct BuildSection {
    #[serde(rename = "target-dir")]
    target_dir: Option<String>,
}

pub fn resolve_target_dir(project: &Path, env: &TargetEnv) -> PathBuf {
    if let Some(dir) = &env.cargo_target_dir {
        return join_if_relative(project, dir);
    }
    let project_cfg = project.join(".cargo").join("config.toml");
    if let Some(dir) = config_target_dir(&project_cfg, project) {
        return dir;
    }
    if let Some(home) = &env.home {
        let home_cfg = home.join(".cargo").join("config.toml");
        if let Some(dir) = config_target_dir(&home_cfg, home) {
            return dir;
        }
    }
    project.join("target")
}

/// Reads `build.target-dir` from a cargo config file. Relative values are
/// resolved against the directory that contains `.cargo`, as cargo does.
fn config_target_dir(config_file: &Path, base: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(config_file).ok()?;
    let parsed: CargoConfig = toml::from_str(&text).ok()?;
    let value = parsed.build.target_dir?;
    Some(join_if_relative(base, Path::new(&value)))
}

fn join_if_relative(base: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}
