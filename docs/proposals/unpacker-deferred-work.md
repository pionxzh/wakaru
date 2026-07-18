# Unpacker Deferred Work

Status: **DEFERRED.** Items set aside during the 2026-07 unpacker follow-up
series (Cocos, Metro, Closure, ncc, Browserify hardening) because each needs
design work beyond a one-concern fix commit. Ordered by priority.

Ground rules for whoever picks these up: follow [AGENTS.md](../../AGENTS.md),
including a focused unit test for every change. Use synthetic module ids and
filenames in tests and commits — never values copied from private fixture
bundles — and update affected documentation in the same commit.

## 1. Closure ModuleManager: forward graph references

`decode_module_graph` / segment attribution in
`crates/core/src/unpacker/closure_module_manager.rs` rejects graphs where a
module's dependency list references an id that appears later in the graph
string. Before accepting that shape, confirm from generated Closure output or
the compiler implementation that forward references are valid. If confirmed,
use a two-pass decode (collect ids first, then resolve edges) rather than
resolve-as-you-go.

## 2. Closure ModuleManager: heuristic boundary inference

Segment-boundary inference (`collect_segment_candidates`) is conservative and
rejects unusual segment shapes (see the marker-gap rules tightened in the
2026-07 series — non-blank text before a marker now rejects the boundary).
Deliberate trade-off: false negatives over false positives. Confirm each new
shape is valid producer output before loosening detection, keep the
embedded-marker rejection test green, and add a synthetic fixture per newly
accepted shape.

## 3. Metro / webpack: factory runtime-name capture

Factories that capture or re-alias the runtime parameter names (require /
module / exports / Metro's 7-slot signature) in ways the normalizers don't
model are currently left partially rewritten. Affects
`crates/core/src/unpacker/metro.rs` and the webpack factory normalizers.
Deferred by the implementing session as needing a shared design — likely a
common "runtime binding capture" analysis instead of per-bundler special
cases. Start by collecting concrete failing shapes as synthetic tests.

## 4. Browserify: readable module filenames

String-keyed dependency maps carry original-ish request paths (e.g.
`./utils` → id 3), but numeric-id modules are still emitted as
`module-<id>.js`. Deriving filenames from the union of dependency-map keys
pointing at a module would give human-readable output. Needs collision rules
(two requests mapping to one id, case-insensitive filesystems — reuse the
dedup approach from the Metro filename work in
`crates/core/src/unpacker/metro.rs`) and stable entry naming.

## See also

- [metro-unpacker.md](metro-unpacker.md) — Metro scope; indexed/file RAM
  bundles and Hermes bytecode remain out of scope there.
- `docs/architecture.md` — per-format unpacker behavior statements; keep them
  true when changing any of the above.
