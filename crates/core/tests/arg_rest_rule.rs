mod common;

use common::{assert_eq_normalized, render_rule};
use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, SyntaxContext, GLOBALS};
use swc_core::ecma::ast::{BindingIdent, Decl, EsVersion, Function, ModuleItem, Pat, Stmt};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;
use wakaru_core::{rules::ArgRest, RewriteLevel};

fn apply(input: &str) -> String {
    apply_with_level(input, RewriteLevel::Standard)
}

fn apply_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |_| ArgRest::new(level))
}

#[test]
fn arguments_index_becomes_rest_args() {
    let input = r#"
function foo() {
    return arguments[0] + arguments[1];
}
"#;
    let expected = r#"
function foo(...args) {
    return args[0] + args[1];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn direct_strict_directive_preserves_function_arguments() {
    let input = r#"
function foo() {
    "use strict";
    return arguments[0] + arguments[1];
}
"#;

    assert_eq_normalized(&apply(input), input);
}

#[test]
fn minimal_does_not_convert_arguments_index_to_rest_args() {
    let input = r#"
function foo() {
    return arguments[0] + arguments[1];
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), input);
}

#[test]
fn arguments_length_becomes_rest_length() {
    let input = r#"
function foo() {
    return arguments.length;
}
"#;
    let expected = r#"
function foo(...args) {
    return args.length;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn arguments_loop_pattern() {
    let input = r#"
function sum() {
    var total = 0;
    for (var i = 0; i < arguments.length; i++) {
        total += arguments[i];
    }
    return total;
}
"#;
    let expected = r#"
function sum(...args) {
    var total = 0;
    for (var i = 0; i < args.length; i++) {
        total += args[i];
    }
    return total;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn arguments_with_variable_index() {
    let input = r#"
function get(i) {
    return arguments[i];
}
"#;
    // Function has a formal param — should not transform
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn parameterless_variable_index_without_length_guard_not_converted() {
    let input = r#"
function get() {
    var i = chooseIndex();
    return arguments[i];
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn shadowed_loop_index_is_not_treated_as_length_guarded() {
    let input = r#"
function get() {
    for (var i = 0; i < arguments.length; i++) {
        {
            let i = chooseIndex();
            return arguments[i];
        }
    }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn length_guarded_post_increment_index_becomes_rest_args() {
    let input = r#"
function join() {
    var out = "";
    for (var i = 0; i < arguments.length;) {
        out += arguments[i++];
    }
    return out;
}
"#;
    let expected = r#"
function join(...args) {
    var out = "";
    for (var i = 0; i < args.length;) {
        out += args[i++];
    }
    return out;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn cached_arguments_length_loop_index_becomes_rest_args() {
    let input = r#"
function join() {
    const len = arguments.length;
    var out = "";
    for (var i = 0; i < len; i++) {
        out += arguments[i];
    }
    return out;
}
"#;
    let expected = r#"
function join(...args) {
    const len = args.length;
    var out = "";
    for (var i = 0; i < len; i++) {
        out += args[i];
    }
    return out;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn while_loop_with_length_guarded_post_increment_index_becomes_rest_args() {
    let input = r#"
function join() {
    var out = "";
    let i = 0;
    while (i < arguments.length) {
        out += arguments[i++];
    }
    return out;
}
"#;
    let expected = r#"
function join(...args) {
    var out = "";
    let i = 0;
    while (i < args.length) {
        out += args[i++];
    }
    return out;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn for_loop_init_arguments_length_alias_becomes_rest_args() {
    let input = r#"
function join() {
    var out = "";
    for (var i = 0, len = arguments.length; i < len; i++) {
        out += arguments[i];
    }
    return out;
}
"#;
    let expected = r#"
function join(...args) {
    var out = "";
    for (var i = 0, len = args.length; i < len; i++) {
        out += args[i];
    }
    return out;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn function_with_params_not_converted() {
    // Accessing the fixed-parameter prefix through `arguments` is still unsafe.
    let input = r#"
function foo(a, b) {
    return arguments[0];
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn function_with_fixed_params_tail_indices_becomes_rest_args() {
    let input = r#"
function foo(a, b) {
    return arguments[2] + arguments[3];
}
"#;
    let expected = r#"
function foo(a, b, ...args) {
    return args[0] + args[1];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn function_already_has_rest_not_converted() {
    let input = r#"
function foo(...rest) {
    return rest[0];
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn bare_arguments_reference_not_converted() {
    // Passing `arguments` as a whole value is unsafe to transform
    let input = r#"
function foo() {
    return bar(arguments);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn arguments_spread_not_converted() {
    let input = r#"
function foo() {
    return bar(...arguments);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn nested_function_arguments_not_conflated() {
    // Inner function's `arguments` should be transformed independently;
    // outer function has no `arguments` so it is left alone.
    let input = r#"
function outer() {
    function inner() {
        return arguments[0];
    }
    return inner;
}
"#;
    let expected = r#"
function outer() {
    function inner(...args) {
        return args[0];
    }
    return inner;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn no_arguments_usage_not_converted() {
    let input = r#"
function foo() {
    return 42;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

// ---------------------------------------------------------------------------
// Class constructor support
// ---------------------------------------------------------------------------

#[test]
fn constructor_arguments_becomes_rest_param() {
    // ArgRest must also visit Constructor nodes, not just Function nodes
    let input = r#"
class Foo {
    constructor() {
        console.log(arguments[0]);
    }
}
"#;
    let expected = r#"
class Foo {
    constructor(...args) {
        console.log(args[0]);
    }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn direct_strict_directive_preserves_constructor_arguments() {
    let input = r#"
class Foo {
    constructor() {
        "use strict";
        console.log(arguments[0]);
    }
}
"#;

    assert_eq_normalized(&apply(input), input);
}

#[test]
fn constructor_babel_copy_loop_removed() {
    // The Babel rest-args copy loop should be removed when rest param is added
    let input = r#"
class Foo {
    constructor() {
        for (var o = arguments.length, i = Array(o), a = 0; a < o; a++) {
            i[a] = arguments[a];
        }
        this.items = i;
    }
}
"#;
    let expected = r#"
class Foo {
    constructor(...i) {
        this.items = i;
    }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ---------------------------------------------------------------------------
// Copy loop removal in regular functions
// ---------------------------------------------------------------------------

#[test]
fn function_babel_copy_loop_removed() {
    let input = r#"
function foo() {
    for (var len = arguments.length, args = Array(len), i = 0; i < len; i++) {
        args[i] = arguments[i];
    }
    return args;
}
"#;
    let expected = r#"
function foo(...args) {
    return args;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn copy_loop_removal_does_not_promote_later_strict_string_to_directive() {
    let input = r#"
function foo() {
    for (var len = arguments.length, args = Array(len), i = 0; i < len; i++) {
        args[i] = arguments[i];
    }
    "use strict";
    return args;
}
class Foo {
    constructor() {
        for (var len = arguments.length, args = Array(len), i = 0; i < len; i++) {
            args[i] = arguments[i];
        }
        "use strict";
        this.args = args;
    }
}
"#;

    assert_eq_normalized(&apply(input), input);
}

#[test]
fn function_babel_tail_copy_loop_removed() {
    let input = r#"
function foo(a, b) {
    for (var len = arguments.length, rest = Array(len > 2 ? len - 2 : 0), i = 2; i < len; i++) {
        rest[i - 2] = arguments[i];
    }
    return bar(a, b, rest);
}
"#;
    let expected = r#"
function foo(a, b, ...rest) {
    return bar(a, b, rest);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn function_typescript_tail_copy_loop_removed() {
    let input = r#"
function collect(first) {
    var rest_items = [];
    for (var _i = 1; _i < arguments.length; _i++) {
        rest_items[_i - 1] = arguments[_i];
    }
    return use(first, rest_items);
}
"#;
    let expected = r#"
function collect(first, ...rest_items) {
    return use(first, rest_items);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn function_typescript_tail_copy_loop_with_offset_removed() {
    let input = r#"
function collect(app_id, version) {
    var rest_items = [];
    for (var _i = 2; _i < arguments.length; _i++) {
        rest_items[_i - 2] = arguments[_i];
    }
    return use(app_id, version, rest_items);
}
"#;
    let expected = r#"
function collect(app_id, version, ...rest_items) {
    return use(app_id, version, rest_items);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn tail_copy_loop_with_wrong_test_is_preserved() {
    let input = r#"
function foo(a, b) {
    for (var len = arguments.length, rest = Array(len > 2 ? len - 2 : 0), i = 2; i <= len; i++) {
        rest[i - 2] = arguments[i];
    }
    return rest;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn tail_copy_loop_with_wrong_write_index_is_preserved() {
    let input = r#"
function foo(a, b) {
    for (var len = arguments.length, rest = Array(len > 2 ? len - 2 : 0), i = 2; i < len; i++) {
        rest[i] = arguments[i];
    }
    return rest;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn tail_copy_loop_with_extra_body_statement_is_preserved() {
    let input = r#"
function foo(a, b) {
    for (var len = arguments.length, rest = Array(len > 2 ? len - 2 : 0), i = 2; i < len; i++) {
        rest[i - 2] = arguments[i];
        observe(arguments.callee);
    }
    return rest;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn copy_loop_preserved_when_not_arguments_pattern() {
    // A for loop that doesn't match the Babel copy pattern should be kept
    let input = r#"
function foo() {
    for (var i = 0; i < 10; i++) {
        console.log(arguments[i]);
    }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn copy_loop_rest_param_preserves_binding_context() {
    let input = r#"
function foo(a) {
    for (var _len = arguments.length, _args = Array(_len > 1 ? _len - 1 : 0), _key = 1; _key < _len; _key++) {
        _args[_key - 1] = arguments[_key];
    }
    console.log(_args);
}
"#;

    let (copy_var_ctxt, rest_param_ctxt) = rest_param_context_from_copy_loop(input);

    assert_eq!(
        rest_param_ctxt, copy_var_ctxt,
        "rest parameter should keep the binding context from the consumed copy variable"
    );
    assert_ne!(
        rest_param_ctxt,
        SyntaxContext::empty(),
        "regression input should use a scoped local binding"
    );
}

fn rest_param_context_from_copy_loop(input: &str) -> (SyntaxContext, SyntaxContext) {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            FileName::Custom("fixture.js".to_string()).into(),
            input.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax::default()),
            EsVersion::latest(),
            StringInput::from(&*fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);
        let mut module = parser.parse_module().expect("input should parse");

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        let copy_var_ctxt = for_loop_copy_var_context(first_function(&module).function.as_ref());

        module.visit_mut_with(&mut ArgRest::new(RewriteLevel::Standard));

        let rest_param_ctxt = last_rest_param_context(first_function(&module).function.as_ref());

        (copy_var_ctxt, rest_param_ctxt)
    })
}

fn first_function(module: &swc_core::ecma::ast::Module) -> &swc_core::ecma::ast::FnDecl {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function))) = &module.body[0] else {
        panic!("expected function declaration");
    };
    function
}

fn for_loop_copy_var_context(function: &Function) -> SyntaxContext {
    let body = function.body.as_ref().expect("expected function body");
    let Stmt::For(for_stmt) = &body.stmts[0] else {
        panic!("expected for statement");
    };
    let Some(swc_core::ecma::ast::VarDeclOrExpr::VarDecl(init)) = &for_stmt.init else {
        panic!("expected var decl in for init");
    };
    let Pat::Ident(BindingIdent { id, .. }) = &init.decls[1].name else {
        panic!("expected identifier for copy var (second declarator)");
    };
    id.ctxt
}

fn last_rest_param_context(function: &Function) -> SyntaxContext {
    let last_param = function.params.last().expect("expected at least one param");
    let Pat::Rest(rest) = &last_param.pat else {
        panic!("expected rest parameter");
    };
    let Pat::Ident(BindingIdent { id, .. }) = rest.arg.as_ref() else {
        panic!("expected identifier in rest parameter");
    };
    id.ctxt
}

// --- nested arrows read the enclosing function's `arguments` ---

#[test]
fn nested_arrow_reading_mapped_index_blocks_rest() {
    // The arrow reads `arguments[0]`, which is mapped to `a` while the
    // parameter list is simple. Adding a rest parameter would unmap it and
    // `g()` would return 1 instead of 2.
    let input = r#"
function f(a) {
    var g = () => arguments[0];
    a = 2;
    return [arguments[1], g()];
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn nested_arrow_tail_index_is_rewritten_with_body() {
    let input = r#"
function f(a) {
    var g = () => arguments[1];
    return [arguments[1], g()];
}
"#;
    let expected = r#"
function f(a, ...args) {
    var g = () => args[0];
    return [args[0], g()];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn nested_arrow_arguments_length_is_rewritten_in_parameterless_function() {
    let input = r#"
function f() {
    var count = () => arguments.length;
    return count() + arguments[0];
}
"#;
    let expected = r#"
function f(...args) {
    var count = () => args.length;
    return count() + args[0];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn arguments_in_default_parameter_blocks_rest() {
    let input = r#"
function f(a = arguments.length) {
    return arguments[1];
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn existing_args_binding_gets_a_fresh_rest_name() {
    // `args[0]` would be captured by the local declaration after printing.
    let input = r#"
function f() {
    var args = 1;
    return arguments[0] + args;
}
"#;
    let expected = r#"
function f(...args_1) {
    var args = 1;
    return args_1[0] + args;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn nested_arrow_param_named_args_gets_a_fresh_rest_name() {
    let input = r#"
function f() {
    var g = (args) => arguments[0] + args;
    return g(arguments[1]);
}
"#;
    let expected = r#"
function f(...args_1) {
    var g = (args) => args_1[0] + args;
    return g(args_1[1]);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn outer_args_reference_gets_a_fresh_rest_name() {
    // The arrow reads the module-level `args`; a rest parameter spelled `args`
    // would shadow it.
    let input = r#"
const args = 9;
function f() {
    return () => [arguments[0], args];
}
"#;
    let expected = r#"
const args = 9;
function f(...args_1) {
    return () => [args_1[0], args];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn fresh_rest_name_skips_taken_suffixes() {
    let input = r#"
function f(args, args_1) {
    return arguments[2];
}
"#;
    let expected = r#"
function f(args, args_1, ...args_2) {
    return args_2[0];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn nested_class_constructor_arguments_not_conflated() {
    // A constructor has its own `arguments`: it gets its own rest parameter,
    // and it must neither enable nor be rewritten by the enclosing function's
    // rest recovery (`f` stays parameterless).
    let input = r#"
function f() {
    class A {
        constructor() {
            if (arguments.length < 1) throw new TypeError("required");
            this.value = arguments[0];
        }
    }
    return A;
}
"#;
    let expected = r#"
function f() {
    class A {
        constructor(...args) {
            if (args.length < 1) throw new TypeError("required");
            this.value = args[0];
        }
    }
    return A;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn class_constructor_inside_nested_arrow_not_conflated() {
    // With one fixed parameter and an `arguments.length` read the constructor
    // keeps `arguments`; the arrow traversal must not attribute that read to
    // `f` either.
    let input = r#"
function f() {
    const install = () => {
        class A {
            constructor(y) {
                if (arguments.length < 1) throw new TypeError("required");
                this.value = y;
            }
        }
        return A;
    };
    return install();
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn copy_binding_shadowed_by_arrow_parameter_gets_a_fresh_rest_name() {
    let input = r#"
function f() {
    for (var len = arguments.length, copy = Array(len), i = 0; i < len; i++) {
        copy[i] = arguments[i];
    }
    const read = (copy) => arguments[0] + copy;
    return [copy[0], read(9)];
}
"#;
    let expected = r#"
function f(...copy_1) {
    const read = (copy) => copy_1[0] + copy;
    return [copy_1[0], read(9)];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn constructor_copy_binding_rename_preserves_shadowed_and_outer_references() {
    let input = r#"
const copy_1 = 100;
class C {
    constructor() {
        for (var len = arguments.length, copy = Array(len), i = 0; i < len; i++) {
            copy[i] = arguments[i];
        }
        const read = (copy) => arguments[0] + copy + copy_1;
        this.value = [copy[0], read(9), { copy }];
    }
}
"#;
    let expected = r#"
const copy_1 = 100;
class C {
    constructor(...copy_2) {
        const read = (copy) => copy_2[0] + copy + copy_1;
        this.value = [copy_2[0], read(9), { copy: copy_2 }];
    }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn copy_loop_length_binding_reused_after_the_loop_stays_declared() {
    // Minifiers reuse the copy loop's length variable as the `_this` alias.
    // Removing the loop must not remove its declaration.
    let input = r#"
function Sub() {
    for (var e = arguments.length, n = Array(e), r = 0; r < e; r++) n[r] = arguments[r];
    return (e = Base.call.apply(Base, [this].concat(n)) || this).state = {}, e;
}
"#;
    let output = apply(input);
    assert!(output.contains("function Sub(...n)"), "{output}");
    assert!(output.contains("var e = n.length;"), "{output}");
    assert!(!output.contains("arguments"), "{output}");
}

#[test]
fn copy_loop_index_binding_reused_after_the_loop_stays_declared() {
    let input = r#"
function count() {
    for (var e = arguments.length, n = Array(e), r = 0; r < e; r++) n[r] = arguments[r];
    use(n);
    return r;
}
"#;
    let output = apply(input);
    assert!(output.contains("function count(...n)"), "{output}");
    assert!(output.contains("var r = n.length;"), "{output}");
    assert!(output.contains("return r;"), "{output}");
}

#[test]
fn tail_copy_index_keeps_its_initial_value_when_arguments_are_missing() {
    let input = r#"
function f(a) {
    var args = [];
    for (var i = 1; i < arguments.length; i++) args[i - 1] = arguments[i];
    return i;
}
"#;
    let expected = r#"
function f(a, ...args) {
    var i = arguments.length < 1 ? 1 : arguments.length;
    return i;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn tail_copy_length_and_index_keep_distinct_terminal_values() {
    let input = r#"
function f(a, b) {
    for (var len = arguments.length, args = Array(len > 2 ? len - 2 : 0), i = 2; i < len; i++) args[i - 2] = arguments[i];
    return [len, i, args];
}
"#;
    let expected = r#"
function f(a, b, ...args) {
    var len = arguments.length, i = arguments.length < 2 ? 2 : arguments.length;
    return [len, i, args];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// A rest array is a copy: a write to `arguments[i]` would stay on the
// arguments object while reads move to the copy, and writes past the end
// change the copy's `length` but not `arguments.length`. Any write blocks the
// rewrite.
#[test]
fn assignment_to_arguments_index_preserves_arguments() {
    let input = r#"
function f() {
    arguments[0] = 7;
    return arguments[0];
}
function g() {
    arguments[0] += 1;
    return arguments.length;
}
function h() {
    (arguments[0]) = 7;
    return arguments[0];
}
"#;
    let expected = r#"
function f() {
    arguments[0] = 7;
    return arguments[0];
}
function g() {
    arguments[0] += 1;
    return arguments.length;
}
function h() {
    arguments[0] = 7;
    return arguments[0];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn update_of_arguments_index_preserves_arguments() {
    let input = r#"
function f() {
    arguments[1]--;
    return arguments[1];
}
function g() {
    return ++arguments[0];
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn delete_of_arguments_index_preserves_arguments() {
    let input = r#"
function f() {
    delete arguments[0];
    return arguments[0];
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn destructuring_and_loop_targets_on_arguments_preserve_arguments() {
    let input = r#"
function f(o) {
    [arguments[1]] = [o];
    return arguments[1];
}
function g(o) {
    ({ a: arguments[1] } = o);
    return arguments[1];
}
function h(o) {
    for (arguments[1] in o);
    return arguments[1];
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn write_in_nested_arrow_preserves_arguments() {
    let input = r#"
function f() {
    const set = () => {
        arguments[0] = 7;
    };
    set();
    return arguments[0];
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn write_to_inner_function_arguments_does_not_block_outer_rewrite() {
    let input = r#"
function outer() {
    function inner() {
        arguments[0] = 7;
        return arguments[0];
    }
    return inner(arguments[0]);
}
"#;
    let expected = r#"
function outer(...args) {
    function inner() {
        arguments[0] = 7;
        return arguments[0];
    }
    return inner(args[0]);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}
