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
fn promotes_named_class_expression_different_name_renames_minified_internal() {
    // Binding is meaningful, class name is minified → use binding, rename internal.
    let input = "const Foo = class r { method() { return new r(); } };";
    let expected = "class Foo {\n    method() {\n        return new Foo();\n    }\n}";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn prefers_meaningful_class_name_over_minified_binding() {
    // Binding is minified, class name is meaningful → use class name, rename binding.
    let input = "const d = class Logger { child() { return new Logger(); } }; new d();";
    let expected =
        "class Logger {\n    child() {\n        return new Logger();\n    }\n}\nnew Logger();";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn both_minified_uses_binding_name() {
    // Both names are minified → use binding (external code references it).
    let input = "const d = class r { method() { return new r(); } };";
    let expected = "class d {\n    method() {\n        return new d();\n    }\n}";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn both_meaningful_uses_binding_name() {
    // Both names are meaningful → use binding (external code references it).
    let input = "const MyLogger = class Logger { child() { return new Logger(); } };";
    let expected = "class MyLogger {\n    child() {\n        return new MyLogger();\n    }\n}";
    assert_eq_normalized(&apply(input), expected);
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
fn renames_self_reference_in_static_method() {
    let input = "const Foo = class Bar { static create() { return new Bar(); } };";
    let expected = "class Foo {\n    static create() {\n        return new Foo();\n    }\n}";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn exported_class_keeps_binding_name_over_class_name() {
    let input = "export const d = class Logger { child() { return new Logger(); } }; new d();";
    let expected = "export class d {\n    child() {\n        return new d();\n    }\n}\nnew d();";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn block_scope_rename_applies_to_all_statements() {
    let input = "function f() { const d = class Logger {}; return new d(); }";
    let expected = "function f() {\n    class Logger {}\n    return new Logger();\n}";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn does_not_choose_class_name_that_conflicts_with_outer_binding() {
    let input = "const Logger = 1; const d = class Logger { method() { return new Logger(); } };";
    let expected =
        "const Logger = 1;\nclass d {\n    method() {\n        return new d();\n    }\n}";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn does_not_choose_class_name_that_conflicts_with_function() {
    let input = "function Logger() {} const d = class Logger {};";
    let expected = "function Logger() {}\nclass d {}";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn does_not_choose_class_name_that_conflicts_with_import() {
    let input = r#"import Logger from "./module"; const d = class Logger {}; console.log(Logger);"#;
    let expected = r#"import Logger from "./module";
class d {}
console.log(Logger);"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn does_not_choose_class_name_that_conflicts_with_named_import() {
    let input =
        r#"import { Logger } from "./module"; const d = class Logger {}; console.log(Logger);"#;
    let expected = r#"import { Logger } from "./module";
class d {}
console.log(Logger);"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn does_not_choose_class_name_that_conflicts_with_function_param() {
    let input = "function f(Logger) { const d = class Logger {}; return new d(); }";
    let expected = "function f(Logger) {\n    class d {}\n    return new d();\n}";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn does_not_choose_class_name_that_conflicts_with_arrow_param() {
    let input = "const f = (Logger) => { const d = class Logger {}; return new d(); };";
    let expected = "const f = (Logger)=>{
    class d {}
    return new d();
};";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn does_not_choose_class_name_that_conflicts_with_catch_param() {
    let input = "try { work(); } catch (Logger) { const d = class Logger {}; recover(d); }";
    let expected = "try {\n    work();\n} catch (Logger) {\n    class d {}\n    recover(d);\n}";
    assert_eq_normalized(&apply(input), expected);
}
