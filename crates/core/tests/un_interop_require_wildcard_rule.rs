mod common;
use common::{assert_eq_normalized, render, render_with_level};
use wakaru_core::RewriteLevel;

#[test]
fn unwraps_wildcard_by_import_path() {
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _a = _interopRequireWildcard(require("a"));
console.log(_a);
"#;
    let expected = r#"
import * as _a from "a";
console.log(_a);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_wildcard_two_args() {
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _b = _interopRequireWildcard(require("b"), true);
console.log(_b);
"#;
    let expected = r#"
import * as _b from "b";
console.log(_b);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_swc_external_interop_require_wildcard() {
    let input = r#"
import { _ as _interop_require_wildcard } from "@swc/helpers/_/_interop_require_wildcard";
var _ns = _interop_require_wildcard(require("my-lib"));
console.log(_ns);
"#;
    let expected = r#"
import * as _ns from "my-lib";
console.log(_ns);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_tslib_namespace_import_star_require() {
    let input = r#"
var tslib_1 = require("tslib");
var foo = tslib_1.__importStar(require("foo"));
console.log(foo);
"#;
    let expected = r#"
import tslib_1 from "tslib";
import * as foo from "foo";
console.log(foo);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_tslib_direct_import_star_require() {
    let input = r#"
var foo = require("tslib").__importStar(require("foo"));
console.log(foo);
"#;
    let expected = r#"
import * as foo from "foo";
console.log(foo);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_wildcard_call_with_shadowed_inner_require() {
    let input = r#"
import _interopRequireWildcard from "@babel/runtime/helpers/interopRequireWildcard";
function load(require) {
    var ns = _interopRequireWildcard(require("a"));
    return ns;
}
"#;

    let output = render(input);
    assert!(
        output.contains("_interopRequireWildcard(require(\"a\"))"),
        "shadowed require argument must not be treated as a module import:\n{output}"
    );
    assert!(
        !output.contains("import * as ns from \"a\""),
        "shadowed require must not be converted to a namespace import:\n{output}"
    );
}

#[test]
fn preserves_wildcard_for_non_require_args() {
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var ns = _interopRequireWildcard(factory());
console.log(ns.default);
"#;
    let output = render(input);
    // Non-require arg must NOT be unwrapped — helper synthesizes namespace object.
    assert!(
        output.contains("_interopRequireWildcard(factory())"),
        "non-require wildcard call should remain:\n{output}"
    );
    assert!(
        output.contains("@babel/runtime/helpers/interopRequireWildcard"),
        "retained wildcard call must keep the helper binding:\n{output}"
    );
}

#[test]
fn removes_wildcard_helper_declaration() {
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _a = _interopRequireWildcard(require("a"));
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn removes_unused_inline_import_star_dependencies() {
    let input = r#"
var __createBinding = (this && this.__createBinding) || function (o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    Object.defineProperty(o, k2, { enumerable: true, get: function() { return m[k]; } });
};
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? function (o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
} : function (o, v) {
    o.default = v;
});
console.log("ready");
"#;
    let expected = r#"
console.log("ready");
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn removes_inline_import_star_and_newly_unused_dependencies() {
    let input = r#"
var __createBinding = (this && this.__createBinding) || function (o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    Object.defineProperty(o, k2, { enumerable: true, get: function() { return m[k]; } });
};
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? function (o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
} : function (o, v) {
    o.default = v;
});
var __importStar = (this && this.__importStar) || function (mod) {
    if (mod && mod.__esModule) return mod;
    var result = {};
    if (mod != null) for (var k in mod) if (k !== "default" && Object.prototype.hasOwnProperty.call(mod, k)) __createBinding(result, mod, k);
    __setModuleDefault(result, mod);
    return result;
};
var ns = __importStar(require("./mod.js"));
console.log(ns);
"#;
    let expected = r#"
import * as ns from "./mod.js";
console.log(ns);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn removes_wildcard_helper_import_dependencies_as_side_effect_imports() {
    let input = r#"
import _typeof from "./typeof.js";
function _getRequireWildcardCache(nodeInterop) {
    if (typeof WeakMap !== "function") return null;
    var cacheBabelInterop = new WeakMap();
    var cacheNodeInterop = new WeakMap();
    return (_getRequireWildcardCache = function(nodeInterop) {
        return nodeInterop ? cacheNodeInterop : cacheBabelInterop;
    })(nodeInterop);
}
function _interopRequireWildcard(obj, nodeInterop) {
    if (!nodeInterop && obj && obj.__esModule) return obj;
    if (obj === null || _typeof(obj) !== "object" && typeof obj !== "function") {
        return { default: obj };
    }
    var cache = _getRequireWildcardCache(nodeInterop);
    if (cache && cache.has(obj)) return cache.get(obj);
    var newObj = {};
    for (var key in obj) {
        if (key !== "default" && Object.prototype.hasOwnProperty.call(obj, key)) {
            newObj[key] = obj[key];
        }
    }
    newObj.default = obj;
    if (cache) cache.set(obj, newObj);
    return newObj;
}
var ns = _interopRequireWildcard(require("./mod.js"));
use(ns);
"#;
    let expected = r#"
import "./typeof.js";
import * as ns from "./mod.js";
use(ns);
"#;

    assert_eq_normalized(&render(input), expected);
}

#[test]
fn removes_wildcard_helper_require_dependencies_as_side_effect_requires() {
    let input = r#"
var _typeof = require("./typeof.js");
function _getRequireWildcardCache(nodeInterop) {
    if (typeof WeakMap !== "function") return null;
    var cacheBabelInterop = new WeakMap();
    var cacheNodeInterop = new WeakMap();
    return (_getRequireWildcardCache = function(nodeInterop) {
        return nodeInterop ? cacheNodeInterop : cacheBabelInterop;
    })(nodeInterop);
}
function _interopRequireWildcard(obj, nodeInterop) {
    if (!nodeInterop && obj && obj.__esModule) return obj;
    if (obj === null || _typeof(obj) !== "object" && typeof obj !== "function") {
        return { default: obj };
    }
    var cache = _getRequireWildcardCache(nodeInterop);
    if (cache && cache.has(obj)) return cache.get(obj);
    var newObj = {};
    for (var key in obj) {
        if (key !== "default" && Object.prototype.hasOwnProperty.call(obj, key)) {
            newObj[key] = obj[key];
        }
    }
    newObj.default = obj;
    if (cache) cache.set(obj, newObj);
    return newObj;
}
var ns = _interopRequireWildcard(require("./mod.js"));
use(ns);
"#;
    let expected = r#"
import "./typeof.js";
import * as ns from "./mod.js";
use(ns);
"#;

    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_side_effecting_second_argument() {
    // Babel emits only a literal nodeInterop flag; a side-effecting second
    // argument is not a recognized producer shape and must not be dropped.
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _a = _interopRequireWildcard(require("a"), probe());
console.log(_a);
"#;
    insta::assert_snapshot!(render(input));
}

#[test]
fn preserves_spread_arguments() {
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _a = _interopRequireWildcard(...args);
console.log(_a);
"#;
    insta::assert_snapshot!(render(input));
}

#[test]
fn reassigned_binding_is_not_converted_to_an_import() {
    // `import * as _a` would make the later assignment a runtime TypeError;
    // the binding must stay a var.
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _a = _interopRequireWildcard(require("a"), true);
_a = fallback();
console.log(_a.x);
"#;
    let output = render(input);
    insta::assert_snapshot!(&output);
    assert_valid_esm_output(output);
}

#[test]
fn parenthesized_writes_also_block_import_conversion() {
    // `(_a) = ...` and `(_a)++` are writes even though the target is wrapped
    // in parens; the binding must stay a var (assignment to an import binding
    // is a runtime TypeError in ESM).
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _a = _interopRequireWildcard(require("a"), true);
(_a) = fallback();
console.log(_a.x);
var _b = _interopRequireWildcard(require("b"), true);
(_b)++;
console.log(_b);
"#;
    let output = render(input);
    assert!(
        !output.contains("import * as _a") && !output.contains("import * as _b"),
        "written bindings must not become namespace imports:\n{output}"
    );
    assert_valid_esm_output(output);
}

/// The whole point of the write guard is that the output stays valid ESM:
/// no assignment may target an import binding, and the module must parse.
fn assert_valid_esm_output(output: String) {
    use wakaru_core::{validate_output_modules, OutputFindingKind};
    let findings = validate_output_modules(&[("entry.js".into(), output)]);
    assert!(
        findings.iter().all(|finding| !matches!(
            finding.kind,
            OutputFindingKind::AssignToImport | OutputFindingKind::ParseError
        )),
        "output must be valid ESM: {findings:?}"
    );
}

#[test]
fn preserves_spread_require_argument() {
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _a = _interopRequireWildcard(require(..."ab"), true);
console.log(_a);
"#;
    let output = render(input);
    assert!(
        !output.contains("import * as _a"),
        "spread require argument must not convert to an import:\n{output}"
    );
}

#[test]
fn typescript_own_keys_factory_restores_namespace_import() {
    let input = include_str!("fixtures/tslib-interop/generated/star.js");
    let focused = common::render_rule(input, |_| wakaru_core::rules::UnInteropRequireWildcard);
    assert!(!focused.contains("__importStar(require("), "{focused}");
    let output = render(input);
    assert!(!output.contains("__importStar"), "{output}");
    assert!(!output.contains("__createBinding"), "{output}");
    assert!(output.contains("import * as provider"), "{output}");
}

#[test]
fn import_star_marker_requires_a_helper_body() {
    let input = r#"
var h = this && this.__importStar || (function() { return function(mod) { return custom(mod); }; })();
var provider = h(require("./provider"));
use(provider);
"#;
    assert!(render(input).contains("h(require("));
}

#[test]
fn typescript_factory_preserves_shared_helper_dependencies() {
    let input = format!(
        "{}\nuse(__createBinding);",
        include_str!("fixtures/tslib-interop/generated/star.js")
    );
    let output = render(&input);
    assert!(!output.contains("__importStar"), "{output}");
    assert!(output.contains("var __createBinding"), "{output}");
}

#[test]
fn compressed_typescript_own_keys_factory_restores_namespace_import() {
    for input in [
        include_str!("fixtures/tslib-interop/generated/star-compressed.js"),
        include_str!("fixtures/tslib-interop/generated/star-mangled.js"),
    ] {
        let focused = common::render_rule(input, |_| wakaru_core::rules::UnInteropRequireWildcard);
        assert!(!focused.contains("this.__importStar"), "{focused}");
        assert!(focused.contains("import * as"), "{focused}");
        let output = render(input);
        assert!(!output.contains("getOwnPropertyNames"), "{output}");
    }
}

#[test]
fn compressed_import_star_keeps_observable_factory_initialization() {
    let input = include_str!("fixtures/tslib-interop/generated/star-compressed.js");
    for input in [
        format!("{input}\nuse(ownKeys);"),
        format!("{input}\neval('ownKeys');"),
        input.replace("ownKeys=function", "ownKeys=effect(),other=function"),
        input.replace(",ownKeys;", ";let ownKeys;"),
    ] {
        let output = common::render_rule(&input, |_| wakaru_core::rules::UnInteropRequireWildcard);
        assert!(output.contains("this.__importStar"), "{output}");
    }
}

#[test]
fn namespace_assigned_to_a_property_becomes_namespace_import() {
    // TypeScript lowers `export * as ns from "dep"` to a property write; the
    // raw `require` result would lose the synthesized namespace.
    let input = r#"
var tslib_1 = require("tslib");
exports.ns = tslib_1.__importStar(require("dep"));
"#;
    let expected = r#"
import tslib_1 from "tslib";
import * as ns from "dep";
export { ns };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn namespace_assigned_to_a_property_gets_a_free_name() {
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var ns = 1;
exports.ns = _interopRequireWildcard(require("dep"));
function read() {
  return ns_1;
}
"#;
    let output = render(input);
    assert!(
        output.contains(r#"import * as ns_2 from "dep";"#),
        "{output}"
    );
    assert!(output.contains("ns_2 as ns"), "{output}");
}

#[test]
fn wildcard_of_a_required_binding_becomes_namespace_import() {
    // sucrase keeps the module in its own binding and wraps that.
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _dep = require("dep");
var ns = _interopRequireWildcard(_dep);
console.log(ns, _dep.value);
"#;
    let output = render(input);
    assert!(output.contains(r#"import * as ns from "dep";"#), "{output}");
}

#[test]
fn wildcard_of_a_written_required_binding_stays() {
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _dep = require("dep");
var ns = _interopRequireWildcard(_dep);
_dep = other;
console.log(ns, _dep);
"#;
    let output = render(input);
    assert!(!output.contains("import * as ns"), "{output}");
}

const ROLLUP_NAMESPACE_DEFAULT: &str = r#"
function _interopNamespaceDefault(e) {
	var n = Object.create(null);
	if (e) {
		Object.keys(e).forEach(function (k) {
			if (k !== 'default') {
				var d = Object.getOwnPropertyDescriptor(e, k);
				Object.defineProperty(n, k, d.get ? d : {
					enumerable: true,
					get: function () { return e[k]; }
				});
			}
		});
	}
	n.default = e;
	return Object.freeze(n);
}
"#;

#[test]
fn rollup_namespace_default_becomes_namespace_import() {
    // rollup's helper sets `default` to the whole module even when it is
    // marked `__esModule`; the source's `import * as` is recovered instead.
    let input = format!(
        r#"
var dep = require("dep");
{ROLLUP_NAMESPACE_DEFAULT}
var dep__namespace = _interopNamespaceDefault(dep);
console.log(dep__namespace, dep.value);
"#
    );
    let output = render(&input);
    assert!(
        output.contains(r#"import * as dep__namespace from "dep";"#),
        "{output}"
    );
    assert!(!output.contains("_interopNamespaceDefault"), "{output}");
}

#[test]
fn rollup_namespace_default_variants_become_namespace_imports() {
    let variants = [
        // generatedCode.constBindings: a `for...in` loop and arrow getters.
        r#"
function _interopNamespaceDefault(e) {
	const n = Object.create(null);
	if (e) {
		for (const k in e) {
			if (k !== 'default') {
				const d = Object.getOwnPropertyDescriptor(e, k);
				Object.defineProperty(n, k, d.get ? d : {
					enumerable: true,
					get: () => e[k]
				});
			}
		}
	}
	n.default = e;
	return Object.freeze(n);
}
"#,
        // freeze: false, generatedCode.symbols
        r#"
function _interopNamespaceDefault(e) {
	var n = Object.create(null, { [Symbol.toStringTag]: { value: 'Module' } });
	if (e) {
		Object.keys(e).forEach(function (k) {
			if (k !== 'default') {
				var d = Object.getOwnPropertyDescriptor(e, k);
				Object.defineProperty(n, k, d.get ? d : {
					enumerable: true,
					get: function () { return e[k]; }
				});
			}
		});
	}
	n.default = e;
	return n;
}
"#,
        // externalLiveBindings: false
        r#"
function _interopNamespaceDefault(e) {
	var n = Object.create(null);
	if (e) {
		for (var k in e) {
			n[k] = e[k];
		}
	}
	n.default = e;
	return Object.freeze(n);
}
"#,
        // interop: "compat" returns a provider with a `default` key unchanged.
        r#"
function _interopNamespaceDefault(e) {
	if (e && typeof e === 'object' && 'default' in e) return e;
	var n = Object.create(null);
	if (e) {
		Object.keys(e).forEach(function (k) {
			if (k !== 'default') {
				var d = Object.getOwnPropertyDescriptor(e, k);
				Object.defineProperty(n, k, d.get ? d : {
					enumerable: true,
					get: function () { return e[k]; }
				});
			}
		});
	}
	n.default = e;
	return Object.freeze(n);
}
"#,
        // interop: "compat", terser
        r#"function _interopNamespaceDefault(e){if(e&&"object"==typeof e&&"default"in e)return e;var t=Object.create(null);return e&&Object.keys(e).forEach(function(r){if("default"!==r){var n=Object.getOwnPropertyDescriptor(e,r);Object.defineProperty(t,r,n.get?n:{enumerable:!0,get:function(){return e[r]}})}}),t.default=e,Object.freeze(t)}"#,
        // terser
        r#"function _interopNamespaceDefault(e){var t=Object.create(null);return e&&Object.keys(e).forEach(function(r){if("default"!==r){var n=Object.getOwnPropertyDescriptor(e,r);Object.defineProperty(t,r,n.get?n:{enumerable:!0,get:function(){return e[r]}})}}),t.default=e,Object.freeze(t)}"#,
    ];
    for helper in variants {
        let input = format!(
            r#"
var dep = require("dep");
{helper}
var ns = _interopNamespaceDefault(dep);
console.log(ns, dep.value);
"#
        );
        let output = render(&input);
        assert!(output.contains(r#"import * as ns from "dep";"#), "{output}");
        assert!(!output.contains("_interopNamespaceDefault"), "{output}");
    }
}

#[test]
fn early_return_other_than_the_default_key_guard_is_not_an_interop() {
    let input = r#"
var dep = require("dep");
function wrap(e) {
	if (e && typeof e === 'object') return e;
	var n = Object.create(null);
	if (e) {
		Object.keys(e).forEach(function (k) {
			Object.defineProperty(n, k, { enumerable: true, get: function () { return e[k]; } });
		});
	}
	n.default = e;
	return Object.freeze(n);
}
var ns = wrap(dep);
console.log(ns);
"#;
    let output = render(input);
    assert!(!output.contains("import * as ns"), "{output}");
    assert!(output.contains("wrap(dep)"), "{output}");
}

#[test]
fn namespace_object_builder_without_default_is_not_an_interop() {
    // The copy loop alone is a plain object builder: no `n.default = e`.
    let input = r#"
var dep = require("dep");
function copyKeys(e) {
	var n = Object.create(null);
	if (e) {
		Object.keys(e).forEach(function (k) {
			Object.defineProperty(n, k, { enumerable: true, get: function () { return e[k]; } });
		});
	}
	return Object.freeze(n);
}
var ns = copyKeys(dep);
console.log(ns);
"#;
    let output = render(input);
    assert!(!output.contains("import * as ns"), "{output}");
    assert!(output.contains("copyKeys(dep)"), "{output}");
}

#[test]
fn module_kept_commonjs_keeps_interop_wildcard() {
    // An aliased `exports` keeps the module CommonJS; an `import` there would
    // make it a module that cannot use `exports`.
    let input = r#"
"use strict";
var _a = _interopRequireWildcard(require("./a"));
function _interopRequireWildcard(e) { if (e && e.__esModule) return e; var n = {}; if (e != null) for (var k in e) if (Object.prototype.hasOwnProperty.call(e, k)) n[k] = e[k]; n.default = e; return n; }
var target = exports;
target.run = function () { return _a.default(); };
"#;
    let output = render(input);
    assert!(!output.contains("import"), "{output}");
    assert!(
        output.contains(r#"_interopRequireWildcard(require("./a"))"#),
        "{output}"
    );
}

#[test]
fn minimal_level_keeps_interop_wildcard() {
    // `UnEsm` does not convert at `minimal`, so the module stays CommonJS.
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var ns = _interopRequireWildcard(require("./a"));
exports.read = function () { return ns.value; };
"#;
    let output = render_with_level(input, RewriteLevel::Minimal);
    assert!(!output.contains("import"), "{output}");
    assert!(output.contains(r#"require("./a")"#), "{output}");
}

const TSC_IMPORT_STAR: &str = r#"
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
Object.defineProperty(exports, "__esModule", { value: true });
"#;

const BABEL_WILDCARD: &str = r#"
function _interopRequireWildcard(e) { if (e && e.__esModule) return e; var n = {}; if (e != null) for (var k in e) if (Object.prototype.hasOwnProperty.call(e, k)) n[k] = e[k]; n.default = e; return n; }
"#;

#[test]
fn typescript_lowered_dynamic_import_becomes_import_call() {
    // TypeScript 5.9, `module: CommonJS`, `esModuleInterop`.
    let input = format!(
        "{TSC_IMPORT_STAR}{}",
        r#"
exports.loadNs = loadNs;
exports.loadByName = loadByName;
async function loadNs() { return await Promise.resolve().then(() => __importStar(require("./dep.js"))); }
function loadByName(name) { return Promise.resolve(`${"./" + name + ".js"}`).then(s => __importStar(require(s))).then((ns) => ns.live); }
"#
    );
    let output = render(&input);
    assert!(output.contains(r#"await import("./dep.js")"#), "{output}");
    assert!(output.contains("return import(`"), "{output}");
    assert!(output.contains(".then((ns)=>ns.live)"), "{output}");
    assert!(!output.contains("require"), "{output}");
    assert!(!output.contains("__importStar"), "{output}");
}

#[test]
fn typescript_es5_lowered_dynamic_import_becomes_import_call() {
    // TypeScript 5.9 with `target: ES5` uses function callbacks; TypeScript
    // 3.9 also lowers a dynamic specifier into the callback.
    let input = format!(
        "{TSC_IMPORT_STAR}{}",
        r#"
exports.loadThen = loadThen;
exports.loadByName = loadByName;
exports.loadOld = loadOld;
function loadThen() { return Promise.resolve().then(function () { return __importStar(require("./dep.js")); }).then(function (ns) { return ns.live; }); }
function loadByName(name) { return Promise.resolve("".concat("./" + name + ".js")).then(function (s) { return __importStar(require(s)); }).then(function (ns) { return ns.live; }); }
function loadOld(name) { return Promise.resolve().then(function () { return __importStar(require("./" + name + ".js")); }); }
"#
    );
    let output = render(&input);
    assert!(
        output.contains(r#"return import("./dep.js").then"#),
        "{output}"
    );
    assert_eq!(output.matches("import(").count(), 3, "{output}");
    assert!(!output.contains("require"), "{output}");
}

#[test]
fn swc_lowered_dynamic_import_becomes_import_call() {
    // swc 1.16, `module.type: "commonjs"`.
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", {
    value: true
});
function _export(target, all) {
    for(var name in all)Object.defineProperty(target, name, {
        enumerable: true,
        get: Object.getOwnPropertyDescriptor(all, name).get
    });
}
_export(exports, {
    get loadByName () {
        return loadByName;
    },
    get loadDefault () {
        return loadDefault;
    }
});
function _getRequireWildcardCache(nodeInterop) {
    if (typeof WeakMap !== "function") return null;
    var cacheBabelInterop = new WeakMap();
    var cacheNodeInterop = new WeakMap();
    return (_getRequireWildcardCache = function(nodeInterop) {
        return nodeInterop ? cacheNodeInterop : cacheBabelInterop;
    })(nodeInterop);
}
function _interop_require_wildcard(obj, nodeInterop) {
    if (!nodeInterop && obj && obj.__esModule) return obj;
    if (obj === null || typeof obj !== "object" && typeof obj !== "function") return {
        default: obj
    };
    var cache = _getRequireWildcardCache(nodeInterop);
    if (cache && cache.has(obj)) return cache.get(obj);
    var newObj = {
        __proto__: null
    };
    var hasPropertyDescriptor = Object.defineProperty && Object.getOwnPropertyDescriptor;
    for(var key in obj){
        if (key !== "default" && Object.prototype.hasOwnProperty.call(obj, key)) {
            var desc = hasPropertyDescriptor ? Object.getOwnPropertyDescriptor(obj, key) : null;
            if (desc && (desc.get || desc.set)) Object.defineProperty(newObj, key, desc);
            else newObj[key] = obj[key];
        }
    }
    newObj.default = obj;
    if (cache) cache.set(obj, newObj);
    return newObj;
}
async function loadDefault() {
    const { default: f } = await Promise.resolve().then(()=>/*#__PURE__*/ _interop_require_wildcard(require("./dep.js")));
    return f();
}
function loadByName(name) {
    return Promise.resolve("./" + name + ".js").then((p)=>/*#__PURE__*/ _interop_require_wildcard(require(p))).then((ns)=>ns.live);
}
"#;
    let output = render(input);
    assert!(output.contains(r#"await import("./dep.js")"#), "{output}");
    assert!(output.contains("return import(`./${name}.js`)"), "{output}");
    assert!(!output.contains("require"), "{output}");
    assert!(!output.contains("_interop_require_wildcard"), "{output}");
}

#[test]
fn promise_callbacks_that_are_not_a_lowered_import_keep_the_require() {
    // Each callback differs from the lowered `import()` in one way: the
    // require reads another binding than the passed specifier, the callback
    // does more than return, the wrapper is not an interop helper, and the
    // callback reads its own `arguments`.
    let input = format!(
        "{BABEL_WILDCARD}{}",
        r#"
Object.defineProperty(exports, "__esModule", { value: true });
exports.a = function (q) { return Promise.resolve("./x").then((p) => _interopRequireWildcard(require(q))); };
exports.b = function () { return Promise.resolve().then(() => { log(); return _interopRequireWildcard(require("./x")); }); };
exports.c = function () { return Promise.resolve().then(() => wrap(require("./x"))); };
exports.d = function () { return Promise.resolve().then(function () { return _interopRequireWildcard(require(arguments[0])); }); };
"#
    );
    let output = render(&input);
    assert!(!output.contains("import("), "{output}");
    assert_eq!(output.matches("require(").count(), 4, "{output}");
}

#[test]
fn shadowed_promise_is_not_a_lowered_import() {
    let input = format!(
        "{BABEL_WILDCARD}{}",
        r#"
Object.defineProperty(exports, "__esModule", { value: true });
exports.load = function (Promise) { return Promise.resolve().then(() => _interopRequireWildcard(require("./x"))); };
"#
    );
    let output = render(&input);
    assert!(!output.contains("import("), "{output}");
}

#[test]
fn direct_eval_keeps_lowered_dynamic_import() {
    // A direct `eval` can declare a `Promise` binding, so `Promise` is not
    // provably the global.
    let input = format!(
        "{BABEL_WILDCARD}{}",
        r#"
Object.defineProperty(exports, "__esModule", { value: true });
exports.load = function (code) { eval(code); return Promise.resolve().then(() => _interopRequireWildcard(require("./x"))); };
"#
    );
    let output = render(&input);
    assert!(output.contains("export const load"), "{output}");
    assert!(!output.contains("import("), "{output}");
}

#[test]
fn module_kept_commonjs_keeps_lowered_dynamic_import() {
    let input = format!(
        "{BABEL_WILDCARD}{}",
        r#"
var target = exports;
target.load = function () { return Promise.resolve().then(() => _interopRequireWildcard(require("./a"))); };
"#
    );
    let output = render(&input);
    assert!(!output.contains("import"), "{output}");
    assert!(
        output.contains(r#"_interopRequireWildcard(require("./a"))"#),
        "{output}"
    );
}

#[test]
fn babel_lowered_dynamic_import_becomes_import_call() {
    // Babel 7.29 preset-env, `modules: "commonjs"`: a non-literal specifier
    // goes through a wrapper and a `new Promise` executor.
    let input = r#"
"use strict";

Object.defineProperty(exports, "__esModule", {
  value: true
});
exports.loadByName = loadByName;
exports.loadNs = loadNs;
function _interopRequireWildcard(e, t) { if ("function" == typeof WeakMap) var r = new WeakMap(), n = new WeakMap(); return (_interopRequireWildcard = function (e, t) { if (!t && e && e.__esModule) return e; var o, i, f = { __proto__: null, default: e }; if (null === e || "object" != typeof e && "function" != typeof e) return f; if (o = t ? n : r) { if (o.has(e)) return o.get(e); o.set(e, f); } for (const t in e) "default" !== t && {}.hasOwnProperty.call(e, t) && ((i = (o = Object.defineProperty) && Object.getOwnPropertyDescriptor(e, t)) && (i.get || i.set) ? o(f, t, i) : f[t] = e[t]); return f; })(e, t); }
async function loadNs() {
  return await Promise.resolve().then(() => _interopRequireWildcard(require("./dep.js")));
}
function loadByName(name) {
  return (specifier => new Promise(r => r(`${specifier}`)).then(s => _interopRequireWildcard(require(s))))("./" + name + ".js").then(ns => ns.live);
}
"#;
    let output = render(input);
    assert!(output.contains(r#"await import("./dep.js")"#), "{output}");
    assert!(
        output.contains("return import(`./${name}.js`).then"),
        "{output}"
    );
    assert!(!output.contains("require"), "{output}");
    assert!(!output.contains("Promise"), "{output}");
}

#[test]
fn rollup_lowered_dynamic_import_becomes_import_call() {
    // rollup 4.63 with `dynamicImportInCjs: false`.
    let input = r#"
'use strict';

function _interopNamespaceDefault(e) {
	var n = Object.create(null);
	if (e) {
		Object.keys(e).forEach(function (k) {
			if (k !== 'default') {
				var d = Object.getOwnPropertyDescriptor(e, k);
				Object.defineProperty(n, k, d.get ? d : {
					enumerable: true,
					get: function () { return e[k]; }
				});
			}
		});
	}
	n.default = e;
	return Object.freeze(n);
}

async function loadNs() { return await Promise.resolve().then(function () { return /*#__PURE__*/_interopNamespaceDefault(require('./dep.js')); }); }
function loadByName(name) { return (function (t) { return Promise.resolve().then(function () { return /*#__PURE__*/_interopNamespaceDefault(require(t)); }); })("./" + name + ".js").then((ns) => ns.live); }

exports.loadByName = loadByName;
exports.loadNs = loadNs;
"#;
    let output = render(input);
    assert!(output.contains("await import('./dep.js')"), "{output}");
    assert!(
        output.contains("return import(`./${name}.js`).then"),
        "{output}"
    );
    assert!(!output.contains("require"), "{output}");
    assert!(!output.contains("_interopNamespaceDefault"), "{output}");
}

#[test]
fn wrapper_that_does_not_pass_its_parameter_is_not_a_lowered_import() {
    let input = format!(
        "{BABEL_WILDCARD}{}",
        r#"
Object.defineProperty(exports, "__esModule", { value: true });
exports.load = function (other) { return ((t) => Promise.resolve().then(() => _interopRequireWildcard(require(other))))("./x"); };
"#
    );
    let output = render(&input);
    assert!(!output.contains(r#"import("./x")"#), "{output}");
    assert!(output.contains("import(other)"), "{output}");
}

#[test]
fn require_inside_a_function_keeps_interop_wildcard() {
    // A `require` inside a function stays a `require`, so the namespace
    // interop around it stays too. A lowered `import()` is the exception.
    let input = format!(
        "{BABEL_WILDCARD}{}",
        r#"
Object.defineProperty(exports, "__esModule", { value: true });
exports.read = function () { var ns = _interopRequireWildcard(require("./a")); return ns.value; };
exports.load = function () { return Promise.resolve().then(() => _interopRequireWildcard(require("./b"))); };
"#
    );
    let output = render(&input);
    assert!(
        output.contains(r#"_interopRequireWildcard(require("./a"))"#),
        "{output}"
    );
    assert!(output.contains(r#"import("./b")"#), "{output}");
    assert!(
        output.contains("function _interopRequireWildcard"),
        "{output}"
    );
}

/// webpack 5.111 bundle of a package compiled by swc 1.16 with
/// `externalHelpers` and `module.type: "commonjs"`: the wildcard helper is
/// its own module, known to the consumer only from cross-module facts.
const SWC_EXTERNAL_HELPER_BUNDLE: &str = r#"/******/ (() => { // webpackBootstrap
/******/ 	var __webpack_modules__ = ({

/***/ 509
(__unused_webpack_module, exports, __webpack_require__) {

"use strict";

Object.defineProperty(exports, "__esModule", ({
    value: true
}));
function _export(target, all) {
    for(var name in all)Object.defineProperty(target, name, {
        enumerable: true,
        get: Object.getOwnPropertyDescriptor(all, name).get
    });
}
_export(exports, {
    get loadLazy () {
        return loadLazy;
    },
    get loadThen () {
        return loadThen;
    }
});
const _interop_require_wildcard = __webpack_require__(544);
async function loadLazy() {
    const ns = await Promise.resolve().then(()=>/*#__PURE__*/ _interop_require_wildcard._(__webpack_require__(931)));
    return ns.value + ns.default();
}
function loadThen() {
    return Promise.resolve().then(()=>/*#__PURE__*/ _interop_require_wildcard._(__webpack_require__(931))).then((ns)=>ns.value);
}


/***/ },

/***/ 931
(__unused_webpack_module, exports) {

"use strict";

Object.defineProperty(exports, "__esModule", ({
    value: true
}));
function _export(target, all) {
    for(var name in all)Object.defineProperty(target, name, {
        enumerable: true,
        get: Object.getOwnPropertyDescriptor(all, name).get
    });
}
_export(exports, {
    get default () {
        return one;
    },
    get value () {
        return value;
    }
});
const value = 41;
function one() {
    return 1;
}


/***/ },

/***/ 544
(__unused_webpack___webpack_module__, __webpack_exports__, __webpack_require__) {

"use strict";
/* harmony export */ __webpack_require__.d(__webpack_exports__, {
/* harmony export */   _: () => (/* binding */ _interop_require_wildcard)
/* harmony export */ });
function _getRequireWildcardCache(nodeInterop) {
    if (typeof WeakMap !== "function") return null;

    var cacheBabelInterop = new WeakMap();
    var cacheNodeInterop = new WeakMap();

    return (_getRequireWildcardCache = function(nodeInterop) {
        return nodeInterop ? cacheNodeInterop : cacheBabelInterop;
    })(nodeInterop);
}
function _interop_require_wildcard(obj, nodeInterop) {
    if (!nodeInterop && obj && obj.__esModule) return obj;
    if (obj === null || typeof obj !== "object" && typeof obj !== "function") return { default: obj };

    var cache = _getRequireWildcardCache(nodeInterop);

    if (cache && cache.has(obj)) return cache.get(obj);

    var newObj = { __proto__: null };
    var hasPropertyDescriptor = Object.defineProperty && Object.getOwnPropertyDescriptor;

    for (var key in obj) {
        if (key !== "default" && Object.prototype.hasOwnProperty.call(obj, key)) {
            var desc = hasPropertyDescriptor ? Object.getOwnPropertyDescriptor(obj, key) : null;
            if (desc && (desc.get || desc.set)) Object.defineProperty(newObj, key, desc);
            else newObj[key] = obj[key];
        }
    }

    newObj.default = obj;

    if (cache) cache.set(obj, newObj);

    return newObj;
}



/***/ }

/******/ 	});
/************************************************************************/
/******/ 	// The module cache
/******/ 	const __webpack_module_cache__ = {};
/******/ 	
/******/ 	// The require function
/******/ 	function __webpack_require__(moduleId) {
/******/ 		// Check if module is in cache
/******/ 		const cachedModule = __webpack_module_cache__[moduleId];
/******/ 		if (cachedModule !== undefined) {
/******/ 			return cachedModule.exports;
/******/ 		}
/******/ 		// Create a new module (and put it into the cache)
/******/ 		const module = __webpack_module_cache__[moduleId] = {
/******/ 			// no module.id needed
/******/ 			// no module.loaded needed
/******/ 			exports: {}
/******/ 		};
/******/ 	
/******/ 		// Execute the module function
/******/ 		__webpack_modules__[moduleId](module, module.exports, __webpack_require__);
/******/ 	
/******/ 		// Return the exports of the module
/******/ 		return module.exports;
/******/ 	}
/******/ 	
/************************************************************************/
/******/ 	/* webpack/runtime/define property getters */
/******/ 	// define getter/value functions for harmony exports
/******/ 	__webpack_require__.d = (exports, definition) => {
/******/ 		for(var key in definition) {
/******/ 			if(__webpack_require__.o(definition, key) && !__webpack_require__.o(exports, key)) {
/******/ 				Object.defineProperty(exports, key, { enumerable: true, get: definition[key] });
/******/ 			}
/******/ 		}
/******/ 	};
/******/ 	
/******/ 	/* webpack/runtime/hasOwnProperty shorthand */
/******/ 	__webpack_require__.o = (obj, prop) => (Object.prototype.hasOwnProperty.call(obj, prop));
/******/ 	
/************************************************************************/
let __webpack_exports__ = {};
const pkg = __webpack_require__(509);
pkg.loadLazy().then((v) => console.log("lazy", v));
pkg.loadThen().then((v) => console.log("then", v));

/******/ })()
;"#;

#[test]
fn cross_module_wildcard_helper_lowered_dynamic_import_becomes_import_call() {
    let output = wakaru_core::driver::test_support::unpack(
        SWC_EXTERNAL_HELPER_BUNDLE,
        wakaru_core::DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    let (_, consumer) = output
        .modules
        .iter()
        .find(|(name, _)| name == "module-509.js")
        .expect("consumer module");
    assert!(
        consumer.contains(r#"await import("./module-931.js")"#),
        "{consumer}"
    );
    assert!(
        consumer.contains(r#"return import("./module-931.js").then"#),
        "{consumer}"
    );
    assert!(!consumer.contains("require"), "{consumer}");
}

#[test]
fn cross_module_call_of_a_non_helper_is_not_a_lowered_import() {
    // The provider's `_` is an ordinary function, so its export fact proves
    // no helper identity.
    let start = SWC_EXTERNAL_HELPER_BUNDLE
        .find("function _interop_require_wildcard(obj, nodeInterop) {")
        .expect("helper start");
    let end = SWC_EXTERNAL_HELPER_BUNDLE[start..]
        .find("\n/***/ }")
        .map(|offset| start + offset)
        .expect("helper end");
    let bundle = format!(
        "{}function _interop_require_wildcard(obj) {{ return {{ wrapped: obj }}; }}\n{}",
        &SWC_EXTERNAL_HELPER_BUNDLE[..start],
        &SWC_EXTERNAL_HELPER_BUNDLE[end..]
    );
    let output = wakaru_core::driver::test_support::unpack(
        &bundle,
        wakaru_core::DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    let (_, consumer) = output
        .modules
        .iter()
        .find(|(name, _)| name == "module-509.js")
        .expect("consumer module");
    assert!(!consumer.contains("import("), "{consumer}");
    assert!(
        consumer.contains(r#"require("./module-931.js")"#),
        "{consumer}"
    );
}
