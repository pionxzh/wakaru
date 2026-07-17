# Releasing

Use the changelog generator to build release notes from git history, then review
the result for superseded intermediate commits before tagging.

```powershell
node scripts\release\generate-changelog.mjs --version 1.2.0 --from v1.1.0 --date 2026-05-25 --prepend CHANGELOG.md
```

Useful options:

- `--skip <hash>` omits a superseded commit while keeping the command reproducible.
- `--include-internal` includes docs, tests, refactors, chores, CI, and build commits.
- `--output <path>` writes the generated entry to a separate file for review.

Before publishing:

1. Confirm `Cargo.toml`, `Cargo.lock`, and npm package metadata match the tag version.
2. Run the release verification checks from [Testing](testing.md).
3. Verify both Rust packages can be assembled:
   `cargo package -p wakaru-core --allow-dirty --no-verify` followed by
   `cargo package -p wakaru --allow-dirty --no-verify --config
   'patch.crates-io.wakaru-core.path="crates/core"'`. The local patch is needed
   only for pre-publication verification; an ordinary façade package resolves
   after the engine version is indexed by crates.io.
4. Publish `wakaru-core` first, then publish the exact-version-dependent
   `wakaru` façade after the registry has indexed the engine version.
5. Check `git tag -l vX.Y.Z` is empty before creating the tag.
6. Inspect `CHANGELOG.md` against `git log --no-merges vPREV..HEAD`.
