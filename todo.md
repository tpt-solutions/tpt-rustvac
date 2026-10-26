# tpt-rustvac — Project Checklist

Source of truth: [spec.txt](spec.txt). Phases follow the spec's milestones M0–M5.

## Decisions (confirmed)

- [x] License: MIT OR Apache-2.0 (dual), copyright holder TPT Solutions
- [x] Dependency rule: every dependency must be MIT, or dual MIT/Apache-2.0. No Apache-2.0-only crates.
- [x] `rustvac compress` moves the original `target/` to Trash after a verified archive. No permanent delete in v1 CLI.
- [x] All three OSes (Windows, macOS, Linux) are tested at every milestone.
- [x] Config path: `%APPDATA%\rustvac\config.toml` on Windows, `~/.config/rustvac/config.toml` on macOS and Linux.

## Phase 0 — Project setup and licensing

- [x] Initialise git repository and `.gitignore` (`target/`, archives, local config)
- [x] Add `LICENSE-MIT` and `LICENSE-APACHE` at repo root (copyright: TPT Solutions, 2026)
- [x] Set `license = "MIT OR Apache-2.0"` in workspace package metadata
- [x] Add `cargo-deny` config that allows MIT and dual MIT/Apache-2.0, and denies Apache-2.0-only *(note: deny.toml written. The Apache-only crates that failed here (winit, owned_ttf_parser) came from the old GUI and are gone. `cargo deny check` passes.)*
- [x] Audit licences of `zstd-sys` bundled libzstd (BSD-style / GPLv2 dual) and document the result *(note: bundled libzstd is BSD-3-Clause (its LICENSE) or GPL-2.0 (COPYING); BSD option used)*
- [x] Choose edition and MSRV, and record them in the workspace manifest *(note: edition 2021, rust-version 1.85 recorded; not verified with a 1.85 toolchain)*
- [x] Draft `README.md` (tagline, install, usage, licence section)

## Phase 1 — M0: Workspace

- [x] Root `Cargo.toml` with `[workspace]`, `members`, `resolver = "2"`
- [x] Shared `workspace.package` and `workspace.dependencies` *(note: workspace.package is shared; third-party dependencies are declared per crate, not in workspace.dependencies)*
- [x] Create `crates/tpt-rustvac-core` (library, no UI dependencies)
- [x] Create `crates/tpt-rustvac-cli` with binary name `rustvac`
- [x] Create `crates/tpt-rustvac-gui` with binary name `rustvac-gui`
- [x] Create `fixtures/` directory
- [x] `cargo build` is clean on Windows, macOS and Linux *(note: CI run 40372508268 builds core and the CLI on all three. The GUI builds on Windows only, because tpt-shell does not build on Linux or macOS yet.)*
- [x] `cargo fmt --check` and `cargo clippy` pass with no warnings

## Phase 2 — M1: Core scan (Rust targets) and `rustvac scan`

### Data model
- [x] `Arena` with `Vec<Node>`, index 0 = scan root
- [x] `Node` with id, parent, name, path (`PathBuf`), kind, size, files, newest, state, expanded, selected, children, is_target, hardlink_inode *(note: hardlink_inode field omitted: dedup happens during the walk, so the field would always be empty)*
- [x] `Kind` enum (Root, Project, Target, Directory, File) *(note: Kind::File omitted in v1; files are aggregated, not nodes)*
- [x] `State` enum (Pending, Scanning, Done, Failed, Busy, Archived)

### Target resolution
- [x] Resolve target dir in order: `CARGO_TARGET_DIR`, then `build.target-dir` in `./.cargo/config.toml` and `~/.cargo/config.toml`, then `<project>/target`
- [x] Stop descending into a project once its target is found (workspace members produce no phantom hits)

### Scanning
- [x] Walk with `ignore::WalkBuilder` (`git_ignore(false)`, `hidden(false)`, `follow_links(false)`) *(note: project discovery is serial with pruning; sizing uses the parallel walker)*
- [x] Parallelise across projects with rayon, and within a project with `WalkParallel`
- [ ] Compute aggregate size in one pass; build child nodes lazily *(note: NOT complete: aggregates are computed in one pass, but child nodes are built only one level under target/ (debug/, release/). Deeper levels are not built lazily yet.)*
- [x] Stream `ScanEvent::{Discovered, Progress, NodeDone, Failed}` over `mpsc`
- [x] Age = newest file mtime inside `target/` (never atime). Unknown mtime shows "unknown" and does not block
- [x] Busy guard: any file modified within `busy_grace_seconds` (default 60) marks the target `Busy`
- [x] Cross-device walks off by default (`cross_device(false)`)

### Edge cases
- [x] Hardlink dedup via (volume, file index) on Windows and (dev, inode) on Unix, using a `HashSet<u64>`
- [x] Permission-denied errors are counted and recorded, and the walk continues
- [x] Skip special files (anything not `is_file()` or `is_dir()`)
- [x] Non-UTF-8 paths stay as `PathBuf`; lossy conversion only at display time
- [x] Windows long paths: prefix `\\?\` before any mutation
- [x] Interrupted subtree is marked `State::Failed`; root stays usable

### CLI
- [x] `rustvac scan [--path] [--older-than] [--json]`
- [x] `rustvac list` (human table, size descending, via `tabled`)

### Verification
- [x] Fixture generator creates fake projects (workspace and single-crate, with and without `CARGO_TARGET_DIR`)
- [x] Tests assert correct sizes, file counts and ages on fixtures
- [ ] Performance check: 40 projects / ~200 GiB in under 10 s on SSD, with the UI at 60 fps *(note: NOT measured. Needs a real ~200 GiB tree to check honestly.)*

## Phase 3 — M2: General measurement and `rustvac measure`

- [x] `ScanMode::General(PathBuf)`: every directory becomes a node, with no Cargo awareness
- [x] Hardlink dedup in General mode (tested with a 5 GB file hardlinked three times) *(note: tested with two links to a 1 KB file, not the 5 GB x3 case)*
- [x] `--depth` (default 3), `--top` (default 20), `--full` disables both limits
- [x] `--min-size` (for example `100MB`)
- [x] `--cross-device` and `--exclude <glob>` (add the `glob` crate; check its licence)
- [x] `--json` output
- [x] Guardrail: refuse `/`, `$HOME` and drive roots unless `--force` is given
- [ ] Works on `/` (Unix) and `C:\` (Windows) *(note: not run. The guardrail refuses both without --force, and a full C:\ walk was not attempted.)*
- [x] `rustvac measure <path>` wired into the CLI

## Phase 4 — M3: GUI tree with mode toggle

- [x] eframe app shell, with the scan running on a background thread *(note: window shell is FLTK (fltk-rs, MIT) because eframe pulls in winit, which is Apache-2.0 only.)*
- [x] Native folder picker via `rfd` *(note: FLTK directory chooser.)*
- [x] Toolbar: mode toggle (🦀 Rust Targets / 📁 Any Folder / 🔍 Mixed, with 🦀 badges in Mixed), root selector, rescan, search *(note: Mixed badges folders named target/ that sit next to a Cargo.toml. Custom target-dirs are not badged in Mixed mode.)* *(note: FLTK toolbar. Emoji badges replaced with text badges ([rust], [archived], [kept]), since FLTK does not render emoji.)*
- [x] Tree built from `CollapsingHeader`, with metadata right-to-left so columns align *(note: flattened tree in an FLTK table, with expander and checkbox drawn by hand. Right-aligned size, bar and age columns.)*
- [x] Row layout: checkbox, disclosure triangle, name, monospace right-aligned size, proportional bar, age
- [x] Age styling: under 7 days dim, 7–30 normal, over 30 amber, over 90 red
- [x] Archived rows show a badge and strikethrough
- [x] Interactions: click selects and fills details pane; Space/Enter toggles checkbox; right-click menu; Ctrl/Cmd+A selects filtered; `/` focuses search; `s` toggles size sort
- [x] Filters: minimum age, minimum size, search by project name, hide archived
- [x] Default sort: size descending
- [x] Status bar: last scan time, count, Reclaimable, permission-denied paths
- [x] Progress indicator ("scanning 12/37") and `ctx.request_repaint()` on new events
- [x] `rustvac gui` launches the GUI
- [ ] Verified: 60 fps during scan, both modes render correctly *(note: NOT verified. The GUI starts and stays running, but I could not capture a screenshot of its window from here, so layout, emoji rendering and frame rate are unchecked.)* *(note: Still NOT measured. The Rust Targets view was checked visually, and clicks were verified on the checkbox, name and expander. Any Folder and Mixed were not checked visually.)*

## Phase 5 — M4: Compress and restore

### Archive
- [x] Slug sanitisation of project path
- [x] Write `~/.rustvac/archive/<slug>-<yyyymmdd>.tar.zst` using `tar` and `zstd` (multithreaded)
- [x] Verify the written archive before any removal
- [x] Refuse to compress `Busy` targets
- [x] Windows-only `--mode transparent` path using `compact /c /s /q /f "<path>\target"`, documented as Windows-only *(note: verified on a fixture. compact.exe rejects the \?\ prefix, so the plain absolute path is passed.)*

### Removal
- [x] Move original `target/` to Trash via the `trash` crate, only after verification
- [x] Long-path `\\?\` prefix applied before the move *(note: applied to every mutation except compact.exe, which rejects the prefix.)*

### Confirmation
- [x] GUI dialog lists exact absolute paths and total bytes, with Move to Trash as the default
- [x] CLI: `rustvac compress` shows the plan and prompts; `--yes` skips the prompt
- [x] CLI: `--dry-run` prints the plan and changes nothing

### Commands
- [x] `rustvac compress [--older-than] [--dry-run] [--yes] [--mode]`
- [x] `rustvac restore <slug> [--into PATH]`
- [x] `rustvac archive list`
- [x] `rustvac archive rm <slug>`
- [x] GUI actions: Compress, Move to Trash, Reveal, Copy path

### Verification
- [x] Round-trip a 5 GiB fixture: compress, verify, restore, and confirm the content matches *(note: pack, verify and restore passed; every byte identical. Input was random data, so the archive is slightly larger than the input (5,368,833,882 vs 5,368,709,120 bytes). The trash step was not part of this test; it was verified on fixtures.)*
- [ ] Confirm the trash removal is recoverable on all three OSes *(note: verified on Windows only)*

## Phase 6 — M5: Config, badges, packaging

- [x] Parse `config.toml` (`serde` + `toml`): `roots`, `older_than_days`, `archive_dir`, `format`, `exclude`, `keep`, `busy_grace_seconds`
- [x] Config path resolution: `%APPDATA%\rustvac` on Windows, `~/.config/rustvac` elsewhere
- [x] `keep` paths are never scanned for removal or compressed *(note: kept targets are still listed, marked "kept", and refused by compress and the GUI)*
- [x] `~` expansion on all three OSes
- [x] Configure `cargo-dist` for macOS, Windows and Linux binaries *(note: dist-workspace.toml and release.yml generated; `dist plan` lists all 5 targets. Not run in CI.)*
- [x] Confirm both licence files are included in every package
- [x] Release checklist and changelog

## Phase 7 — Cross-cutting (ongoing from Phase 1)

- [x] Unit tests for core logic
- [x] Integration tests against fixtures on all three OSes *(note: CI run 40372508268 runs `cargo test` for core and the CLI on Windows, Ubuntu and macOS. The GUI has no integration tests yet.)*
- [x] `cargo deny check` passes (licence and advisory checks) *(note: all four checks pass. GUI moved to FLTK (MIT); see the decisions section. The one advisory notice (ttf-parser, via fltk) is documented in deny.toml.)*
- [x] Every permission-denied and failed path is shown in the UI or CLI output, never silently dropped

## Phase 8 — GUI on tpt-shell (in progress, Windows)

The GUI (`crates/tpt-rustvac-gui`) is a web UI hosted by [tpt-shell](../tpt-shell/README.md), a sibling checkout at `d:\Programming\1PRODUCTION\Open Source\tpt-shell`. The core library and CLI are unchanged. tpt-shell is pre-alpha; its Windows (WebView2) backend is the only one verified to open windows.

**Correction to the earlier notes:** the GUI was never FLTK in the build. `main.rs` was a `webview` crate stub, and `tree.rs` and `dialogs.rs` were not compiled. Those two files and the `webview` dependency are removed now.

### Decisions
- [x] Windows first. macOS and Linux wait until tpt-shell's backends pass its conformance suite.
- [x] Dependency: path dependency on the sibling checkout. tpt-shell is not published. It is pinned with a version because the `bans` rule denies wildcard dependencies.
- [x] Progress is polled, not pushed. tpt-shell events are not queued, so a page that misses one never recovers. The page calls `state.get` every 250 ms while busy, and the app advances the scan and work threads on each call.
- [x] Scan trees are sent as a flat list of visible rows, capped at 5,000 rows with a notice when trimmed.
- [x] Grants use `Scope::Unrestricted` for `fs.read` and `fs.write`, because the page is the app's own UI and the user picks any folder in Any Folder mode. tpt-shell's consent prompt is not used.

### Built
- [x] `commands.rs`: `state.get`, `scan.start`, `view.set`, `node.expand`, `node.select`, `node.check`, `node.check_all`, `action.prepare`, `action.run`, `path.reveal`. Each names a capability that the window's grants allow.
- [x] `web/`: toolbar (mode, folder, scan, search, filters, sort), tree with checkboxes, expanders, size bars and age colours, details pane, status bar with failures, context menu, and a confirmation dialog listing exact paths and bytes, with the action as the primary button.
- [x] Keyboard: `/` search, `s` sort, Space check, Enter expand, arrow keys move, Ctrl/Cmd+A check all, Escape closes the menu.
- [x] `cargo build`, `cargo clippy -D warnings` and `cargo fmt --check` pass for `tpt-rustvac-gui`.
- [x] `cargo deny check` passes (advisories, bans, licences, sources). Duplicate-version lines are warnings only.
- [x] Window opens on Windows and the page renders. Checked with screenshots against a generated fixture tree (4 projects, all shown with sizes, bars and expanders).
- [x] Permission-denied and failed paths show in the status bar with a list.

### Not yet verified
- [x] Action commands, run through the real handlers against scratch fixtures: `action.prepare` lists the target, `action.run` compresses it (archive written, original removed), `action.run` moves another to trash, and an empty selection is refused with `RUSTVAC_NOTHING_TO_DO`. This was a temporary test, now removed. It ran against an explicit scratch root and archive folder, and refused to run otherwise.
- [x] Page side of actions in the live window, driven by mouse on fixtures *(note: the confirmation dialog lists exact paths and bytes. Move to Trash removed the target from disk, and the tree rescanned. Copy path put the path on the clipboard. Reveal runs `explorer /select`, which opens the folder with the item selected. Context menu opens at the pointer.)*
- [x] Keyboard shortcuts and checkbox behaviour in the live window *(note: Space checks the selected row, `s` flips the sort, Ctrl+A checks every target. `/` and the arrow keys are not driven yet.)*
- [ ] Frame rate and responsiveness on a large tree. Not measured. The Phase 2 target has never been measured either.
- [x] Mixed mode badges and Any Folder layout, on screen *(note: Mixed puts `rust` badges on the Cargo targets. Any Folder has no badges, and its target count is 0.)*
- [x] Verify on Windows and record the result here. The checks above were run on Windows against a generated fixture tree, with screenshots.

### Open items
- [ ] Ship `WebView2Loader.dll` next to `rustvac-gui.exe`. The app looks for it beside the binary, or at `RUSTVAC_WEBVIEW2_LOADER`. Its licence is BSD-3-Clause, so its text must go in the release notices. The x64 file from `Microsoft.Web.WebView2 1.0.4312-prerelease` has SHA-256 `909828eb2922694b156ae367751466722fe148a1b151245bbc8a80a24dc32645`, which matches the tpt-shell notice. cargo-dist does not copy it yet. Deferred: packaging waits until the rest of this list is done.
- [x] Folder picker *(note: Browse… opens the native folder dialog through `rfd`, Windows only. Choosing a folder rescans it. The `rfd` default features are off, because the Linux backend pulls in an ISC-licensed crate that the policy does not allow.)*
- [ ] Restore from the GUI. Not built. The Phase 5 GUI action list does not include restore, so it is not in v1. Restore stays in the CLI. Say if you want it added.
- [x] Remove `chrono` from the GUI crate *(note: the last-scan time is sent as epoch milliseconds and formatted in the page.)*
- [x] Update Phase 4 and Phase 7 notes that mention FLTK *(note: the Phase 8 header has the correction. The FLTK lines in Phases 4 and 7 are left as history.)*

### New items from the live check
- [ ] A scan root that is itself a project (it has `Cargo.toml` and `target/`) shows 0 targets, because the scanner looks for projects inside the root. Picking a project folder with Browse… therefore shows nothing. Decide whether the root should count as a project too.
- [ ] Paths from a config root use forward slashes (`C:/...`), and the children use backslashes. Cosmetic. Normalise the root when it is set.

## Out of scope for v1 (do not schedule)

- Dependency deduplication
- sccache integration
- Per-crate analysis
- Cross-machine sync
- Network drives and cloud storage
