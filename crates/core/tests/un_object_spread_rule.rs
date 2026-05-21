mod common;
use common::{assert_eq_normalized, render};

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
