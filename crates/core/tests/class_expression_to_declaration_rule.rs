mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::ClassExpressionToDeclaration;

fn apply(input: &str) -> String {
    render_rule(input, |_| ClassExpressionToDeclaration)
}

#[test]
fn promotes_anonymous_class_expression() {
    let input = "const Foo = class { constructor() {} method() { return 1; } };";
    let expected = "class Foo {\n    constructor(){}\n    method() {\n        return 1;\n    }\n}";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn promotes_named_class_expression_same_name() {
    let input = "const Foo = class Foo { constructor() {} };";
    let expected = "class Foo {\n    constructor(){}\n}";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn preserves_named_class_expression_with_different_internal_name() {
    let input = "const Foo = class r { method() { return new r(); } };";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_named_class_expression_name_property() {
    let input = "const d = class Logger { child() { return new Logger(); } }; new d();";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_named_class_expression_when_both_names_are_minified() {
    let input = "const d = class r { method() { return new r(); } };";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_named_class_expression_when_both_names_are_meaningful() {
    let input = "const MyLogger = class Logger { child() { return new Logger(); } };";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_named_class_expression_static_name_observation() {
    let input = "let className; const expr = class C { static f = className = this.name; };";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_named_class_expression_binding_in_heritage() {
    let input = r#"
let probe;
const cls = class C extends (probe = function() { return C; }, Base) {
  method() { return C; }
};
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn promotes_exported_class_expression() {
    let input = "export const Foo = class { method() {} };";
    let expected = "export class Foo {\n    method(){}\n}";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn does_not_promote_let_binding() {
    let input = "let Foo = class { method() {} };";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn does_not_promote_var_binding() {
    let input = "var Foo = class { method() {} };";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn does_not_promote_non_class_expression() {
    let input = "const Foo = function() {};";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn does_not_promote_multi_declarator() {
    let input = "const Foo = class {}, Bar = class {};";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn promotes_class_with_extends() {
    let input = "const Child = class extends Parent { constructor() { super(); } };";
    let expected = "class Child extends Parent {\n    constructor(){\n        super();\n    }\n}";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn promotes_in_block_scope() {
    let input = "function outer() { const Foo = class { method() {} }; return new Foo(); }";
    let expected =
        "function outer() {\n    class Foo {\n        method(){}\n    }\n    return new Foo();\n}";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn promotes_class_with_static_methods() {
    let input = "const Foo = class { static create() { return new Foo(); } };";
    let expected = "class Foo {\n    static create() {\n        return new Foo();\n    }\n}";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn preserves_differently_named_self_reference_in_static_method() {
    let input = "const Foo = class Bar { static create() { return new Bar(); } };";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_exported_class_with_different_internal_name() {
    let input = "export const d = class Logger { child() { return new Logger(); } }; new d();";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_block_scoped_class_with_different_internal_name() {
    let input = "function f() { const d = class Logger {}; return new d(); }";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn does_not_choose_class_name_that_conflicts_with_outer_binding() {
    let input = "const Logger = 1; const d = class Logger { method() { return new Logger(); } };";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn does_not_choose_class_name_that_conflicts_with_function() {
    let input = "function Logger() {} const d = class Logger {};";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn does_not_choose_class_name_that_conflicts_with_import() {
    let input = r#"import Logger from "./module"; const d = class Logger {}; console.log(Logger);"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn does_not_choose_class_name_that_conflicts_with_named_import() {
    let input =
        r#"import { Logger } from "./module"; const d = class Logger {}; console.log(Logger);"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn does_not_choose_class_name_that_conflicts_with_function_param() {
    let input = "function f(Logger) { const d = class Logger {}; return new d(); }";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn does_not_choose_class_name_that_conflicts_with_arrow_param() {
    let input = "const f = (Logger) => { const d = class Logger {}; return new d(); };";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn does_not_choose_class_name_that_conflicts_with_catch_param() {
    let input = "try { work(); } catch (Logger) { const d = class Logger {}; recover(d); }";
    assert_eq_normalized(&apply(input), input);
}
