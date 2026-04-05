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
    assert!(output.contains("_objectSpread2"), "should not transform with real target");
}

#[test]
fn extends_preserves_non_empty_target() {
    let input = r#"
var _extends = require("@babel/runtime/helpers/extends");
var x = _extends(target, source);
"#;
    let output = render(input);
    assert!(output.contains("_extends"), "should not transform _extends with real target");
}

#[test]
fn removes_helper_declaration() {
    let input = r#"
var _objectSpread2 = require("@babel/runtime/helpers/objectSpread2");
var x = _objectSpread2({}, y);
"#;
    let output = render(input);
    assert!(!output.contains("_objectSpread2"), "helper should be removed");
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
    assert!(output.contains("_extends"), "should not transform with real target");
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
    assert!(output.contains("init"), "should not detect unrelated function as extends");
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
    assert!(output.contains("copyProps"), "should not detect descriptor utility as objectSpread");
}
