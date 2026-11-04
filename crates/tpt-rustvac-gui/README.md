# tpt-rustvac-gui

The `rustvac-gui` window: the [rustvac](https://github.com/tpt-solutions/tpt-rustvac)
engine behind a tree view, hosted by [tpt-shell](https://github.com/tpt-solutions/tpt-shell).

- Rust Targets, Any Folder and Mixed modes
- Filters and keyboard shortcuts
- Two-step confirmation before anything is archived

Windows only so far. It uses WebView2. The WebView2 loader (`vendor/WebView2Loader.dll`) is compiled
into the binary and written to `%APPDATA%\rustvac\runtime\` on first run. Set
`RUSTVAC_WEBVIEW2_LOADER` to use a different loader. See `vendor/NOTICE.md`.

Licensed under MIT or Apache-2.0, at your option.
