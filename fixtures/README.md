# Fixtures

Test data for `tpt-rustvac-core`.

- Tests build their own fixture trees in temp directories, using `crates/tpt-rustvac-core/tests/support`.
- To write a visible tree for manual testing:

```
cargo run -p tpt-rustvac-core --example make_fixtures -- fixtures/generated
cargo run -p tpt-rustvac-cli -- scan --path fixtures/generated
```

`fixtures/generated/` is git-ignored.
