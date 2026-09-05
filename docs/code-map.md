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
          filename_recovery.rs      — recovered module names
        trace.rs                    — rule trace orchestration and formatting
        diagnostics.rs              — post-transform diagnostic warning collection
        discovery.rs                — internal structural-detection helper
        output.rs                   — internal path normalization and dedup helpers
        io.rs                       — parse/print helpers
        types.rs                    — driver options, outputs, and warning types
      facts.rs                      — post-Stage-2 cross-module fact extraction
      commonjs_default_object_composition.rs — exact mutable-default composition recovery
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
        *.rs                        — one file per transformation rule
      unpacker/
        mod.rs                      — unpack_bundle() dispatch
        webpack4.rs                 — webpack4 splitter + normalization
        webpack5.rs                 — webpack5 splitter (incl. runtime entry, ncc + chunk)
        browserify.rs               — browserify-family splitter (incl. Cocos Creator 2.x)
        closure_module_manager.rs   — Closure ModuleManager/gstatic splitter
        systemjs.rs                 — System.register splitter + ESM reconstruction
        esbuild.rs                  — esbuild/Bun splitter (CJS factories + scope-hoisted)
        amd.rs                      — AMD define() bundle splitter
        wrappers.rs                 — UMD/AMD wrapper unwrapping for detection retry
        metro.rs                    — Metro plain-bundle detection and extraction
        scope_hoist.rs              — heuristic scope-hoisted splitting (esbuild, Bun, Rollup, Vite)
      utils/
        paren.rs, swc_safety.rs     — paren stripping, SWC panic guards
    tests/
      common/mod.rs                 — test helpers (see docs/testing.md)
      *_rule.rs                     — per-rule unit tests
      *_unpack.rs                   — per-bundler unpack/pipeline snapshot tests
                                      (webpack4 + raw, webpack5 chunk, bundle_unpack
                                      = webpack5 + browserify, Closure ModuleManager,
                                      esbuild, systemjs, amd, metro, rollup, bun,
                                      multi-file)
      webpack_fixtures.rs           — generated webpack4/5 + ncc fixture coverage
      cocos_creator_unpack.rs       — Cocos 2.x detection + dependency-map coverage
      noop_pipeline.rs              — stability tests
      snapshots/                    — insta snapshot files

  formatter/
    src/
      lib.rs                        — optional emitted-code formatting

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
