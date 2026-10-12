# Python behavioral reference

This is the preserved Python implementation used to validate the Rust rewrite.
It is a development reference, not the supported production runtime. Source and
test hashes from the starting worktree are recorded in `baseline_manifest.json`.
The manifest also records two annotation corrections: the installed
`SortedDict` has no generic type parameters. These edits have no runtime effect.

Run the reference suite from the repository root:

```sh
uv run --directory reference/python --extra dev pytest
```

The Rust implementation intentionally adds bounded capacity, validated integer
inputs, account ownership, instance-scoped order IDs, and atomic rejection
counters. See `../../docs/rust_migration.md` for the complete compatibility
contract and differential replay instructions.
