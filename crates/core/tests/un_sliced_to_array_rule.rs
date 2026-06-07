mod common;
use common::{assert_eq_normalized, render, render_pipeline_until};
use wakaru_core::facts::{HelperExportFact, HelperKind, ModuleFacts, ModuleFactsMap};
use wakaru_core::rules::UnSlicedToArray;
use wakaru_core::RewriteLevel;

#[test]
fn unwraps_sliced_to_array() {
    let input = r#"
var _slicedToArray = require("@babel/runtime/helpers/slicedToArray");
var _ref = _slicedToArray(a, 2);
var name = _ref[0];
var value = _ref[1];
"#;
    // slicedToArray just unwraps; destructuring reconstruction is done by downstream rules
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn handles_zero_length() {
    let input = r#"
var _slicedToArray = require("@babel/runtime/helpers/slicedToArray");
var _ref = _slicedToArray(a, 0);
"#;
    let expected = r#"
var [] = a;
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_elision_only_slice_side_effects() {
    let input = r#"
function _arrayWithHoles(arr) {
    if (Array.isArray(arr)) return arr;
}
function _slicedToArray(arr, i) {
    return _arrayWithHoles(arr) || _iterableToArrayLimit(arr, i) || _unsupportedIterableToArray(arr, i) || _nonIterableRest();
}
function read(iter) {
    var _ref = _slicedToArray(iter, 1);
    return done();
}
"#;
    let expected = r#"
function read(iter) {
    var [,] = iter;
    return done();
}
"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnSlicedToArray"), expected);
}

#[test]
fn handles_esm_import() {
    let input = r#"
var _slicedToArray = require("@babel/runtime/helpers/esm/slicedToArray");
var _ref = _slicedToArray(expr, 3);
var x = _ref[0];
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn handles_babel_runtime_esm_import() {
    let input = r#"
import _slicedToArray from "@babel/runtime/helpers/slicedToArray";
var _ref = _slicedToArray(pair, 2);
var key = _ref[0];
var value = _ref[1];
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn handles_swc_external_helper_import() {
    let input = r#"
import { _ as _sliced_to_array } from "@swc/helpers/_/_sliced_to_array";
var _ref = _sliced_to_array(pair, 2);
var key = _ref[0];
var value = _ref[1];
"#;
    let expected = r#"
const [key, value] = pair;
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_cross_module_default_object_helper_member_fact() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            default_object_helper_exports: vec![HelperExportFact {
                exported: "_".into(),
                local: Some("sliced".into()),
                kind: HelperKind::SlicedToArray,
            }],
            ..Default::default()
        },
    );

    let input = r#"
import helpers from "./helpers.js";
var _useState = helpers._(useState(value), 2), current = _useState[0], setCurrent = _useState[1];
use(current, setCurrent);
"#;
    let expected = r#"
import helpers from "./helpers.js";
var _useState = useState(value), current = _useState[0], setCurrent = _useState[1];
use(current, setCurrent);
"#;
    assert_eq_normalized(
        &common::render_rule(input, |_| UnSlicedToArray::new_with_facts(&facts)),
        expected,
    );
}

#[test]
fn folds_cross_module_helper_assignment_group() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            default_object_helper_exports: vec![HelperExportFact {
                exported: "_".into(),
                local: Some("sliced".into()),
                kind: HelperKind::SlicedToArray,
            }],
            ..Default::default()
        },
    );

    let input = r#"
import helpers from "./helpers.js";
function Component() {
    var tuple;
    var value;
    var setter;
    value = (tuple = helpers._(React.useState(undefined), 2))[0];
    setter = tuple[1];
    setter(value);
}
"#;
    let expected = r#"
import helpers from "./helpers.js";
function Component() {
    var [value, setter] = React.useState(undefined);
    setter(value);
}
"#;
    assert_eq_normalized(
        &common::render_rule(input, |_| UnSlicedToArray::new_with_facts(&facts)),
        expected,
    );
}

#[test]
fn keeps_assignment_group_when_prior_var_temp_is_read() {
    let input = r#"
import _slicedToArray from "@babel/runtime/helpers/slicedToArray";
function Component(pair) {
    var tuple;
    var value;
    var setter;
    before(tuple);
    value = (tuple = _slicedToArray(pair, 2))[0];
    setter = tuple[1];
    setter(value);
}
"#;
    assert_eq_normalized(
        &common::render_rule(input, |_| UnSlicedToArray::new()),
        input,
    );
}

#[test]
fn keeps_assignment_group_for_prior_let_decls() {
    let input = r#"
import _slicedToArray from "@babel/runtime/helpers/slicedToArray";
function Component(pair) {
    let tuple;
    let value;
    let setter;
    value = (tuple = _slicedToArray(pair, 2))[0];
    setter = tuple[1];
    setter(value);
}
"#;
    assert_eq_normalized(
        &common::render_rule(input, |_| UnSlicedToArray::new()),
        input,
    );
}

#[test]
fn folds_helper_decl_followed_by_assignment_group() {
    let input = r#"
import _slicedToArray from "@babel/runtime/helpers/slicedToArray";
function Component() {
    var current;
    var setCurrent;
    var tuple = _slicedToArray(useState(value), 2);
    current = tuple[0];
    setCurrent = tuple[1];
    use(current, setCurrent);
}
"#;
    let expected = r#"
function Component() {
    var [current, setCurrent] = useState(value);
    use(current, setCurrent);
}
"#;
    assert_eq_normalized(
        &common::render_rule(input, |_| UnSlicedToArray::new()),
        expected,
    );
}

#[test]
fn minimal_keeps_helper_decl_followed_by_assignment_group() {
    let input = r#"
import _slicedToArray from "@babel/runtime/helpers/slicedToArray";
function Component() {
    var current;
    var setCurrent;
    var tuple = _slicedToArray(useState(value), 2);
    current = tuple[0];
    setCurrent = tuple[1];
    use(current, setCurrent);
}
"#;
    let expected = r#"
function Component() {
    var current;
    var setCurrent;
    var tuple = useState(value);
    current = tuple[0];
    setCurrent = tuple[1];
    use(current, setCurrent);
}
"#;
    assert_eq_normalized(
        &common::render_rule(input, |_| {
            UnSlicedToArray::new_with_level(RewriteLevel::Minimal)
        }),
        expected,
    );
}

#[test]
fn minimal_keeps_nested_assignment_group() {
    let input = r#"
import _slicedToArray from "@babel/runtime/helpers/slicedToArray";
function Component(pair) {
    var tuple;
    var value;
    var setter;
    value = (tuple = _slicedToArray(pair, 2))[0];
    setter = tuple[1];
    setter(value);
}
"#;
    assert_eq_normalized(
        &common::render_rule(input, |_| {
            UnSlicedToArray::new_with_level(RewriteLevel::Minimal)
        }),
        input,
    );
}

#[test]
fn folds_helper_ref_assignment_followed_by_assignment_group() {
    let input = r#"
import { _ as _sliced_to_array } from "@swc/helpers/_/_sliced_to_array";
function Component() {
    var current;
    var setCurrent;
    var ref;
    ref = _sliced_to_array(useState(value), 2);
    current = ref[0];
    setCurrent = ref[1];
    use(current, setCurrent);
}
"#;
    let expected = r#"
function Component() {
    var [current, setCurrent] = useState(value);
    use(current, setCurrent);
}
"#;
    assert_eq_normalized(
        &common::render_rule(input, |_| UnSlicedToArray::new()),
        expected,
    );
}

#[test]
fn unwraps_tslib_namespace_read_require() {
    let input = r#"
var tslib_1 = require("tslib");
var _pair = tslib_1.__read(pair, 2);
var key = _pair[0];
var value = _pair[1];
"#;
    let expected = r#"
import tslib_1 from "tslib";
const [key, value] = pair;
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_tslib_direct_read_require() {
    let input = r#"
var _pair = require("tslib").__read(pair, 2);
var key = _pair[0];
var value = _pair[1];
"#;
    let expected = r#"
const [key, value] = pair;
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn skips_invalid_arg_counts() {
    let input = r#"
var _slicedToArray = require("@babel/runtime/helpers/slicedToArray");
_slicedToArray();
_slicedToArray(a);
_slicedToArray(a, 2, 3);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn removes_helper_declaration() {
    let input = r#"
var _slicedToArray = require("@babel/runtime/helpers/slicedToArray");
var _ref = _slicedToArray(a, 2);
var name = _ref[0];
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

// ---------------------------------------------------------------------------
// Body-shape detection: inlined helper forms
// ---------------------------------------------------------------------------

#[test]
fn detects_inlined_babel6_sliced_to_array() {
    // Babel 6: references Symbol.iterator
    let input = r#"
function _slicedToArray(arr, i) {
    if (Array.isArray(arr)) {
        return arr;
    } else if (Symbol.iterator in Object(arr)) {
        var _arr = [];
        var _n = true;
        var _d = false;
        var _e = undefined;
        try {
            for (var _i = arr[Symbol.iterator](), _s; !(_n = (_s = _i.next()).done); _n = true) {
                _arr.push(_s.value);
                if (i && _arr.length === i) break;
            }
        } catch (err) { _d = true; _e = err; }
        finally { try { if (!_n && _i["return"]) _i["return"](); } finally { if (_d) throw _e; } }
        return _arr;
    } else {
        throw new TypeError("Invalid attempt to destructure non-iterable instance");
    }
}
var _ref = _slicedToArray(pair, 2);
var key = _ref[0];
var value = _ref[1];
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn detects_inlined_babel7_sliced_to_array() {
    // Babel 7+: logical-OR chain of sub-helper calls.
    // Module must also contain a sub-helper with Array.isArray for corroboration.
    let input = r#"
function _arrayWithHoles(arr) {
    if (Array.isArray(arr)) return arr;
}
function _slicedToArray(arr, i) {
    return _arrayWithHoles(arr) || _iterableToArrayLimit(arr, i) || _unsupportedIterableToArray(arr, i) || _nonIterableRest();
}
var _ref = _slicedToArray(pair, 2);
var key = _ref[0];
var value = _ref[1];
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn detects_minified_sliced_to_array() {
    let input = r#"
function c(e) {
    if (Array.isArray(e)) return e;
}
function r(e, t) {
    return c(e) || o(e, t) || s(e, t) || l();
}
var _ref = r(pair, 2);
var key = _ref[0];
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn folds_inline_sliced_helper_chain_with_temp_length() {
    let input = r#"
function read() {
    var e;
    var t;
    e = useState("counter");
    t = 2;
    var b = ((e) => {
        if (Array.isArray(e)) {
            return e;
        }
    })(e) || ((e, t) => {
        var r = e == null ? null : typeof Symbol !== "undefined" && e[Symbol.iterator] || e["@@iterator"];
        if (r != null) {
            var n;
            var o;
            var l;
            var a;
            var u = [];
            var i = true;
            var c = false;
            try {
                l = (r = r.call(e)).next;
                if (t === 0) {
                    if (Object(r) !== r) {
                        return;
                    }
                    i = false;
                } else {
                    for(; !(i = (n = l.call(r)).done) && (u.push(n.value), u.length !== t); i = true);
                }
            } catch (e) {
                c = true;
                o = e;
            } finally {
                try {
                    if (!i && r.return != null && (a = r.return(), Object(a) !== a)) {
                        return;
                    }
                } finally {
                    if (c) {
                        throw o;
                    }
                }
            }
            return u;
        }
    })(e, t) || ((e, t) => {
        if (e) {
            if (typeof e === "string") {
                return c(e, t);
            }
            var r = Object.prototype.toString.call(e).slice(8, -1);
            return r === "Map" || r === "Set" ? Array.from(e) : undefined;
        }
    })(e, t) || (() => {
        throw new TypeError("Invalid attempt to destructure non-iterable instance.");
    })();
    var p = b[0];
    var d = b[1];
    return use(p, d);
}
"#;
    let expected = r#"
function read() {
    var e;
    var t;
    e = useState("counter");
    t = 2;
    var [p, d] = e;
    return use(p, d);
}
"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnSlicedToArray"), expected);
}

#[test]
fn folds_repro_derived_inline_sliced_helper_chain_with_assigned_source() {
    let input = r#"
const pairRef = function(value) {
    if (Array.isArray(value)) {
        return value;
    }
}(pairTemp = readPair()) || function(value) {
    var iterator = value == null ? null : typeof Symbol !== "undefined" && value[Symbol.iterator] || value["@@iterator"];
    if (iterator != null) {
        return Array.from(value).slice(0, 2);
    }
}(pairTemp) || function(value) {
    if (value) {
        return Array.from(value);
    }
}(pairTemp) || function() {
    throw new TypeError("Invalid attempt to destructure non-iterable instance.");
}();
var first = pairRef[0];
var second = pairRef[1];
var pairTemp;
use(first, second);
"#;
    let expected = r#"
const [first, second] = readPair();
var pairTemp;
use(first, second);
"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnSlicedToArray"), expected);
}

#[test]
fn keeps_inline_sliced_helper_chain_when_assigned_source_temp_escapes() {
    let input = r#"
const pairRef = function(value) {
    if (Array.isArray(value)) {
        return value;
    }
}(pairTemp = readPair()) || function(value) {
    var iterator = value == null ? null : typeof Symbol !== "undefined" && value[Symbol.iterator] || value["@@iterator"];
    if (iterator != null) {
        return Array.from(value).slice(0, 2);
    }
}(pairTemp) || function(value) {
    if (value) {
        return Array.from(value);
    }
}(pairTemp) || function() {
    throw new TypeError("Invalid attempt to destructure non-iterable instance.");
}();
var first = pairRef[0];
var second = pairRef[1];
use(first, second, pairTemp);
"#;
    let output = render_pipeline_until(input, "UnSlicedToArray");
    assert_eq_normalized(&output, input);
}

#[test]
fn detects_var_assigned_sliced_to_array() {
    let input = r#"
function _arrayWithHoles(arr) {
    if (Array.isArray(arr)) return arr;
}
var _slicedToArray = function(arr, i) {
    return _arrayWithHoles(arr) || _iterableToArrayLimit(arr, i) || _unsupportedIterableToArray(arr, i) || _nonIterableRest();
};
var _ref = _slicedToArray(pair, 2);
var key = _ref[0];
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn folds_swc_sliced_to_array_declaration_group() {
    let input = r#"
function _array_like_to_array(arr, len) {
    return arr;
}
function _array_with_holes(arr) {
    if (Array.isArray(arr)) return arr;
}
function _iterable_to_array_limit(arr, i) {
    return arr;
}
function _unsupported_iterable_to_array(o, minLen) {
    return _array_like_to_array(o, minLen);
}
function _non_iterable_rest() {
    throw new TypeError("Invalid attempt to destructure non-iterable instance.");
}
function _sliced_to_array(arr, i) {
    return _array_with_holes(arr) || _iterable_to_array_limit(arr, i) || _unsupported_iterable_to_array(arr, i) || _non_iterable_rest();
}
var _useState = _sliced_to_array(useState(value), 2), current = _useState[0], setCurrent = _useState[1];
use(current, setCurrent);
"#;
    let expected = r#"
const [current, setCurrent] = useState(value);
use(current, setCurrent);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn folds_nested_sliced_to_array_statement_group() {
    let input = r#"
function _arrayWithHoles(arr) {
    if (Array.isArray(arr)) return arr;
}
function _slicedToArray(arr, i) {
    return _arrayWithHoles(arr) || _iterableToArrayLimit(arr, i) || _unsupportedIterableToArray(arr, i) || _nonIterableRest();
}
function read(pair) {
    var _ref = _slicedToArray(pair, 2);
    var key = _ref[0];
    var value = _ref[1];
    return use(key, value);
}
"#;
    let expected = r#"
function read(pair) {
    var [key, value] = pair;
    return use(key, value);
}
"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnSlicedToArray"), expected);
}

#[test]
fn recovers_array_destructured_default_parameter_from_nested_helper() {
    let input = r#"
function _arrayWithHoles(arr) {
    if (Array.isArray(arr)) return arr;
}
function _slicedToArray(arr, i) {
    return _arrayWithHoles(arr) || _iterableToArrayLimit(arr, i) || _unsupportedIterableToArray(arr, i) || _nonIterableRest();
}
function first() {
    let _ref = arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : [],
        _ref2 = _slicedToArray(_ref, 2),
        head = _ref2[0],
        _ref2$ = _ref2[1],
        second = _ref2$ === void 0 ? fallback : _ref2$;
    return use(head, second);
}
"#;
    let expected = r#"
function first([head, second = fallback] = []) {
    return use(head, second);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_ref_when_sliced_to_array_temp_is_reused() {
    let input = r#"
function _arrayWithHoles(arr) {
    if (Array.isArray(arr)) return arr;
}
function _slicedToArray(arr, i) {
    return _arrayWithHoles(arr) || _iterableToArrayLimit(arr, i) || _unsupportedIterableToArray(arr, i) || _nonIterableRest();
}
var _ref = _slicedToArray(pair, 2), key = _ref[0], value = _ref[1];
use(key, value, _ref);
"#;
    let expected = r#"
const _ref = pair;
const key = _ref[0];
const value = _ref[1];
use(key, value, _ref);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn does_not_fold_plain_ref_index_reads_without_helper_call() {
    let input = r#"
var _ref = arr;
var key = _ref[0];
var value = _ref[1];
use(key, value);
"#;
    let expected = r#"
const _ref = arr;
const key = _ref[0];
const value = _ref[1];
use(key, value);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_unreferenced_name_collision_dependency() {
    let input = r#"
var _slicedToArray = require("@babel/runtime/helpers/slicedToArray");
function _arrayWithHoles(arr) {
    return custom(arr);
}
var _ref = _slicedToArray(pair, 2);
var key = _ref[0];
var value = _ref[1];
use(key, value);
"#;
    let expected = r#"
function _arrayWithHoles(arr) {
    return custom(arr);
}
const [key, value] = pair;
use(key, value);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_dependencies_when_sliced_to_array_helper_call_remains() {
    let input = r#"
function _arrayWithHoles(arr) {
    if (Array.isArray(arr)) return arr;
}
function _slicedToArray(arr, i) {
    return _arrayWithHoles(arr) || _iterableToArrayLimit(arr, i) || _unsupportedIterableToArray(arr, i) || _nonIterableRest();
}
_slicedToArray(pair);
"#;
    let output = render(input);

    assert!(
        output.contains("function _arrayWithHoles"),
        "retained slicedToArray helper must keep _arrayWithHoles dependency:\n{output}"
    );
    assert!(
        output.contains("function _slicedToArray"),
        "untransformed slicedToArray helper should remain:\n{output}"
    );
}

#[test]
fn no_false_positive_two_param_unrelated() {
    // A two-param function that doesn't match the helper shape
    let input = r#"
function slice(arr, count) {
    return arr.slice(0, count);
}
var x = slice(items, 3);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn no_false_positive_symbol_iterator_utility() {
    // A 2-param function that references Symbol.iterator but isn't slicedToArray
    let input = r#"
function maybeIter(arr, count) {
    if (Symbol.iterator in Object(arr)) return take(arr, count);
    return arr.slice(0, count);
}
var x = maybeIter(items, 2);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn no_false_positive_or_chain_two_params() {
    // A 2-param OR chain that isn't slicedToArray
    let input = r#"
function resolve(a, b) {
    return tryFirst(a) || trySecond(a, b) || tryThird(a, b) || giveUp();
}
var x = resolve(items, 2);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}
