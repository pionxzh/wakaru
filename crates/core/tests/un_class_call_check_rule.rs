mod common;
use common::render;

#[test]
fn keeps_negated_class_call_check_iife_on_function() {
    // The constructor is still a function, so Foo.call(obj) must throw.
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
    let output = render(input);
    assert!(
        output.contains("Cannot call a class as a function"),
        "function constructor must keep the guard:\n{output}"
    );
    assert!(
        output.contains("(this, Foo)"),
        "the check must still receive Foo:\n{output}"
    );
    assert!(
        output.contains("export function Foo"),
        "must stay a function:\n{output}"
    );
}

#[test]
fn keeps_plain_class_call_check_iife_on_function() {
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
    let output = render(input);
    assert!(
        output.contains("Cannot call a class as a function"),
        "function constructor must keep the guard:\n{output}"
    );
    assert!(
        output.contains("(this, Foo)"),
        "the check must still receive Foo:\n{output}"
    );
    assert!(
        output.contains("export function Foo"),
        "must stay a function:\n{output}"
    );
}

#[test]
fn keeps_function_expr_class_call_check_on_function() {
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
    let output = render(input);
    assert!(
        output.contains("Cannot call a class as a function"),
        "function constructor must keep the guard:\n{output}"
    );
    assert!(
        output.contains("(this, Foo)"),
        "the check must still receive Foo:\n{output}"
    );
    assert!(
        output.contains("export function Foo"),
        "must stay a function:\n{output}"
    );
}

#[test]
fn keeps_named_class_call_check_on_function() {
    // The helper stays referenced. Removing it would let Foo.call(obj) write this.x.
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
    let output = render(input);
    assert!(
        output.contains("Cannot call a class as a function"),
        "function constructor must keep the guard:\n{output}"
    );
    assert!(
        output.contains("(this, Foo)"),
        "the check must still receive Foo:\n{output}"
    );
    assert!(
        output.contains("export function Foo"),
        "must stay a function:\n{output}"
    );
}

#[test]
fn keeps_babel_runtime_import_class_call_check_on_function() {
    let input = r#"
var _classCallCheck = require("@babel/runtime/helpers/classCallCheck");
function Foo() {
    _classCallCheck(this, Foo);
    this.x = 1;
}
"#;
    let output = render(input);
    assert!(
        output.contains("_classCallCheck(this, Foo)"),
        "function constructor must keep the guard:\n{output}"
    );
    assert!(
        output.contains("@babel/runtime/helpers/classCallCheck"),
        "must not drop the helper import while the call remains:\n{output}"
    );
}

#[test]
fn keeps_swc_external_class_call_check_on_function() {
    let input = r#"
import { _ as _class_call_check } from "@swc/helpers/_/_class_call_check";
function Foo() {
    _class_call_check(this, Foo);
    this.x = 1;
}
"#;
    let output = render(input);
    assert!(
        output.contains("(this, Foo)"),
        "function constructor must keep the guard:\n{output}"
    );
    assert!(
        output.contains("@swc/helpers/_/_class_call_check"),
        "must not invent a different import while the call remains:\n{output}"
    );
    assert!(
        !output.contains("class Foo"),
        "must stay a function:\n{output}"
    );
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
fn keeps_call_in_function_expression_assigned_to_declarator() {
    // No prototype methods, so this stays a function. The declarator name is
    // not a class binding.
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
        output.contains("_classCallCheck(this, Foo)"),
        "declarator function must keep the guard:\n{output}"
    );
    assert!(
        !output.contains("class Foo"),
        "must not invent a class to justify deleting the guard:\n{output}"
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

#[test]
fn removes_guard_after_es6_class_recovery() {
    // Inner name `t` differs from `Foo`. The guard must not block recovery,
    // and the recovered class must not keep the dead check.
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
var Foo = (function() {
    function t() {
        _classCallCheck(this, t);
        this.x = 1;
    }
    t.prototype.start = function() { return this.x; };
    return t;
})();
"#;
    let output = render(input);
    assert!(
        output.contains("class Foo"),
        "expected class recovery:\n{output}"
    );
    assert!(
        !output.contains("Cannot call a class as a function"),
        "recovered class must drop the guard:\n{output}"
    );
    assert!(
        !output.contains("_classCallCheck"),
        "unused helper must be removed:\n{output}"
    );
    assert!(
        !output.contains("import ") && !output.contains("export "),
        "must not invent import or export:\n{output}"
    );
}

#[test]
fn removes_guard_after_prototype_class_recovery() {
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
function Foo() {
    _classCallCheck(this, Foo);
    this.x = 1;
}
Foo.prototype.start = function() { return this.x; };
"#;
    let output = render(input);
    assert!(
        output.contains("class Foo"),
        "expected class recovery:\n{output}"
    );
    assert!(
        !output.contains("Cannot call a class as a function"),
        "recovered class must drop the guard:\n{output}"
    );
    assert!(
        !output.contains("_classCallCheck"),
        "unused helper must be removed:\n{output}"
    );
}

#[test]
fn keeps_guard_when_same_module_call_skips_class_recovery() {
    // Leftover Foo.call still needs [[Call]], so the constructor stays a function.
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
var Foo = (function() {
    function t() {
        _classCallCheck(this, t);
        this.x = 1;
    }
    t.prototype.start = function() { return this.x; };
    return t;
})();
function make() {
    return Foo.call(this);
}
"#;
    let output = render(input);
    assert!(
        !output.contains("class Foo"),
        "leftover Foo.call must skip class recovery:\n{output}"
    );
    assert!(
        output.contains("Cannot call a class as a function"),
        "skipped constructor must keep the guard:\n{output}"
    );
    assert!(
        output.contains("Foo.call(this)"),
        "leftover call must remain:\n{output}"
    );
}

#[test]
fn keeps_guard_when_prototype_call_skips_class_recovery() {
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
function Foo() {
    _classCallCheck(this, Foo);
    this.x = 1;
}
Foo.prototype.start = function() { return this.x; };
function make() {
    return Foo.call(this);
}
"#;
    let output = render(input);
    assert!(
        !output.contains("class Foo"),
        "leftover Foo.call must skip prototype class recovery:\n{output}"
    );
    assert!(
        output.contains("_classCallCheck(this, Foo)"),
        "function must keep the guard:\n{output}"
    );
}

#[test]
fn keeps_non_instanceof_type_error() {
    let input = r#"
function Foo() {
    if (!this) {
        throw new TypeError("this is missing");
    }
    this.x = 1;
}
"#;
    let output = render(input);
    assert!(
        output.contains("this is missing"),
        "unrelated TypeError must survive:\n{output}"
    );
}

#[test]
fn nested_same_name_function_does_not_drop_outer_guard() {
    // The inner `Foo` is a different binding. Neither function is a class.
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
function Foo() {
    _classCallCheck(this, Foo);
    this.x = 1;
    function Foo() {
        _classCallCheck(this, Foo);
    }
}
"#;
    let output = render(input);
    assert!(
        output.matches("_classCallCheck(this, Foo)").count() == 2,
        "both function guards must survive:\n{output}"
    );
    assert!(
        !output.contains("class Foo"),
        "must not invent a class:\n{output}"
    );
}

#[test]
fn class_constructor_drops_its_guard_and_keeps_nested_function_guard() {
    // The nested function has its own name, so the constructor's `Bar` is the
    // class binding. The nested call is a different binding and must stay.
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
class Bar {
    constructor() {
        _classCallCheck(this, Bar);
        function Inner() {
            _classCallCheck(this, Inner);
        }
    }
}
"#;
    let output = render(input);
    assert!(output.contains("class Bar"), "class must remain:\n{output}");
    assert!(
        output.contains("_classCallCheck(this, Inner)"),
        "nested function guard must stay:\n{output}"
    );
    assert!(
        !output.contains("_classCallCheck(this, Bar)"),
        "class constructor guard is redundant:\n{output}"
    );
}

#[test]
fn shadowed_class_name_inside_constructor_keeps_both_guards() {
    // `function Bar` is hoisted, so both `Bar` references are the inner
    // function, not the class. Deleting either call would drop that function's
    // no-`new` throw.
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
class Bar {
    constructor() {
        _classCallCheck(this, Bar);
        function Bar() {
            _classCallCheck(this, Bar);
        }
    }
}
"#;
    let output = render(input);
    assert!(
        output.matches("_classCallCheck(this, Bar)").count() == 2,
        "shadowed name must not be treated as the class:\n{output}"
    );
}

fn render_ts(source: &str) -> String {
    wakaru_core::decompile(
        source,
        wakaru_core::DecompileOptions {
            filename: "fixture.ts".to_string(),
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code
}

#[test]
fn derived_zero_arg_super_stays_after_guard_removal() {
    // `super()` does not forward arguments. Omitting the constructor would
    // turn it into `super(...arguments)`.
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
function Base() {}
function Sub() {
    _classCallCheck(this, Sub);
    Base.call(this);
}
Sub.prototype = Object.create(Base.prototype);
Sub.prototype.constructor = Sub;
Sub.prototype.start = function() { return 1; };
"#;
    let output = render(input);
    assert!(
        output.contains("class Sub"),
        "expected class recovery:\n{output}"
    );
    assert!(
        output.contains("super()"),
        "zero-arg super must stay:\n{output}"
    );
    assert!(
        !output.contains("...arguments"),
        "must not widen super() into super(...arguments):\n{output}"
    );
    assert!(
        !output.contains("Cannot call a class as a function"),
        "recovered class must drop the guard:\n{output}"
    );
}

#[test]
fn parameterized_constructor_keeps_empty_body_after_guard_removal() {
    let input = r#"
function _classCallCheck(instance, Constructor) {
    if (!(instance instanceof Constructor)) {
        throw new TypeError("Cannot call a class as a function");
    }
}
class Foo {
    constructor(a, b) {
        _classCallCheck(this, Foo);
    }
}
"#;
    let output = render(input);
    assert!(
        output.contains("constructor(a, b)"),
        "parameter list must stay:\n{output}"
    );
    assert!(
        !output.contains("_classCallCheck"),
        "class constructor guard is redundant:\n{output}"
    );
}

#[test]
fn explicit_empty_constructor_stays() {
    let input = r#"
class Keep {
    constructor() {}
    doStuff() { return 42; }
}
"#;
    let output = render(input);
    assert!(
        output.contains("constructor()"),
        "explicit empty constructor must stay:\n{output}"
    );
}

#[test]
fn typescript_constructor_signature_stays() {
    let input = r#"
declare class Foo {
    constructor();
}
"#;
    let output = render_ts(input);
    assert!(
        output.contains("constructor()"),
        "body-less TypeScript constructor must stay:\n{output}"
    );
}
