use std::fs;

use wakaru_core::{unpack, DecompileOptions};

#[test]
fn webpack5_unpack_extracts_multiple_modules() {
    let source_path = "../../testcases/webpack5/dist/index.js";
    let source = fs::read_to_string(source_path).expect("failed to read webpack5 testcase");

    let output = unpack(
        &source,
        DecompileOptions {
            filename: source_path.to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let pairs = output.modules;

    assert!(
        pairs.len() > 1,
        "expected webpack5 unpack to split modules, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert!(
        pairs.iter().any(|(name, _)| name == "entry.js"),
        "expected webpack5 unpack to include entry.js, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
}

#[test]
fn webpack5_require_n_default_interop_is_recovered() {
    let source = r#"
(() => {
  var __webpack_modules__ = ({
    "./src/cjs.js": ((module) => {
      module.exports = function greet(name) {
        return "Hello, " + name;
      };
    }),
    "./src/index.js": ((__unused_webpack_module, __webpack_exports__, __webpack_require__) => {
      __webpack_require__.r(__webpack_exports__);
      var _cjs__WEBPACK_IMPORTED_MODULE_0__ = __webpack_require__("./src/cjs.js");
      var _cjs__WEBPACK_IMPORTED_MODULE_0___default = __webpack_require__.n(_cjs__WEBPACK_IMPORTED_MODULE_0__);
      console.log(_cjs__WEBPACK_IMPORTED_MODULE_0___default()("Ada"));
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__("./src/index.js");
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-require-n.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let index = output
        .modules
        .iter()
        .find(|(name, _)| name == "src/index.js")
        .map(|(_, code)| code)
        .expect("expected index module");

    assert!(
        index.contains("import ") && index.contains(r#""./src/cjs.js""#),
        "expected recovered import in webpack5 require.n module:\n{index}"
    );
    assert!(
        !index.contains("require.n") && !index.contains("__esModule"),
        "webpack5 require.n helper should not survive:\n{index}"
    );
}

#[test]
fn browserify_unpack_extracts_multiple_modules() {
    let source_path = "../../testcases/browserify/dist/index.js";
    let source = fs::read_to_string(source_path).expect("failed to read browserify testcase");

    let output = unpack(
        &source,
        DecompileOptions {
            filename: source_path.to_string(),
            ..Default::default()
        },
    )
    .expect("browserify unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let pairs = output.modules;

    assert!(
        pairs.len() > 1,
        "expected browserify unpack to split modules, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert!(
        pairs.iter().any(|(name, _)| name == "entry.js"),
        "expected browserify unpack to include entry.js, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
}
