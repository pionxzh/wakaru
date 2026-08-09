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
