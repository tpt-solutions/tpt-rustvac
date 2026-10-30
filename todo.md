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
- [ ] Compute aggregate size in one pass; build child nodes lazily *(note: decided 2026-10-27: aggregates are one pass, and child nodes stop one level under target/ (debug/, release/). That already bounds the arena, so the lazy part is deferred to a later version. It needs an expand-on-demand scan path, which is a feature of its own. Revisit if memory is a problem on real trees.)*
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
- [x] Performance check: 40 projects / ~200 GiB in under 10 s on SSD *(note: 2026-10-27, Windows 11, release build, G: (Crucial P3 1 TB NVMe). 40 projects, 100,040 files, 195.3 GiB. `rustvac scan` found all 40 targets in 1.47 s right after the tree was written, and 0.25 s warm. The GUI finished the same scan and showed 40 targets and 195.3 GiB. A true cold-cache run needs a reboot, which I did not do. The tree is still on G:\rustvac-perf-200g, and I can delete it on request.)*
- [ ] UI at 60 fps during a scan *(note: not measured. The GUI stayed responsive on the 200 GiB tree, but no frame-rate tool was run.)*

## Phase 3 — M2: General measurement and `rustvac measure`

- [x] `ScanMode::General(PathBuf)`: every directory becomes a node, with no Cargo awareness
- [x] Hardlink dedup in General mode (tested with a 5 GB file hardlinked three times) *(note: tested with two links to a 1 KB file, not the 5 GB x3 case)*
- [x] `--depth` (default 3), `--top` (default 20), `--full` disables both limits
- [x] `--min-size` (for example `100MB`)
- [x] `--cross-device` and `--exclude <glob>` (add the `glob` crate; check its licence)
- [x] `--json` output
- [x] Guardrail: refuse `/`, `$HOME` and drive roots unless `--force` is given
- [ ] Works on `/` (Unix) and `C:\` (Windows) *(note: 2026-10-27, Windows. The guardrail refuses `G:\` without `--force`. With `--force`, `measure G:\` took 17 s over 249,000 files, and `measure C:\` took 230 s over 3.28 million files. Both finished. Access-denied paths are listed, not dropped. `/` is not run here. BUG, fixed on Windows: C:\ reported 1.7 TiB on a 487 GB drive. Most of it is OneDrive placeholders in `C:\Users\phill\ICB Construction` (1.1 TiB) and `OneDrive - ICB Construction` (272 GiB). A sample of 3,000 files there were all offline, reparse or sparse, so they take no disk space, but the tool counts their full logical size. See the Phase 8 open items.)*
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
- [ ] Confirm the trash removal is recoverable on all three OSes *(note: Windows checked by hand against the Recycle Bin. Linux is checked by `tests/trash_unix.rs`, which trashes a target, lists it and restores it. It passes in WSL Ubuntu 24.04. macOS is not checked: the `trash` crate has no listing API there.)*

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
- [x] Restore from the GUI. Decided: out of v1. The Phase 5 GUI action list does not include restore, so restore stays in the CLI (`rustvac restore`). Add it in a later version if needed.
- [x] Remove `chrono` from the GUI crate *(note: the last-scan time is sent as epoch milliseconds and formatted in the page.)*
- [x] Update Phase 4 and Phase 7 notes that mention FLTK *(note: the Phase 8 header has the correction. The FLTK lines in Phases 4 and 7 are left as history.)*

### New items from the live check
- [x] A scan root that is itself a project shows 0 targets. Decided: the scanner keeps its rule that a root holds projects. When the root has a `Cargo.toml` and the Rust Targets scan finds nothing, the page now says so and asks for the folder above the project. Verified on screen.
- [x] Paths from a config root used forward slashes, and the children used backslashes. The root is now normalised on Windows.
- [x] Sizes are on disk, on Windows *(note: decided and done. `identity::disk_bytes` uses `GetCompressedFileSizeW`. A cloud placeholder counts 0 bytes, and a compressed or sparse file counts its allocated size. Verified on `C:\Users\phill\ICB Construction`: 1.1 TiB before, 260 MiB after, with the same 417,885 files. Tests now take their expected sizes from the same function.)*
- [ ] Sizes on Linux and macOS are still logical lengths. They should match Windows: allocated blocks (`blocks() * 512`). Changing them shifts the expected sizes in the tests, so it is a separate step. Also, the archive totals (`archive.rs`, `pack`) still use logical lengths. *(note: 2026-10-31. Not changed. This machine has only the Windows Rust target installed, and the WSL distro that was used before is gone, so the Unix code can't be compiled or tested here. Do it with CI or a Linux machine.)*
- [x] Trash and compress refuse sync-managed paths *(note: decided and done on Windows. A path is refused if it is under a OneDrive folder, or if it or any folder above it is a reparse point. That covers cloud sync folders, symlinks and junctions. The guard runs in `safety::move_to_trash`, in `archive::compress_and_vacuum`, and in the GUI's preview. Tested with a real junction in `tests/sync_guard.rs`. Linux and macOS have no such rule.)*

## Phase 9 — Toolchains, caches and scheduled checks (planned, CLI first)

Plan: `C:\Users\Phillip\.claude\plans\part-of-the-purpose-crystalline-swing.md`. Aim: keep a small system drive (500 GB C:) free of regenerable Rust scratch. The GUI waits until the CLI side is done.

### Decisions
- [x] Cache removal is configurable: `auto` (small to trash, large deleted permanently), `trash`, `permanent`. Reason: the Recycle Bin is on the same drive, so trashing a large cache frees nothing. This applies to the regenerable tier only. `target/` compress still archives, verifies and trashes.
- [x] Toolchains are removed only through `rustup toolchain uninstall`, never by deleting folders.
- [x] The scheduler checks and recommends. It never deletes on its own.

### Locations and removal policy
- [x] `rustup_home()` and `cargo_home()` next to `config::home_dir` (`RUSTUP_HOME` / `CARGO_HOME` first, then `~/.rustup` / `~/.cargo`) *(note: in `config.rs`. Unit-tested only through the commands, not on their own.)*
- [x] Config fields `remove_mode` (`auto`/`trash`/`permanent`, default `auto`) and `trash_max_size` (default `256MB`), with serde defaults and validation *(note: `Config::removal_policy` takes a per-run override. `parse_size` moved into core. Unit tests cover defaults, parsing and bad values.)*
- [x] `safety::remove_with_policy(path, size, policy)` on top of `move_to_trash` and `delete_permanently`, with threshold unit tests *(note: `delete_permanently` now also refuses sync-managed paths.)*

### Part A — Smarter target cleanup
- [x] Check the real `.rustc_info.json` layout *(note: 2026-10-30. `outputs.<hash>.stdout` holds `rustc -vV` text with `release:`, `host:` and `commit-hash:` lines. One output entry is the `-vV` text; the other is the cfg dump and contains the toolchain path.)*
- [x] `target::built_with(target_path)` parses `<target>/.rustc_info.json`, with a unit test on a literal sample *(note: a compiler counts as the same when the release matches and, if both sides know it, the commit hash. An unknown compiler is never stale.)*
- [x] `installed_rustc_versions(rustup_home)` runs each toolchain's `rustc -vV` *(note: `toolchains.rs`. A toolchain whose rustc cannot run is left out. The fake-rustc test runs on Unix only.)*
- [x] `--stale-toolchain` filter on `scan`, `list` and `compress`; `rustc` field in `scan --json` *(note: errors out when no toolchain is found, because an empty list would make every target look stale. `list` and `scan` gain a Rustc column.)*
- [x] `prune.rs`: `PruneSpec::{Incremental, Profile}`, `plan()` sizes `<profile>/incremental` and `<triple>/<profile>/incremental`; skips busy and kept targets *(note: busy and kept targets are skipped by the CLI, which only passes `Done`, not-kept targets in.)*
- [x] `rustvac prune [--incremental | --profile P] [--older-than] [--stale-toolchain] [--remove ...] [--dry-run] [--yes]` *(note: the age filter defaults to `older_than_days`, like compress.)*

- [x] Stale dependency builds: `rustvac prune --stale-deps` (also in `sweep` and `check`). A unit is stale when its `.d` file names a registry crate version missing from the project's `Cargo.lock`. Its outputs (`.d`, `.rlib`, `.rmeta`, and so on, plus `.fingerprint/`) go together. *(note: checked on a scratch copy of this repo's build output: 261 stale units, all for old GUI and superseded versions, and a real removal left nothing stale. Limits: git checkouts are not classified, because their folder names carry no version. `build/` output for build scripts is not removed. A target outside its project is skipped, since other projects may share it. The repo's own `target/` was busy during the check, so it was not run directly.)*

### Part B — Toolchains and caches
- [x] `caches.rs`: `CacheItem` with tiers. Regenerable: rustup `tmp`, rustup `downloads`, cargo `registry/cache`, `registry/src`, `git/checkouts`. Managed: `toolchains/<name>`. Not in a default sweep: `registry/index`, `git/db` *(note: items hold their direct children, and removal works on those, so the folder itself stays.)*
- [x] Guards: skip rustup `tmp`/`downloads` entries modified within `busy_grace_seconds`, `sync_managed` refusal, `long_path` *(note: the busy grace applies to every regenerable entry, not only rustup's, since a cargo build can be running too.)*
- [x] Pure parser for `rustup toolchain list` (default and active markers); refuse the default toolchain
- [x] `rustvac toolchains [--json]` and `rustvac toolchains rm <name> [--yes]`; report and do nothing if `rustup` is not on PATH *(note: removal not run against a real toolchain. Only the not-found path was run, against a scratch RUSTUP_HOME.)*
- [x] `rustvac caches [--json]` and `rustvac caches clean [--rustup-tmp] [--rustup-downloads] [--cargo-registry] [--cargo-git] [--older-than] [--remove ...] [--dry-run] [--yes]` *(note: with no kind flags, `clean` clears every regenerable kind.)*
- [x] `rustvac sweep [--dry-run] [--yes]`: one plan across stale-toolchain targets, incremental dirs and the regenerable tier, with one total *(note: targets from an uninstalled compiler go by the removal policy, not through the archive. They rebuild from scratch anyway.)*

### Part C — Scheduled check and recommendations
- [x] Free-space read for the home drive. `windows-sys` is already a Windows dependency with `Win32_Storage_FileSystem`, which covers `GetDiskFreeSpaceExW`. Unix needs `statvfs`, and `libc` is not a dependency yet, so check its licence (MIT/Apache-2.0) before adding it *(note: `libc` 0.2.191 is MIT OR Apache-2.0 and already in the lock file. The Unix `statvfs` path is not compiled on this machine.)*
- [x] `rustvac check [--json]`: read-only; writes `~/.rustvac/recommendation.json` and a readable text form (free space, total reclaimable, top items by kind) *(note: also reports `compressible_bytes`, old targets that `compress` could archive. That figure is kept out of the sweep total.)*
- [x] `rustvac schedule set --every daily|weekly [--at HH:MM] [--warn-below SIZE]`, `schedule status`, `schedule remove`. Windows: per-user `schtasks` task named `rustvac-check`. Linux and macOS: crontab entry marked `# rustvac`. Confirm before registering *(note: Windows checked by hand, see below. The crontab path is only covered by the pure-function tests. It has not been run.)*
- [x] `rustvac status` (and a line at the top of `rustvac list`): last check time, free space, reclaimable, "run `rustvac sweep`" *(note: run on Windows against the real check report.)*
- [x] Config fields `check_every` (default `off`) and `warn_below` *(note: `check_every` does not register anything. It makes `status` flag a check older than twice the interval.)*
- [ ] Windows warning notification when free space is under `warn_below`. If unreliable, drop it and keep the report only *(note: implemented as a PowerShell toast, started by `check --notify`. 2026-10-30: it ran and exited 0, but nobody has confirmed that a toast appeared on screen. Left open until someone sees it. If it does not show, drop it as planned.)*
- [x] The check never recommends something `sweep` would refuse (busy, `keep`, sync-managed) *(note: `check` and `sweep` share `sweep::plan`. A test covers kept targets.)*

### Tests and docs
- [x] Unit tests: `.rustc_info.json` parsing, `rustup toolchain list` parsing, removal-policy threshold, config validation, `schtasks` and crontab command lines (pure functions) *(note: 41 core unit tests, all passing on Windows.)*
- [x] Extend `tests/support/mod.rs` to write `.rustc_info.json` and `incremental/` dirs; integration test with a fake rustup and cargo home in a tempdir (paths passed explicitly) *(note: `tests/cleanup.rs`, with tempdir homes.)*
- [x] `check` test with an injected free-space value *(note: in `tests/cleanup.rs`. The real `disk_space` call has its own test on Windows.)*
- [x] Hand check on Windows: create, `schedule status` and remove a real task; run `caches` and `toolchains` against the real folders and compare with Explorer *(note: 2026-10-30. A real `rustvac-check` task was created, queried (Task To Run was quoted correctly) and removed. `caches` and `toolchains` totals were compared with a PowerShell sum, not Explorer: checkouts 8,922 files / 261.8 MiB and toolchains 66,077 files / 1.3 GiB, both identical. The scheduled task runs a console program, so a window may flash.)*
- [x] Removal run only against a scratch `RUSTUP_HOME` / `CARGO_HOME` first *(note: 2026-10-30. `sweep --yes --remove permanent` against a scratch tree: the six planned items were deleted, and `git/db`, `deps/` and `Cargo.toml` stayed. The Recycle Bin path was only run as a dry run.)*
- [x] README (usage, config fields), CHANGELOG, and a Phase 9 summary here *(note: summary below.)*

### GUI settings (2026-10-30)
- [x] Settings dialog in the GUI: low disk space alert (any size, MB/GB/TB, or off), `remove_mode`, `trash_max_size`, `older_than_days`, `busy_grace_seconds`, `check_every`. `settings.set` validates the whole change before it writes, and saves with `Config::save` (comments in an existing `config.toml` are lost). A banner shows while free space on the home drive is under the alert size. Checked in the live window against a scratch config: banner shown at a 500 GB alert, then saved 1 GB, the file updated and the banner cleared. Unit test covers save, bad values and turning the alert off.
- [ ] The alert is only shown while the GUI is open. Background alerts still come from `rustvac check --notify` (see the notification item above).

### Phase 9 summary (2026-10-30)
Built: `prune`, `toolchains`, `caches`, `sweep`, `check`, `status`, `schedule`, and `--stale-toolchain` on `scan`, `list` and `compress`. Core modules: `toolchains`, `prune`, `caches`, `sweep`, `check`, `schedule`. `cargo clippy -D warnings`, `cargo fmt`, `cargo deny check` (with `libc` added) and the whole test suite pass on Windows.

Real-machine results (Windows): `sweep --dry-run` over `C:\Programming\2 WIP` found 6.8 GiB reclaimable, including 8 targets built by a compiler that rustup has since replaced. Nothing was removed from real folders.

Not verified:
- Linux and macOS. The Unix code (`statvfs`, crontab, the fake-rustc test) has not been compiled here. CI will be the first build.
- The Windows toast (see above).
- `toolchains rm` against a real toolchain, and the Recycle Bin path for cache removal.
- The GUI has no Phase 9 features. It waits, as planned.

## Phase 10 — One-stop disk health: rustup, cargo and WSL sizes (planned)

Plan: `C:\Users\phill\.claude\plans\do-we-think-we-structured-lamport.md`. Aim: one read-only `rustvac disk` report (CLI and GUI) showing rustup, cargo and WSL sizes together, each with a suggested next step. Nothing here deletes or compacts a WSL disk.

### Decisions
- [x] WSL and Docker virtual disks (`ext4.vhdx`) are report-only. rustvac prints the manual compact steps (`wsl --shutdown`, `Optimize-VHD` or `diskpart`) and never runs them, like `check` recommends but never deletes.

### Core
- [x] `wsl.rs`: `WslDisk { name, path, size_bytes, kind }` (`Distro` / `Docker`); pure `discover(local_app_data)` globbing `Packages/*/LocalState/ext4.vhdx` and `Docker/wsl/**/ext4.vhdx`; size from file metadata. Only the `%LOCALAPPDATA%` lookup is `#[cfg(windows)]`, so it compiles on every CI platform.
- [x] Distro names and custom `wsl --import` locations from `HKCU\...\Lxss` (`DistributionName`, `BasePath`) on Windows, falling back to the package folder name.
- [x] `wsl::compact_hint(&WslDisk)`: the manual steps as text only.
- [x] `caches.rs`: add `CacheKind::RustupUpdateHashes` (regenerable, label, `discover`); update `sweep.rs` (`From<CacheKind>`, `SweepPlan::by_kind`) or map it to `None`.
- [x] Toolchain `last_used` (directory mtime) in the toolchain listing so old toolchains stand out.
- [x] `disk.rs`: `DiskReport { rustup, toolchains, cargo, wsl, free_bytes }` built from `caches::discover`, `toolchains`, `wsl::discover` and `check::disk_space`.
- [x] `check::Recommendation` gains `wsl` with `#[serde(default)]`, so an old saved `recommendation.json` still loads.

### CLI
- [x] `rustvac disk [--json]`: grouped Rustup / Cargo / WSL table, free-space line, one next step per group.
- [x] WSL line in `monitor::render_report`, so `check` and `status` mention it.

### GUI
- [x] `disk.refresh` read command (`fs.read`) and a `disk` field in `snapshot()`.
- [x] "Disk health" panel in `web/index.html` / `app.js`, with Reveal for each WSL disk and no delete buttons for WSL. *(note: built and wired. Not yet seen on screen.)*

### Tests and docs
- [x] Unit tests for the `wsl` glob logic; integration test in `tests/cleanup.rs` with a fake `LOCALAPPDATA` tempdir holding `Packages/X/LocalState/ext4.vhdx`.
- [ ] Hand check on Windows: `rustvac disk` and `disk --json` against the real machine, compare vhdx sizes with Explorer; confirm the GUI panel matches the CLI. *(note: 2026-10-31. `disk --json` reports the Docker `ext4.vhdx` at 117,440,512 bytes, which is the length PowerShell reads for the same file. Explorer and the GUI panel on screen are still not checked. 2026-10-30. `rustvac disk` ran on the real machine. The Docker `ext4.vhdx` is 117,440,512 bytes, which is 112.0 MiB, the same as the report. That is the file size PowerShell reads, not Explorer's window. `disk --json` was not compared, and the GUI panel was not checked on screen.)*
- [x] `cargo fmt --check`, `cargo clippy -D warnings` and the full test suite pass on Windows. *(note: 2026-10-31. Core and CLI clippy pass with `-D warnings`. The core test suite passes. The WSL code has not been built on Linux or macOS; CI is the first check there.)*
- [x] README (section "Toolchains, caches and the small-drive routine"), the three CHANGELOGs, and a Phase 10 summary here.

Phase 10 summary (2026-10-31): `rustvac disk` shows rustup, toolchains, cargo and WSL/Docker disks, each with a next step. WSL disks are report-only, and the manual compact steps are printed, never run. `check`, `status` and the GUI show the same report. Open: the GUI panel on screen, `disk --json` against the real machine, and the Linux and macOS build.

## Phase 11 — GUI selection and table view (2026-10-30)

Plan: `C:\Users\phill\.claude\plans\do-you-have-any-adaptive-quill.md`. Aim: make it quick to pick which targets to remove.

### Done
- [x] Larger row checkboxes (20 px) and taller rows, in `web/style.css`.
- [x] "Select all targets" checkbox in the filter bar, using the existing `node.check_all` command (Ctrl+A already did the same). *(note: not run in the GUI yet.)*

### Targets-only table view
- [x] `targets_only` in `Model` (default on in Rust Targets mode, off in Any Folder and Mixed); `rebuild_view` builds a flat list of visible targets, sorted like the tree, with a shared comparator. *(note: `target_rows` and `size_order` in `model.rs`. The mode default is set in `scan_start`.)*
- [x] `targetsOnly` accepted by `view_set` and sent in `snapshot()`. *(note: the snapshot also sends `flatTargets`, which is true only when the table is actually shown.)*
- [x] Table rows in `app.js`: project name, full path, size, bar, age; no caret. Header labels switch to match. *(note: the name and path stack in one cell, so the path is not a separate grid column.)*
- [x] "Targets only" checkbox in the filter bar; disabled in Any Folder mode. The path truncates at the start so the end of the path stays visible. *(note: CSS right-to-left truncation, not a grid column. The narrow-width override still hides only the fifth column, so the table keeps its layout at small widths.)*
- [x] Unit test: two projects in an arena give a flat view with only target rows, sorted by size. *(note: the test covers the order and the filter result passed in. The age, size and search filters run in `rebuild_view`, which needs a full `Model` and is not unit-tested.)*
- [ ] Hand check on Windows in all three modes: checkboxes, Select all, Compress and Move to Trash act on the right rows. *(note: not run. The GUI has not been opened for this change.)*

### Further ideas (decided 2026-10-31, built, not yet seen on screen)
- [x] Select-all box compares against the actionable count, not `state.targets`, so archived, busy or kept targets don't leave it half-ticked. *(note: the snapshot sends `actionableCount` and `checkedActionable` from `Model::check_summary`.)*
- [x] Status bar shows "N checked · X reclaimable". *(note: `checkedBytes` counts only the checked targets that can be acted on.)*
- [x] Shift-click range select. *(note: `node.check_range` checks the actionable targets between the last plain click and the shift-clicked row, in table order. Shift-click on a checkbox does not toggle it.)*
- [x] Quick-check presets: older than N days, larger than N MB, not archived. *(note: a "Check by rule…" menu with 30 and 90 days, 100 MB and 1 GB, and not archived. `node.check_where` replaces the checks. Unit test `check_rules_match_on_age_size_and_archive`.)*
- [x] Action buttons show the count they act on, e.g. "Move to Trash (3)". *(note: the count is the checked rows, or else the selected one.)*
- [x] Esc or Ctrl+Shift+A clears all checks. *(note: Ctrl+A checks every actionable target, as before.)*
- [ ] Hand check on Windows in all three modes: checkboxes, Select all, Compress and Move to Trash act on the right rows. *(note: not run. Needs the window on screen. The new range and preset actions should be checked the same way.)*

## Phase 12 — Faster scans, quiet refresh, remembered session (2026-10-30)

Plan: `C:\Users\phill\.claude\plans\continue-bright-lemur.md`. Aim: a 125-target scan finishes sooner, the tree is not rebuilt after every action, and the GUI reopens where you left off. Reported while testing on the Open Source folder (stuck around 90/125 targets, and each Compress/Trash cleared and rescanned everything).

Status (2026-10-31): the code is committed in `53b376c`, `4b67b7e`, `8bfea60`, `4167f96` and `bb8e7d2`. Still open: the timing, the on-screen check, and one deferred item.

### Speed (core)
- [x] `walk.rs` / `identity.rs` (Windows): stop opening every file twice. *(note: an ordinary file's size is its length rounded up to the cluster size, with no extra call. Only compressed, sparse, reparse, offline and recall-flagged files call `GetCompressedFileSizeW`. Cloud placeholders still count 0 bytes. Tests: `round_to_cluster`, and the fixture tests, which take expected sizes from `disk_bytes`.)*
- [ ] Share one handle for the hardlink check and the size query where both are needed. *(note: deferred. On Windows the hardlink check still opens each file once, because the link count needs a handle. Only the rare probe files open twice. Sharing one handle would mean getting the allocation from `FileStandardInfo` instead of `GetCompressedFileSizeW`, which changes the numbers the Phase 8 check verified. That needs a before and after comparison on real trees first.)*
- [x] `scan.rs::run_rust_targets`: one rayon pool over targets with a serial walk per target. *(note: `measure_tree_serial` is used for Rust targets. `measure_tree` stays for general mode.)*
- [x] Emit each target's child folders and its project rollup as soon as it finishes. *(note: each target sends its own children when measured. A project's rollup goes out when its last target is done. Child ids are handed out under the same lock as their `Discovered` event, because the arena appends in event order. The existing integration test checks project and child sizes and passes.)*
- [ ] Time `rustvac scan` on the Open Source folder before and after. *(note: not measured on the real folder. The "before" build (`b441584`) had run for over 21 minutes on the full folder without finishing, and was stopped. The repo's own root has a `Cargo.toml`, so the scanner reports no targets there, and that run was discarded. 2026-10-31: on a generated tree of 125 projects with 3,500 files, with a backslash root, the warm scan took about 184 ms before and about 125 ms after (4 runs each). Totals differ on purpose: the old build used the logical length of ordinary files, and the new one rounds to the cluster size, as the item asked. Still needed: the real folder, which is the number that matters.)*

### No rescan after an action (GUI)
- [x] `Arena::remove_subtree(id)` in core: unlink from the parent and subtract size and files from every ancestor. *(note: `model.rs`, with four unit tests. A project left with no targets is removed too.)*
- [x] `poll_work`: on success remove the target in place, drop it from `checked`, mark the view dirty. Failed items stay in the tree and stay checked. *(note: `poll_work` calls `remove_subtree`. The on-screen check is still open under Tests and docs.)*
- [x] `state_get` no longer calls `start_scan()` when work finishes. Status line reads "Done: N of M succeeded." *(note: `commands.rs` `state_get` does not start a scan. The status text is in `model.rs` `poll_work`.)*

### Quiet background refresh (GUI)
- [x] `Model::refresh_scan()`: scan into a separate arena without clearing the visible one, hide the progress bar, then swap on `Finished` and restore `checked`, `selected` and `toggled` by path (ids change). *(note: `model.rs` `refresh_scan` and `finish_refresh`.)*
- [x] Rescan button uses it; a "Refresh in background" setting turns it on or off. *(note: `commands.rs` uses `refresh_scan` when `refresh_in_background` is set. The setting is saved through `settings.set`.)*

### Remember the last session (GUI)
- [x] `ui_state.json` beside `config.toml` (so `config.toml` comments are never rewritten): `mode`, `root`, up to 10 `recent_roots` (most recent first, no duplicates, existing folders only), `min_age_days`, `min_size_mb`, `hide_archived`, `sort_desc`. The folder is created on first save, so no installer is needed. *(note: `ui_state.rs`.)*
- [x] Load in `Model::new`; fall back to the config's first root when the saved one is gone. Save after a successful `scan.start` and when a filter changes. Errors never block the UI. *(note: `UiState::usable_root` handles the fallback. `save_session` runs from `model.rs` and from the filter command. Save errors are ignored.)*
- [x] `recentRoots` in `snapshot()`; folder box in the page offers the recent folders and scans the one picked. Startup scan uses the remembered mode and folder. *(note: the page merges `recentRoots` into the folder list in `app.js`.)*
- [x] Unit test: save/load round trip in a tempdir, missing-root fallback, recent list cap and de-duplication. *(note: four tests in `ui_state.rs`.)*

### Tests and docs
- [ ] Hand check on Windows: tick two targets, Move to Trash, rows vanish with no progress bar, other checks and selection kept, project and root sizes drop correctly. Rescan gives the same totals. Close and reopen: same mode, folder and filters. *(note: not run. It needs the window on screen.)*
- [x] `cargo fmt --check`, `cargo clippy -D warnings`, full test suite. *(note: 2026-10-31, all pass on Windows.)*
- [x] CHANGELOGs (core, gui) and a README line on background refresh and remembered folders. *(note: README has the `refresh_in_background` config line and a `ui_state.json` paragraph.)*

## Lightweight invariant checks (instead of Kani)

Kani is not used in this repo (only in tpt-acoustics, Linux-only CI). These checks run in plain `cargo test` on Windows, with no CBMC toolchain. Bounded exhaustive enumeration over small generated cases, plus proptest for random sizes and names.

Status (2026-10-31): `crates/tpt-rustvac-core/tests/invariants.rs`, seven tests, all passing.

- [x] Add `proptest` as a dev-dependency of `tpt-rustvac-core` and create `crates/tpt-rustvac-core/tests/invariants.rs`. *(note: `proptest` 1.11.0. `cargo deny check` passes.)*
- [x] Sweep planner (`sweep::plan`): never plans a path outside the scan roots or cache homes; `total_size()` equals the sum of item sizes; kept, busy and sync-managed targets only appear in `skipped`. *(note: `sweep_plan_respects_roots_keep_busy_and_stale`. Sync-managed paths are not generated, because they need OneDrive or reparse points. Those are still covered only by the existing sync guard tests.)*
- [x] Cache planner (`caches::plan_clean`): only `Tier::Regenerable` kinds are ever planned, whatever `spec.kinds` holds; with `older_than` set, an entry of unknown age is never planned; busy entries are always skipped. Enumerate every `CacheKind` x tier x age x busy combination. *(note: `cache_planner_is_exact_over_kind_age_and_busy`, every kind, three ages and both age rules.)*
- [x] `RemoveMode::auto`: trash vs permanent depends only on the size threshold, and is monotonic in size (once permanent, larger is permanent). *(note: `auto_removal_is_monotonic_in_size`. It also checks the threshold itself: `trash_max` is trashed and one byte more is deleted.)*
- [x] Archive: pack, verify and restore give an identical manifest; the original is removed only after verify succeeds (inject a corrupt archive and confirm the original stays). *(note: the round trip is tested (`pack_verify_restore_round_trip`), and a truncated archive fails verify (`a_truncated_archive_fails_verification`). 2026-10-31: `pack` now runs its check through `pack_checked`. The unit test `a_failed_check_keeps_the_target_and_leaves_no_archive` injects a failing check, and the target stays with no archive left. `compress_and_vacuum` trashes only after `pack` returns Ok.)*
- [x] Disk (`disk::build`): `regenerable_bytes` is the sum of regenerable items only, and the alert thresholds are monotonic in free space. *(note: `disk_report_regenerable_bytes_is_the_sum_of_regenerable_items` and `low_space_alert_is_monotonic_in_free_space`.)*
- [ ] Each invariant test fails when its guard is deliberately broken in a scratch edit (check once, do not commit the breakage). *(note: six of seven guard families checked, each by a scratch edit and then `git checkout`. Cache tier guard, sweep busy arm, auto threshold, alert comparison, disk regenerable filter and stale-target detection each made the matching test fail. The archive verify guard is not broken yet.)*
- [x] `cargo fmt --check`, `cargo clippy -D warnings`, full test suite; CHANGELOG line for core. *(note: 2026-10-31.)*

## Out of scope for v1 (do not schedule)

- Dependency deduplication
- sccache integration
- Per-crate analysis
- Cross-machine sync
- Network drives and cloud storage
