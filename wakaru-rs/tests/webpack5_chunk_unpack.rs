use wakaru_rs::{unpack, DecompileOptions};

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

    let pairs = unpack(
        source,
        DecompileOptions {
            filename: "chunk.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 chunk unpack should succeed");

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

    let pairs = unpack(
        source,
        DecompileOptions {
            filename: "chunk.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 chunk with string keys should unpack");

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

    let pairs = unpack(
        source,
        DecompileOptions {
            filename: "chunk.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 chunk with wp4 require.d should unpack");

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

    let pairs = unpack(
        source,
        DecompileOptions {
            filename: "chunk.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 chunk with window base should unpack");

    assert_eq!(pairs.len(), 1);
}
