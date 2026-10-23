# Code Map

Use this as a file lookup after [architecture.md](architecture.md). It lists
entry points, not every source file. The task-based documentation index is
[README.md](README.md).

The driver's `trace_rules` observer captures per-rule before/after snapshots;
`format_trace_events` formats those events as unified diffs. Usage and the
single-file trace boundary are documented in [debugging.md](debugging.md).

```
crates/
  wakaru/
    src/
      lib.rs                        — stable compiler-like public façade
      decompile.rs                  — owned single-file operation adapter
      unpack.rs                     — Vec and incremental UnpackJob operations
      source.rs, options.rs         — owned inputs and private builder options
      output.rs, error.rs           — artifacts, reports, diagnostics, fatal errors
      debug.rs, sourcemap.rs, vue.rs — standalone auxiliary namespaces
      bun.rs, gtm.rs                — container extraction namespaces (Bun executables, GTM containers)
      rsc.rs                        — experimental React Server Components payload decoding

  cli/
    src/
      main.rs                       — CLI entry point (clap)

  core/
    src/
      lib.rs                        — internal engine exports
      driver.rs                     — internal driver consolidation
      driver/
        single_file.rs              — decompile() orchestration
        unpack/
          mod.rs                    — prepared-input intake and execution adapters
          phases.rs                 — two-phase pipeline and fact barrier
          merge.rs                  — multi-input module and filename coordination
          scope_split.rs            — recursive scope-hoisted splitting
          webpack_commonjs_runtime.rs — detector-proven runtime normalization
          dead_module.rs            — dead-module elimination
          link_check.rs             — missing_imported_name: imports a sibling ESM output does not export
          filename_recovery.rs      — recovered module names
          interop_report.rs         — `debug interop` report (each module's provider interface and require edges)
        trace.rs                    — rule trace orchestration and formatting
        export_storage.rs           — `debug cjs-exports` driver (pipeline up to UnEsm, then the storage report)
        diagnostics.rs              — post-transform diagnostic warning collection
        discovery.rs                — internal structural-detection helper
        output.rs                   — internal path normalization and dedup helpers
        io.rs                       — parse/print helpers
        types.rs                    — driver options, outputs, and warning types
      facts.rs                      — post-Stage-2 cross-module fact extraction
      gtm.rs                        — Google Tag Manager container extraction (façade: `wakaru::gtm`)
      rsc.rs                        — React Server Components payload decoding and Next.js `__next_f` extraction (façade: `wakaru::rsc`)
      commonjs_default_object_composition.rs — exact mutable-default composition recovery
      commonjs_export_residual.rs   — `exports` accesses left in ESM output (`commonjs_export_unrecovered`)
      kept_require.rs               — requires that stay calls: positions, the default read of a kept require, ES-source evidence, and their warnings
      interface_plan.rs             — the plan built at barrier 0 from every module's require edges and helper exports: which providers stay CommonJS because a consumer writes to them, which sources each consumer reads a whole `module.exports = v` value from, and which interop helpers each consumer reaches through a bundled provider
      provider_helpers.rs           — the interop helpers a module exports, read before `UnEsm` (export storage + helper body)
      provider_interface.rs         — what `require` of a provider returned and which import reads it, from the provider's facts (Phase 2 passes)
      require_edges.rs              — one record per `require()` call (form, interop, uses), keyed by the call's span
      sourcemap_rename.rs           — source-map-driven name recovery
      namespace_decomposition.rs    — cross-module namespace-to-named-import rewrite
      reexport_consolidation.rs     — cross-module re-export consolidation
      rules/
        mod.rs                      — rule module declarations and public exports
        pipeline.rs                 — rule descriptor registry and pipeline execution
        transpiler_helper_utils/    — shared helper detection (module dir)
          mod.rs                    — helper-kind types, LocalHelperContext, shared AST predicates
          collect.rs                — module-level helper scan/orchestration
          matchers.rs               — Babel/SWC body-shape matchers + per-node detection dispatch
          ts_helpers.rs             — TypeScript/tslib detection (raw TsHelperKind channel)
          paths.rs                  — runtime import-path constants + path classification
          lifecycle.rs              — helper-declaration reference tracking + removal
        match_context.rs            — binding-aware slots for helper body matchers
        helper_matcher.rs           — shared helper binding/lifecycle primitives
        rename_utils.rs             — shared binding rename utilities
        un_for_await.rs             — `for await` protocol matcher run by UnForOf (not a registered rule)
        lowered_dynamic_import.rs   — lowered `import()` matcher, run by UnInteropRequireWildcard and by a Phase 2 pass for a cross-module helper (not a registered rule)
        webpack_lazy_import.rs      — webpack/Rspack lazy-load matcher (`require.e(c).then(() => require("./p"))`, `require.t`) and the Phase 2 pass that restores `import()` from provider facts (not a registered rule)
        cross_module_interop_import.rs — Phase 2 passes: `helper(require(x))` with a default/wildcard interop helper from another module becomes an import; any other imported function keeps its call around an import of `x` (not registered rules)
        un_esm/export_getters.rs    — swc / esbuild / sucrase export getter helpers and webpack `require.d` lowered to per-name definitions
        un_esm/export_storage.rs    — per-name CommonJS export storage model (getter / mirror / property)
        un_esm/harmony_requires.rs  — webpack harmony module detection and the loads Terser inlined into their use
        un_esm/leading_requires.rs  — the requires a top-level statement runs before any other effect, moved into bindings
        un_esm/whole_value_members.rs — member reads of a whole `module.exports = v` provider, read through one binding that becomes the default import
        cocos_rf.rs                 — Cocos Creator 2.x `cc._RF.push/pop` registration-frame matching shared by UnEnum and export storage
        *.rs                        — one file per transformation rule
      unpacker/
        mod.rs                      — unpack_bundle() dispatch
        webpack4.rs                 — webpack4 splitter + normalization
        webpack5.rs                 — webpack5 splitter (incl. runtime entry, ncc + chunk)
        turbopack.rs                — Turbopack chunk detection + translation to webpack factories
        browserify.rs               — browserify-family splitter (incl. Cocos Creator 2.x)
        closure_module_manager.rs   — Closure ModuleManager/gstatic splitter
        systemjs.rs                 — System.register splitter + ESM reconstruction
        esbuild.rs                  — esbuild/Bun splitter entry points + detection orchestration
        esbuild/
          factories.rs              — lazy-module helper detection, factory collection
          glob_import.rs            — `__glob` maps of lazy-only modules collapsed back into `import()` of the template
          lazy_import.rs            — modules reached only through a lowered `import()`: inlined as plain modules, loaders rewritten
          scope.rs                  — scope-hoisted module extraction
          scope_boundaries.rs       — `__export(ns, {...})` boundary detection
          ownership.rs              — factory ownership planning (groups, writers, demotion, merges)
          emit.rs                   — factory group / merged module / entry.js emission
          synthesis.rs              — shared import/export synthesis, owned-declaration filtering
          bindings.rs               — top-level binding indexes, reference/write collectors
        amd.rs                      — AMD define() bundle splitter
        wrappers.rs                 — UMD/AMD wrapper unwrapping for detection retry
        metro.rs                    — Metro plain-bundle detection and extraction
        scope_hoist.rs              — heuristic scope-hoisted splitting (esbuild, Bun, Rollup, Vite)
        surrounding.rs              — top-level code around the bundle statement, kept in entry.js (webpack 5, Browserify)
        chunk_enumeration.rs        — fail-closed lazy-chunk URL / relative-import enumeration (`debug enumerate-chunks`)
        chunk_loading_global.rs     — an input's chunk-loading global names (its build identity for multi-input module-id rewrites)
      utils/
        paren.rs, swc_safety.rs     — paren stripping, SWC panic guards
    tests/
      common/mod.rs                 — test helpers (see docs/testing.md)
      *_rule.rs                     — per-rule unit tests
      *_unpack.rs                   — per-bundler unpack/pipeline snapshot tests
                                      (webpack4 + raw, webpack5 chunk, bundle_unpack
                                      = webpack5 + browserify, Closure ModuleManager,
                                      esbuild, systemjs, amd, metro, rollup, bun,
                                      turbopack, multi-file)
      webpack_fixtures.rs           — generated webpack4/5 + ncc fixture coverage
      cocos_creator_unpack.rs       — Cocos 2.x detection + dependency-map coverage
      noop_pipeline.rs              — stability tests
      snapshots/                    — insta snapshot files

  formatter/
    src/
      lib.rs                        — optional emitted-code formatting
      source_map.rs                 — carries an output source map across formatting (token alignment)

  wasm/
    src/
      lib.rs                        — wasm-bindgen entry point (decompile + unpack)

docs/
  architecture.md                   — pipeline and component responsibilities
  code-map.md                       — this file
  unpacking.md                      — detector shapes and extraction boundaries
  public-api.md                     — supported Rust façade contract and design decisions
  testing.md                        — test patterns, helpers, organization
  debugging.md                      — rule tracing, snapshot debugging, fixture workflow
  helper-detection.md               — transpiler helper detection design
  fact-system.md                    — cross-module fact system
  rule-dependency-inventory.md      — rule dependency relationships
  rewrite-assumptions.md            — semantic assumptions and rewrite policy
  releasing.md                      — changelog and release workflow
  test262-roundtrip.md              — semantic round-trip runner and baselines
  test262-baselines/                — tracked Test262 baseline summaries
  proposals/                        — design proposals (deferred or in progress)
  learnings/                        — approaches that were built, measured, and rejected
```
