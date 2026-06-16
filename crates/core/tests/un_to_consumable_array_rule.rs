mod common;
use common::{assert_eq_normalized, render, render_rule};
use wakaru_core::facts::{
    ModuleFacts, ModuleFactsMap, TypeScriptHelperExportFact, TypeScriptHelperKind,
};
use wakaru_core::rules::UnToConsumableArray;

#[test]
fn replaces_to_consumable_array_with_spread() {
    let input = r#"
var _toConsumableArray = require("@babel/runtime/helpers/toConsumableArray");
var x = _toConsumableArray(a);
"#;
    let expected = r#"
const x = [...a];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_esm_import() {
    let input = r#"
var _toConsumableArray = require("@babel/runtime/helpers/esm/toConsumableArray");
var x = _toConsumableArray(arr);
"#;
    let expected = r#"
const x = [...arr];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_babel_runtime_esm_import() {
    let input = r#"
import _toConsumableArray from "@babel/runtime/helpers/toConsumableArray";
var x = _toConsumableArray(items);
"#;
    let expected = r#"
const x = [...items];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_helper_when_untransformed_calls_remain() {
    let input = r#"
var _toConsumableArray = require("@babel/runtime/helpers/toConsumableArray");
var x = _toConsumableArray(a, b);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn removes_helper_declaration() {
    let input = r#"
var _toConsumableArray = require("@babel/runtime/helpers/toConsumableArray");
var x = _toConsumableArray(a);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

// ---------------------------------------------------------------------------
// Body-shape detection: inlined helper forms
// ---------------------------------------------------------------------------

#[test]
fn detects_inlined_babel6_form() {
    // Babel 6: Array.isArray + Array.from
    let input = r#"
function _toConsumableArray(arr) {
    if (Array.isArray(arr)) {
        for (var i = 0, arr2 = Array(arr.length); i < arr.length; i++) arr2[i] = arr[i];
        return arr2;
    } else {
        return Array.from(arr);
    }
}
var x = _toConsumableArray(items);
"#;
    let expected = r#"
const x = [...items];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_inlined_babel7_form() {
    // Babel 7+: logical-OR chain of sub-helper calls.
    // The module must also contain a sub-helper with Array.isArray/Array.from
    // for the OR-chain to be accepted (prevents false positives).
    // The now-dead `_arrayWithoutHoles` sub-helper is removed transitively once
    // `_toConsumableArray` is folded and removed.
    let input = r#"
function _arrayWithoutHoles(arr) {
    if (Array.isArray(arr)) return _arrayLikeToArray(arr);
}
function _toConsumableArray(arr) {
    return _arrayWithoutHoles(arr) || _iterableToArray(arr) || _unsupportedIterableToArray(arr) || _nonIterableSpread();
}
var x = _toConsumableArray(items);
"#;
    let expected = r#"
const x = [...items];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_minified_to_consumable_array() {
    // Minified: short name, same structure
    let input = r#"
function a(e) {
    if (Array.isArray(e)) {
        for (var t = 0, n = new Array(e.length); t < e.length; t++) n[t] = e[t];
        return n;
    } else {
        return Array.from(e);
    }
}
var x = a(items);
"#;
    let expected = r#"
const x = [...items];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_var_assigned_to_consumable_array() {
    // The now-dead `_arrayWithoutHoles` sub-helper is removed transitively once
    // the `_toConsumableArray` function-expression binding is folded and removed.
    let input = r#"
function _arrayWithoutHoles(arr) {
    if (Array.isArray(arr)) return _arrayLikeToArray(arr);
}
var _toConsumableArray = function(arr) {
    return _arrayWithoutHoles(arr) || _iterableToArray(arr) || _unsupportedIterableToArray(arr) || _nonIterableSpread();
};
var x = _toConsumableArray(items);
"#;
    let expected = r#"
const x = [...items];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn no_false_positive_single_param_unrelated() {
    // A single-param function that doesn't match the helper shape
    let input = r#"
function transform(arr) {
    return arr.map(function(x) { return x + 1; });
}
var x = transform(items);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn no_false_positive_or_chain_fallback() {
    // A normal fallback pipeline that happens to be a 1-arg OR chain
    let input = r#"
function choose(arr) {
    return parse(arr) || normalize(arr) || fallback(arr) || die();
}
var x = choose(items);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn unwraps_typescript_spread_array_helper() {
    let input = r#"
var __spreadArray = (this && this.__spreadArray) || function (to, from, pack) {
    return to.concat(from);
};
var out = __spreadArray(__spreadArray([head], items, true), [tail], false);
"#;
    let expected = r#"
const out = [head, ...items, tail];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_tslib_named_spread_array_import() {
    let input = r#"
import { __spreadArray } from "tslib";
var out = __spreadArray([head], items, true);
"#;
    let expected = r#"
const out = [head, ...items];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_cross_module_ts_spread_array_helper_fact() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            ts_helper_exports: vec![TypeScriptHelperExportFact {
                exported: "__spreadArray".into(),
                local: Some("__spreadArray".into()),
                kind: TypeScriptHelperKind::SpreadArray,
            }],
            ..Default::default()
        },
    );

    let input = r#"
import { __spreadArray as spread } from "./helpers.js";
var out = spread([head], items, true);
"#;
    let expected = r#"
import { __spreadArray as spread } from "./helpers.js";
var out = [head, ...items];
"#;
    assert_eq_normalized(
        &render_rule(input, |_| UnToConsumableArray::new_with_facts(&facts)),
        expected,
    );
}

#[test]
fn unwraps_cross_module_ts_spread_array_namespace_fact() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            ts_helper_exports: vec![TypeScriptHelperExportFact {
                exported: "__spreadArray".into(),
                local: Some("__spreadArray".into()),
                kind: TypeScriptHelperKind::SpreadArray,
            }],
            ..Default::default()
        },
    );

    let input = r#"
import * as helpers from "./helpers.js";
var out = helpers.__spreadArray([head], items, true);
"#;
    let expected = r#"
import * as helpers from "./helpers.js";
var out = [head, ...items];
"#;
    assert_eq_normalized(
        &render_rule(input, |_| UnToConsumableArray::new_with_facts(&facts)),
        expected,
    );
}

#[test]
fn unwraps_tslib_namespace_spread_array_require() {
    let input = r#"
var tslib_1 = require("tslib");
var out = tslib_1.__spreadArray([head], items, true);
"#;
    let expected = r#"
import tslib_1 from "tslib";
const out = [head, ...items];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_tslib_namespace_spread_array_alias() {
    let input = r#"
var tslib_1 = require("tslib");
var spread = tslib_1.__spreadArray;
var out = spread([head], items, true);
"#;
    let expected = r#"
import tslib_1 from "tslib";
const out = [head, ...items];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_nested_typescript_spread_array_helper() {
    let input = r#"
var __spreadArray = (this && this.__spreadArray) || function (to, from, pack) {
    return to.concat(from);
};
var out = __spreadArray(__spreadArray(__spreadArray([], left_items, true), [middle], false), right_items, true);
"#;
    let expected = r#"
const out = [...left_items, middle, ...right_items];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_non_helper_named_spread_array() {
    let input = r#"
var __spreadArray = customSpreadArray;
var out = __spreadArray([head], items, true);
"#;
    let output = render(input);
    assert!(
        output.contains("customSpreadArray([") && !output.contains("...items"),
        "non-helper __spreadArray call should be preserved as a call: {output}"
    );
}

#[test]
fn handles_swc_external_to_consumable_array_import() {
    let input = r#"
import { _ as _to_consumable_array } from "@swc/helpers/_/_to_consumable_array";
var x = _to_consumable_array(items);
"#;
    let expected = r#"
const x = [...items];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_swc_external_to_consumable_array_unaliased_import() {
    let input = r#"
import { _ } from "@swc/helpers/_/_to_consumable_array";
var x = _(items);
"#;
    let expected = r#"
const x = [...items];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn folds_swc_external_to_consumable_array_concat_spread() {
    let input = r#"
import { _ as _to_consumable_array } from "@swc/helpers/_/_to_consumable_array";
var out = [head].concat(_to_consumable_array(items), [tail]);
"#;
    let expected = r#"
const out = [head, ...items, tail];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_maybe_array_like_to_consumable_array() {
    let input = r#"
function _maybeArrayLike(r, a, e) { if (a && !Array.isArray(a) && "number" == typeof a.length) { var y = a.length; return _arrayLikeToArray(a, void 0 !== e && e < y ? e : y); } return r(a, e); }
function _toConsumableArray(r) { return _arrayWithoutHoles(r) || _iterableToArray(r) || _unsupportedIterableToArray(r) || _nonIterableSpread(); }
function _nonIterableSpread() { throw new TypeError("Invalid attempt to spread non-iterable instance."); }
function _unsupportedIterableToArray(r, a) { if (r) { if ("string" == typeof r) return _arrayLikeToArray(r, a); var t = {}.toString.call(r).slice(8, -1); return "Object" === t && r.constructor && (t = r.constructor.name), "Map" === t || "Set" === t ? Array.from(r) : "Arguments" === t || /^(?:Ui|I)nt(?:8|16|32)(?:Clamped)?Array$/.test(t) ? _arrayLikeToArray(r, a) : void 0; } }
function _iterableToArray(r) { if ("undefined" != typeof Symbol && null != r[Symbol.iterator] || null != r["@@iterator"]) return Array.from(r); }
function _arrayWithoutHoles(r) { if (Array.isArray(r)) return _arrayLikeToArray(r); }
function _arrayLikeToArray(r, a) { (null == a || a > r.length) && (a = r.length); for (var e = 0, n = Array(a); e < a; e++) n[e] = r[e]; return n; }
const out = [head].concat(_maybeArrayLike(_toConsumableArray, items), [tail]);
use(out);
"#;
    let expected = r#"
const out = [head, ...items, tail];
use(out);
"#;
    assert_eq_normalized(&render(input), expected);
}
