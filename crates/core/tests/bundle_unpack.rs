use std::fs;

use wakaru_core::driver::test_support::{unpack, unpack_raw};
use wakaru_core::{validate_output_modules, BundleFormat, DecompileOptions};

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
fn browserify_accepts_a_nonliteral_dependency_map() {
    let source = r#"
var entryDependencies = { "./value": 2 };
(function() { return function() {}; })()({
  1: [function(require, module) {
    module.exports = require("./value");
  }, entryDependencies],
  2: [function(require, module) {
    module.exports = "value";
  }, {}]
}, {}, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify dynamic dependency-map bundle should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Browserify]);
    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("Browserify entry should exist");
    assert!(
        entry.contains(r#"require("./value")"#),
        "an unproven dynamic map must preserve the original request:\n{entry}"
    );
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
fn browserify_uses_unambiguous_dependency_requests_as_filenames() {
    let source = r#"
(function() { return function() {}; })()({
  1: [function(require, module) {
    module.exports = require("./lib/utility");
  }, { "./lib/utility": 2 }],
  2: [function(require, module) {
    module.exports = "utility";
  }, {}]
}, {}, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify request-path bundle should unpack");
    let names = output
        .modules
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["entry.js", "lib/utility.js"]);
    let entry = &output.modules[0].1;
    assert!(
        entry.contains(r#"require("./lib/utility.js")"#),
        "dependency rewrite must consume the readable filename:\n{entry}"
    );
}

#[test]
fn browserify_preserves_case_insensitive_javascript_extensions() {
    let source = r#"
(function() { return function() {}; })()({
  1: [function(require, module) {
    module.exports = require("./UTILITY.JS");
  }, { "./UTILITY.JS": 2 }],
  2: [function(require, module) {
    module.exports = "utility";
  }, {}]
}, {}, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify uppercase-extension bundle should unpack");
    assert!(
        output.modules.iter().any(|(name, _)| name == "UTILITY.JS"),
        "an existing JavaScript extension must not be duplicated"
    );
    let entry = &output.modules[0].1;
    assert!(
        entry.contains(r#"require("./UTILITY.JS")"#),
        "dependency rewrite must preserve the hinted extension:\n{entry}"
    );
}

#[test]
fn browserify_falls_back_when_requests_disagree_on_a_module_name() {
    let source = r#"
(function() { return function() {}; })()({
  1: [function(require, module) {
    module.exports = [require("./first"), require("./second")];
  }, { "./first": 2, "./second": 2 }],
  2: [function(require, module) {
    module.exports = "shared";
  }, {}]
}, {}, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify alias bundle should unpack");
    assert!(
        output.modules.iter().any(|(name, _)| name == "module-2.js"),
        "ambiguous aliases must retain the stable numeric fallback"
    );
    let entry = &output.modules[0].1;
    assert_eq!(entry.matches(r#"require("./module-2.js")"#).count(), 2);
}

#[test]
fn browserify_reserves_entries_and_deduplicates_hint_paths_case_insensitively() {
    let source = r#"
(function() { return function() {}; })()({
  1: [function(require, module) {
    module.exports = [require("./entry"), require("./Utility"), require("./utility")];
  }, { "./entry": 2, "./Utility": 3, "./utility": 4 }],
  2: [function(require, module) { module.exports = "named like entry"; }, {}],
  3: [function(require, module) { module.exports = "upper"; }, {}],
  4: [function(require, module) { module.exports = "lower"; }, {}]
}, {}, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify colliding-name bundle should unpack");
    let names = output
        .modules
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        ["entry.js", "entry-2.js", "Utility.js", "utility-2.js"]
    );
}

#[test]
fn browserify_deconflicts_factory_param_rename_capture() {
    let source = r#"
(function() { return function() {}; })()({
  1: [function(r, m, e) {
    function invoke(require) {
      return [require, r("./dependency")];
    }
    m.exports = invoke;
  }, { "./dependency": 2 }],
  2: [function(r, m, e) { m.exports = "dependency"; }, {}]
}, {}, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify capture should be repaired and unpacked");
    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("Browserify entry should exist");
    assert!(
        entry.contains("function invoke(_require)")
            && entry.contains(r#"require("./dependency.js")"#),
        "the nested binding must be renamed before the runtime loader:\n{entry}"
    );
}

#[test]
fn webpack5_deconflicts_factory_param_rename_capture() {
    let source = r#"
(() => {
  var __webpack_modules__ = ({
    1: ((m, e, r) => {
      function invoke(require) {
        return [require, r(2)];
      }
      m.exports = invoke;
    }),
    2: ((m) => { m.exports = "dependency"; })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(id) {
    var m = __webpack_module_cache__[id] = { exports: {} };
    __webpack_modules__[id](m, m.exports, __webpack_require__);
    return m.exports;
  }
  __webpack_require__(1);
})();
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("webpack5 capture should be repaired and unpacked");
    let module = output
        .modules
        .iter()
        .find(|(name, _)| name == "module-1.js")
        .map(|(_, code)| code)
        .expect("webpack5 module should exist");
    assert!(
        module.contains("function invoke(_require)")
            && module.contains(r#"require("./module-2.js")"#),
        "the nested binding must be renamed before the runtime loader:\n{module}"
    );
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
fn webpack5_css_module_composition_recovers_one_mutable_default_object() {
    let source = r#"
(() => {
  var __webpack_modules__ = ({
    0: ((module, exports, __webpack_require__) => {
      module.exports = {};
      Object.assign(module.exports, __webpack_require__(1) || {});
      Object.assign(module.exports, __webpack_require__(2) || {});
    }),
    1: ((module) => {
      module.exports = { alpha: "alpha-token", shared: "first-token" };
    }),
    2: ((module) => {
      module.exports = { beta: "beta-token", shared: "second-token" };
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-css-composition.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 CSS composition should unpack");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let composed = output
        .modules
        .iter()
        .find(|(name, _)| name == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected composed module");
    assert!(
        !composed.contains("module.exports") && !composed.contains("require("),
        "the runtime composition should become a valid ESM module:\n{composed}"
    );
    assert_eq!(composed.matches("Object.assign(").count(), 2, "{composed}");
    assert!(
        !composed.contains("export default {};"),
        "the exported default must be the object mutated by both copies:\n{composed}"
    );
    assert!(
        composed.contains("Object.assign(_defaultObject,")
            && composed.contains("export default _defaultObject;"),
        "the recovered copies and export must share one object:\n{composed}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
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
fn webpack4_non_javascript_module_ids_emit_javascript_filenames() {
    let source = r#"
!function(__webpack_modules__) {
  function __webpack_require__(moduleId) {
    var module = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  return __webpack_require__("./src/index.ts");
}({
  "./src/style/index.less": function(module) {
    module.exports = "compiled style";
  },
  "./src/index.ts": function(module, exports, __webpack_require__) {
    module.exports = __webpack_require__("./src/style/index.less");
  }
});
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-non-js-id.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 non-JavaScript module id should unpack");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let index = output
        .modules
        .iter()
        .find(|(name, _)| name == "src/index.ts")
        .map(|(_, code)| code)
        .expect("the JavaScript-like TypeScript path should be retained");
    assert!(
        index.contains(r#""./style/index.less.js""#),
        "the consumer must use the derived JavaScript filename:\n{index}"
    );
    assert!(
        output
            .modules
            .iter()
            .any(|(name, _)| name == "src/style/index.less.js"),
        "expected loader-produced JavaScript to append .js"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack4_global_var_injection_exposes_named_exports() {
    let source = r#"
!function(modules) {
  function require(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, require);
    return module.exports;
  }
  return require("./src/entry.js");
}({
  "./node_modules/webpack/buildin/global.js": function(module) {
    module.exports = globalThis;
  },
  "./src/global-user.js": function(module, exports, __webpack_require__) {
    (function(injectedGlobal) {
      exports.getGlobal = function() {
        return injectedGlobal;
      };
    }).call(this, __webpack_require__("./node_modules/webpack/buildin/global.js"));
  },
  "./src/entry.js": function(module, exports, __webpack_require__) {
    module.exports = __webpack_require__("./src/global-user.js").getGlobal();
  }
});
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-global-injection.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 global injection should unpack");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let provider = output
        .modules
        .iter()
        .find(|(name, _)| name == "src/global-user.js")
        .map(|(_, code)| code)
        .expect("expected injected-global provider");
    assert!(
        provider.contains("export") && provider.contains("getGlobal"),
        "the wrapper must expose its CommonJS assignment to UnEsm:\n{provider}"
    );
    assert!(
        !provider.contains(".call(this") && !provider.contains("require("),
        "the webpack wrapper and nested require must be recovered:\n{provider}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack4_injected_global_fallback_exposes_default_export() {
    let source = r#"
!function(modules) {
  function require(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, require);
    return module.exports;
  }
  return require(0);
}([
  function(module, exports, require) {
    module.exports = require(1);
  },
  function(module, exports, require) {
    (function(injectedGlobal) {
      function selectGlobal(candidate) {
        return candidate && candidate.Math === Math && candidate;
      }
      module.exports = selectGlobal("object" === typeof globalThis && globalThis)
        || selectGlobal("object" === typeof self && self)
        || selectGlobal("object" === typeof window && window)
        || selectGlobal("object" === typeof injectedGlobal && injectedGlobal)
        || Function("return this")();
    }).call(this, require(2));
  },
  function(module) {
    globalThis.injectionLoads = (globalThis.injectionLoads || 0) + 1;
    module.exports = globalThis;
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-global-fallback.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 global fallback should unpack");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let provider = output
        .modules
        .iter()
        .find(|(name, _)| name == "module-1.js")
        .map(|(_, code)| code)
        .expect("expected global detector module");
    assert!(
        provider.contains("export default"),
        "the wrapper must expose module.exports to UnEsm:\n{provider}"
    );
    assert!(
        provider.contains(r#"import "./module-2.js""#),
        "the unidentified provider must remain as an eager side-effect import:\n{provider}"
    );
    assert!(
        !provider.contains("injectedGlobal"),
        "the obsolete injected fallback binding must be removed:\n{provider}"
    );
    assert!(
        output
            .modules
            .iter()
            .any(|(name, code)| name == "module-2.js" && code.contains("injectionLoads")),
        "the side-effectful injected provider must be emitted"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack4_reused_loader_parameter_becomes_a_local_after_module_loads() {
    let source = r#"
!function(modules) {
  function load(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  return load(0);
}([
  function(module, exports, load) {
    var dependency = load(1);
    load = /compiled/;
    module.exports = [dependency, load.test("compiled")];
  },
  function(module) {
    module.exports = "dependency";
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-reused-loader.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 reused loader parameter should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Webpack4]);

    let entry = output
        .modules
        .iter()
        .find(|(_, code)| code.contains("/compiled/"))
        .map(|(_, code)| code)
        .expect("expected webpack4 module that reuses the loader");
    assert!(
        entry.contains("import "),
        "expected recovered import:\n{entry}"
    );
    assert!(
        !entry.contains("require"),
        "the reused factory parameter must not become a free require binding:\n{entry}"
    );
    assert!(
        entry.contains("const _load = /compiled/;") && entry.contains("_load.test"),
        "the post-load value must be declared and read through the local:\n{entry}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack4_reused_exports_parameter_preserves_the_runtime_export_lifetime() {
    let source = r#"
!function(modules) {
  function load(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  return load(0);
}([
  function(module, publicValue, load) {
    publicValue.ready = true;
    publicValue = load(1);
    consume(publicValue.value);
  },
  function(module) {
    module.exports = { value: "dependency" };
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-reused-exports.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 exports parameter reuse should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack4]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(_, code)| code.contains("consume"))
        .map(|(_, code)| code)
        .expect("expected recovered exports-reuse module");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert!(
        !entry.contains("exports ="),
        "the parameter's second lifetime must be a declared local:\n{entry}"
    );
    assert!(entry.contains("import _publicValue from"), "{entry}");
    assert!(entry.contains("consume(_publicValue.value)"), "{entry}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_exports_and_loader_parameters_split_in_evaluation_order() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, publicValue, load) => {
      publicValue.ready = true;
      const read = (
        publicValue = load(1),
        load = load(2),
        () => [publicValue.value, load.value]
      );
      module.exports = read;
    }),
    1: ((module) => {
      module.exports = { value: "dependency" };
    }),
    2: ((module) => {
      module.exports = { value: "runtime" };
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-interleaved-runtime-parameters.js".to_string(),
            ..Default::default()
        },
    )
    .expect("interleaved exports and loader lifetimes should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered interleaved module");
    let dependency = entry.find("./module-1.js").expect("first dependency");
    let runtime = entry.find("./module-2.js").expect("second dependency");
    assert!(dependency < runtime, "dependency order changed:\n{entry}");
    assert!(!entry.contains("exports ="), "{entry}");
    assert!(entry.contains("import _publicValue from"), "{entry}");
    assert!(entry.contains("import _load from"), "{entry}");
    assert!(entry.contains("_publicValue.value"), "{entry}");
    assert!(entry.contains("_load.value"), "{entry}");
    assert!(entry.contains("export default read"), "{entry}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);

    let mapped = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-interleaved-runtime-parameters.js".to_string(),
            emit_source_map: true,
            ..Default::default()
        },
    )
    .expect("source-map materialization should preserve both localized lifetimes");
    let mapped_entry = mapped
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected mapped interleaved module");
    assert!(
        mapped_entry.contains("_publicValue.value"),
        "{mapped_entry}"
    );
    assert!(mapped_entry.contains("_load.value"), "{mapped_entry}");
    assert!(mapped
        .source_maps
        .iter()
        .any(|(filename, _)| filename == "module-0.js"));
    assert_eq!(validate_output_modules(&mapped.modules), vec![]);
}

#[test]
fn webpack5_reused_module_parameter_preserves_the_runtime_module_lifetime() {
    let source = r#"
(() => {
  var modules = ({
    0: ((context, exports, load) => {
      context.exports.ready = true;
      context = load(1);
      consume(context.value);
    }),
    1: ((module) => {
      module.exports = { value: "dependency" };
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-reused-module.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 module parameter reuse should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered module-reuse module");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert!(
        !entry.contains("module ="),
        "the parameter's second lifetime must be a declared local:\n{entry}"
    );
    assert!(entry.contains("import _context from"), "{entry}");
    assert!(entry.contains("consume(_context.value)"), "{entry}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_exports_in_for_in_rhs_preserves_the_consumed_value() {
    let source = r#"
(() => {
  var modules = ({
    0: ((context, publicValue, load) => {
      var additions = load(1);
      function api(value) { return value; }
      for (var key in (((publicValue = context.exports = api).own = true), additions)) {
        publicValue[key] = additions[key];
      }
      consume(publicValue);
    }),
    1: ((module) => {
      module.exports = { extra: "value" };
    }),
    2: ((module, exports, load) => {
      var own = load(0).own;
      module.exports = own;
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(2);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-for-in-exports-alias.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a consumed exports alias reset in a for-in RHS should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered for-in alias module");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert!(entry.contains("export default _publicValue"), "{entry}");
    assert!(entry.contains("_publicValue.own = true"), "{entry}");
    assert!(entry.contains("_publicValue[key]"), "{entry}");
    assert!(!entry.contains("exports ="), "{entry}");
    assert!(entry.contains("consume(_publicValue)"), "{entry}");
    let consumer = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-2.js")
        .map(|(_, code)| code)
        .expect("expected consumer of the attached callable property");
    assert!(!consumer.contains("import { own"), "{consumer}");
    assert!(consumer.contains(".own"), "{consumer}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_hoisted_function_capture_keeps_runtime_parameter_reuse_opaque() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      observeLoader();
      load = load(1);
      function observeLoader() {
        consume(load(2));
      }
      module.exports = load;
    }),
    1: ((module) => {
      module.exports = "runtime";
    }),
    2: ((module) => {
      module.exports = "dependency";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-hoisted-runtime-capture.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a hoisted capture should isolate only its factory");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().any(|warning| {
        warning.filename == "module-0.js"
            && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let opaque = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected opaque hoisted-capture factory");
    assert!(opaque.contains("function observeLoader"), "{opaque}");
    assert!(opaque.contains("load(2)"), "{opaque}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack4_unprovable_exports_reuse_isolates_only_its_factory() {
    let source = r#"
!function(modules) {
  function load(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  return load(2);
}([
  function(module, publicValue) {
    if (globalThis.replaceExports) publicValue = globalThis.replacement;
    module.exports = publicValue;
  },
  function(module) {
    module.exports = "stable";
  },
  function(module, exports, load) {
    module.exports = [load(0), load(1)];
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-conditional-exports-reuse.js".to_string(),
            ..Default::default()
        },
    )
    .expect("an unprovable exports lifetime should isolate only its factory");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack4]);
    let failures = output
        .warnings
        .iter()
        .filter(|warning| {
            warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
        })
        .collect::<Vec<_>>();
    assert_eq!(
        failures.len(),
        1,
        "unexpected warnings: {:?}",
        output.warnings
    );
    assert_eq!(failures[0].filename, "module-0.js");
    assert!(
        failures[0].message.contains("runtime-parameter reuse"),
        "unexpected diagnostic: {}",
        failures[0].message
    );
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-2.js")
        .map(|(_, code)| code)
        .expect("expected recoverable sibling factory");
    assert!(entry.contains("require(0)"), "{entry}");
    assert!(!entry.contains("./module-0.js"), "{entry}");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_exports_reuse_that_reads_the_old_value_stays_opaque() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, publicValue) => {
      publicValue = chooseValue(publicValue, globalThis.replacement);
      module.exports = publicValue;
    }),
    1: ((module) => {
      module.exports = "stable";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(1);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-read-before-exports-reuse.js".to_string(),
            ..Default::default()
        },
    )
    .expect("value-flow reuse should isolate only its factory");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().any(|warning| {
        warning.filename == "module-0.js"
            && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let opaque = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected opaque value-flow factory");
    assert!(opaque.contains("chooseValue(publicValue"), "{opaque}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_post_loader_write_module_decorator_shape_stays_opaque() {
    let source = r#"
(() => {
  var modules = ({
    0: ((context, exports, load) => {
      load = load(1);
      context = load.hmd(context);
      context.exports = "local";
    }),
    1: ((module) => {
      module.exports = { hmd: value => value };
    }),
    2: ((module) => {
      module.exports = "stable";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(2);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-post-write-decorator-lookalike.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a second-lifetime decorator lookalike should isolate its factory");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().any(|warning| {
        warning.filename == "module-0.js"
            && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let opaque = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected opaque decorator-lookalike factory");
    assert!(opaque.contains("load.hmd(context)"), "{opaque}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack4_opaque_loader_reuse_preserves_other_structural_modules() {
    let source = r#"
!function(modules) {
  function load(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  return load(2);
}([
  function(module, exports, load) {
    if (globalThis.useAlternate) load = globalThis.alternateLoader;
    module.exports = load;
  },
  function(module) {
    module.exports = "stable";
  },
  function(module, exports, load) {
    module.exports = [load(0), load(1)];
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-mixed-loader-reuse.js".to_string(),
            ..Default::default()
        },
    )
    .expect("one unsupported webpack 4 factory must not discard its container");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack4]);
    assert_eq!(output.modules.len(), 3);
    assert!(output.warnings.iter().any(|warning| {
        warning.filename == "module-0.js"
            && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(_, code)| code.contains("require(0)"))
        .map(|(_, code)| code)
        .expect("consumer should remain structurally recovered");
    assert!(entry.contains("require(0)"), "{entry}");
    assert!(!entry.contains("./module-0.js"), "{entry}");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_loader_assignment_can_initialize_from_a_module() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      const get = (load = load(1)).get;
      module.exports = get();
    }),
    1: ((module) => {
      module.exports = { get: () => "ok" };
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-reused-loader.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 reused loader parameter should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);

    let entry = output
        .modules
        .iter()
        .find(|(_, code)| code.contains(".get"))
        .map(|(_, code)| code)
        .expect("expected webpack5 module that reuses the loader");
    assert!(
        entry.contains("import "),
        "expected recovered import:\n{entry}"
    );
    assert!(
        !entry.contains("require"),
        "the nested loader assignment must not retain a free require:\n{entry}"
    );
    assert!(
        entry.contains("import _load from") && entry.contains("const get = _load.get;"),
        "the assigned module value must have a declared owner:\n{entry}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_loader_function_keeps_followup_property_initialization() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      const dependency = load(1);
      (load = function(value) { return value; }).normalize = value => String(value);
      module.exports = [dependency, load("ok"), load.normalize(2)];
    }),
    1: ((module) => {
      module.exports = "dependency";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-reused-loader-function.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 function reuse should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);

    let entry = output
        .modules
        .iter()
        .find(|(_, code)| code.contains("normalize"))
        .map(|(_, code)| code)
        .expect("expected module with the recovered function local");
    assert!(
        entry.contains("import "),
        "expected recovered import:\n{entry}"
    );
    assert!(
        !entry.contains("require"),
        "the function-valued local must not retain a free require:\n{entry}"
    );
    assert!(
        entry.contains("const _load =") && entry.contains("_load.normalize ="),
        "the property writer must target a declared recovered local:\n{entry}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_loader_normalizes_only_the_loader_lifetime() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      "use strict";
      load.r(exports);
      const before = load(1);
      const eager = load.bind(load, 1);
      const getter = load.n(before);
      const namespaceValue = load.t(before, 2).value;
      load = load(2);
      load.r(exports);
      const direct = load(1);
      const lateEager = load.bind(load, 1);
      const lateGetter = load.n(before);
      const lateNamespace = load.t(before, 2);
      const runtimeGlobal = load.g;
      module.exports = [
        before,
        eager,
        getter,
        namespaceValue,
        direct,
        lateEager,
        lateGetter,
        lateNamespace,
        runtimeGlobal,
        load
      ];
    }),
    1: ((module) => {
      module.exports = "dependency";
    }),
    2: ((module) => {
      module.exports = function runtimeValue(value) { return value; };
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-loader-lifetimes.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a proven loader/local lifetime boundary should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(
        output.warnings.iter().all(|warning| {
            warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
        }),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered loader-reuse module");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert!(entry.contains("./module-2.js"), "{entry}");
    assert!(
        entry.contains("_load.r(exports)")
            && entry.contains("_load(1)")
            && entry.contains("_load.bind(_load, 1)")
            && entry.contains("_load.n")
            && entry.contains("_load.t")
            && entry.contains("_load.g"),
        "post-write calls and helpers belong to the localized value:\n{entry}"
    );
    assert!(
        !entry.contains("require.r")
            && !entry.contains("require.t")
            && entry.matches("_load.r(exports)").count() == 1,
        "pre-write webpack helpers should be consumed:\n{entry}"
    );
    // The arbitrary post-write runtime value still receives the factory's
    // original `exports` object. That object has no proven ESM replacement;
    // keep the residual visible rather than pretending the graph is clean.
    let findings = validate_output_modules(&output.modules);
    assert_eq!(findings.len(), 1, "unexpected findings: {findings:#?}");
    assert_eq!(
        findings[0].kind,
        wakaru_core::OutputFindingKind::EsmCommonJsResidual
    );
    assert_eq!(findings[0].filename, "module-0.js");
}

#[test]
fn webpack4_reused_loader_normalizes_prewrite_runtime_helpers() {
    let source = r#"
!function(modules) {
  function load(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  return load(0);
}([
  function(module, exports, load) {
    "use strict";
    load.r(exports);
    load.d(exports, "value", function() { return value; });
    var value = load(1);
    load = load(2);
    load.r(exports);
    var direct = load(1);
    module.exports = [value, direct, load];
  },
  function(module) {
    module.exports = "dependency";
  },
  function(module) {
    module.exports = function runtimeValue(value) { return value; };
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-loader-helpers.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack 4 runtime helpers before a reuse boundary should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack4]);
    assert!(
        output.warnings.iter().all(|warning| {
            warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
        }),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let entry = output
        .modules
        .iter()
        .find(|(_, code)| code.contains("runtimeValue") || code.contains("_load"))
        .map(|(_, code)| code)
        .expect("expected recovered webpack 4 loader-reuse module");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert!(entry.contains("./module-2.js"), "{entry}");
    assert!(entry.contains("_load.r(exports)"), "{entry}");
    assert!(entry.contains("_load(1)"), "{entry}");
    assert!(
        !entry.contains("require.r")
            && !entry.contains("require.d")
            && entry.matches("_load.r(exports)").count() == 1,
        "{entry}"
    );
    // The arbitrary post-write runtime value still receives the factory's
    // original `exports` object. That object has no proven ESM replacement;
    // keep the residual visible rather than pretending the graph is clean.
    let findings = validate_output_modules(&output.modules);
    assert_eq!(findings.len(), 1, "unexpected findings: {findings:#?}");
    assert_eq!(
        findings[0].kind,
        wakaru_core::OutputFindingKind::EsmCommonJsResidual
    );
    assert_eq!(findings[0].filename, "module-0.js");
}

#[test]
fn webpack5_reused_loader_var_redeclaration_starts_the_local_lifetime() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      var load = load(1);
      module.exports = load;
    }),
    1: ((module) => {
      module.exports = "localized";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-loader-redeclaration.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a var redeclaration should identify the loader lifetime boundary");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered redeclaration module");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert!(!entry.contains("require(1)"), "{entry}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_named_loader_normalizes_only_prewrite_ids() {
    let source = r#"
(() => {
  var modules = ({
    "./entry.js": ((module, exports, load) => {
      const before = load("./before.js");
      load = load("./runtime.js");
      const after = load("./before.js");
      module.exports = [before, after, load];
    }),
    "./before.js": ((module) => {
      module.exports = "before";
    }),
    "./runtime.js": ((module) => {
      module.exports = function runtimeValue(value) { return value; };
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })("./entry.js");
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-named-loader-lifetimes.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a proven named-ID loader boundary should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "entry.js")
        .map(|(_, code)| code)
        .expect("expected recovered named-ID entry");
    assert!(entry.contains("./before.js"), "{entry}");
    assert!(entry.contains("./runtime.js"), "{entry}");
    assert!(
        entry.contains("_load(\"./before.js\")"),
        "the post-write string call must retain the localized runtime:\n{entry}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_recovers_proven_commonjs_reads_without_runtime_residuals() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      load(1);
      load(2);
      module.exports = "entry";
    }),
    1: ((module) => {
      const api = () => "ready";
      module.exports = api;
      if (typeof window !== "undefined") {
        window.syntheticApi = module.exports;
      }
    }),
    2: ((module, exports) => {
      exports.second = exports.first = void 0;
      exports.first = function(value) { return value + 1; };
      exports.second = function(value) { return exports.first(value); };
      consume(exports.second(1));
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-commonjs-read-recovery.js".to_string(),
            ..Default::default()
        },
    )
    .expect("the synthetic webpack container should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    let default_module = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-1.js")
        .map(|(_, code)| code)
        .expect("expected the default-exporting module");
    assert!(
        default_module.contains("window.syntheticApi = api"),
        "{default_module}"
    );
    let named_module = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-2.js")
        .map(|(_, code)| code)
        .expect("expected the named-exporting module");
    assert!(named_module.contains("first(value)"), "{named_module}");
    assert!(
        named_module.contains("consume(second(1))"),
        "{named_module}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);

    let mapped = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-commonjs-read-recovery.js".to_string(),
            emit_source_map: true,
            ..Default::default()
        },
    )
    .expect("source-map materialization should run the same recovery");
    assert_eq!(validate_output_modules(&mapped.modules), vec![]);
    assert!(mapped
        .source_maps
        .iter()
        .any(|(filename, _)| filename == "module-1.js"));
    assert!(mapped
        .source_maps
        .iter()
        .any(|(filename, _)| filename == "module-2.js"));
}

#[test]
fn webpack5_observable_self_require_stays_at_the_commonjs_boundary() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      load(1);
    }),
    1: ((module, exports, load) => {
      var self = load(1);
      observe(exports === self);
      for (var key in self) globalThis[key] = self[key];
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    for emit_source_map in [false, true] {
        let output = unpack(
            source,
            DecompileOptions {
                filename: "webpack5-self-require.js".to_string(),
                emit_source_map,
                ..Default::default()
            },
        )
        .expect("the synthetic webpack self-require should unpack");

        assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
        let module = output
            .modules
            .iter()
            .find(|(filename, _)| filename == "module-1.js")
            .map(|(_, code)| code)
            .expect("expected the self-requiring factory module");
        assert!(
            module.contains(r#"require("./module-1.js")"#)
                && !module.contains(r#"import self from "./module-1.js""#),
            "an observed factory exports alias must keep the self-require at the CommonJS boundary:\n{module}"
        );
        let findings = validate_output_modules(&output.modules);
        assert!(
            findings.iter().all(|finding| {
                finding.kind != wakaru_core::OutputFindingKind::MissingImportedName
            }),
            "CommonJS-boundary preservation must avoid a false default-export contract: {findings:#?}"
        );
    }
}

#[test]
fn webpack5_self_required_empty_default_remains_linkable_to_consumers() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      function asDefault(value) {
        return value && value.__esModule ? value : { default: value };
      }
      var dependency = asDefault(load(1));
      consume(dependency.default.open);
    }),
    1: ((module, exports, load) => {
      var current = load(1);
      for (var key in current) globalThis[key] = current[key];
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let raw = unpack_raw(
        source,
        &DecompileOptions {
            filename: "webpack5-self-default-consumer.js".to_string(),
            ..Default::default()
        },
    )
    .expect("raw extraction should preserve the detector body");
    let raw_provider = raw
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-1.js")
        .map(|(_, code)| code)
        .expect("expected raw self-required provider");
    assert!(
        raw_provider.contains(r#"require("./module-1.js")"#)
            && !raw_provider.contains("module.exports = {};"),
        "runtime-default restoration must stay out of raw output:\n{raw_provider}"
    );

    for emit_source_map in [false, true] {
        let output = unpack(
            source,
            DecompileOptions {
                filename: "webpack5-self-default-consumer.js".to_string(),
                emit_source_map,
                diagnostics: true,
                ..Default::default()
            },
        )
        .expect("the synthetic webpack self-require consumer should unpack");

        assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
        assert!(
            output
                .warnings
                .iter()
                .all(|warning| warning.kind != wakaru_core::UnpackWarningKind::TdzViolation),
            "the restored self default must initialize before its first read: {:#?}",
            output.warnings
        );
        let provider = output
            .modules
            .iter()
            .find(|(filename, _)| filename == "module-1.js")
            .map(|(_, code)| code)
            .expect("expected self-required provider");
        let default = provider
            .find("export default")
            .expect("webpack's runtime-created object should become a default export");
        let first_read = provider
            .find("for(")
            .expect("expected the self-required object read");
        assert!(
            default < first_read,
            "the self default must initialize before the loop reads it:\n{provider}"
        );
        let findings = validate_output_modules(&output.modules);
        assert!(
            findings.iter().all(|finding| {
                finding.kind != wakaru_core::OutputFindingKind::MissingImportedName
            }),
            "a consumer must not default-import an unexported self-required value: {findings:#?}"
        );
    }
}

#[test]
fn webpack5_recovers_coupled_lazy_default_helpers_across_phase1_paths() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      const helper = load(1);
      consume(helper("value"), helper.default);
      module.exports = "entry";
    }),
    1: ((module) => {
      function helper(value) {
        module.exports = helper = (next) => typeof next;
        module.exports.default = module.exports;
        return helper(value);
      }
      module.exports = helper;
      module.exports.default = module.exports;
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    for emit_source_map in [false, true] {
        let output = unpack(
            source,
            DecompileOptions {
                filename: "webpack5-coupled-lazy-default.js".to_string(),
                emit_source_map,
                ..Default::default()
            },
        )
        .expect("the coupled lazy helper should unpack");

        assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
        let helper = output
            .modules
            .iter()
            .find(|(filename, _)| filename == "module-1.js")
            .map(|(_, code)| code)
            .expect("expected the helper module");
        assert!(
            helper.contains("export { helper as default }"),
            "the mutable helper needs a live default binding:\n{helper}"
        );
        assert_eq!(
            helper.matches("helper.default = helper").count(),
            2,
            "both CommonJS property mirrors must remain:\n{helper}"
        );
        assert!(!helper.contains("module.exports"), "{helper}");
        assert_eq!(validate_output_modules(&output.modules), vec![]);
        if emit_source_map {
            assert!(output
                .source_maps
                .iter()
                .any(|(filename, _)| filename == "module-1.js"));
        }
    }
}

#[test]
fn webpack5_recovers_default_only_compat_adapter_without_runtime_residuals() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      module.exports = load(1);
    }),
    1: ((module, exports, load) => {
      var typeHelpers = load(2);
      Object.defineProperty(exports, "__esModule", {
        value: true
      });
      Object.defineProperty(exports, "default", {
        enumerable: true,
        get: function() {
          return entry;
        }
      });
      function entry(value) {
        return value;
      }
      ("function" == typeof exports.default || "object" === typeHelpers._(exports.default) && null !== exports.default) && void 0 === exports.default.__esModule && (Object.defineProperty(exports.default, "__esModule", {
        value: true
      }), Object.assign(exports.default, exports), module.exports = exports.default);
    }),
    2: ((module, exports) => {
      function typeOf(value) {
        return typeof value;
      }
      exports._ = typeOf;
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    for emit_source_map in [false, true] {
        let output = unpack(
            source,
            DecompileOptions {
                filename: "webpack5-default-only-compat-adapter.js".to_string(),
                emit_source_map,
                ..Default::default()
            },
        )
        .expect("the exact default-only adapter should unpack");

        assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
        let provider = output
            .modules
            .iter()
            .find(|(filename, _)| filename == "module-1.js")
            .map(|(_, code)| code)
            .expect("expected the default-only provider");
        assert!(
            provider.contains("export { entry as default }"),
            "{provider}"
        );
        assert!(provider.contains("(entry) === \"object\""), "{provider}");
        assert!(provider.contains("entry.default = entry"), "{provider}");
        assert!(!provider.contains("exports.default"), "{provider}");
        assert!(!provider.contains("module.exports"), "{provider}");
        assert_eq!(validate_output_modules(&output.modules), vec![]);
        if emit_source_map {
            assert!(
                output
                    .source_maps
                    .iter()
                    .any(|(filename, _)| filename == "module-1.js"),
                "the recovered provider should retain its source map"
            );
        }
    }
}

#[test]
fn webpack5_reused_loader_splits_a_mid_initializer_sequence() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      var absent = load(137), marker,
        build = (marker = load(1), load = load(2), function() { return load; });
      module.exports = [absent, marker, build(), load];
    }),
    1: ((module) => {
      module.exports = "dependency";
    }),
    2: ((module) => {
      module.exports = "localized";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-loader-initializer-sequence.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a mid-initializer loader boundary should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(
        output.warnings.iter().all(|warning| {
            warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
        }),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered initializer module");
    assert!(
        entry.contains("require(137)"),
        "an absent numeric id must remain an honest runtime call:\n{entry}"
    );
    assert!(
        !entry.contains("module-137") && !entry.contains("from \"137\""),
        "an absent numeric id must not synthesize an ESM edge:\n{entry}"
    );
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert!(entry.contains("./module-2.js"), "{entry}");
    assert!(
        entry.matches("_load").count() >= 3,
        "the suffix closure and later reads must use the localized value:\n{entry}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_loader_splits_a_top_level_sequence_in_order() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      observe("before"), load = alternateRuntime, observe("after", load);
      module.exports = load;
    }),
    1: ((module) => {
      module.exports = "stable";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-loader-top-level-sequence.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a top-level sequence boundary should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered sequence module");
    let before = entry.find("observe(\"before\")").expect("prefix effect");
    let local = entry.find("alternateRuntime").expect("localized write");
    let after = entry.find("observe(\"after\"").expect("suffix effect");
    assert!(
        before < local && local < after,
        "evaluation order changed:\n{entry}"
    );
    assert!(entry.contains("observe(\"after\", _load)"), "{entry}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_loader_keeps_consumed_mid_sequence_opaque() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      const value = (observe("before"), load = load(1)).value;
      module.exports = value;
    }),
    1: ((module) => {
      module.exports = { value: "stable" };
    }),
    2: ((module) => {
      module.exports = "independent";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(2);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-consumed-loader-sequence.js".to_string(),
            ..Default::default()
        },
    )
    .expect("an unsupported factory should stay isolated");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().any(|warning| {
        warning.filename == "module-0.js"
            && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let opaque = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected opaque factory");
    assert!(opaque.contains("observe(\"before\")"), "{opaque}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_loader_recovers_conditional_runtime_global_reads() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      load = typeof load.g === "object" && load.g && load.g.Object === Object && load.g;
      module.exports = load;
    }),
    1: ((module) => {
      module.exports = "independent";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-conditional-runtime-global.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack runtime-global reads should identify the old loader lifetime");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered runtime-global module");
    assert!(
        !entry.contains("require.g") && !entry.contains("load.g"),
        "{entry}"
    );
    assert!(entry.contains("global"), "{entry}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_loader_keeps_bare_old_lifetime_values_opaque() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      load = typeof factory === "function"
        ? factory.call(exports, load, exports, module)
        : factory;
      module.exports = load;
    }),
    1: ((module) => {
      module.exports = "independent";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(1);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-bare-loader-lifetime.js".to_string(),
            ..Default::default()
        },
    )
    .expect("an unsupported factory should stay isolated");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().any(|warning| {
        warning.filename == "module-0.js"
            && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let opaque = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected opaque factory");
    assert!(opaque.contains("factory.call(exports, load"), "{opaque}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_opaque_loader_reuse_preserves_other_structural_modules() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      const opaque = load(1);
      const stable = load(2);
      module.exports = [opaque, stable];
    }),
    1: ((module, exports, load) => {
      if (globalThis.useAlternate) load = globalThis.alternateLoader;
      module.exports = load;
    }),
    2: ((module) => {
      module.exports = "stable";
    })
  });
  var cache = {};
  function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  load(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-mixed-loader-reuse.js".to_string(),
            ..Default::default()
        },
    )
    .expect("one unsupported factory must not discard its container");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert_eq!(output.modules.len(), 4);
    let failures = output
        .warnings
        .iter()
        .filter(|warning| {
            warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
        })
        .collect::<Vec<_>>();
    assert_eq!(
        failures.len(),
        1,
        "unexpected warnings: {:?}",
        output.warnings
    );
    assert_eq!(failures[0].filename, "module-1.js");

    let opaque = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-1.js")
        .map(|(_, code)| code)
        .expect("opaque factory should be preserved");
    assert!(opaque.contains("if (globalThis.useAlternate)"), "{opaque}");

    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("recoverable entry should be emitted");
    assert!(
        entry.contains("require(1)"),
        "an opaque target must follow the absent-id convention:\n{entry}"
    );
    assert!(
        !entry.contains("./module-1.js"),
        "an opaque target must not gain a synthetic graph edge:\n{entry}"
    );
    assert!(
        entry.contains("./module-2.js"),
        "the independent structural edge should still recover:\n{entry}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);

    let mapped = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-mixed-loader-reuse.js".to_string(),
            emit_source_map: true,
            ..Default::default()
        },
    )
    .expect("source-map materialization must preserve the opaque sidecar");
    assert!(mapped.warnings.iter().any(|warning| {
        warning.filename == "module-1.js"
            && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    assert!(mapped
        .source_maps
        .iter()
        .all(|(filename, _)| filename != "module-1.js"));
    assert!(mapped
        .source_maps
        .iter()
        .any(|(filename, _)| filename == "module-2.js"));
}

#[test]
fn webpack_named_id_loader_reuse_keeps_whole_container_fallback() {
    let webpack4 = r#"
!function(modules) {
  function load(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  return load("./entry.js");
}({
  "./opaque.js": function(module, exports, load) {
    if (globalThis.useAlternate) load = globalThis.alternateLoader;
    module.exports = load;
  },
  "./entry.js": function(module, exports, load) {
    module.exports = load("./opaque.js");
  }
});
"#;
    let webpack5 = r#"
(() => {
  var modules = ({
    "./opaque.js": ((module, exports, load) => {
      if (globalThis.useAlternate) load = globalThis.alternateLoader;
      module.exports = load;
    }),
    "./entry.js": ((module, exports, load) => {
      module.exports = load("./opaque.js");
    })
  });
  var cache = {};
  function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  load("./entry.js");
})();
"#;

    for (filename, source) in [
        ("webpack4-named-loader-reuse.js", webpack4),
        ("webpack5-named-loader-reuse.js", webpack5),
    ] {
        let output = unpack_raw(
            source,
            &DecompileOptions {
                filename: filename.to_string(),
                ..Default::default()
            },
        )
        .expect("path-like absent IDs must keep the historical input fallback");
        assert!(
            output.detected_formats.is_empty(),
            "{filename} must not synthesize a path-like edge"
        );
        assert_eq!(output.modules.len(), 1, "{filename}");
        assert!(output.modules[0].1.contains("useAlternate"), "{filename}");
    }
}

#[test]
fn webpack5_opaque_loader_reuse_retains_entry_provenance() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      if (globalThis.useAlternate) load = globalThis.alternateLoader;
      module.exports = load;
    }),
    1: ((module) => {
      module.exports = "stable";
    })
  });
  var cache = {};
  function __nccwpck_require__(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, __nccwpck_require__);
    return module.exports;
  }
  module.exports = __nccwpck_require__(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-opaque-entry.js".to_string(),
            ..Default::default()
        },
    )
    .expect("an opaque entry should coexist with recoverable siblings");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output
        .provenance
        .iter()
        .any(|provenance| { provenance.filename == "module-0.js" && provenance.is_entry }));
    assert!(output.warnings.iter().any(|warning| {
        warning.filename == "module-0.js"
            && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_loader_reuse_demotion_reaches_an_order_independent_fixed_point() {
    fn run(module_table: &str) -> Vec<String> {
        let source = format!(
            r#"
(() => {{
  var modules = ({{ {module_table} }});
  var cache = {{}};
  (function load(id) {{
    var module = cache[id] = {{ exports: {{}} }};
    modules[id](module, module.exports, load);
    return module.exports;
  }})(2);
}})();
"#
        );
        let output = unpack(
            &source,
            DecompileOptions {
                filename: "webpack5-loader-fixed-point.js".to_string(),
                ..Default::default()
            },
        )
        .expect("the stable factory should keep the container recoverable");
        assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
        assert_eq!(validate_output_modules(&output.modules), vec![]);
        let mut failed = output
            .warnings
            .iter()
            .filter(|warning| {
                warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
            })
            .map(|warning| warning.filename.clone())
            .collect::<Vec<_>>();
        failed.sort();
        failed
    }

    let dependent = r#"0: ((module, exports, load) => {
      load = load(1);
      module.exports = load;
    })"#;
    let inherently_opaque = r#"1: ((module, exports, load) => {
      if (globalThis.useAlternate) load = globalThis.alternateLoader;
      module.exports = load;
    })"#;
    let stable = r#"2: ((module) => { module.exports = "stable"; })"#;

    let forward = run(&format!("{dependent}, {inherently_opaque}, {stable}"));
    let reverse = run(&format!("{stable}, {inherently_opaque}, {dependent}"));
    assert_eq!(
        forward,
        vec!["module-1.js"],
        "an absent numeric target stays an honest runtime call instead of demoting its caller"
    );
    assert_eq!(reverse, forward);
}

#[test]
fn webpack5_non_runtime_parameter_normalization_failure_still_rejects_the_container() {
    let source = r#"
(() => {
  var modules = ({
    0: ((m, e, r) => {
      globalThis.originalRequire = require;
      m.exports = r(1);
    }),
    1: ((module) => { module.exports = "stable"; })
  });
  var cache = {};
  function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  load(0);
})();
"#;

    let output = unpack_raw(
        source,
        &DecompileOptions {
            filename: "webpack5-fatal-normalization.js".to_string(),
            ..Default::default()
        },
    )
    .expect("fatal detector normalization should retain the input fallback");
    assert!(output.detected_formats.is_empty());
    assert_eq!(output.modules.len(), 1);
    assert!(output.modules[0].1.contains("originalRequire"));
}

#[test]
fn webpack5_reused_loader_with_conditional_first_write_fails_closed() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      if (useAlternate) load = alternateLoader;
      module.exports = load;
    })
  });
  var cache = {};
  function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  load(0);
})();
"#;

    let output = unpack_raw(
        source,
        &DecompileOptions {
            filename: "webpack5-conditional-loader-reuse.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unsupported loader reuse should preserve the input");
    assert!(
        output.detected_formats.is_empty(),
        "an unprovable conditional boundary must reject structural extraction"
    );
    assert_eq!(output.modules.len(), 1, "expected one fallback module");
    assert!(
        output.modules[0].1.contains("if (useAlternate)"),
        "fallback output must preserve the conditional write"
    );
}

#[test]
fn webpack5_reused_loader_initializer_that_reads_old_loader_fails_closed() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      load = (() => load.runtime)();
      module.exports = load;
    })
  });
  var cache = {};
  function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  load(0);
})();
"#;

    let output = unpack_raw(
        source,
        &DecompileOptions {
            filename: "webpack5-read-before-loader-reuse.js".to_string(),
            ..Default::default()
        },
    )
    .expect("read-before-write loader reuse should preserve the input");
    assert!(
        output.detected_formats.is_empty(),
        "an initializer that executes a read of the old loader must reject extraction"
    );
    assert_eq!(output.modules.len(), 1, "expected one fallback module");
    assert!(
        output.modules[0].1.contains("load.runtime"),
        "fallback output must preserve the old loader read"
    );
}

#[test]
fn webpack5_extensionless_string_id_gets_a_resolvable_javascript_filename() {
    let source = r#"
(() => {
  var __webpack_modules__ = ({
    "pkg/side-effect": ((module) => {
      module.exports = "loaded";
    }),
    "./src/index.js": ((module, exports, __webpack_require__) => {
      __webpack_require__("pkg/side-effect");
      module.exports = "entry";
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
            filename: "webpack5-path-id.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 path-like module id should unpack");
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
        index.contains(r#""../pkg/side-effect.js""#),
        "the path-like module id must be relative to its consumer:\n{index}"
    );
    assert!(
        output
            .modules
            .iter()
            .any(|(name, _)| name == "pkg/side-effect.js"),
        "expected the extensionless module id to append .js"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_string_id_collisions_and_queries_keep_distinct_edges() {
    let source = r#"
(() => {
  var __webpack_modules__ = ({
    "./widgets/Button.less": ((module) => {
      module.exports = "compiled style";
    }),
    "./widgets/Button.less.js": ((module) => {
      module.exports = "authored JavaScript";
    }),
    "./App.vue?vue&type=script": ((module) => {
      module.exports = "script block";
    }),
    "./App.vue?vue&type=style&index=0": ((module) => {
      module.exports = "style block";
    }),
    "./src/index.js": ((module, exports, __webpack_require__) => {
      const style = __webpack_require__("./widgets/Button.less");
      const authored = __webpack_require__("./widgets/Button.less.js");
      const script = __webpack_require__("./App.vue?vue&type=script");
      const vueStyle = __webpack_require__("./App.vue?vue&type=style&index=0");
      module.exports = [style, authored, script, vueStyle];
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
            filename: "webpack5-string-id-collisions.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 colliding and queried module ids should unpack");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let names = output
        .modules
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"widgets/Button.less.js"), "{names:?}");
    assert!(names.contains(&"widgets/Button.less_2.js"), "{names:?}");
    assert!(names.contains(&"App.vue.js"), "{names:?}");
    assert!(names.contains(&"App.vue_2.js"), "{names:?}");
    assert!(
        names.iter().all(|name| !name.contains(['?', '#'])),
        "{names:?}"
    );

    let index = output
        .modules
        .iter()
        .find(|(name, _)| name == "src/index.js")
        .map(|(_, code)| code)
        .expect("expected index module");
    for specifier in [
        "../widgets/Button.less.js",
        "../widgets/Button.less_2.js",
        "../App.vue.js",
        "../App.vue_2.js",
    ] {
        assert!(
            index.contains(&format!(r#""{specifier}""#)),
            "missing {specifier} in consumer:\n{index}"
        );
    }
    assert_eq!(validate_output_modules(&output.modules), vec![]);
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
        names.contains(&"..../node_modules/@wakaru/cli/bin/wakaru.js"),
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
        entry.contains(r#""./calculator.js""#) && entry.contains(r#""./greeting.js""#),
        "browserify dependency maps should target emitted module filenames:\n{entry}"
    );
    assert!(
        !entry.contains(r#""./calculator""#) && !entry.contains(r#""./greeting""#),
        "original browserify request names should be remapped:\n{entry}"
    );
}

fn composition_bundle(consumer_body: &str, providers: &str) -> String {
    format!(
        r#"
(() => {{
  var __webpack_modules__ = ({{
    0: ((module, exports, __webpack_require__) => {{
{consumer_body}
    }}),
{providers}
  }});
  var __webpack_module_cache__ = {{}};
  function __webpack_require__(moduleId) {{
    if (__webpack_module_cache__[moduleId]) return __webpack_module_cache__[moduleId].exports;
    var module = __webpack_module_cache__[moduleId] = {{ exports: {{}} }};
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }}
  __webpack_require__(0);
}})();
"#
    )
}

fn unpack_composition(source: &str) -> Vec<(String, String)> {
    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-composition-case.js".to_string(),
            ..Default::default()
        },
    )
    .expect("composition bundle should unpack");
    output.modules
}

#[test]
fn webpack5_composition_fails_closed_on_provider_residual_require() {
    // The provider's conditional require never becomes an import fact, so it
    // is a residual dependency edge the plan cannot order around.
    let source = composition_bundle(
        r#"      module.exports = {};
      Object.assign(module.exports, __webpack_require__(1) || {});"#,
        r#"    1: ((module, exports, __webpack_require__) => {
      module.exports = { a: 1 };
      if (globalThis.__DEV__) __webpack_require__(2);
    }),
    2: ((module) => { module.exports = {}; })"#,
    );
    let modules = unpack_composition(&source);
    let consumer = &modules.iter().find(|(n, _)| n == "module-0.js").unwrap().1;
    assert!(
        consumer.contains("Object.assign(module.exports"),
        "consumer must stay an honest residual over a residual provider:\n{consumer}"
    );
}

#[test]
fn webpack5_composition_keeps_duplicate_source_copies_in_order() {
    let source = composition_bundle(
        r#"      module.exports = {};
      Object.assign(module.exports, __webpack_require__(1) || {});
      Object.assign(module.exports, __webpack_require__(1) || {});"#,
        r#"    1: ((module) => { module.exports = { get n() { return ++globalThis.k; } }; })"#,
    );
    let modules = unpack_composition(&source);
    let consumer = &modules.iter().find(|(n, _)| n == "module-0.js").unwrap().1;
    assert_eq!(
        consumer.matches("import ").count(),
        1,
        "duplicate sources share one import:\n{consumer}"
    );
    assert_eq!(
        consumer.matches("Object.assign(_defaultObject").count(),
        2,
        "both copies must survive in order (the getter fires twice):\n{consumer}"
    );
}

#[test]
fn webpack5_composition_fails_closed_on_multi_source_assign() {
    let source = composition_bundle(
        r#"      module.exports = {};
      Object.assign(module.exports, __webpack_require__(1) || {}, __webpack_require__(2) || {});"#,
        r#"    1: ((module) => { module.exports = { a: 1 }; }),
    2: ((module) => { module.exports = { b: 2 }; })"#,
    );
    let modules = unpack_composition(&source);
    let consumer = &modules.iter().find(|(n, _)| n == "module-0.js").unwrap().1;
    assert!(
        consumer.contains("Object.assign(module.exports"),
        "a three-argument copy is not the proven shell:\n{consumer}"
    );
}

#[test]
fn webpack5_composition_fails_closed_on_nonempty_fallback() {
    let source = composition_bundle(
        r#"      module.exports = {};
      Object.assign(module.exports, __webpack_require__(1) || { fallback: 1 });"#,
        r#"    1: ((module) => { module.exports = { a: 1 }; })"#,
    );
    let modules = unpack_composition(&source);
    let consumer = &modules.iter().find(|(n, _)| n == "module-0.js").unwrap().1;
    assert!(
        consumer.contains("Object.assign(module.exports"),
        "a non-empty fallback is not the proven shell:\n{consumer}"
    );
}

#[test]
fn webpack5_composition_fails_closed_on_copy_before_init() {
    let source = composition_bundle(
        r#"      Object.assign(module.exports, __webpack_require__(1) || {});
      module.exports = {};"#,
        r#"    1: ((module) => { module.exports = { a: 1 }; })"#,
    );
    let modules = unpack_composition(&source);
    let consumer = &modules.iter().find(|(n, _)| n == "module-0.js").unwrap().1;
    assert!(
        consumer.contains("Object.assign(module.exports"),
        "the empty-object init must come first:\n{consumer}"
    );
}

#[test]
fn webpack5_composition_fails_closed_on_mid_body_strict_directive() {
    // Only a leading directive is a directive; a later "use strict" string
    // is an extra statement outside the proven shell.
    let source = composition_bundle(
        r#"      module.exports = {};
      "use strict";
      Object.assign(module.exports, __webpack_require__(1) || {});"#,
        r#"    1: ((module) => { module.exports = { a: 1 }; })"#,
    );
    let modules = unpack_composition(&source);
    let consumer = &modules.iter().find(|(n, _)| n == "module-0.js").unwrap().1;
    assert!(
        consumer.contains("Object.assign(module.exports"),
        "a mid-body directive-lookalike is an extra statement:\n{consumer}"
    );
}

#[test]
fn webpack5_does_not_misdetect_method_object_without_require_lifecycle() {
    // Regression: esbuild browser bundles contain vendor objects whose props
    // are all functions (e.g. rxjs's Immediate polyfill). Once the outer IIFE
    // is unwrapped, that body becomes a detection candidate — a real webpack5
    // modules object must be backed by the proven require lifecycle
    // (cache write + indexed invocation + returned exports); named-only
    // property access must not count as webpack5 evidence.
    let source = r#"
(() => {
    var handles = {};
    var Immediate = {
        setImmediate(cb) { handles[1] = cb; return 1; },
        clearImmediate(handle) { delete handles[handle]; }
    };
    var id = Immediate.setImmediate(() => console.log("tick"));
    Immediate.clearImmediate(id);
    console.log("done");
})();
"#;
    let output = unpack(source, DecompileOptions::default()).expect("unpack should succeed");
    let names: Vec<_> = output.modules.iter().map(|(n, _)| n.clone()).collect();
    assert!(
        !names.iter().any(|n| n.starts_with("module-")),
        "method-only object misdetected as a webpack5 modules object: {names:?}"
    );
}
