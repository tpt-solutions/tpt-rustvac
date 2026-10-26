//! The tpt-rustvac window. The page in `web/` is embedded in the binary and hosted by
//! tpt-shell; it talks to the scan through the commands in `commands.rs`.

mod commands;
mod model;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tpt_shell::{embed_dir, App, Grant, Scope, Window};

use model::Model;

/// The origin tpt-shell serves the embedded page from. Grants name it.
const ORIGIN: &str = "https://tpt-app.localhost";
const LOADER: &str = "WebView2Loader.dll";

fn main() -> tpt_shell::Result<()> {
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
