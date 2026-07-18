# SmartInline: Import-Alias Inlining via Module Facts

Status: **FUTURE HOOK, not started.** Recorded so the decision is not
re-derived; no work is planned until someone wants the recovery.

The frozen-source redesign (landed as `ebccdb53`) deliberately excludes
imported bindings from generic alias inlining: ESM imports are live bindings,
so "no local writes" proves nothing — the exporter can reassign, and the
alias would then diverge from its source.

That proof is only impossible module-locally. Unpacked bundles are a closed
world, and the fact system (`docs/fact-system.md`) already runs a two-phase
barrier over all modules. A cross-module fact of the form "this export is
never reassigned after initialization anywhere in the bundle" would make an
import alias exactly as frozen as a proven local, at which point the same
generated-name + adjacency + const-only policy gate from `ebccdb53` applies
unchanged.

Sketch, if picked up:

1. During the fact phase, record per export whether any module (including the
   exporter) writes to it after its initializing assignment.
2. Expose that as a module fact consumed by SmartInline's source
   classification; imports whose source export carries "never reassigned"
   join the frozen-local class.
3. Keep every other exclusion (unresolved globals, outer lexicals,
   `eval`/`with`) — this hook widens one class only.

The governing invariant from the redesign still applies: entry proofs may
cross block boundaries within one activation, never a function-like
activation boundary (see `docs/rewrite-assumptions.md`).
