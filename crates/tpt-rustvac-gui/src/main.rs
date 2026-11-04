//! The tpt-rustvac window. The page in `web/` is embedded in the binary and hosted by
//! tpt-shell; it talks to the scan through the commands in `commands.rs`.

mod commands;
mod model;
mod ui_state;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tpt_rustvac_core::config::default_config_path;
use tpt_shell::{embed_dir, App, Grant, Scope, Window};

use model::Model;

/// The origin tpt-shell serves the embedded page from. Grants name it.
const ORIGIN: &str = "https://tpt-app.localhost";

/// The WebView2 loader, compiled in so the exe runs on its own. See `vendor/NOTICE.md`.
#[cfg(windows)]
const WEBVIEW2_LOADER: &[u8] = include_bytes!("../vendor/WebView2Loader.dll");

fn main() -> tpt_shell::Result<()> {
    install_crash_log();
    let loader = loader_path()?;
    let mut model = Model::new();
    model.start_scan();
    let model = Arc::new(Mutex::new(model));

    let mut app = App::new().name("rustvac");
    if let Some(loader) = loader {
        app = app.loader(loader);
    }
    let app = app
        .assets(embed_dir!("web"))
        .window(Window::new("main").title("tpt-rustvac").size(1100.0, 640.0))
        // The page is the app's own UI, so the grants cover every path the user can pick.
        .grant(grant("fs.read"))
        .grant(grant("fs.write"));
    commands::register(app, &model).run()
}

fn grant(capability: &str) -> Grant {
    Grant {
        capability: capability.into(),
        window: "main".into(),
        origin: ORIGIN.into(),
        scope: Scope::Unrestricted,
        consent: false,
    }
}

/// Appends every panic, on any thread, to `rustvac.log` beside `config.toml`, with a backtrace.
/// The default hook still prints to stderr. Logging failures are ignored, so they never
/// hide the original panic.
fn install_crash_log() {
    let log =
        default_config_path().and_then(|config| config.parent().map(|dir| dir.join("rustvac.log")));
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let Some(log) = &log else { return };
        let when = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let backtrace = std::backtrace::Backtrace::force_capture();
        let entry = format!("[unix {when}] panic: {info}\n{backtrace}\n");
        if let Some(dir) = log.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
        {
            use std::io::Write;
            let _ = file.write_all(entry.as_bytes());
        }
    }));
}

/// The WebView2 loader to use: the file named by `RUSTVAC_WEBVIEW2_LOADER` if set, else the
/// copy compiled into the exe, written out on first use. `None` off Windows, where no loader
/// is needed.
fn loader_path() -> tpt_shell::Result<Option<PathBuf>> {
    if let Some(path) = std::env::var_os("RUSTVAC_WEBVIEW2_LOADER") {
        return Ok(Some(path.into()));
    }
    extract_loader()
}

#[cfg(windows)]
fn extract_loader() -> tpt_shell::Result<Option<PathBuf>> {
    let dir = default_config_path()
        .and_then(|config| config.parent().map(|dir| dir.join("runtime")))
        .unwrap_or_else(|| std::env::temp_dir().join("rustvac"));
    // The name carries a hash of the contents, so a new build never overwrites a loader that
    // another running copy has open.
    let path = dir.join(format!("WebView2Loader-{:016x}.dll", fnv1a(WEBVIEW2_LOADER)));
    let fail = |e: std::io::Error| {
        tpt_shell::Error::Backend(format!(
            "could not write WebView2Loader.dll to {}: {e}",
            dir.display()
        ))
    };
    let present = std::fs::metadata(&path).is_ok_and(|m| m.len() == WEBVIEW2_LOADER.len() as u64);
    if !present {
        std::fs::create_dir_all(&dir).map_err(fail)?;
        let tmp = dir.join(format!("WebView2Loader-{}.tmp", std::process::id()));
        std::fs::write(&tmp, WEBVIEW2_LOADER).map_err(fail)?;
        std::fs::rename(&tmp, &path).map_err(fail)?;
    }
    Ok(Some(path))
}

#[cfg(not(windows))]
fn extract_loader() -> tpt_shell::Result<Option<PathBuf>> {
    Ok(None)
}

/// 64-bit FNV-1a. Only names the extracted loader; it is not a security check.
#[cfg(windows)]
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, &b| (hash ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}
