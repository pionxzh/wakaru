mod common;
use common::{assert_eq_normalized, render};

#[test]
fn removes_negated_class_call_check_iife() {
    let input = r#"
export function Foo() {
    !((e, t) => {
        if (!(e instanceof t)) {
            throw new TypeError("Cannot call a class as a function");
        }
    })(this, Foo);
    this.x = 1;
}
"#;
    let expected = r#"
export function Foo() {
    this.x = 1;
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn removes_plain_class_call_check_iife() {
    let input = r#"
export function Foo() {
    ((e, t) => {
        if (!(e instanceof t)) {
            throw new TypeError("Cannot call a class as a function");
        }
    })(this, Foo);
    this.x = 1;
}
"#;
    let expected = r#"
export function Foo() {
    this.x = 1;
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn removes_function_expr_class_call_check() {
    let input = r#"
export function Foo() {
    !(function(e, t) {
        if (!(e instanceof t)) {
            throw new TypeError("Cannot call a class as a function");
        }
    })(this, Foo);
    this.x = 1;
}
"#;
    let expected = r#"
export function Foo() {
    this.x = 1;
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn removes_named_class_call_check_function() {
    // When _classCallCheck is a module-level function, calls should be removed
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
export function Foo() {
    _classCallCheck(this, Foo);
    this.x = 1;
}
"#;
    let expected = r#"
export function Foo() {
    this.x = 1;
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_non_class_call_check_iife() {
    // An IIFE that doesn't match the classCallCheck pattern should be preserved
    let input = r#"
export function Foo() {
    !((e, t) => {
        console.log(e, t);
    })(this, Foo);
    this.x = 1;
}
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}
