mod common;
use common::{assert_eq_normalized, render, render_rule};
use wakaru_core::facts::{
    HelperExportFact, HelperKind, ModuleFacts, ModuleFactsMap, TypeScriptHelperExportFact,
    TypeScriptHelperKind,
};
use wakaru_core::rules::UnObjectSpread;

#[test]
fn replaces_object_spread2_with_spread_syntax() {
    let input = r#"
var _objectSpread2 = require("@babel/runtime/helpers/objectSpread2");
var x = _objectSpread2({}, y);
"#;
    let expected = r#"
const x = { ...y };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_multiple_spread_sources() {
    let input = r#"
var _objectSpread2 = require("@babel/runtime/helpers/objectSpread2");
var x = _objectSpread2({}, a, b, c);
"#;
    let expected = r#"
const x = { ...a, ...b, ...c };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_extends_helper() {
    let input = r#"
var _extends = require("@babel/runtime/helpers/extends");
var x = _extends({}, obj1, obj2);
"#;
    let expected = r#"
const x = { ...obj1, ...obj2 };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_babel_runtime_esm_import_extends() {
    let input = r#"
import _extends from "@babel/runtime/helpers/extends";
var x = _extends({}, app_info, base_info);
"#;
    let expected = r#"
const x = { ...app_info, ...base_info };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_swc_external_extends_import() {
    let input = r#"
import { _ as _extends } from "@swc/helpers/_/_extends";
var x = _extends({}, app_info, base_info);
"#;
    let expected = r#"
const x = { ...app_info, ...base_info };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_tslib_named_assign_import() {
    let input = r#"
import { __assign } from "tslib";
var x = __assign({ id: app_id }, app_info, { name: value });
"#;
    let expected = r#"
const x = { id: app_id, ...app_info, name: value };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_tslib_namespace_assign_require() {
    let input = r#"
var tslib_1 = require("tslib");
var x = tslib_1.__assign({ id: app_id }, app_info, { name: value });
"#;
    let expected = r#"
import tslib_1 from "tslib";
const x = { id: app_id, ...app_info, name: value };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_babel_runtime_esm_import_object_spread() {
    let input = r#"
import _objectSpread2 from "@babel/runtime/helpers/objectSpread2";
var x = _objectSpread2({}, app_info, { app_name: name });
"#;
    let expected = r#"
const x = { ...app_info, app_name: name };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_swc_external_object_spread_imports() {
    let input = r#"
import { _ as _object_spread } from "@swc/helpers/_/_object_spread";
import { _ as _object_spread_props } from "@swc/helpers/_/_object_spread_props";
var x = _object_spread_props(_object_spread({}, app_info), { app_name: name });
"#;
    let expected = r#"
const x = { ...app_info, app_name: name };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_shadowed_require_runtime_object_spread_helper() {
    let input = r#"
function require(path) {
    return load(path);
}
var _objectSpread2 = require("@babel/runtime/helpers/objectSpread2");
var x = _objectSpread2({}, y);
use(x);
"#;

    let output = render(input);
    assert!(
        output.contains("_objectSpread2({}, y)"),
        "shadowed require helper binding must not be rewritten:\n{output}"
    );
    assert!(
        !output.contains("{ ...y }"),
        "shadowed require helper binding must not produce object spread:\n{output}"
    );
}

#[test]
fn handles_swc_numeric_namespace_object_spread() {
    let input = r#"
const Y = require(39889);
const out = Y.pi(Y.pi({}, app_info), { app_name: name });
"#;
    let expected = r#"
const out = { ...app_info, app_name: name };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_shadowed_numeric_require_object_spread_namespace() {
    let input = r#"
function require(id) {
    return load(id);
}
const Y = require(39889);
const out = Y.pi(Y.pi({}, app_info), { app_name: name });
use(out);
"#;

    let output = render(input);
    assert!(
        output.contains("Y.pi(Y.pi({}, app_info),"),
        "shadowed numeric require namespace must not be rewritten:\n{output}"
    );
    assert!(
        !output.contains("{ ...app_info, app_name: name }"),
        "shadowed numeric require namespace must not produce object spread:\n{output}"
    );
}

#[test]
fn swc_numeric_namespace_object_spread_requires_pi_export() {
    let input = r#"
const Y = require(39889);
const out = Y.notPi(Y.notPi({}, app_info), { app_name: name });
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn swc_numeric_namespace_object_spread_preserves_mutating_target() {
    let input = r#"
const Y = require(39889);
const out = Y.pi(target, app_info);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn handles_cross_module_extends_helper_fact() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            helper_exports: vec![HelperExportFact {
                exported: "default".into(),
                local: Some("extends".into()),
                kind: HelperKind::Extends,
            }],
            ..Default::default()
        },
    );

    let input = r#"
import _extends from "./helpers.js";
var x = _extends({}, app_info, base_info);
"#;
    let expected = r#"
import _extends from "./helpers.js";
var x = { ...app_info, ...base_info };
"#;
    assert_eq_normalized(
        &render_rule(input, |_| UnObjectSpread::new_with_facts(&facts)),
        expected,
    );
}

#[test]
fn handles_cross_module_named_extends_helper_fact() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            helper_exports: vec![HelperExportFact {
                exported: "Z".into(),
                local: Some("Z".into()),
                kind: HelperKind::Extends,
            }],
            ..Default::default()
        },
    );

    let input = r#"
import { Z as _extends } from "./helpers.js";
var x = _extends({}, app_info, base_info);
"#;
    let expected = r#"
import { Z as _extends } from "./helpers.js";
var x = { ...app_info, ...base_info };
"#;
    assert_eq_normalized(
        &render_rule(input, |_| UnObjectSpread::new_with_facts(&facts)),
        expected,
    );
}

#[test]
fn handles_cross_module_ts_assign_helper_fact() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            ts_helper_exports: vec![TypeScriptHelperExportFact {
                exported: "__assign".into(),
                local: Some("__assign".into()),
                kind: TypeScriptHelperKind::Assign,
            }],
            ..Default::default()
        },
    );

    let input = r#"
import { __assign as assign } from "./helpers.js";
var x = assign({ id: app_id }, app_info, { name: value });
"#;
    let expected = r#"
import { __assign as assign } from "./helpers.js";
var x = { id: app_id, ...app_info, name: value };
"#;
    assert_eq_normalized(
        &render_rule(input, |_| UnObjectSpread::new_with_facts(&facts)),
        expected,
    );
}

#[test]
fn handles_cross_module_ts_assign_namespace_fact() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            ts_helper_exports: vec![TypeScriptHelperExportFact {
                exported: "__assign".into(),
                local: Some("__assign".into()),
                kind: TypeScriptHelperKind::Assign,
            }],
            ..Default::default()
        },
    );

    let input = r#"
import * as helpers from "./helpers.js";
var x = helpers.__assign({ id: app_id }, app_info, { name: value });
"#;
    let expected = r#"
import * as helpers from "./helpers.js";
var x = { id: app_id, ...app_info, name: value };
"#;
    assert_eq_normalized(
        &render_rule(input, |_| UnObjectSpread::new_with_facts(&facts)),
        expected,
    );
}

#[test]
fn handles_cross_module_default_object_helper_member_fact() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            default_object_helper_exports: vec![HelperExportFact {
                exported: "_".into(),
                local: Some("spread".into()),
                kind: HelperKind::ObjectSpread,
            }],
            ..Default::default()
        },
    );

    let input = r#"
import helpers from "./helpers.js";
var x = helpers._({}, app_info, { name: value });
"#;
    let expected = r#"
import helpers from "./helpers.js";
var x = { ...app_info, name: value };
"#;
    assert_eq_normalized(
        &render_rule(input, |_| UnObjectSpread::new_with_facts(&facts)),
        expected,
    );
}

#[test]
fn handles_nested_babel_pattern() {
    // Babel generates nested _objectSpread2 calls, each with {} as first arg:
    // _objectSpread2(_objectSpread2({}, a), {}, { b: 1 })
    let input = r#"
var _objectSpread2 = require("@babel/runtime/helpers/objectSpread2");
var x = _objectSpread2({}, a, { b: 1 });
"#;
    let expected = r#"
const x = { ...a, b: 1 };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_esm_object_spread() {
    let input = r#"
var _objectSpread2 = require("@babel/runtime/helpers/esm/objectSpread2");
var x = _objectSpread2({}, y);
"#;
    let expected = r#"
const x = { ...y };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_fallback_object_spread() {
    let input = r#"
var _objectSpread = require("@babel/runtime/helpers/objectSpread");
var x = _objectSpread({}, y);
"#;
    let expected = r#"
const x = { ...y };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_non_empty_first_arg() {
    // Both _extends and _objectSpread2 mutate their first arg.
    // Non-empty first arg must be preserved.
    let input = r#"
var _objectSpread2 = require("@babel/runtime/helpers/objectSpread2");
var x = _objectSpread2(target, { a: 1 });
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn extends_preserves_non_empty_target() {
    let input = r#"
var _extends = require("@babel/runtime/helpers/extends");
var x = _extends(target, source);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn removes_helper_declaration() {
    let input = r#"
var _objectSpread2 = require("@babel/runtime/helpers/objectSpread2");
var x = _objectSpread2({}, y);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

// ---------------------------------------------------------------------------
// Body-shape detection: inlined helper forms
// ---------------------------------------------------------------------------

#[test]
fn detects_inlined_extends() {
    let input = r#"
function _extends() {
    _extends = Object.assign || function(target) {
        for (var i = 1; i < arguments.length; i++) {
            var source = arguments[i];
            for (var key in source) {
                if (Object.prototype.hasOwnProperty.call(source, key)) {
                    target[key] = source[key];
                }
            }
        }
        return target;
    };
    return _extends.apply(this, arguments);
}
var x = _extends({}, obj1, obj2);
"#;
    let expected = r#"
const x = { ...obj1, ...obj2 };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_babel_extends_bind_apply_null() {
    let input = r#"
function _extends() {
    return _extends = Object.assign ? Object.assign.bind() : function(n) {
        for (var e = 1; e < arguments.length; e++) {
            var t = arguments[e];
            for (var r in t) {
                ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]);
            }
        }
        return n;
    }, _extends.apply(null, arguments);
}
const out = _extends({}, app_info, { name: value }, base_info);
use(out);
"#;
    let expected = r#"
const out = { ...app_info, name: value, ...base_info };
use(out);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_fresh_non_empty_helper_target() {
    let input = r#"
var _extends = require("@babel/runtime/helpers/extends");
var x = _extends({ id: app_id }, app_info);
"#;
    let expected = r#"
const x = { id: app_id, ...app_info };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_nested_babel_object_spread_with_fresh_target() {
    let input = r#"
function _objectSpread(e) {
    for (var r = 1; r < arguments.length; r++) {
        var t = null != arguments[r] ? arguments[r] : {};
        r % 2 ? ownKeys(Object(t), !0).forEach(function(r) {
            _defineProperty(e, r, t[r]);
        }) : Object.getOwnPropertyDescriptors ? Object.defineProperties(e, Object.getOwnPropertyDescriptors(t)) : ownKeys(Object(t)).forEach(function(r) {
            Object.defineProperty(e, r, Object.getOwnPropertyDescriptor(t, r));
        });
    }
    return e;
}
const out = _objectSpread(_objectSpread({}, app_info), {}, { name: value }, base_info);
use(out);
"#;
    let expected = r#"
const out = { ...app_info, name: value, ...base_info };
use(out);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_ts_assign_helper() {
    let input = r#"
var __assign = (this && this.__assign) || function () {
    __assign = Object.assign || function(t) {
        for (var s, i = 1, n = arguments.length; i < n; i++) {
            s = arguments[i];
            for (var p in s) if (Object.prototype.hasOwnProperty.call(s, p))
                t[p] = s[p];
        }
        return t;
    };
    return __assign.apply(this, arguments);
};
var out = __assign(__assign({ id: app_id }, app_info), { name: value });
use(out);
"#;
    let expected = r#"
const out = { id: app_id, ...app_info, name: value };
use(out);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_swc_object_spread_helpers() {
    let input = r#"
function _define_property(obj, key, value) {
    if (key in obj) {
        Object.defineProperty(obj, key, { value: value, enumerable: true, configurable: true, writable: true });
    } else {
        obj[key] = value;
    }
    return obj;
}
function _object_spread(target) {
    for (var i = 1; i < arguments.length; i++) {
        var source = arguments[i] != null ? arguments[i] : {};
        var ownKeys = Object.keys(source);
        if (typeof Object.getOwnPropertySymbols === "function") {
            ownKeys = ownKeys.concat(Object.getOwnPropertySymbols(source).filter(function(sym) {
                return Object.getOwnPropertyDescriptor(source, sym).enumerable;
            }));
        }
        ownKeys.forEach(function(key) {
            _define_property(target, key, source[key]);
        });
    }
    return target;
}
function ownKeys(object, enumerableOnly) {
    var keys = Object.keys(object);
    if (Object.getOwnPropertySymbols) {
        var symbols = Object.getOwnPropertySymbols(object);
        if (enumerableOnly) {
            symbols = symbols.filter(function(sym) {
                return Object.getOwnPropertyDescriptor(object, sym).enumerable;
            });
        }
        keys.push.apply(keys, symbols);
    }
    return keys;
}
function _object_spread_props(target, source) {
    source = source != null ? source : {};
    if (Object.getOwnPropertyDescriptors) {
        Object.defineProperties(target, Object.getOwnPropertyDescriptors(source));
    } else {
        ownKeys(Object(source)).forEach(function(key) {
            Object.defineProperty(target, key, Object.getOwnPropertyDescriptor(source, key));
        });
    }
    return target;
}
var out = _object_spread(_object_spread_props(_object_spread({}, app_info), { name: value }), base_info);
use(out);
"#;
    let expected = r#"
const out = { ...app_info, name: value, ...base_info };
use(out);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn removes_minified_babel_object_spread_helper_dependencies() {
    let input = r#"
function l(e) {
    l = typeof Symbol === "function" && typeof Symbol.iterator === "symbol" ? function(e) {
        return typeof e;
    } : function(e) {
        return e && typeof Symbol === "function" && e.constructor === Symbol && e !== Symbol.prototype ? "symbol" : typeof e;
    };
    return l(e);
}
function a(e, t) {
    var r = Object.keys(e);
    if (Object.getOwnPropertySymbols) {
        var n = Object.getOwnPropertySymbols(e);
        if (t) {
            n = n.filter(function(t) {
                return Object.getOwnPropertyDescriptor(e, t).enumerable;
            });
        }
        r.push.apply(r, n);
    }
    return r;
}
function i(e, t, r) {
    t = function(e) {
        var t = function(e) {
            if (l(e) != "object" || !e) {
                return e;
            }
            var t = e[Symbol.toPrimitive];
            if (t !== undefined) {
                var r = t.call(e, "string");
                if (l(r) != "object") {
                    return r;
                }
                throw new TypeError("@@toPrimitive must return a primitive value.");
            }
            return String(e);
        }(e);
        return l(t) == "symbol" ? t : t + "";
    }(t);
    if (t in e) {
        Object.defineProperty(e, t, {
            value: r,
            enumerable: true,
            configurable: true,
            writable: true
        });
    } else {
        e[t] = r;
    }
    return e;
}
function o(e) {
    for (var t = 1; t < arguments.length; t++) {
        var r = arguments[t] != null ? arguments[t] : {};
        if (t % 2) {
            a(Object(r), true).forEach(function(t) {
                i(e, t, r[t]);
            });
        } else if (Object.getOwnPropertyDescriptors) {
            Object.defineProperties(e, Object.getOwnPropertyDescriptors(r));
        } else {
            a(Object(r)).forEach(function(t) {
                Object.defineProperty(e, t, Object.getOwnPropertyDescriptor(r, t));
            });
        }
    }
    return e;
}
var out = o({}, app_info, { name: value });
use(out);
"#;
    let expected = r#"
var out = { ...app_info, name: value };
use(out);
"#;

    assert_eq_normalized(&render_rule(input, |_| UnObjectSpread::new()), expected);
}

#[test]
fn preserves_dependencies_when_object_spread_helper_call_remains() {
    let input = r#"
function ownKeys(object, enumerableOnly) {
    var keys = Object.keys(object);
    if (Object.getOwnPropertySymbols) {
        var symbols = Object.getOwnPropertySymbols(object);
        if (enumerableOnly) {
            symbols = symbols.filter(function(sym) {
                return Object.getOwnPropertyDescriptor(object, sym).enumerable;
            });
        }
        keys.push.apply(keys, symbols);
    }
    return keys;
}
function _objectSpread2(target) {
    for (var i = 1; i < arguments.length; i++) {
        var source = arguments[i] != null ? arguments[i] : {};
        i % 2 ? ownKeys(Object(source), true).forEach(function(key) {
            Object.defineProperty(target, key, {
                value: source[key],
                enumerable: true,
                configurable: true,
                writable: true
            });
        }) : Object.getOwnPropertyDescriptors ? Object.defineProperties(target, Object.getOwnPropertyDescriptors(source)) : ownKeys(Object(source)).forEach(function(key) {
            Object.defineProperty(target, key, Object.getOwnPropertyDescriptor(source, key));
        });
    }
    return target;
}
var out = _objectSpread2(target, source);
use(out);
"#;

    let output = render_rule(input, |_| UnObjectSpread::new());

    assert!(
        output.contains("function ownKeys"),
        "retained object spread helper must keep ownKeys dependency:\n{output}"
    );
    assert!(
        output.contains("function _objectSpread2"),
        "untransformed object spread helper should remain:\n{output}"
    );
}

#[test]
fn detects_esbuild_object_spread_helpers() {
    let input = r#"
var __defProp = Object.defineProperty;
var __defProps = Object.defineProperties;
var __getOwnPropDescs = Object.getOwnPropertyDescriptors;
var __getOwnPropSymbols = Object.getOwnPropertySymbols;
var __hasOwnProp = Object.prototype.hasOwnProperty;
var __propIsEnum = Object.prototype.propertyIsEnumerable;
var __defNormalProp = (obj, key, value) => key in obj ? __defProp(obj, key, { enumerable: true, configurable: true, writable: true, value }) : obj[key] = value;
var __spreadValues = (a, b) => {
    for (var prop in b || (b = {})) if (__hasOwnProp.call(b, prop)) __defNormalProp(a, prop, b[prop]);
    if (__getOwnPropSymbols) for (var prop of __getOwnPropSymbols(b)) {
        if (__propIsEnum.call(b, prop)) __defNormalProp(a, prop, b[prop]);
    }
    return a;
};
var __spreadProps = (a, b) => __defProps(a, __getOwnPropDescs(b));
const out = __spreadValues(__spreadProps(__spreadValues({}, app_info), { name: value }), base_info);
use(out);
"#;
    let expected = r#"
const out = { ...app_info, name: value, ...base_info };
use(out);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn ignores_esbuild_aliases_of_shadowed_object_binding() {
    let input = r#"
const Object = fake();
var __defProps = Object.defineProperties, __getOwnPropDescs = Object.getOwnPropertyDescriptors;
var copyDescs = (a, b) => __defProps(a, __getOwnPropDescs(b));
use(copyDescs({}, source));
"#;
    let output = render_rule(input, UnObjectSpread::new_with_mark);
    assert_eq_normalized(&output, input);
}

#[test]
fn detects_terser_inlined_esbuild_spread_values() {
    let input = r#"
var __defProp = Object.defineProperty;
var __getOwnPropSymbols = Object.getOwnPropertySymbols;
var __hasOwnProp = Object.prototype.hasOwnProperty;
var __propIsEnum = Object.prototype.propertyIsEnumerable;
var __defNormalProp = (obj, key, value) => key in obj ? __defProp(obj, key, { enumerable: true, configurable: true, writable: true, value }) : obj[key] = value;
var __spreadValues;
const out = ((a, b) => {
    for (var prop in b || (b = {})) __hasOwnProp.call(b, prop) && __defNormalProp(a, prop, b[prop]);
    if (__getOwnPropSymbols) for (var prop of __getOwnPropSymbols(b)) __propIsEnum.call(b, prop) && __defNormalProp(a, prop, b[prop]);
    return a;
})({ id: app_id }, app_info);
use(out);
"#;
    let expected = r#"
const out = { id: app_id, ...app_info };
use(out);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_terser_mangled_inlined_esbuild_spread_values() {
    // Produced by esbuild ES2017 output minified with Terser compress+mangle.
    let input = r#"
var e=Object.defineProperty,r=Object.getOwnPropertySymbols,t=Object.prototype.hasOwnProperty,o=Object.prototype.propertyIsEnumerable,a=(r,t,o)=>t in r?e(r,t,{enumerable:!0,configurable:!0,writable:!0,value:o}):r[t]=o,p;const n=((e,p)=>{for(var n in p||(p={}))t.call(p,n)&&a(e,n,p[n]);if(r)for(var n of r(p))o.call(p,n)&&a(e,n,p[n]);return e})({id:app_id},app_info);use(n);
"#;
    let expected = r#"
const n = { id: app_id, ...app_info };
use(n);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_terser_inlined_esbuild_spread_props() {
    let input = r#"
var __defProp = Object.defineProperty;
var __defProps = Object.defineProperties;
var __getOwnPropDescs = Object.getOwnPropertyDescriptors;
var __getOwnPropSymbols = Object.getOwnPropertySymbols;
var __hasOwnProp = Object.prototype.hasOwnProperty;
var __propIsEnum = Object.prototype.propertyIsEnumerable;
var __defNormalProp = (obj, key, value) => key in obj ? __defProp(obj, key, { enumerable: true, configurable: true, writable: true, value }) : obj[key] = value;
var __spreadValues = (a, b) => {
    for (var prop in b || (b = {})) __hasOwnProp.call(b, prop) && __defNormalProp(a, prop, b[prop]);
    if (__getOwnPropSymbols) for (var prop of __getOwnPropSymbols(b)) __propIsEnum.call(b, prop) && __defNormalProp(a, prop, b[prop]);
    return a;
};
var __spreadProps;
const out = __spreadValues(((a, b) => __defProps(a, __getOwnPropDescs(b)))(__spreadValues({}, app_info), { name: value }), base_info);
use(out);
"#;
    let expected = r#"
const out = { ...app_info, name: value, ...base_info };
use(out);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_mangled_esbuild_object_spread_helpers() {
    let input = r#"
var e = Object.defineProperty,
    r = Object.defineProperties,
    t = Object.getOwnPropertyDescriptors,
    o = Object.getOwnPropertySymbols,
    n = Object.prototype.hasOwnProperty,
    l = Object.prototype.propertyIsEnumerable,
    i = (r, t, o) => t in r ? e(r, t, { enumerable: true, configurable: true, writable: true, value: o }) : r[t] = o,
    s = (e, r) => {
        for (var t in r || (r = {})) n.call(r, t) && i(e, t, r[t]);
        if (o) for (var t of o(r)) l.call(r, t) && i(e, t, r[t]);
        return e;
    },
    c = (e, o) => r(e, t(o));
const out = c(s({}, rest), { session });
use(out);
"#;
    let expected = r#"
const out = { ...rest, session };
use(out);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_terser_mangled_esbuild_spread_values_and_props_from_matrix() {
    // Produced by esbuild ES2017 output minified with Terser compress+mangle.
    let input = r#"
var e=Object.defineProperty,r=Object.defineProperties,t=Object.getOwnPropertyDescriptors,o=Object.getOwnPropertySymbols,a=Object.prototype.hasOwnProperty,n=Object.prototype.propertyIsEnumerable,p=(r,t,o)=>t in r?e(r,t,{enumerable:!0,configurable:!0,writable:!0,value:o}):r[t]=o,b=(e,r)=>{for(var t in r||(r={}))a.call(r,t)&&p(e,t,r[t]);if(o)for(var t of o(r))n.call(r,t)&&p(e,t,r[t]);return e},c;const i=b(((e,o)=>r(e,t(o)))(b({},app_info),{name:value}),base_info);use(i);
"#;
    let expected = r#"
const i = { ...app_info, name: value, ...base_info };
use(i);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_minified_extends() {
    let input = r#"
function n() {
    return n = Object.assign || function(e) {
        for (var t = 1; t < arguments.length; t++) {
            var r = arguments[t];
            for (var o in r)
                Object.prototype.hasOwnProperty.call(r, o) && (e[o] = r[o]);
        }
        return e;
    }, n.apply(this, arguments);
}
var x = n({}, a, b);
"#;
    let expected = r#"
const x = { ...a, ...b };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_inlined_object_spread2() {
    let input = r#"
function _objectSpread2(target) {
    for (var i = 1; i < arguments.length; i++) {
        var source = null != arguments[i] ? arguments[i] : {};
        i % 2 ? ownKeys(Object(source), !0).forEach(function(key) {
            Object.defineProperty(target, key, { value: source[key], enumerable: true, configurable: true, writable: true });
        }) : Object.getOwnPropertyDescriptors ? Object.defineProperties(target, Object.getOwnPropertyDescriptors(source)) : ownKeys(Object(source)).forEach(function(key) {
            Object.defineProperty(target, key, Object.getOwnPropertyDescriptor(source, key));
        });
    }
    return target;
}
var x = _objectSpread2({}, y);
"#;
    let expected = r#"
const x = { ...y };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_inline_object_spread_callee() {
    let input = r#"
var x = (function(target) {
    for (var i = 1; i < arguments.length; i++) {
        var source = null != arguments[i] ? arguments[i] : {};
        i % 2 ? ownKeys(Object(source), true).forEach(function(key) {
            target[key] = source[key];
        }) : Object.getOwnPropertyDescriptors ? Object.defineProperties(target, Object.getOwnPropertyDescriptors(source)) : ownKeys(Object(source)).forEach(function(key) {
            Object.defineProperty(target, key, Object.getOwnPropertyDescriptor(source, key));
        });
    }
    return target;
})({ cursor: pointer }, padding && { padding: padding });
"#;
    let expected = r#"
var x = { cursor: pointer, ...padding && { padding: padding } };
"#;

    assert_eq_normalized(&render_rule(input, |_| UnObjectSpread::new()), expected);
}

#[test]
fn inline_object_spread_callee_preserves_extra_target_write_side_effects() {
    let input = r#"
var x = (function(target) {
    for (var i = 1; i < arguments.length; i++) {
        var source = arguments[i] != null ? arguments[i] : {};
        Object.getOwnPropertyDescriptors ? Object.defineProperties(target, Object.getOwnPropertyDescriptors(source)) : Object.keys(source).forEach(function(key) {
            target[key] = source[key];
        });
    }
    target.extra = expensive();
    return target;
})({}, y);
"#;

    let output = render_rule(input, |_| UnObjectSpread::new());

    assert!(
        output.contains("expensive()"),
        "retained inline helper call must keep side effects:\n{output}"
    );
    assert!(
        !output.contains("var x = { ...y };"),
        "inline helper with target side effects must not be collapsed:\n{output}"
    );
}

#[test]
fn detects_var_assigned_extends() {
    let input = r#"
var _extends = function() {
    _extends = Object.assign || function(target) {
        for (var i = 1; i < arguments.length; i++) {
            var source = arguments[i];
            for (var key in source) {
                if (Object.prototype.hasOwnProperty.call(source, key)) {
                    target[key] = source[key];
                }
            }
        }
        return target;
    };
    return _extends.apply(this, arguments);
};
var x = _extends({}, a);
"#;
    let expected = r#"
const x = { ...a };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn inlined_extends_preserves_non_empty_target() {
    let input = r#"
function _extends() {
    _extends = Object.assign || function(target) {
        for (var i = 1; i < arguments.length; i++) {
            var source = arguments[i];
            for (var key in source) {
                if (Object.prototype.hasOwnProperty.call(source, key)) {
                    target[key] = source[key];
                }
            }
        }
        return target;
    };
    return _extends.apply(this, arguments);
}
var x = _extends(target, source);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn detects_object_assign_or_polyfill_as_extends() {
    // Babel 6 / pre-evaluated form: var _extends = Object.assign || function(target) { ... }
    let input = r#"
var f = Object.assign || function(e) {
    for (var t = 1; t < arguments.length; t++) {
        var n = arguments[t];
        for (var r in n) {
            if (Object.prototype.hasOwnProperty.call(n, r)) {
                e[r] = n[r];
            }
        }
    }
    return e;
};
var x = f({}, a, { b: 1 });
"#;
    let expected = r#"
const x = { ...a, b: 1 };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn object_assign_or_polyfill_preserves_non_empty_target() {
    let input = r#"
var f = Object.assign || function(e) {
    for (var t = 1; t < arguments.length; t++) {
        var n = arguments[t];
        for (var r in n) {
            if (Object.prototype.hasOwnProperty.call(n, r)) {
                e[r] = n[r];
            }
        }
    }
    return e;
};
var x = f(target, source);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn no_false_positive_zero_param_unrelated() {
    // A zero-param function that doesn't match extends shape
    let input = r#"
function init() {
    return Object.create(null);
}
var x = init();
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn no_false_positive_descriptor_copy_utility() {
    // A 1-param function using arguments + Object.defineProperty that isn't objectSpread
    let input = r#"
function copyProps(target) {
    for (var i = 1; i < arguments.length; i++) {
        var source = arguments[i];
        Object.defineProperty(target, "meta", { value: source, enumerable: false });
    }
    return target;
}
var x = copyProps({}, source);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}
