mod common;
use common::{assert_eq_normalized, render};

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
