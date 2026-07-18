mod common;

use common::normalize;
use wakaru_core::{unpack, unpack_raw, unpack_webpack4, unpack_webpack4_raw, DecompileOptions};

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
fn runtime_getter_exports_stay_as_getters_in_raw_output() {
    let source = r#"
!function(modules) {
  function __webpack_require__(id) {}
  __webpack_require__.s = 0;
  __webpack_require__(0);
}([
  function(module, exports, require) {
    require.r(exports);
    require.d(exports, "$G", function() { return V; });
    function V(value) {
      return value;
    }
  }
]);
"#;

    let code = raw_module(source, "entry.js");
    assert!(
        code.contains("require.r(exports);"),
        "raw output should preserve the ESM marker used by later rules:\n{code}"
    );
    assert!(
        code.contains(r#"require.d(exports, "$G", function() {"#),
        "raw output should preserve runtime export getter:\n{code}"
    );
    assert!(
        !code.contains("exports.$G"),
        "raw output should not lower runtime getter to eager assignment:\n{code}"
    );
}

#[test]
fn runtime_getter_exports_become_esm_after_rules() {
    let source = r#"
!function(modules) {
  function __webpack_require__(id) {}
  __webpack_require__.s = 0;
  __webpack_require__(0);
}([
  function(module, exports, require) {
    require.r(exports);
    require.d(exports, "$G", function() { return V; });
    function V(value) {
      return value;
    }
  }
]);
"#;

    let result = unpack(
        source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 unpack should succeed");
    let code = result
        .modules
        .into_iter()
        .find(|(filename, _)| filename == "entry.js")
        .map(|(_, code)| normalize(&code))
        .expect("expected entry.js module");
    assert!(
        code.contains("export function $G(") || code.contains("export { V as $G }"),
        "runtime getter export should become ESM named export after Stage 1+2 rules:\n{code}"
    );
    assert!(
        !code.contains("require.r(") && !code.contains("require.d("),
        "driver output should consume webpack runtime export helpers:\n{code}"
    );
}

#[test]
fn default_export_function_keeps_later_var_dependency_hoisted() {
    let source = r#"
!function(modules) {
  function __webpack_require__(id) {
    var module = { exports: {} };
    modules[id].call(module.exports, module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__.d = function(exports, name, getter) {
    Object.defineProperty(exports, name, { enumerable: true, get: getter });
  };
  __webpack_require__.r = function(exports) {
    Object.defineProperty(exports, "__esModule", { value: true });
  };
  __webpack_require__.s = 0;
  __webpack_require__(0);
}([
  function(module, exports, require) {
    require.r(exports);
    require.d(exports, "default", function() { return exported; });
    var dep = require(1);
    function exported(target, values) {
      return values.map((value) => read.default(target, value)).filter(Boolean);
    }
    let alias;
    alias = dep;
    var read = alias;
  },
  function(module, exports, require) {
    exports.default = function(target, value) {
      return value;
    };
  }
]);
"#;

    let result = unpack(
        source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 unpack should succeed");
    let code = result
        .modules
        .into_iter()
        .find(|(filename, _)| filename == "entry.js")
        .map(|(_, code)| normalize(&code))
        .expect("expected entry.js module");

    assert!(
        code.contains("var read = alias;"),
        "later dependency read by exported function must stay var-hoisted:\n{code}"
    );
    assert!(
        !code.contains("const read = alias;"),
        "later dependency read by exported function was converted to TDZ const:\n{code}"
    );
}

#[test]
fn unpack_driver_simplifies_sequences_recreated_after_late_pass() {
    let source = r#"
!function(modules) {
  function __webpack_require__(id) {
    var module = { exports: {} };
    modules[id].call(module.exports, module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__.s = 0;
  __webpack_require__(0);
}([
  function(module, exports, require) {
    function f(e, t) {
      if (e) return t = make(), t && use(t), t;
      return e = t = null, e;
    }
    exports.f = f;
  }
]);
"#;

    let result = unpack(
        source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 unpack should succeed");
    let code = result
        .modules
        .into_iter()
        .find(|(filename, _)| filename == "entry.js")
        .map(|(_, code)| normalize(&code))
        .expect("expected entry.js module");

    assert!(
        code.contains("t = make();"),
        "sequence side effects should be split into statements:\n{code}"
    );
    assert!(
        code.contains("return t;"),
        "sequence return value should remain as the return expression:\n{code}"
    );
    assert!(
        !code.contains("return t = make(),"),
        "unpack output should not keep collapsed return sequences:\n{code}"
    );
    assert!(
        code.contains("e = null;") && code.contains("t = null;"),
        "assignment chains exposed by late sequence splitting should be split:\n{code}"
    );
    assert!(
        !code.contains("e = t = null"),
        "unpack output should not keep chained assignments exposed late:\n{code}"
    );
}

#[test]
fn unpack_driver_recovers_optional_calls_exposed_by_late_cleanup() {
    let source = r#"
!function(modules) {
  function __webpack_require__(id) {
    var module = { exports: {} };
    modules[id].call(module.exports, module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__.s = 0;
  __webpack_require__(0);
}([
  function(module, exports, require) {
    var G = function() {
      function U() {
        this.value = {};
        this.onDefaultValueFallback = null;
      }
      U.prototype.get = function(U, B, G) {
        var Y;
        var Z = this.getValue(U, B);
        var J = Array.isArray(B) ? "array" : typeof B;
        var X = Array.isArray(Z) ? "array" : typeof Z;
        return G ? G(Z) ? Z : ((Y = this.onDefaultValueFallback) === null || Y === undefined || Y.call(this, this, U, J, X), B) : Z;
      };
      U.prototype.getValue = function(U, B) {
        return U == null ? this.value : (B == null && (B = null), this.value[U] == null) ? B : this.value[U];
      };
      return U;
    }();
    exports.default = G;
  }
]);
"#;

    let result = unpack(
        source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 unpack should succeed");
    let code = result
        .modules
        .into_iter()
        .find(|(filename, _)| filename == "entry.js")
        .map(|(_, code)| normalize(&code))
        .expect("expected entry.js module");

    assert!(
        code.contains("this.onDefaultValueFallback?.(this, U, J, X);"),
        "late cleanup should recover optional calls exposed by class/IIFE recovery:\n{code}"
    );
    assert!(
        code.contains("if (B == null)"),
        "late cleanup should expand short-circuit assignment statements exposed late:\n{code}"
    );
    assert!(
        !code.contains("(Y = this.onDefaultValueFallback) === null")
            && !code.contains("B == null && (B = null)"),
        "unpack output should not leave lowered optional calls or short-circuit assignment statements:\n{code}"
    );
}

#[test]
fn raw_unpack_driver_preserves_pre_rule_module_shape() {
    let source = r#"
!function(modules) {
  function __webpack_require__(id) {
    var module = { exports: {} };
    modules[id].call(module.exports, module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__.d = function(exports, name, getter) {
    Object.defineProperty(exports, name, { enumerable: true, get: getter });
  };
  __webpack_require__.s = 0;
  __webpack_require__(0);
}([
  function(module, exports, require) {
    require.d(exports, "named", function() { return named; });
    require(1);
    const value = { ok: true };
    exports.default = value;
    const named = "named";
  },
  function(module, exports, require) {}
]);
"#;

    let result = unpack_raw(
        source,
        &DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("raw webpack4 unpack should succeed");
    let code = result
        .modules
        .into_iter()
        .find(|(filename, _)| filename == "entry.js")
        .map(|(_, code)| normalize(&code))
        .expect("expected entry.js module");

    assert!(
        code.contains(r#"require("./module-1.js")"#),
        "raw unpack should still rewrite webpack numeric require ids:\n{code}"
    );
    assert!(
        code.contains(r#"require.d(exports, "named", function() {"#),
        "raw unpack should preserve webpack runtime getter exports before decompile rules:\n{code}"
    );
    assert!(
        code.contains("exports.default = value"),
        "raw unpack should preserve CommonJS default export assignments before decompile rules:\n{code}"
    );
    assert!(
        !code.contains(".call"),
        "raw unpack should unwrap the webpack factory call:\n{code}"
    );
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
fn factory_param_rename_capture_is_deconflicted() {
    let source = r#"
!function(modules) {
  function __webpack_require__(id) {}
  __webpack_require__.s = 0;
  __webpack_require__(0);
}([
  function(m, e, r) {
    function invoke(require) {
      return [require, r(1)];
    }
    m.exports = invoke;
  },
  function(m, e, r) { m.exports = "dependency"; }
]);
"#;

    let code = raw_module(source, "entry.js");
    assert!(
        code.contains("function invoke(_require)") && code.contains(r#"require("./module-1.js")"#),
        "the nested binding must be renamed before the runtime loader:\n{code}"
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

#[test]
fn unpack_driver_simplifies_sequences_exposed_by_curly_braces() {
    let source = r#"
!function(modules) {
  function __webpack_require__(id) {}
  __webpack_require__.s = 0;
  __webpack_require__(0);
}([
  function(module, exports, require) {
    function* fib(limit) {
      var current = 0;
      var next = 1;
      while (current < limit)
        yield current, [current, next] = [next, current + next];
    }
    exports.fib = fib;
  }
]);
"#;

    let result = unpack(
        source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 unpack should succeed");
    let code = result
        .modules
        .into_iter()
        .find(|(filename, _)| filename == "entry.js")
        .map(|(_, code)| normalize(&code))
        .expect("expected entry.js module");

    assert!(
        code.contains("yield current;\n        [current, next] = ["),
        "late sequence cleanup should split yield/update sequence:\n{code}"
    );
}
