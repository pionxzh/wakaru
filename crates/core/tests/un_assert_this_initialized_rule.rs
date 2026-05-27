mod common;
use common::{assert_eq_normalized, render, render_pipeline};

#[test]
fn simplifies_assert_this_initialized_call() {
    let input = r#"
function p(e) {
    if (e === undefined) {
        throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
    }
    return e;
}
export function Foo() {
    this.method = this.method.bind(p(this));
}
"#;
    let expected = r#"
export function Foo() {
    this.method = this.method.bind(this);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn simplifies_bang_guard_form() {
    let input = r#"
function p(e) {
    if (!e) {
        throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
    }
    return e;
}
var x = p(this);
"#;
    let expected = r#"
const x = this;
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn simplifies_nested_calls() {
    // p(p(this)) should simplify to just `this` via post-order visiting
    let input = r#"
function p(e) {
    if (e === undefined) {
        throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
    }
    return e;
}
export function Foo() {
    this.method = this.method.bind(p(p(this)));
}
"#;
    let expected = r#"
export function Foo() {
    this.method = this.method.bind(this);
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn simplifies_calls_exposed_by_class_recovery() {
    let input = r#"
function p(e) {
    if (void 0 === e) {
        throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
    }
    return e;
}
var Foo = (function(n) {
    function u() {
        var o;
        return (o = n.call(this) || this).setWrappedInstance = o.setWrappedInstance.bind(p(p(o))), o;
    }
    u.prototype = Object.create(n && n.prototype);
    u.prototype.constructor = u;
    return u;
})(Base);
"#;
    let expected = r#"
class Foo extends Base {
    constructor() {
        super();
        this.setWrappedInstance = this.setWrappedInstance.bind(this);
    }
}
"#;
    assert_eq_normalized(&render_pipeline(input), expected);
}

#[test]
fn removes_declaration_when_all_calls_replaced() {
    let input = r#"
function p(e) {
    if (e === undefined) {
        throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
    }
    return e;
}
var x = p(this);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn preserves_non_matching_function() {
    let input = r#"
function validate(e) {
    if (e === null) {
        throw new Error("invalid");
    }
    return e;
}
export var x = validate(obj);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn preserves_identifier_argument_guard() {
    let input = r#"
function _assertThisInitialized(e) {
    if (e === undefined) {
        throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
    }
    return e;
}
function _possibleConstructorReturn(t, e) {
    if (e && (typeof e === "object" || typeof e === "function")) {
        return e;
    }
    if (e !== undefined) {
        throw new TypeError("Derived constructors may only return object or undefined");
    }
    return _assertThisInitialized(t);
}
var value = _possibleConstructorReturn(self, call);
"#;
    let expected = r#"
function _assertThisInitialized(e) {
    if (e === undefined) {
        throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
    }
    return e;
}
function _possibleConstructorReturn(t, e) {
    if (e && (typeof e === "object" || typeof e === "function")) {
        return e;
    }
    if (e !== undefined) {
        throw new TypeError("Derived constructors may only return object or undefined");
    }
    return _assertThisInitialized(t);
}
const value = _possibleConstructorReturn(self, call);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_reference_error_with_different_message() {
    // Same shape but different error message — not a Babel helper
    let input = r#"
function check(e) {
    if (!e) {
        throw new ReferenceError("variable is not defined");
    }
    return e;
}
export var x = check(obj);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}
