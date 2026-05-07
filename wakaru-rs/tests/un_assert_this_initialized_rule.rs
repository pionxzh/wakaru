mod common;
use common::{assert_eq_normalized, render};

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
    assert!(
        !output.contains("ReferenceError"),
        "helper should be removed, got: {output}"
    );
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
    assert!(
        output.contains("validate"),
        "should not transform non-matching function, got: {output}"
    );
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
    assert!(
        output.contains("check"),
        "should not transform non-Babel ReferenceError, got: {output}"
    );
}
