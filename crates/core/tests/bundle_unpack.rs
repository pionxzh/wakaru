use std::fs;

use wakaru_core::{unpack, unpack_raw, BundleFormat, DecompileOptions};

#[test]
fn browserify_accepts_a_nonliteral_cache_argument() {
    let source = r#"
var sharedCache = {};
(function() { return function() {}; })()({
  1: [function(require, module) {
    module.exports = "entry";
  }, {}]
}, sharedCache, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify variable-cache bundle should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Browserify]);
    assert!(output.modules.iter().any(|(name, _)| name == "entry.js"));
}

#[test]
fn browserify_accepts_numeric_dependency_request_keys() {
    let source = r#"
(function() { return function() {}; })()({
  1: [function(require, module) {
    module.exports = require("2048");
  }, { 2048: 2 }],
  2: [function(require, module) {
    module.exports = "value";
  }, {}]
}, {}, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify numeric-request bundle should unpack");
    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("Browserify entry should exist");
    assert!(entry.contains("require(\"./module-2.js\")"), "{entry}");
}

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
        index.contains("import ") && index.contains(r#""./cjs.js""#),
        "expected recovered import in webpack5 require.n module:\n{index}"
    );
    assert!(
        !index.contains("require.n") && !index.contains("__esModule"),
        "webpack5 require.n helper should not survive:\n{index}"
    );
}

#[test]
fn webpack4_string_module_ids_use_relative_output_imports() {
    let source = r#"
!function(__webpack_modules__) {
  function __webpack_require__(moduleId) {
    var module = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  return __webpack_require__("./src/index.js");
}({
  "./src/value.js": function(module) {
    module.exports = "ok";
  },
  "./src/index.js": function(module, exports, __webpack_require__) {
    var value = __webpack_require__("./src/value.js");
    module.exports = value;
  }
});
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-string-ids.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 unpack should succeed");
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
        index.contains("import ") && index.contains(r#""./value.js""#),
        "expected import relative to src/index.js:\n{index}"
    );
    assert!(
        !index.contains(r#""./src/value.js""#),
        "import must not be relative to the bundle root:\n{index}"
    );
}

#[test]
fn webpack5_string_module_id_with_overlapping_dots_cannot_emit_parent_path() {
    let source = r#"
(() => {
  var __webpack_modules__ = ({
    "....//node_modules/@wakaru/cli/bin/wakaru": ((module) => {
      module.exports = "pwned";
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  console.log(__webpack_require__("....//node_modules/@wakaru/cli/bin/wakaru"));
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-overlap-path.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let names: Vec<&str> = output
        .modules
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(
        names.contains(&"..../node_modules/@wakaru/cli/bin/wakaru"),
        "expected sanitized overlap path, got {names:?}"
    );
    assert!(
        names
            .iter()
            .all(|name| !name.split('/').any(|part| part == "..")),
        "module filenames must not contain parent components: {names:?}"
    );
}

#[test]
fn webpack5_require_g_is_recovered_as_global() {
    let source = r#"
(() => {
  var __webpack_modules__ = ({
    "./src/browser-process.js": ((module) => {
      module.exports = { env: {} };
    }),
    "./src/global.js": ((__unused_webpack_module, exports, __webpack_require__) => {
      exports.envProcess = __webpack_require__.g.process?.env && typeof __webpack_require__.g.process?.env === "object"
        ? __webpack_require__.g.process
        : __webpack_require__("./src/browser-process.js");
      exports.readLocal = function(require) {
        return require.g;
      };
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var cachedModule = __webpack_module_cache__[moduleId];
    if (cachedModule !== undefined) return cachedModule.exports;
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__.g = (function() {
    if (typeof globalThis === "object") return globalThis;
    try {
      return this || new Function("return this")();
    } catch (e) {
      if (typeof window === "object") return window;
    }
  })();
  __webpack_require__("./src/global.js");
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-require-g.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let global = output
        .modules
        .iter()
        .find(|(name, _)| name == "src/global.js")
        .map(|(_, code)| code)
        .expect("expected global module");

    assert!(
        global.contains("global.process?.env")
            && global.contains("typeof global.process?.env === \"object\"")
            && global.contains("global.process"),
        "expected webpack require.g to recover as global:\n{global}"
    );
    assert!(
        !global.contains("require.g.process"),
        "webpack require.g.process should not survive:\n{global}"
    );
    assert!(
        global.contains("=>require.g"),
        "inner parameter named require should not be rewritten:\n{global}"
    );
}

#[test]
fn webpack5_amd_and_module_decorators_are_recovered() {
    let source = r#"
(() => {
    var __webpack_modules__ = ({
    "./src/runtime-helpers.js": ((module, exports, __webpack_require__) => {
      module = __webpack_require__.hmd(module);
      __webpack_require__.d(exports, { named: function() { return named; } }), module = __webpack_require__.hmd(module);
      const named = 1;
      exports.amd = __webpack_require__.amdO;
      exports.load = function(name) {
        return module.require(name);
      };
      exports.localRequire = function(require) {
        return require.amdO;
      };
    }),
    "./src/node-module.js": ((module, exports, __webpack_require__) => {
      module = __webpack_require__.nmd(module);
      exports.children = module.children;
      exports.localModule = function(module) {
        module = __webpack_require__.nmd(module);
        return module.children;
      };
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var cachedModule = __webpack_module_cache__[moduleId];
    if (cachedModule !== undefined) return cachedModule.exports;
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__("./src/runtime-helpers.js");
  __webpack_require__("./src/node-module.js");
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-runtime-helpers.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let runtime_helpers = output
        .modules
        .iter()
        .find(|(name, _)| name == "src/runtime-helpers.js")
        .map(|(_, code)| code)
        .expect("expected runtime helpers module");
    assert!(
        runtime_helpers.contains(r#"typeof define === "function" && define.amd"#),
        "expected require.amdO to recover as AMD detection:\n{runtime_helpers}"
    );
    assert!(
        !runtime_helpers.contains("module = require.hmd(module)")
            && !runtime_helpers.contains("amd = require.amdO"),
        "webpack hmd/amdO helpers should not survive:\n{runtime_helpers}"
    );
    assert!(
        runtime_helpers.contains("=>require.amdO"),
        "inner parameter named require should not be rewritten:\n{runtime_helpers}"
    );

    let node_module = output
        .modules
        .iter()
        .find(|(name, _)| name == "src/node-module.js")
        .map(|(_, code)| code)
        .expect("expected node module");
    let nmd_decorator_count = node_module.matches("module = require.nmd(module);").count();
    assert!(
        nmd_decorator_count == 1,
        "only shadowed local module decorator should remain:\n{node_module}"
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

    let entry = pairs
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("expected browserify entry module");
    assert!(
        entry.contains(r#""./module-2.js""#) && entry.contains(r#""./module-3.js""#),
        "browserify dependency maps should target emitted module filenames:\n{entry}"
    );
    assert!(
        !entry.contains(r#""./calculator""#) && !entry.contains(r#""./greeting""#),
        "original browserify request names should be remapped:\n{entry}"
    );
}
