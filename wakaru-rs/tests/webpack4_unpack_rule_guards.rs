mod common;

use common::normalize;
use wakaru_rs::{unpack_webpack4, unpack_webpack4_raw};

fn raw_modules(source: &str) -> Vec<(String, String)> {
    unpack_webpack4_raw(source).expect("raw webpack4 unpack should succeed")
}

fn raw_module(source: &str, filename: &str) -> String {
    raw_modules(source)
        .into_iter()
        .find(|(name, _)| name == filename)
        .map(|(_, code)| normalize(&code))
        .unwrap_or_else(|| panic!("expected module {filename} to exist"))
}

#[test]
fn require_n_getter_accessor_rewrites_to_call_in_raw_output() {
    let source = r#"
!function(modules) {
  function __webpack_require__(id) {}
  __webpack_require__.s = 0;
  __webpack_require__(0);
}([
  function(module, exports, require) {
    var mod = require(1);
    var getter = require.n(mod);
    console.log(getter.a);
  },
  function(module, exports, require) {}
]);
"#;

    let code = raw_module(source, "entry.js");
    assert!(
        code.contains(r#"require("./module-1.js")"#),
        "expected require id rewrite:\n{code}"
    );
    assert!(
        code.contains("getter()"),
        "expected `.a` accessor to lower to a getter call:\n{code}"
    );
    assert!(
        code.contains("__esModule"),
        "expected explicit require.n getter logic:\n{code}"
    );
}

#[test]
fn require_d_non_exports_target_is_preserved() {
    let source = r#"
!function(modules) {
  function __webpack_require__(id) {}
  __webpack_require__.s = 0;
  __webpack_require__(0);
}([
  function(module, exports, require) {
    require.d(otherTarget, "named", function() { return value; });
  }
]);
"#;

    let code = raw_module(source, "entry.js");
    assert!(
        code.contains(r#"require.d(otherTarget, "named""#),
        "non-exports require.d call should stay intact:\n{code}"
    );
    assert!(
        !code.contains("exports.named ="),
        "non-exports require.d call was rewritten unsafely:\n{code}"
    );
}

#[test]
fn shadowed_require_is_not_rewritten() {
    let source = r#"
!function(modules) {
  function __webpack_require__(id) {}
  __webpack_require__.s = 0;
  __webpack_require__(0);
}([
  function(module, exports, require) {
    function inner(require) {
      return require(1);
    }
    inner(require);
    return require(1);
  },
  function(module, exports, require) {}
]);
"#;

    let code = raw_module(source, "entry.js");
    assert!(
        code.contains(r#"return require(1);"#),
        "shadowed require call inside nested scope should stay numeric:\n{code}"
    );
    assert!(
        code.contains(r#"return require("./module-1.js");"#),
        "top-level require call should still rewrite:\n{code}"
    );
}

#[test]
fn entry_detection_ignores_unrelated_dot_s_assignment() {
    let source = r#"
!function(modules) {
  window.s = 0;
}([
  function(module, exports, require) {
    exports.value = 1;
  }
]);
"#;

    let pairs = raw_modules(source);
    assert_eq!(pairs.len(), 1, "expected one extracted module");
    assert_eq!(pairs[0].0, "module-0.js");
}

#[test]
fn unsupported_bundle_shape_returns_none_cleanly() {
    let source = r#"
console.log("not a webpack bundle");
"#;

    assert!(unpack_webpack4_raw(source).is_none());
    assert!(unpack_webpack4(source).is_none());
}
