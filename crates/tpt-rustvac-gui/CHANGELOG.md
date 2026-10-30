# Changelog

All notable changes to this crate are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- A Disk health panel (`disk.refresh`): rustup, toolchain, cargo and WSL/Docker disk sizes with next steps. WSL disks have Reveal and the manual compact steps, and no delete button. Measured on a background thread.
- Selection: shift-click checks a range of rows, a "Check by rule" menu checks targets older than 30 or 90 days, larger than 100 MB or 1 GB, or not archived, and Esc or Ctrl+Shift+A clears the checks.
- The status bar shows how many targets are checked and the space they would free. The action buttons show how many they act on.
- The select-all box compares against the targets that can be acted on, so a busy or kept target does not leave it half-ticked.
- A targets-only table in Rust Targets mode, with a "Select all targets" box and a targets-only checkbox. Row checkboxes are larger.
- Compress and Move to Trash remove the finished rows in place. The tree is not rescanned, and checks on the other rows are kept.
- Rescan can refresh in the background (`refresh_in_background` in config). The visible tree stays on screen and keeps checks and selection.
- The window remembers its last mode, folder, recent folders and filters in `ui_state.json`.
- Tree view in a tpt-shell window with Rust Targets / Any Folder / Mixed modes, filters,
  keyboard shortcuts, and a two-step confirmation dialog. Windows only so far.
