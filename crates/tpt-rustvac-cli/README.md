# tpt-rustvac-cli

The `rustvac` command. Finds Cargo `target/` folders, shows their size and age, and
archives the ones you don't need.

```
rustvac scan --path ~/Code --older-than 2d
rustvac measure ~/Downloads --depth 3 --top 20
rustvac compress --older-than 2d --dry-run
rustvac prune --stale-deps --dry-run
rustvac restore <slug> --into ~/Code/my-project
rustvac archive list
rustvac gui
```

Add `--json` to `scan` or `measure` for scripting. Nothing is removed without a
confirmation, and archives are verified before the original is moved to the Trash.

See the [project README](https://github.com/tpt-solutions/tpt-rustvac#readme) for
safety rules and configuration.

Licensed under MIT or Apache-2.0, at your option.
