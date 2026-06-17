mod common;
use common::{assert_eq_normalized, render};

#[test]
fn simplifies_possible_constructor_return_call() {
    let input = r#"
function _possibleConstructorReturn(self, call) {
    if (!self) {
        throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
    }
    if (!call || typeof call != "object" && typeof call != "function") {
        return self;
    }
    return call;
}
export function Foo() {
    var x = _possibleConstructorReturn(this, Parent.call(this, args));
}
"#;
    let expected = r#"
export function Foo() {
    const x = Parent.call(this, args);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn simplifies_minified_possible_constructor_return() {
    // Minified: short names, same body shape
    let input = r#"
function d(e, t) {
    if (!e) {
        throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
    }
    if (!t || typeof t != "object" && typeof t != "function") {
        return e;
    }
    return t;
}
export function Foo() {
    var r = d(this, Parent.call(this));
    return d(r, n);
}
"#;
    let expected = r#"
export function Foo() {
    const r = Parent.call(this);
    return n;
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn removes_declaration_when_all_calls_replaced() {
    let input = r#"
function d(e, t) {
    if (!e) {
        throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
    }
    if (!t || typeof t != "object" && typeof t != "function") {
        return e;
    }
    return t;
}
var x = d(this, Parent.call(this));
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn simplifies_minified_ternary_form() {
    // Minified form: 2 statements — if-throw + return-ternary
    let input = r#"
function d(e, t) {
    if (!e) throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
    return !t || "object" != typeof t && "function" != typeof t ? e : t;
}
var x = d(this, Parent.call(this));
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn handles_multiple_pcr_helpers_in_same_module() {
    // Module-24 has multiple possibleConstructorReturn helpers (d, m, E, ...)
    let input = r#"
function d(e, t) {
    if (!e) {
        throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
    }
    if (!t || typeof t != "object" && typeof t != "function") {
        return e;
    }
    return t;
}
function m(e, t) {
    if (!e) {
        throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
    }
    if (!t || typeof t != "object" && typeof t != "function") {
        return e;
    }
    return t;
}
var x = d(this, Parent.call(this));
var y = m(this, Other.call(this));
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn removes_babel_runtime_import_possible_constructor_return() {
    let input = r#"
var _possibleConstructorReturn = require("@babel/runtime/helpers/possibleConstructorReturn");
export function Foo() {
    var x = _possibleConstructorReturn(this, Parent.call(this, args));
}
"#;
    let expected = r#"
export function Foo() {
    const x = Parent.call(this, args);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn removes_swc_external_possible_constructor_return() {
    let input = r#"
import { _ as _possible_constructor_return } from "@swc/helpers/_/_possible_constructor_return";
export function Foo() {
    var x = _possible_constructor_return(this, Parent.call(this, args));
}
"#;
    let expected = r#"
export function Foo() {
    const x = Parent.call(this, args);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_non_matching_functions() {
    let input = r#"
function validate(self, call) {
    if (!self) {
        throw new Error("invalid self");
    }
    return call;
}
var x = validate(obj, fn());
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}
