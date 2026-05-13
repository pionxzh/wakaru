use wakaru_core::{unpack, DecompileOptions};

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
        output.warnings.is_empty(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    output.modules
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
fn webpack5_chunk_unpacks_modules() {
    let source = r#"
(self.webpackChunk_N_E = self.webpackChunk_N_E || []).push([
  [888],
  {
    2189: function(module, exports, require) {
      "use strict";
      require.r(exports);
      require.d(exports, {
        M: function() { return i; },
        u: function() { return o; }
      });
      var r = require(7294);
      var o = r.createContext({ isButtonGroup: false });
      var i = function() { return "hello"; };
    },
    5432: function(module, exports, require) {
      "use strict";
      require.r(exports);
      require.d(exports, {
        Z: function() { return s; }
      });
      var n = require(7294);
      function s(props) {
        return n.createElement("div", null, props.children);
      }
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
        filenames.contains(&"module-2189.js"),
        "expected module-2189.js, got {:?}",
        filenames
    );
    assert!(
        filenames.contains(&"module-5432.js"),
        "expected module-5432.js, got {:?}",
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
