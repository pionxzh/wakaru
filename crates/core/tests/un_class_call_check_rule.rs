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
fn removes_babel_runtime_import_class_call_check() {
    let input = r#"
var _classCallCheck = require("@babel/runtime/helpers/classCallCheck");
function Foo() {
    _classCallCheck(this, Foo);
    this.x = 1;
}
"#;
    let expected = r#"
function Foo() {
    this.x = 1;
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn removes_swc_external_class_call_check() {
    let input = r#"
import { _ as _class_call_check } from "@swc/helpers/_/_class_call_check";
function Foo() {
    _class_call_check(this, Foo);
    this.x = 1;
}
"#;
    let expected = r#"
function Foo() {
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

#[test]
fn preserves_call_with_side_effecting_arguments() {
    // Helper identity alone does not prove the argument frame: removing this
    // statement would delete the evaluation of probe() and bar().
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
export function Foo() {
    _classCallCheck(probe(), bar());
    this.x = 1;
}
"#;
    let output = render(input);
    assert!(
        output.contains("probe()"),
        "argument evaluation must survive:\n{output}"
    );
    assert!(
        output.contains("bar()"),
        "argument evaluation must survive:\n{output}"
    );
}

#[test]
fn preserves_call_whose_second_argument_is_not_the_enclosing_binding() {
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
export function Foo() {
    _classCallCheck(this, Other);
    this.x = 1;
}
"#;
    let output = render(input);
    assert!(
        output.contains("Other"),
        "non-constructor call must survive:\n{output}"
    );
}

#[test]
fn preserves_call_with_extra_arguments() {
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
export function Foo() {
    _classCallCheck(this, Foo, extra());
    this.x = 1;
}
"#;
    let output = render(input);
    assert!(
        output.contains("extra()"),
        "extra-argument call must survive:\n{output}"
    );
}

#[test]
fn removes_call_in_function_expression_assigned_to_declarator() {
    // Babel's class IIFE shape: the constructor is a function expression
    // assigned to a var; the call references the declarator binding.
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
var Foo = function() {
    _classCallCheck(this, Foo);
    this.x = 1;
};
"#;
    let output = render(input);
    assert!(
        !output.contains("_classCallCheck") && !output.contains("instanceof"),
        "canonical declarator-form call should be removed:\n{output}"
    );
}

#[test]
fn removes_call_inside_recovered_class_constructor() {
    // A residual call inside `class Bar`'s own constructor is definitionally
    // satisfied — a class constructor cannot be called without `new` — so the
    // class binding counts as the enclosing-constructor frame.
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
class Bar {
    constructor() {
        _classCallCheck(this, Bar);
        this.x = 1;
    }
}
export { Bar };
"#;
    let output = render(input);
    assert!(
        !output.contains("_classCallCheck"),
        "residual call and helper should be removed:\n{output}"
    );
}

#[test]
fn inline_iife_preserves_side_effecting_second_argument() {
    // The inline IIFE form must satisfy the same argument frame as the named
    // helper: a side-effecting or non-enclosing second argument fails closed.
    let input = r#"
function Foo() {
    ((e, t) => {
        if (!(e instanceof t)) throw new TypeError("Cannot call a class as a function");
    })(this, bar());
}
"#;
    let output = render(input);
    assert!(
        output.contains("bar()"),
        "argument evaluation must survive:\n{output}"
    );
}

#[test]
fn inline_iife_preserves_spread_arguments() {
    let input = r#"
function Foo() {
    ((e, t) => {
        if (!(e instanceof t)) throw new TypeError("Cannot call a class as a function");
    })(this, ...values);
}
"#;
    let output = render(input);
    assert!(
        output.contains("values"),
        "spread argument must survive:\n{output}"
    );
}
