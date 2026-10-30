# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- `rustvac disk`: one read-only report of rustup, cargo and WSL/Docker virtual disk sizes, with a next step for each group. WSL disks are report only; the manual compact steps are printed, never run. `check` and `status` mention WSL disks, `rustup update-hashes` joins the regenerable caches, and the toolchain list shows when each folder last changed. The GUI has a Disk health panel.
- `tpt-rustvac-core`: flat-arena data model, streaming scan engine, Rust target discovery
  with `CARGO_TARGET_DIR` and `.cargo/config.toml` resolution, general folder measurement,
  hardlink dedup, `.tar.zst` archives with verification, restore, and config loading.
- `rustvac` CLI: `scan`, `list`, `measure`, `compress`, `restore`, `archive list|rm`, `gui`.
- `rustvac-gui`: tree view in a tpt-shell window, with Rust Targets / Any Folder / Mixed modes,
  filters, keyboard shortcuts, and a two-step confirmation dialog. Windows only so far.
- Release packaging through cargo-dist (Windows, macOS and Linux).
- `rustvac scan|list|compress --stale-toolchain` and a `rustc` field in `scan --json`, read
  from each target's `.rustc_info.json`.
- `rustvac prune` for `incremental/` folders or a whole profile inside targets.
- `rustvac toolchains` and `toolchains rm` (through rustup), and `rustvac caches` and
  `caches clean` for rustup and cargo scratch.
- `rustvac sweep`: one plan across old-compiler targets, incremental folders and caches.
- `rustvac check`, `status` and `schedule set|status|remove`: a read-only drive check that
  saves a recommendation, run on a schedule by `schtasks` or crontab. It never deletes.
- Config: `remove_mode`, `trash_max_size`, `check_every`, `warn_below`.
- `rustvac-gui`: a Settings dialog (low disk space alert size, cache removal mode and trash limit, age and busy limits, check frequency) that saves to `config.toml`, and a banner when free space on the home drive is under the alert size. `Config::save` writes the file; comments in an existing file are not kept.
- `delete_permanently` now refuses sync-managed paths, as the trash does.
