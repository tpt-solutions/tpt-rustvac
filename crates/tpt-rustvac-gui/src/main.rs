//! The tpt-rustvac window. The page in `web/` is embedded in the binary and hosted by
//! tpt-shell; it talks to the scan through the commands in `commands.rs`.

mod commands;
mod model;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tpt_rustvac_core::config::default_config_path;
use tpt_shell::{embed_dir, App, Grant, Scope, Window};

use model::Model;

/// The origin tpt-shell serves the embedded page from. Grants name it.
const ORIGIN: &str = "https://tpt-app.localhost";
const LOADER: &str = "WebView2Loader.dll";

fn main() -> tpt_shell::Result<()> {
    install_crash_log();
    let mut model = Model::new();
    model.start_scan();
    let model = Arc::new(Mutex::new(model));

    let app = App::new()
        .name("rustvac")
        .loader(loader_path())
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

/// `WebView2Loader.dll` from the app's folder, or the path in `RUSTVAC_WEBVIEW2_LOADER`.
fn loader_path() -> PathBuf {
    if let Some(path) = std::env::var_os("RUSTVAC_WEBVIEW2_LOADER") {
        return path.into();
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(LOADER)))
        .unwrap_or_else(|| PathBuf::from(LOADER))
}
