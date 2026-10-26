# tpt-rustvac

Find your Rust target folders. Measure any folder. Vacuum the rest.

`rustvac` finds Cargo `target/` folders, shows how big they are and how long since
they were built, and archives the ones you don't need. `rustvac-gui` is the same
engine behind a native window.

## Usage

```
rustvac scan --path ~/Code --older-than 2d     # Rust targets, size and age
rustvac list                                   # same, as a table, largest first
rustvac measure ~/Downloads --depth 3 --top 20 # any folder, by subfolder
rustvac measure ~/Downloads --min-size 100MB
rustvac compress --older-than 2d --dry-run     # show the plan, change nothing
rustvac compress --older-than 2d               # confirm, then archive and trash
rustvac restore <slug> --into ~/Code/my-project
rustvac archive list
rustvac archive rm <slug>
rustvac gui
```

Add `--json` to `scan` or `measure` for scripting.

## Safety

- Scanning is read-only.
- Nothing is removed without a confirmation. `compress` asks first, unless you pass `--yes`.
- Archives are verified (zstd checksum, tar structure, file count and byte total) before
  the original is touched.
- The original `target/` is moved to the Recycle Bin / Trash, not deleted.
- A target modified in the last 60 seconds is marked busy and is never compressed.
  This is configurable with `busy_grace_seconds`.
- Scanning `/`, a drive root or your home folder is refused unless you pass `--force`.
- Folders in the `keep` list are never compressed.

## Configuration

`%APPDATA%\rustvac\config.toml` on Windows, `~/.config/rustvac/config.toml` elsewhere.

```toml
roots = ["~/Code", "~/side-projects"]
older_than_days = 2
archive_dir = "~/.rustvac/archive"
format = "tar.zst"
exclude = ["**/vendor/**", "**/node_modules/**"]
keep = ["~/Code/the-one-project-that-takes-40-minutes-to-build"]
busy_grace_seconds = 60
```

## Building

```
cargo build --release
cargo run -p tpt-rustvac-cli -- scan --path ./fixtures/generated
cargo run -p tpt-rustvac-gui
```

To build a visible fixture tree for trying things out:

```
cargo run -p tpt-rustvac-core --example make_fixtures -- fixtures/generated
```

## Layout

| Crate | Purpose |
| --- | --- |
| `tpt-rustvac-core` | Scan, measure, archive, restore. No UI code. |
| `tpt-rustvac-cli` | The `rustvac` binary. |
| `tpt-rustvac-gui` | The `rustvac-gui` binary. A web UI in a window, hosted by tpt-shell (WebView2 on Windows). Needs `WebView2Loader.dll` next to the binary. |

## Scope (v1)

In: Rust target discovery, general folder measurement, archive and restore, CLI and GUI.

Out: dependency deduplication, sccache integration, per-crate analysis, cross-machine sync,
network and cloud drives.

## Licence

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Copyright (c) TPT Solutions.

Third-party code: the zstd compression library bundled by `zstd-sys` is available under
BSD-3-Clause (its `LICENSE`) or GPL-2.0 (its `COPYING`). This project uses the BSD-3-Clause option.
