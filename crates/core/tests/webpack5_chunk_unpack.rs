use wakaru_core::driver::test_support::{unpack, unpack_raw};
use wakaru_core::{validate_output_modules, DceMode, DecompileOptions};

fn expect_unpack(source: &str, filename: &str) -> Vec<(String, String)> {
    let output = unpack(
        source,
        DecompileOptions {
            filename: filename.to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    output.modules
}

fn expect_heuristic_unpack_raw(source: &str, filename: &str) -> Vec<(String, String)> {
    let output = unpack_raw(
        source,
        &DecompileOptions {
            filename: filename.to_string(),
            heuristic_split: true,
            ..Default::default()
        },
    )
    .expect("raw unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    output.modules
}

#[test]
fn webpack5_empty_factory_recovers_runtime_default_object_across_phase2_paths() {
    let source = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([
  [0],
  {
    100: function(module, exports, require) {
      var dependency = require(200);
      module.exports = dependency.missing;
    },
    200: function(module, exports, require) {}
  }
]);
"#;

    for emit_source_map in [false, true] {
        let output = unpack(
            source,
            DecompileOptions {
                filename: "chunk.js".to_string(),
                emit_source_map,
                ..Default::default()
            },
        )
        .expect("webpack5 empty-factory chunk should unpack");
        assert_eq!(validate_output_modules(&output.modules), vec![]);
        let provider = output
            .modules
            .iter()
            .find(|(filename, _)| filename == "module-200.js")
            .map(|(_, code)| code.as_str())
            .expect("empty factory should still be emitted");
        assert_eq!(provider.trim(), "export default {};");
    }

    let raw = unpack_raw(
        source,
        &DecompileOptions {
            filename: "chunk.js".to_string(),
            ..Default::default()
        },
    )
    .expect("raw webpack5 empty-factory chunk should unpack");
    let raw_provider = raw
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-200.js")
        .map(|(_, code)| code.as_str())
        .expect("raw output should retain the empty factory module");
    assert!(
        raw_provider.trim().is_empty(),
        "raw output must remain detector passthrough, got:\n{raw_provider}"
    );
}

#[test]
fn webpack5_chunk_rewrites_numeric_require() {
    // When modules within the same chunk reference each other by numeric ID,
    // require(N) should become require("./module-N.js") so un_esm can convert to import.
    let source = r#"
(self.webpackChunk_N_E = self.webpackChunk_N_E || []).push([
  [0],
  {
    100: function(module, exports, require) {
      "use strict";
      require.r(exports);
      require.d(exports, {
        helper: function() { return helper; }
      });
      function helper() { return 42; }
    },
    200: function(module, exports, require) {
      "use strict";
      require.r(exports);
      var h = require(100);
      exports.default = h.helper();
    }
  }
]);
"#;

    let pairs = expect_unpack(source, "chunk.js");

    let mod_200 = pairs
        .iter()
        .find(|(name, _)| name == "module-200.js")
        .expect("module-200.js should exist");

    // require(100) should be rewritten to an import from ./module-100.js
    assert!(
        !mod_200.1.contains("require(100)"),
        "module-200 should not have raw require(100), got:\n{}",
        mod_200.1
    );
    assert!(
        mod_200.1.contains("./module-100.js"),
        "module-200 should reference ./module-100.js, got:\n{}",
        mod_200.1
    );
}

#[test]
fn webpack5_chunk_localizes_unprovable_loader_reuse_to_one_factory() {
    let source = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([
  [7],
  {
    100: function(module, exports, load) {
      if (globalThis.useAlternate) load = globalThis.alternateLoader;
      module.exports = load;
    },
    200: function(module) {
      module.exports = "stable";
    },
    300: function(module, exports, load) {
      module.exports = [load(100), load(200)];
    }
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "chunk.js".to_string(),
            ..Default::default()
        },
    )
    .expect("one opaque chunk factory should not discard recoverable siblings");
    assert!(output.warnings.iter().any(|warning| {
        warning.filename == "module-100.js"
            && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let consumer = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-300.js")
        .map(|(_, code)| code)
        .expect("recoverable consumer should be emitted");
    assert!(consumer.contains("require(100)"), "{consumer}");
    assert!(!consumer.contains("./module-100.js"), "{consumer}");
    assert!(consumer.contains("./module-200.js"), "{consumer}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_chunk_with_string_ids_localizes_unprovable_loader_reuse_to_one_factory() {
    let source = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([
  [7],
  {
    "NOtv": function(module, exports, load) {
      if (globalThis.useAlternate) load = globalThis.alternateLoader;
      module.exports = load;
    },
    "UUUU": function(module) {
      module.exports = "stable";
    },
    "+BL1": function(module, exports, load) {
      module.exports = [load("NOtv"), load("UUUU")];
    }
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "chunk.js".to_string(),
            ..Default::default()
        },
    )
    .expect("one opaque chunk factory should not discard recoverable siblings");
    let failures = output
        .warnings
        .iter()
        .filter(|warning| {
            warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
        })
        .map(|warning| warning.filename.as_str())
        .collect::<Vec<_>>();
    assert_eq!(failures, ["module-NOtv.js"], "{:?}", output.warnings);
    let consumer = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-+BL1.js")
        .map(|(_, code)| code)
        .expect("recoverable consumer should be emitted");
    assert!(!consumer.contains("./module-NOtv.js"), "{consumer}");
    assert!(consumer.contains(r#"require("NOtv")"#), "{consumer}");
    assert!(consumer.contains("./module-UUUU.js"), "{consumer}");
    assert!(
        output.warnings.iter().any(|warning| {
            warning.filename == "module-+BL1.js"
                && warning.kind == wakaru_core::UnpackWarningKind::RuntimeResidual
                && warning.message.contains("`\"NOtv\"` in an opaque module")
        }),
        "{:?}",
        output.warnings
    );
}

#[test]
fn webpack5_chunk_named_id_opaque_provider_lends_its_consumer_no_facts() {
    // The consumer's unresolved id is also a valid relative path to the
    // opaque module's emitted file. Its output must match the one for a
    // provider absent from the table: the opaque module's empty facts must
    // not read as proven ones.
    let chunk = |provider: &str| {
        format!(
            r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([
  [7],
  {{
    {provider}
    "./main.js": function(module, exports, load) {{
      var o = load("./provider.js");
      exports.run = function() {{ return o.foo(); }};
      exports.again = function() {{ return o.bar; }};
    }},
    "./util.js": function(module, exports) {{
      exports.x = 1;
    }}
  }}
]);
"#
        )
    };
    let consumer = |source: &str| {
        let output = unpack(
            source,
            DecompileOptions {
                filename: "chunk.js".to_string(),
                ..Default::default()
            },
        )
        .expect("the named-id chunk should unpack");
        output
            .modules
            .into_iter()
            .find(|(filename, _)| filename == "main.js")
            .map(|(_, code)| code)
            .expect("the consumer should be emitted")
    };

    let opaque = consumer(&chunk(
        r#""./provider.js": function(module, exports, load) {
      if (globalThis.useAlternate) load = globalThis.alternateLoader;
      exports.foo = load;
      exports.bar = 1;
    },"#,
    ));
    let absent = consumer(&chunk(""));
    assert_eq!(opaque, absent);
}

#[test]
fn webpack5_chunk_heuristic_skips_scope_split_without_import_bearing_entry() {
    let source = r#"
(self.webpackChunk_N_E = self.webpackChunk_N_E || []).push([
  [0],
    {
    100: function(module, exports, require) {
      "use strict";
      var external = require(200);
      function helperA1() { return external.value; }
      function helperA2() { return helperA1() + 1; }
      function helperA3() { return helperA2() * 2; }
      function helperA4() { return helperA3() + 3; }
      function publicA() { return helperA4(); }

      function helperB1() { return 10; }
      function helperB2() { return helperB1() + 10; }
      function helperB3() { return helperB2() * 20; }
      function helperB4() { return helperB3() + 30; }
      function publicB() { return helperB4(); }

      require.r(exports);
      require.d(exports, {
        publicA: function() { return publicA; },
        publicB: function() { return publicB; }
      });
    },
    200: function(module, exports, require) {
      "use strict";
      exports.value = 40;
    }
  }
]);
"#;

    let pairs = expect_heuristic_unpack_raw(source, "chunk.js");
    let filenames: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();

    assert_eq!(
        filenames,
        vec!["module-100.js", "module-200.js"],
        "unsafe scope split should be rejected until the entry imports recovered chunks"
    );
}

#[test]
fn webpack5_chunk_unpacks_modules() {
    let source = r#"
(self.webpackChunk_N_E = self.webpackChunk_N_E || []).push([
  [123],
  {
    11111: function(module, exports, require) {
      "use strict";
      require.r(exports);
      require.d(exports, {
        M: function() { return i; },
        u: function() { return o; }
      });
      var o = 1;
      var i = 2;
    },
    22222: function(module, exports, require) {
      "use strict";
      require.r(exports);
      require.d(exports, {
        Z: function() { return s; }
      });
      function s() { return 0; }
    }
  }
]);
"#;

    let pairs = expect_unpack(source, "chunk.js");

    assert_eq!(
        pairs.len(),
        2,
        "expected 2 modules, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );

    // Check module IDs are used as filenames
    let filenames: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();
    assert!(
        filenames.contains(&"module-11111.js"),
        "expected module-11111.js, got {:?}",
        filenames
    );
    assert!(
        filenames.contains(&"module-22222.js"),
        "expected module-22222.js, got {:?}",
        filenames
    );

    // Check that require.r and require.d were normalized
    for (name, code) in &pairs {
        assert!(
            !code.contains("require.r("),
            "module {name} still has require.r"
        );
        assert!(
            !code.contains("require.d("),
            "module {name} still has require.d"
        );
    }
}

#[test]
fn webpack5_standalone_chunk_keeps_public_helper_modules() {
    // Webpack lazy chunks have no in-file entry. Their module exports are
    // consumed by the runtime in another physical asset, so absence of a local
    // binding importer is not proof that a pure helper module is dead.
    let source = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([
  [123],
  {
    44320: function(module, exports, require) {
      require.d(exports, { A: function() { return extend; } });
      function extend() {
        return (extend = Object.assign ? Object.assign.bind() : function(target) {
          for (var i = 1; i < arguments.length; i++) {
            var source = arguments[i];
            for (var key in source) {
              if (Object.prototype.hasOwnProperty.call(source, key)) target[key] = source[key];
            }
          }
          return target;
        }).apply(null, arguments);
      }
    },
    88915: function(module, exports, require) {
      require.d(exports, { A: function() { return rest; } });
      function rest(source, excluded) {
        if (source == null) return {};
        var target = {};
        for (var key in source) {
          if (Object.prototype.hasOwnProperty.call(source, key) && excluded.indexOf(key) < 0) {
            target[key] = source[key];
          }
        }
        return target;
      }
    }
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "chunk.js".to_string(),
            dce_mode: DceMode::TransformOnly,
            ..Default::default()
        },
    )
    .expect("standalone webpack helper chunk should unpack");

    let filenames = output
        .modules
        .iter()
        .map(|(filename, _)| filename.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        filenames.len(),
        2,
        "public chunk modules were lost: {filenames:?}"
    );
    assert!(filenames.contains(&"module-44320.js"), "{filenames:?}");
    assert!(filenames.contains(&"module-88915.js"), "{filenames:?}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_mixed_chunk_keeps_helper_without_local_importer() {
    // Mixed standalone chunk: an app module (impure, no imports) plus a pure
    // exported helper that no module in this asset imports. The helper's
    // consumers live in another physical asset behind the webpack runtime
    // registry, so it must survive even though the total-drop guard cannot
    // fire (the app module survives on its own).
    let source = r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([
  [456],
  {
    10001: function(module, exports, require) {
      require.d(exports, { boot: function() { return boot; } });
      function boot() {
        console.log("app module booted");
      }
      boot();
    },
    10002: function(module, exports, require) {
      require.d(exports, { A: function() { return extend; } });
      function extend() {
        return (extend = Object.assign ? Object.assign.bind() : function(target) {
          for (var i = 1; i < arguments.length; i++) {
            var source = arguments[i];
            for (var key in source) {
              if (Object.prototype.hasOwnProperty.call(source, key)) target[key] = source[key];
            }
          }
          return target;
        }).apply(null, arguments);
      }
    }
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "chunk.js".to_string(),
            dce_mode: DceMode::TransformOnly,
            ..Default::default()
        },
    )
    .expect("mixed webpack chunk should unpack");

    let filenames = output
        .modules
        .iter()
        .map(|(filename, _)| filename.as_str())
        .collect::<Vec<_>>();
    assert!(filenames.contains(&"module-10001.js"), "{filenames:?}");
    assert!(
        filenames.contains(&"module-10002.js"),
        "externally consumed helper was dropped: {filenames:?}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_chunk_unpacks_arrow_and_method_factories() {
    let source = r#"
(self.webpackChunk_N_E = self.webpackChunk_N_E || []).push([
  [7],
  {
    100: (module, exports, require) => {
      "use strict";
      var dep = require(200);
      exports.default = dep.value;
    },
    200(module, exports, require) {
      "use strict";
      exports.value = 42;
    }
  }
]);
"#;

    let pairs = expect_unpack(source, "chunk.js");

    assert_eq!(pairs.len(), 2);
    let mod_100 = pairs
        .iter()
        .find(|(name, _)| name == "module-100.js")
        .expect("module-100.js should exist");
    assert!(
        mod_100.1.contains("./module-200.js"),
        "numeric require should be rewritten for arrow factory:\n{}",
        mod_100.1
    );
    assert!(
        pairs.iter().any(|(name, _)| name == "module-200.js"),
        "method factory module should be emitted"
    );
}

#[test]
fn webpack5_chunk_with_logical_assignment_push() {
    // rspack writes the chunk-loading global as `(G ||= []).push(...)` when
    // its target supports logical assignment.
    let source = r#"
(globalThis.rspackChunkapp ||= []).push([
  [10],
  {
    100(module, exports, require) {
      "use strict";
      var dep = require(200);
      exports.default = dep.value;
    },
    200(module, exports, require) {
      "use strict";
      exports.value = 42;
    }
  }
]);
"#;

    let pairs = expect_unpack(source, "chunk.js");

    assert_eq!(pairs.len(), 2, "{pairs:#?}");
    let mod_100 = file(&pairs, "module-100.js");
    assert!(
        mod_100.contains("./module-200.js"),
        "numeric require should be rewritten:\n{mod_100}"
    );
    file(&pairs, "module-200.js");
}

#[test]
fn webpack5_chunk_logical_assignment_needs_an_empty_array() {
    let source = r#"
(globalThis.rspackChunkapp ||= [1]).push([
  [10],
  {
    100(module, exports) {
      exports.value = 42;
    }
  }
]);
"#;

    let pairs = expect_unpack(source, "chunk.js");

    assert!(
        pairs.iter().all(|(name, _)| name != "module-100.js"),
        "a non-empty fallback is not a chunk-loading global:\n{pairs:#?}"
    );
}

#[test]
fn webpack5_chunk_rewrites_numeric_require_in_method_factory() {
    let source = r#"
(self.webpackChunk_N_E = self.webpackChunk_N_E || []).push([
  [7],
  {
    100(module, exports, require) {
      "use strict";
      var dep = require(200);
      exports.default = dep.value;
    },
    200(module, exports, require) {
      "use strict";
      exports.value = 42;
    }
  }
]);
"#;

    let pairs = expect_unpack(source, "chunk.js");

    assert_eq!(pairs.len(), 2);
    let mod_100 = pairs
        .iter()
        .find(|(name, _)| name == "module-100.js")
        .expect("module-100.js should exist");
    assert!(
        !mod_100.1.contains("require(200)"),
        "raw numeric require should not remain in method factory:\n{}",
        mod_100.1
    );
    assert!(
        mod_100.1.contains("./module-200.js"),
        "numeric require should be rewritten for method factory:\n{}",
        mod_100.1
    );
}

#[test]
fn webpack5_chunk_with_string_keys() {
    let source = r#"
(self.webpackChunk_N_E = self.webpackChunk_N_E || []).push([
  [123],
  {
    "68494": function(U, B, G) {
      "use strict";
      G.r(B);
      G.d(B, {
        "default": function() { return V; }
      });
      function V() { return 42; }
    }
  }
]);
"#;

    let pairs = expect_unpack(source, "chunk.js");

    assert_eq!(pairs.len(), 1);
    assert!(
        !pairs[0].1.contains("G.r("),
        "require.r should be normalized"
    );
    assert!(
        !pairs[0].1.contains("G.d("),
        "require.d should be normalized"
    );
}

#[test]
fn webpack5_chunk_with_webpack4_style_require_d() {
    // Chunks can use webpack4-style require.d(exports, "name", getter)
    let source = r#"
(self.webpackChunk_N_E = self.webpackChunk_N_E || []).push([
  [1],
  {
    100: function(module, exports, require) {
      "use strict";
      require.r(exports);
      require.d(exports, "foo", function() { return bar; });
      var bar = 42;
    }
  }
]);
"#;

    let pairs = expect_unpack(source, "chunk.js");

    assert_eq!(pairs.len(), 1);
    assert!(
        !pairs[0].1.contains("require.d("),
        "require.d should be normalized"
    );
}

#[test]
fn webpack5_chunk_with_window_base() {
    let source = r#"
(window["webpackJsonp"] = window["webpackJsonp"] || []).push([
  [0],
  {
    10: function(module, exports, require) {
      "use strict";
      require.r(exports);
      exports.default = "hello";
    }
  }
]);
"#;

    let pairs = expect_unpack(source, "chunk.js");

    assert_eq!(pairs.len(), 1);
}

#[test]
fn export_initialization_chains_survive_repeated_unpack_recovery() {
    for declaration in ["const", "var"] {
        let source = format!(
            r#"
(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {{
    100: function(module, exports, require) {{
        exports.last = exports.run = exports.first = void 0;
        const first = () => 1;
        exports.first = first;
        {declaration} run = () => 2;
        exports.run = run;
        const last = () => 3;
        exports.last = last;
        function dynamic(code) {{ return eval(code); }}
    }}
}}]);
"#
        );
        for emit_source_map in [false, true] {
            let output = unpack(
                &source,
                DecompileOptions {
                    filename: "chunk.js".into(),
                    emit_source_map,
                    diagnostics: true,
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(!output.has_errors(), "{:?}", output.warnings);
            let findings = validate_output_modules(&output.modules);
            assert!(findings.is_empty(), "{findings:?}\n{:?}", output.modules);
            assert!(
                output
                    .modules
                    .iter()
                    .any(|(_, source)| source.contains("export") && source.contains("run")),
                "{:?}",
                output.modules
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Code beside the push: the chunk prelude
// ---------------------------------------------------------------------------

/// A BannerPlugin `raw: true` banner at the report stage, added after
/// minification as its own statement; `{name}` is the global it writes.
fn report_stage_banner(name: &str) -> String {
    format!(
        r#"!function(){{try{{globalThis.{name}={{v:1}}}}catch(e){{console.error("bad banner")}}}}();"#
    )
}

/// An ESM module (240) and a module that stays CommonJS (311: a computed
/// export name).
const CHUNK_PUSH: &str = r#"(self.webpackChunk_app=self.webpackChunk_app||[]).push([[204],{240(e,n,d){function a(e){return"label:"+e}d.r(n),d.d(n,{label:()=>a})},311(e,n){n[globalThis.key]=1,e.exports.other=n}}])"#;

fn file<'a>(pairs: &'a [(String, String)], name: &str) -> &'a str {
    &pairs
        .iter()
        .find(|(file, _)| file == name)
        .unwrap_or_else(|| {
            panic!(
                "{name} should exist, got {:?}",
                pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
            )
        })
        .1
}

fn assert_prelude_required_by_every_module(pairs: &[(String, String)]) {
    assert!(
        file(pairs, "module-240.js").starts_with(r#"import "./prelude-204.js";"#),
        "the ESM module must import the prelude first, got:\n{}",
        file(pairs, "module-240.js")
    );
    assert!(
        file(pairs, "module-311.js").starts_with(r#"require("./prelude-204.js");"#),
        "the CommonJS module must require the prelude first, got:\n{}",
        file(pairs, "module-311.js")
    );
}

#[test]
fn banner_statement_before_the_push_goes_to_the_prelude() {
    // shape: producer webpack@5.111.1 mode=production (Terser), BannerPlugin
    // raw=true stage=PROCESS_ASSETS_STAGE_REPORT (added after minification,
    // so it stays its own statement); global renamed to `globalThis.banner`.
    let source = format!(
        "{}\n\"use strict\";{CHUNK_PUSH};",
        report_stage_banner("banner")
    );

    let pairs = expect_unpack(&source, "204.js");
    let prelude = file(&pairs, "prelude-204.js");
    assert!(
        prelude.contains("globalThis.banner = {"),
        "the banner must be kept in the prelude, got:\n{prelude}"
    );
    assert!(
        !prelude.contains("webpackChunk_app") && !prelude.contains("use strict"),
        "the push and the directive are not prelude code, got:\n{prelude}"
    );
    assert_prelude_required_by_every_module(&pairs);
    assert!(
        !pairs.iter().any(|(name, _)| name == "entry.js"),
        "a chunk has no entry, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    assert!(
        validate_output_modules(&pairs).is_empty(),
        "output must validate: {:?}",
        validate_output_modules(&pairs)
    );
}

#[test]
fn raw_unpack_keeps_the_prelude_and_its_requires() {
    // shape: producer webpack@5.111.1 mode=production (Terser), BannerPlugin
    // raw=true stage=PROCESS_ASSETS_STAGE_REPORT.
    let source = format!("{}\n{CHUNK_PUSH};", report_stage_banner("banner"));

    let output = unpack_raw(
        &source,
        &DecompileOptions {
            filename: "204.js".to_string(),
            ..Default::default()
        },
    )
    .expect("raw unpack should succeed");
    let pairs = output.modules;
    assert!(file(&pairs, "prelude-204.js").contains("globalThis.banner"));
    for name in ["module-240.js", "module-311.js"] {
        assert!(
            file(&pairs, name).contains(r#"require("./prelude-204.js")"#),
            "{name} must require the prelude, got:\n{}",
            file(&pairs, name)
        );
    }
}

#[test]
fn directives_beside_the_push_write_no_prelude() {
    // shape: producer webpack@5.111.1 mode=production (Terser).
    let source = format!(r#""use strict";{CHUNK_PUSH};"#);

    let pairs = expect_unpack(&source, "204.js");
    assert!(
        !pairs.iter().any(|(name, _)| name.starts_with("prelude")),
        "a directive alone must not write a prelude, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    assert!(!file(&pairs, "module-311.js").contains("prelude"));
}

#[test]
fn each_chunk_input_gets_its_own_prelude() {
    // shape: producer webpack@5.111.1 mode=production (Terser), BannerPlugin
    // raw=true stage=PROCESS_ASSETS_STAGE_REPORT with the chunk name in the
    // banner; two async chunks unpacked together.
    let first = format!("{}\n{CHUNK_PUSH};", report_stage_banner("banner_a"));
    let second = format!(
        r#"{}
(self.webpackChunk_app=self.webpackChunk_app||[]).push([[305],{{410(e,n,d){{e.exports=d(311)}}}}]);"#,
        report_stage_banner("banner_b")
    );

    let output = wakaru_core::driver::test_support::unpack_files(
        vec![
            wakaru_core::driver::test_support::UnpackInput {
                source: first,
                filename: "204.js".to_string(),
            },
            wakaru_core::driver::test_support::UnpackInput {
                source: second,
                filename: "305.js".to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("unpack should succeed");
    let pairs = output.modules;
    assert!(file(&pairs, "prelude-204.js").contains("banner_a"));
    assert!(file(&pairs, "prelude-305.js").contains("banner_b"));
    let consumer = file(&pairs, "module-410.js");
    assert!(
        consumer.contains("./prelude-305.js") && consumer.contains("./module-311.js"),
        "the second chunk's module must load its own prelude and link across inputs, got:\n{consumer}"
    );
    assert_prelude_required_by_every_module(&pairs);
}

// ---------------------------------------------------------------------------
// A push merged into a top-level sequence
// ---------------------------------------------------------------------------

/// The release and debug-id snippet `@sentry/webpack-plugin` injects before
/// every chunk, with synthetic release and debug ids.
/// esbuild's `--keep-names` helper, declared before the push and called in
/// the factories, around module 240's `require.d` getter too. shape:
/// producer esbuild@0.25.0 --minify --keep-names over a producer
/// webpack@5.111.1-style chunk.
const KEEP_NAMES_CHUNK: &str = r#""use strict";var u=Object.defineProperty;var l=(a,e)=>u(a,"name",{value:e,configurable:!0});(self.webpackChunk_app=self.webpackChunk_app||[]).push([[204],{240(a,e,n){n.d(e,{label:l(()=>p,"label")});const p=l(function(t){return"label:"+t},"label")},311(a,e,n){a.exports={make:l(function(){return n(240).label(1)},"make")}},312(a,e){e.flag=1}}]);"#;

#[test]
fn chunk_module_reading_a_prelude_declaration_imports_it() {
    // Split, the helper is module-scoped in the prelude: the prelude
    // exports it, and the modules that call it bind it. A module that reads
    // no prelude name still loads the prelude first.
    let pairs = expect_unpack(KEEP_NAMES_CHUNK, "204.js");
    let prelude = file(&pairs, "prelude-204.js");
    assert!(
        prelude.contains("export const l ="),
        "the prelude must export the helper, got:\n{prelude}"
    );
    assert!(
        !prelude.contains("webpackChunk_app") && !prelude.contains("use strict"),
        "the push and the directive are not prelude code, got:\n{prelude}"
    );
    // The helper's call around the getter is the getter itself, so the
    // export is recovered.
    let reader = file(&pairs, "module-240.js");
    assert!(
        reader.starts_with(r#"import { l } from "./prelude-204.js";"#)
            && reader.contains("export const label = l(function"),
        "module 240 must import the helper and export `label`, got:\n{reader}"
    );
    // Module 311 stays CommonJS (its `require` is inside a function).
    let commonjs = file(&pairs, "module-311.js");
    assert!(
        commonjs.starts_with(r#"const { l } = require("./prelude-204.js");"#),
        "module 311 must bind the helper, got:\n{commonjs}"
    );
    let other = file(&pairs, "module-312.js");
    assert!(
        other.starts_with(r#"import "./prelude-204.js";"#),
        "a module reading no prelude name must still load it, got:\n{other}"
    );
    assert!(
        validate_output_modules(&pairs).is_empty(),
        "output must validate: {:?}",
        validate_output_modules(&pairs)
    );
}

#[test]
fn raw_chunk_binds_prelude_names_with_require() {
    let output = unpack_raw(
        KEEP_NAMES_CHUNK,
        &DecompileOptions {
            filename: "204.js".to_string(),
            ..Default::default()
        },
    )
    .expect("raw unpack should succeed");
    let pairs = output.modules;
    let reader = file(&pairs, "module-240.js");
    assert!(
        reader.contains(r#"var { l } = require("./prelude-204.js");"#),
        "got:\n{reader}"
    );
    assert!(
        reader.contains("label: ()=>p") && !reader.contains(r#"l(()=>p, "label")"#),
        "the getter must lose its helper call, got:\n{reader}"
    );
    assert!(file(&pairs, "module-312.js").contains(r#"require("./prelude-204.js");"#));
    assert!(file(&pairs, "prelude-204.js").contains("export { l };"));
}

/// Raw module 240 of `source`, which must split.
fn raw_module_240(source: &str) -> String {
    let output = unpack_raw(
        source,
        &DecompileOptions {
            filename: "204.js".to_string(),
            ..Default::default()
        },
    )
    .expect("raw unpack should succeed");
    file(&output.modules, "module-240.js").to_string()
}

#[test]
fn getter_wrapped_by_a_reassigned_helper_keeps_the_call() {
    // The code before the push reassigns `l` after declaring it, so the
    // declaration does not prove what `l` is when the factories run; its
    // call around the getter stays. shape: hypothetical.
    let source =
        KEEP_NAMES_CHUNK.replace(r#"configurable:!0});"#, r#"configurable:!0});l=(a,e)=>a;"#);

    let module = raw_module_240(&source);
    assert!(
        module.contains(r#"var { l } = require("./prelude-204.js");"#)
            && module.contains(r#"l(()=>p, "label")"#),
        "got:\n{module}"
    );
}

#[test]
fn helper_reassigned_after_the_push_stays_unsplit() {
    // shape: hypothetical.
    let source = format!("{KEEP_NAMES_CHUNK}l=function(f){{return f}};");

    let pairs = expect_unpack(&source, "204.js");
    assert_eq!(
        pairs.len(),
        1,
        "the input must stay one file, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn getter_wrapped_by_another_helper_keeps_the_call() {
    // `l` takes the name from its argument but is not the keep-names
    // helper, so its call around the getter stays. shape: hypothetical.
    let source = KEEP_NAMES_CHUNK.replace(
        r#"var l=(a,e)=>u(a,"name",{value:e,configurable:!0});"#,
        r#"var l=(a,e)=>u(a,"name",{value:e+"!",configurable:!0});"#,
    );

    let module = raw_module_240(&source);
    assert!(module.contains(r#"l(()=>p, "label")"#), "got:\n{module}");
}

#[test]
fn chunk_module_reading_a_declaration_after_the_push_stays_unsplit() {
    // Only a declaration before the push is bound. shape: hypothetical.
    let source = r#"(self.webpackChunk_app=self.webpackChunk_app||[]).push([[204],{240(e,n,d){function a(e){return decode(0)+e}d.r(n),d.d(n,{label:()=>a})}}]);
var decode=function(i){return"label:"};"#;

    let pairs = expect_unpack(source, "204.js");
    assert_eq!(
        pairs.len(),
        1,
        "the input must stay one file, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn chunk_module_writing_a_prelude_declaration_stays_unsplit() {
    // An import binding cannot be assigned. shape: hypothetical.
    let source = r#"var count=0;
(self.webpackChunk_app=self.webpackChunk_app||[]).push([[204],{240(e,n){n.next=function(){return++count}}}]);"#;

    let pairs = expect_unpack(source, "204.js");
    assert_eq!(
        pairs.len(),
        1,
        "the input must stay one file, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn prelude_declaration_used_only_by_the_prelude_still_splits() {
    // shape: hypothetical.
    let source = format!(
        r#"var strings=["release-a"];globalThis.release=strings[0];
{CHUNK_PUSH};"#
    );

    let pairs = expect_unpack(&source, "204.js");
    assert!(file(&pairs, "prelude-204.js").contains("globalThis.release"));
    assert_prelude_required_by_every_module(&pairs);
}

const DEBUG_ID_SNIPPET: &str = r#"!function(){try{var e="undefined"!=typeof window?window:"undefined"!=typeof global?global:"undefined"!=typeof globalThis?globalThis:"undefined"!=typeof self?self:{};e.SENTRY_RELEASE={id:"release-a"};var n=(new e.Error).stack;n&&(e._sentryDebugIds=e._sentryDebugIds||{},e._sentryDebugIds[n]="00000000-0000-4000-8000-000000000001",e._sentryDebugIdIdentifier="sentry-dbid-00000000-0000-4000-8000-000000000001")}catch(e){}}()"#;

#[test]
fn snippet_merged_into_a_sequence_with_the_push_goes_to_a_prelude() {
    // shape: producer webpack@5.111.1 + @sentry/webpack-plugin@5.4.1 + Terser
    // (the plugin injects the snippet before minification, and Terser merges
    // it with the push into one statement).
    let source = format!("{DEBUG_ID_SNIPPET},{CHUNK_PUSH};");

    let pairs = expect_unpack(&source, "204.js");
    let prelude = file(&pairs, "prelude-204.js");
    assert!(
        prelude.contains(r#"id: "release-a""#) && prelude.contains("_sentryDebugIds"),
        "the snippet must be kept in the prelude, got:\n{prelude}"
    );
    assert!(
        !prelude.contains("webpackChunk_app"),
        "the push is not prelude code, got:\n{prelude}"
    );
    assert_prelude_required_by_every_module(&pairs);
    assert!(
        validate_output_modules(&pairs).is_empty(),
        "output must validate: {:?}",
        validate_output_modules(&pairs)
    );
}

#[test]
fn code_after_the_push_in_the_sequence_goes_to_the_prelude() {
    // shape: producer webpack@5.111.1 mode=production (Terser), BannerPlugin
    // raw=true footer=true at its default stage; the footer writes a
    // loaded flag.
    let source =
        format!(r#""use strict";{CHUNK_PUSH},self.loaded=self.loaded||{{}},self.loaded.chunk=1;"#);

    let pairs = expect_unpack(&source, "204.js");
    let prelude = file(&pairs, "prelude-204.js");
    assert!(
        prelude.contains("self.loaded = self.loaded || {};")
            && prelude.contains("self.loaded.chunk = 1;"),
        "the footer must be kept in the prelude, got:\n{prelude}"
    );
    assert!(
        !prelude.contains("use strict"),
        "the directive is not prelude code, got:\n{prelude}"
    );
    assert_prelude_required_by_every_module(&pairs);
}

#[test]
fn push_inside_a_function_of_a_sequence_is_not_a_chunk() {
    // Only top-level sequence elements are statements; a push-shaped call in
    // a nested function runs only when that function is called.
    // shape: hypothetical.
    let source = format!("a(),function(){{{CHUNK_PUSH}}};");

    let pairs = expect_unpack(&source, "page.js");
    assert_eq!(
        pairs.len(),
        1,
        "the input must stay one file, got {pairs:?}"
    );
    assert!(pairs[0].1.contains("webpackChunk_app"));
}

fn esm_chunk_source(declare: &str, ids: &str, modules: &str) -> String {
    format!(
        "export {declare} {ids}=[11111];export {declare} {modules}={{11111(e,t,r){{r.r(t),r.d(t,{{default:()=>n}});function n(){{return 1}}}},22222(e,t){{t.value=2}}}};"
    )
}

#[test]
fn esm_chunk_unpacks_under_each_export_naming() {
    // webpack 5 `output.chunkFormat: "module"` renamed these exports twice,
    // and newer releases may declare them with `var`. Rspack has its own ids
    // name.
    for (declare, ids, modules) in [
        ("const", "ids", "modules"),
        ("const", "__webpack_ids__", "__webpack_modules__"),
        ("const", "__webpack_esm_ids__", "__webpack_esm_modules__"),
        ("var", "__webpack_esm_ids__", "__webpack_esm_modules__"),
        ("const", "__rspack_esm_ids", "__webpack_modules__"),
    ] {
        let source = esm_chunk_source(declare, ids, modules);
        let pairs = expect_unpack(&source, "chunk.js");
        let mut filenames: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();
        filenames.sort();
        assert_eq!(
            filenames,
            vec!["module-11111.js", "module-22222.js"],
            "naming `{ids}`/`{modules}` should split"
        );
        let module = file(&pairs, "module-11111.js");
        assert!(!module.contains("r.r("), "got:\n{module}");
        assert!(module.contains("as default"), "got:\n{module}");
    }
}

#[test]
fn esm_chunk_with_separate_id_export_unpacks() {
    let source = "export const id = 11111;\nexport const ids = [11111];\nexport const modules = {\n  11111: (module, exports) => { exports.value = 1; }\n};\n";
    let pairs = expect_unpack(source, "chunk.js");
    let filenames: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(filenames, vec!["module-11111.js"]);
}

fn assert_stays_one_file(source: &str) {
    let pairs = expect_unpack(source, "chunk.js");
    assert_eq!(
        pairs.len(),
        1,
        "the input must stay one file, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    assert!(!pairs[0].0.starts_with("module-"), "got {:?}", pairs[0].0);
}

#[test]
fn esm_chunk_exporting_runtime_modules_stays_unsplit() {
    let source = format!(
        "{}export const __webpack_esm_runtime__=e=>{{e.x=1}};",
        esm_chunk_source("const", "__webpack_esm_ids__", "__webpack_esm_modules__")
    );
    assert_stays_one_file(&source);
}

#[test]
fn esm_chunk_beside_other_module_declarations_stays_unsplit() {
    let chunk = esm_chunk_source("const", "__webpack_esm_ids__", "__webpack_esm_modules__");
    assert_stays_one_file(&format!("import {{ run }} from \"./runtime.js\";\n{chunk}"));
    assert_stays_one_file(&format!("{chunk}\nexport const label = \"chunk\";"));
    // Another name declared beside the modules export.
    assert_stays_one_file(
        "export const __webpack_esm_ids__=[11111];export const __webpack_esm_modules__={11111(e,t){t.value=1}},label=\"chunk\";",
    );
}

#[test]
fn esm_chunk_banner_goes_to_the_prelude() {
    // shape: hypothetical, a banner statement before the chunk exports.
    let source = format!(
        "performance.mark(\"chunk\");\n{}",
        esm_chunk_source("const", "__rspack_esm_ids", "__webpack_modules__")
    );

    let pairs = expect_unpack(&source, "chunk.js");
    let prelude = file(&pairs, "prelude-11111.js");
    assert!(
        prelude.contains("performance.mark(\"chunk\")") && !prelude.contains("export"),
        "only the banner belongs in the prelude, got:\n{prelude}"
    );
    for name in ["module-11111.js", "module-22222.js"] {
        let module = file(&pairs, name);
        assert!(
            module.contains("./prelude-11111.js"),
            "{name} must load the prelude, got:\n{module}"
        );
    }
    assert!(
        validate_output_modules(&pairs).is_empty(),
        "output must validate: {:?}",
        validate_output_modules(&pairs)
    );
}

#[test]
fn module_exporting_a_modules_table_without_ids_is_not_a_chunk() {
    // An ordinary module can export a table of functions named `modules`.
    let source =
        "export const modules = {\n  11111: (module, exports) => { exports.value = 1; }\n};\n";
    assert_stays_one_file(source);
}
