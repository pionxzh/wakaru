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

Before tagging:

1. Confirm `Cargo.toml`, `Cargo.lock`, and npm package metadata match the tag version.
   The npm versions are rewritten from the tag at publish time; the Cargo
   version is not, and the release workflow fails if it differs from the tag.
2. Run the release verification checks from [Testing](testing.md). If the
   sibling checkout `../wakaru-private-artificial` exists, also run the release
   checks its `README.md` lists.
3. Check `git tag -l vX.Y.Z` is empty before creating the tag.
4. Inspect `CHANGELOG.md` against `git log --no-merges vPREV..HEAD`.
5. Review `npm/README.md` against the release: its claims must match what the
   tagged version actually ships (e.g. a bundler format merged after the
   previous tag needs adding; an unreleased one must not appear).
6. If any change since the previous tag removed, renamed, or changed the type
   or meaning of a field covered by
   [machine-readable output compatibility](cli.md#machine-readable-output-compatibility),
   the release needs a new major version.

Pushing the tag runs `.github/workflows/rust-release.yml`, which builds the
platform binaries and, once all of them succeed, publishes everything:

- **crates.io**: `wakaru-core` and the exact-version-dependent `wakaru` façade
  in one `cargo publish` invocation, which orders them and waits for the engine
  to be indexed. The other workspace crates carry `publish = false`. The job
  authenticates through crates.io Trusted Publishing (GitHub OIDC), so there is
  no registry token in the repository secrets; each of the two crates has the
  repository and workflow file registered on its crates.io settings page.
  Versions already on crates.io are skipped, so re-running a failed workflow is
  safe.
- **npm**: the platform packages, `@wakaru/cli`, and the bare `wakaru` alias.
- **GitHub Release** with the archives attached and auto-generated notes.

Every publish job waits for the whole build matrix, so a failed build leg
publishes nothing. Delete the tag, fix the build, and tag the same version
again. Do not drop a matrix leg to get the release out.

After the workflow finishes, replace the generated release notes with the
reviewed ones (`gh release edit vX.Y.Z --notes-file notes.md`). Past releases
use a short lede naming the headliners, a few themed sections, and the compare
link.

The bare `wakaru` npm package (`npm/alias/`) is a thin shim around
`@wakaru/cli`, pinned to the exact release version. The release workflow
publishes it automatically after the main package: it regenerates
`npm/alias/README.md` from `npm/README.md` (title and install commands
rewritten to the bare name — edit `npm/README.md`, never the alias copy) and
rewrites the version plus the pinned dependency. The committed alias README
is just the latest generated snapshot.
