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
