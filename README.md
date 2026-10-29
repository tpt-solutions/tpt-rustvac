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

### Toolchains, caches and the small-drive routine

Targets are not the only regenerable Rust scratch. These commands cover the rest.

```
rustvac scan --stale-toolchain                 # targets built by a rustc you no longer have
rustvac prune --incremental --dry-run          # incremental/ folders inside targets
rustvac prune --profile debug                  # a whole profile folder
rustvac toolchains                             # installed toolchains, with sizes
rustvac toolchains rm nightly-2026-01-01-...   # through rustup, never by deleting folders
rustvac caches                                 # rustup tmp/downloads, cargo registry and git
rustvac caches clean --cargo-registry --older-than 30d
rustvac sweep --dry-run                        # one plan across all of the above
rustvac sweep                                  # confirm, then remove
```

`rustvac check` is a read-only check of free space on your home drive and of what a sweep
would reclaim. It writes `~/.rustvac/recommendation.json` and `.txt`. `rustvac status` shows
the result, and `rustvac list` prints it as its first line.

```
rustvac check --warn-below 50GB
rustvac schedule set --every daily --at 09:00 --warn-below 50GB
rustvac schedule status
rustvac schedule remove
```

`schedule set` registers a per-user task named `rustvac-check` (`schtasks` on Windows, a
`# rustvac` line in your crontab on Linux and macOS). It asks first. The task only runs
`rustvac check`, so it never deletes anything by itself. On Windows the task window may flash
when it runs. `--notify` (added to the task when you give `--warn-below`) shows a Windows
notification when free space is under the limit. Elsewhere only the report is written.

What a sweep touches:

| Tier | Items | How |
| --- | --- | --- |
| Regenerable | rustup `tmp` and `downloads`, cargo `registry/cache`, `registry/src`, `git/checkouts`, `incremental/` folders, targets from an uninstalled rustc | Trash or permanent delete, by `remove_mode` |
| Managed | `toolchains/<name>` | Only through `rustup toolchain uninstall`. The default toolchain is refused. |
| Manual | `registry/index`, `git/db` | Listed, never in a sweep. They need the network to rebuild. |

`target/` compress is unchanged: it archives, verifies and then trashes.

## Safety

- Scanning is read-only.
- Nothing is removed without a confirmation. `compress` asks first, unless you pass `--yes`.
- Archives are verified (zstd checksum, tar structure, file count and byte total) before
  the original is touched.
- The original `target/` is moved to the Recycle Bin / Trash, not deleted.
- A target modified in the last 60 seconds is marked busy and is never compressed.
  This is configurable with `busy_grace_seconds`.
- Scanning `/`, a drive root or your home folder is refused unless you pass `--force`.
- Folders in the `keep` list are never compressed, pruned or swept.
- Cache entries modified within the busy grace period are skipped, since an install may be
  running. Paths under OneDrive, and symlinks and junctions, are refused on Windows.
- Permanent deletion happens only for the regenerable tier, and only as `remove_mode` allows.

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
remove_mode = "auto"        # auto | trash | permanent, for caches, incremental/ and old-compiler targets
trash_max_size = "256MB"    # auto: up to this goes to the trash, larger is deleted permanently
check_every = "off"         # off | daily | weekly. `rustvac status` flags a check that is overdue
warn_below = "50GB"         # warn when free space on the home drive is under this
```

`auto` exists because the Recycle Bin is on the same drive: trashing a large cache frees
nothing. Override it per run with `--remove auto|trash|permanent`. `RUSTUP_HOME` and
`CARGO_HOME` are honoured, and default to `~/.rustup` and `~/.cargo`.

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
