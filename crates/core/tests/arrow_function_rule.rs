mod common;

use common::{assert_eq_normalized, render_pipeline, render_rule};
use wakaru_core::rules::ArrowFunction;

fn apply(input: &str) -> String {
    render_rule(input, ArrowFunction::new)
}

fn apply_pipeline(input: &str) -> String {
    render_pipeline(input)
}

#[test]
fn duplicate_params_stay_function() {
    // Arrow parameter lists reject duplicate names as an early error, so a
    // sloppy-mode function with duplicate params must keep its shape.
    let input = r#"
(function (a, a) {
  use(a);
})(1, 2);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn duplicate_params_stay_function_for_bind_this() {
    let input = r#"
register(function (a, a) {
  use(a);
}.bind(this));
"#;
    // The fixer normalizes parens around the callee; the function itself
    // must keep its shape and `.bind(this)`.
    let expected = r#"
register((function (a, a) {
  use(a);
}).bind(this));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn direct_eval_stays_function() {
    let input = r#"
const run = function (value) {
  eval(code);
  return value;
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn direct_eval_stays_function_for_bind_this() {
    let input = r#"
register(function (value) {
  eval("arguments[0]");
  return value;
}.bind(this));
"#;
    let expected = r#"
register((function (value) {
  eval("arguments[0]");
  return value;
}).bind(this));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn direct_eval_without_function_sensitive_names_can_be_arrow() {
    let input = r#"
const load = function () {
  return eval("require('crypto')");
};
"#;
    let expected = r#"
const load = () => {
  return eval("require('crypto')");
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn direct_eval_in_nested_function_does_not_block_outer_arrow() {
    let input = r#"
const outer = function () {
  return function () {
    return eval("this");
  };
};
"#;
    let expected = r#"
const outer = () => {
  return function () {
    return eval("this");
  };
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn direct_eval_in_nested_arrow_blocks_outer_arrow() {
    let input = r#"
const outer = function () {
  return () => eval("arguments");
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn bind_this_eval_mentioning_this_can_be_arrow() {
    let input = r#"
register(function (value) {
  return eval("this.value");
}.bind(this));
"#;
    let expected = r#"
register((value) => {
  return eval("this.value");
});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn single_return_becomes_arrow_expression() {
    let input = r#"
const double = [1, 2, 3].map(function(x) { return x * 2; });
"#;
    let expected = r#"
const double = [1, 2, 3].map(x => {
    return x * 2;
});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn multi_statement_body_keeps_block() {
    let input = r#"
arr.forEach(function(x) {
    console.log(x);
    doSomething(x);
});
"#;
    let expected = r#"
arr.forEach(x => {
    console.log(x);
    doSomething(x);
});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn zero_params_arrow() {
    let input = r#"
const fn = function() { return 42; };
"#;
    let expected = r#"
const fn = () => {
    return 42;
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn function_used_as_constructor_not_converted() {
    let input = r#"
const CustomError = function() {};
const error = new CustomError();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn assigned_function_used_as_constructor_not_converted() {
    let input = r#"
CustomError = function() {};
const error = new CustomError();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn constructor_function_body_still_processes_nested_functions() {
    let input = r#"
const C = function() {
    return values.map(function(value) {
        return value;
    });
};
new C();
"#;
    let expected = r#"
const C = function() {
    return values.map(value => {
        return value;
    });
};
new C();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn reflect_construct_new_target_not_converted() {
    let input = r#"
Reflect.construct(Base, [], function() {});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn reflect_construct_target_not_converted() {
    let input = r#"
Reflect.construct(function() {}, []);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn closure_reflect_construct_probe_keeps_constructor_binding() {
    // Closure's ES5 runtime probes the target's prototype and passes it to
    // Reflect.construct in both constructible positions.
    let input = r#"
function probe(Base) {
    var Candidate = function() {
        throw Error();
    };
    Object.defineProperty(Candidate.prototype, "value", {
        set: function() {
            throw Error();
        }
    });
    Reflect.construct(Candidate, []);
    Reflect.construct(Base, [], Candidate);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn prototype_observation_keeps_function() {
    let input = r#"
const Parser = function() {};
Parser.prototype.parse = parse;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn instanceof_rhs_keeps_function() {
    let input = r#"
const Wrapper = function(value) {
    return Object(value);
};
use(value instanceof Wrapper);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn class_super_keeps_function() {
    let input = r#"
const Base = function() {};
class Derived extends Base {}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn direct_new_callee_keeps_function() {
    let input = r#"
new (function() {})();
"#;
    let expected = r#"
new function() {}();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn bound_constructor_keeps_function_target() {
    let input = r#"
const Bound = function() {}.bind(null);
new Bound();
"#;
    let expected = r#"
const Bound = (function() {}).bind(null);
new Bound();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn constructor_use_propagates_through_alias() {
    let input = r#"
const Original = function() {};
const Alias = Original;
new Alias();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn constructor_use_propagates_through_long_reverse_ordered_alias_chain() {
    let mut input = String::from("const ctor0 = function() {};\n");
    for index in 1..=256 {
        input.push_str(&format!("const ctor{index} = ctor{};\n", index - 1));
    }
    input.push_str("new ctor256();\n");

    let output = apply(&input);

    assert!(output.contains("const ctor0 = function() {}"));
    assert!(!output.contains("const ctor0 = ()=>{}"));
}

#[test]
fn constructor_use_propagates_to_member_assignment() {
    let input = r#"
namespace.C = function() {};
new namespace.C();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn constructor_use_propagates_member_suffix_through_object_alias() {
    let input = r#"
const namespace = {};
namespace.Constructor = function() {};
const alias = namespace;
new alias.Constructor();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn constructor_analysis_terminates_on_property_growing_alias_cycle() {
    let input = r#"
let first = second.left;
let second = first.right;
first.Constructor = function() {};
new first.Constructor();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn conditional_new_callee_keeps_function_bindings() {
    let input = r#"
const first = function() {};
const second = function() {};
new (condition ? first : second)();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn logical_new_callee_keeps_function_bindings() {
    let input = r#"
const primary = function() {};
const fallback = function() {};
new (primary || fallback)();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn sequence_new_callee_keeps_function_binding() {
    let input = r#"
const ctor = function() {};
new (0, ctor)();
"#;
    let output = apply(input);
    assert!(output.contains("const ctor = function() {}"), "{output}");
}

#[test]
fn assignment_new_callee_keeps_function_binding() {
    let input = r#"
const makeCtor = function() {};
let cache;
new (cache = makeCtor)();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn constructor_use_propagates_through_bind_alias() {
    let input = r#"
const target = function() {};
const bound = target.bind(null);
new bound();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn constructor_use_propagates_through_conditional_alias() {
    let input = r#"
const first = function() {};
const second = function() {};
const chosen = condition ? first : second;
new chosen();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn constructor_use_propagates_through_logical_assignment_alias() {
    let input = r#"
const ctor = function() {};
let cached;
cached ||= ctor;
new cached();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn constructor_use_propagates_through_nullish_assignment_alias() {
    let input = r#"
const ctor = function() {};
let cached;
cached ??= ctor;
new cached();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn constructor_use_propagates_through_destructured_alias() {
    let input = r#"
const namespace = {};
namespace.Constructor = function() {};
const { Constructor } = namespace;
new Constructor();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn constructor_use_propagates_through_renamed_nested_destructured_alias() {
    let input = r#"
const wrapper = {};
wrapper.inner = {};
wrapper.inner.Constructor = function() {};
const { inner: { Constructor: Local } } = wrapper;
new Local();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn constructor_use_propagates_through_destructured_default_alias() {
    let input = r#"
const fallbackCtor = function() {};
const { Constructor = fallbackCtor } = getNamespace();
new Constructor();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn constructor_use_preserves_inline_destructured_defaults() {
    let inputs = [
        r#"
const { Constructor = function() {} } = {};
new Constructor();
"#,
        r#"
let Constructor;
({ Constructor = function() {} } = {});
new Constructor();
"#,
    ];

    for input in inputs {
        let output = apply(input);
        assert_eq_normalized(&output, input);
    }
}

#[test]
fn constructor_use_propagates_through_rest_destructured_alias() {
    let input = r#"
const namespace = {};
namespace.Constructor = function() {};
const { ignored, ...rest } = namespace;
new rest.Constructor();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn multi_params_arrow() {
    let input = r#"
const add = function(a, b) { return a + b; };
"#;
    let expected = r#"
const add = (a, b) => {
    return a + b;
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn function_with_this_not_converted() {
    // `this` binding is different in arrow functions — must not convert
    let input = r#"
const obj = { fn: function() { return this.x; } };
"#;
    let expected = r#"
const obj = { fn: function() { return this.x; } };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn function_with_this_in_nested_arrow_not_converted() {
    // Nested arrows capture `this` from the function expression, so converting
    // the outer function would change the arrow's `this` binding.
    let input = r#"
const fn = function() {
    return () => this.x;
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn function_with_new_target_not_converted() {
    let input = r#"
const make = function() {
    return new.target;
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn function_with_new_target_in_nested_arrow_not_converted() {
    let input = r#"
const make = function() {
    return () => new.target;
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn nested_function_new_target_does_not_block_outer_arrow() {
    let input = r#"
const outer = function() {
    return function() {
        return new.target;
    };
};
"#;
    let expected = r#"
const outer = () => {
    return function() {
        return new.target;
    };
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn function_with_arguments_converted_via_arg_rest() {
    // ArgRest rewrites arguments[N] → args[N] first, then ArrowFunction can convert.
    // Arrow functions have no own `arguments`, but after ArgRest runs that is no
    // longer a blocker.
    let input = r#"
const fn = function() { return arguments[0]; };
"#;
    let expected = r#"
const fn = (...args) => args[0];
"#;
    let output = apply_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_not_converted() {
    // Arrow functions cannot be generators
    let input = r#"
const gen = function* () { yield 1; };
"#;
    let expected = r#"
const gen = function* () { yield 1; };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn named_function_expr_not_converted() {
    // Named function expressions expose their name via `.name`, even when they
    // do not reference themselves.
    let input = r#"
f = function fact(n) { return n; };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn named_function_expr_with_shadowed_name_not_converted() {
    let input = r#"
f = function fact() {
    function inner(fact) {
        return fact;
    }
    return inner(1);
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn named_function_expr_name_observation_not_converted_in_pipeline() {
    let input = r#"
export const observed = function named() {}?.name;
"#;
    let expected = r#"
export const observed = (function named() {})?.name;
"#;
    let output = apply_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn default_exported_function_expression_not_converted() {
    let input = r#"
export default function() {
    return values.map(function(value) {
        return value;
    });
}
"#;
    let expected = r#"
export default function() {
    return values.map(value => {
        return value;
    });
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn named_exported_function_expression_not_converted() {
    // A named export remains constructable by another module even when this
    // file never uses `new`. Nested callbacks may still become arrows.
    let input = r#"
export const Name = function() {
    return values.map(function(v) {
        return v;
    });
};
"#;
    let expected = r#"
export const Name = function() {
    return values.map(v => {
        return v;
    });
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn named_exported_empty_function_stays_constructible() {
    let input = r#"
export const Name = function() {};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn named_exported_empty_function_stays_constructible_in_pipeline() {
    let input = r#"
export const Name = function() {};
"#;
    let output = apply_pipeline(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn named_export_specifier_keeps_function() {
    let input = r#"
const Name = function() {};
export { Name };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn named_export_alias_keeps_local_function() {
    let input = r#"
const local = function() {};
export { local as Name };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn named_exported_alias_chain_keeps_source_function_constructible() {
    // The export aliases the value through local bindings, rather than only
    // renaming the local binding in the export specifier.
    let input = r#"
const Impl = function() {};
const Alias = Impl;
const Name = Alias;
export { Name };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn named_exported_destructuring_default_keeps_function_constructible() {
    let input = r#"
export const { Name = function() {} } = source;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn named_exported_async_function_can_convert_to_arrow() {
    let input = r#"
export const load = async function() {
    return 1;
};
"#;
    let expected = r#"
export const load = async () => {
    return 1;
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn named_exported_conditional_preserves_only_constructible_function_branch() {
    let input = r#"
export const Factory = condition
    ? function() { return syncValue; }
    : async function() { return asyncValue; };
"#;
    let expected = r#"
export const Factory = condition
    ? function() { return syncValue; }
    : async () => { return asyncValue; };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn assigned_then_named_export_keeps_function() {
    let input = r#"
var Name;
Name = function() {};
export { Name };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn named_exported_paren_function_expression_not_converted() {
    let input = r#"
export const Name = (function() {});
"#;
    let expected = r#"
export const Name = function() {};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn named_exported_sequence_result_not_converted() {
    let input = r#"
export const Name = (0, function() {});
"#;
    let expected = r#"
export const Name = (0, function() {});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn inner_shadow_same_short_name_still_converts() {
    // Binding identity is (sym, ctxt). An inner `Name` is not the export.
    let input = r#"
export const Name = function() {
    const Name = function() {
        return 42;
    };
    return Name;
};
"#;
    let expected = r#"
export const Name = function() {
    const Name = () => {
        return 42;
    };
    return Name;
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn exported_helper_callback_argument_still_converts() {
    let input = r#"
export const Name = helper(function() {
    return 1;
});
"#;
    let expected = r#"
export const Name = helper(() => {
    return 1;
});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn exported_iife_call_still_converts_callee() {
    let input = r#"
export const C = (function(x) {
    return x + 1;
})(1);
"#;
    let expected = r#"
export const C = ((x) => {
    return x + 1;
})(1);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn reexport_from_module_does_not_preserve_unrelated_local() {
    let input = r#"
export { foo } from "./dep.js";
const Name = function() {
    return 1;
};
"#;
    let expected = r#"
export { foo } from "./dep.js";
const Name = () => {
    return 1;
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn object_method_value_not_converted_to_arrow() {
    // Object method values may use `this`; the obj-method shorthand rule handles
    // them separately. Arrow conversion must not fire here.
    let input = r#"
({foo: function() {}});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn bind_this_converted_to_arrow() {
    // `fn.bind(this)` explicitly locks `this`, making the function semantically
    // equivalent to an arrow — safe to convert
    let input = r#"
a(function(x) { this.x = x; }.bind(this));
"#;
    let expected = r#"
a((x) => {
    this.x = x;
});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_anonymous_function_converted() {
    // Async anonymous function expressions without `this`/`arguments` can safely
    // become async arrow functions
    let input = r#"
f = async function() { return 1; };
"#;
    let expected = r#"
f = async () => {
    return 1;
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_named_function_not_converted() {
    let input = r#"
f = async function named() { return 1; };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

// --- parameter initializers share the function's own bindings ---

#[test]
fn arguments_in_default_parameter_stays_function() {
    // `arguments.length` in the initializer reads the callee's own arguments
    // object (0 here). An arrow would read `outer`'s arguments (2).
    let input = r#"
function outer() {
  var f = function(a = arguments.length) { return a; };
  return f();
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn this_in_default_parameter_stays_function() {
    let input = r#"
var f = function(a = this.x) { return a; };
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn new_target_in_default_parameter_stays_function() {
    // `new.target` is a syntax error inside a top-level arrow parameter list.
    let input = r#"
var f = function(a = new.target) { return a; };
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn arguments_in_destructured_default_stays_function() {
    let input = r#"
var f = function({ a = arguments[0] }) { return a; };
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn direct_eval_in_default_parameter_stays_function() {
    let input = r#"
var f = function(a = eval("arguments")) { return a; };
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn plain_default_parameter_still_converts() {
    // Control: an initializer without function-only bindings is fine.
    let input = r#"
var f = function(a = 1, { b = 2 } = {}) { return a + b; };
"#;
    let expected = r#"
var f = (a = 1, { b = 2 } = {}) => {
    return a + b;
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn bind_this_with_arguments_in_default_stays_function() {
    let input = r#"
a(function(x = arguments[1]) { this.x = x; }.bind(this));
"#;
    let expected = r#"
a((function(x = arguments[1]) {
    this.x = x;
}).bind(this));
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn bind_this_with_this_in_default_converts() {
    // `.bind(this)` locks the same `this` the initializer would see.
    let input = r#"
a(function(x = this.y) { this.x = x; }.bind(this));
"#;
    let expected = r#"
a((x = this.y) => {
    this.x = x;
});
"#;
    assert_eq_normalized(&apply(input), expected);
}

// Babel's createClass helper after Terser: it defines methods on the first
// argument's prototype and returns it, and the result is constructed later.
const MINIFIED_CREATE_CLASS: &str = r#"
function t(r, t) {
    for (var e = 0; e < t.length; e++) {
        var o = t[e];
        Object.defineProperty(r, o.key, o);
    }
}
function e(r, e, n) {
    return e && t(r.prototype, e), n && t(r, n), Object.defineProperty(r, "prototype", { writable: !1 }), r;
}
"#;

fn with_minified_create_class(code: &str) -> String {
    format!("{MINIFIED_CREATE_CLASS}{code}")
}

#[test]
fn minified_create_class_argument_stays_constructible() {
    let input = with_minified_create_class(
        r#"
var i = e(function() {
    return values.map(function(value) {
        return value;
    });
}, [{ key: "m", get: function() { return 1; } }]);
use(new i().m);
"#,
    );
    let expected = with_minified_create_class(
        r#"
var i = e(function() {
    return values.map((value) => {
        return value;
    });
}, [{ key: "m", get: function() { return 1; } }]);
use(new i().m);
"#,
    );
    assert_eq_normalized(&apply(&input), &expected);
}

#[test]
fn create_class_assigned_argument_stays_constructible() {
    let input = with_minified_create_class(
        r#"
function define() {
    let c;
    let d;
    e(c = function() {}, []);
    e((0, d = function() {}), []);
    return [c, d];
}
"#,
    );
    assert_eq_normalized(&apply(&input), &input);
}

#[test]
fn create_class_argument_binding_stays_constructible() {
    let input = with_minified_create_class(
        r#"
var c = function() {};
e(c, []);
"#,
    );
    assert_eq_normalized(&apply(&input), &input);
}

#[test]
fn inline_babel_create_class_argument_stays_constructible() {
    let input = r#"
function _createClass(Constructor, protoProps, staticProps) {
    if (protoProps) _defineProperties(Constructor.prototype, protoProps);
    if (staticProps) _defineProperties(Constructor, staticProps);
    Object.defineProperty(Constructor, "prototype", { writable: false });
    return Constructor;
}
var Foo = _createClass(function() {});
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn runtime_create_class_argument_stays_constructible() {
    let input = r#"
import _createClass from "@babel/runtime/helpers/createClass";
import { _ as _create_class } from "@swc/helpers/_/_create_class";
var Foo = _createClass(function() {}, []);
var Bar = _create_class(function() {}, []);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn unresolved_create_class_argument_stays_constructible() {
    let input = r#"
var Foo = _createClass(function() {}, []);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn create_class_async_argument_still_converts() {
    // An async function has no [[Construct]], so the arrow loses nothing.
    let input = with_minified_create_class(
        r#"
e(async function() { return 1; }, []);
"#,
    );
    let expected = with_minified_create_class(
        r#"
e(async () => { return 1; }, []);
"#,
    );
    assert_eq_normalized(&apply(&input), &expected);
}

#[test]
fn create_class_later_arguments_still_convert() {
    let input = with_minified_create_class(
        r#"
e(Ctor, function(value) { return value; });
"#,
    );
    let expected = with_minified_create_class(
        r#"
e(Ctor, (value) => { return value; });
"#,
    );
    assert_eq_normalized(&apply(&input), &expected);
}

#[test]
fn unproven_create_class_name_still_converts() {
    // The name alone does not prove the helper: this one returns its argument
    // without touching a prototype.
    let input = r#"
function createClass(f) {
    return f;
}
createClass(function() { return 1; });
"#;
    let expected = r#"
function createClass(f) {
    return f;
}
createClass(() => { return 1; });
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn shadowed_create_class_binding_still_converts() {
    let input = with_minified_create_class(
        r#"
function wrap(e) {
    e(function() { return 1; });
}
"#,
    );
    let expected = with_minified_create_class(
        r#"
function wrap(e) {
    e(() => { return 1; });
}
"#,
    );
    assert_eq_normalized(&apply(&input), &expected);
}

#[test]
fn minified_no_class_calls_create_class_stays_constructible_in_pipeline() {
    // Babel `noClassCalls` omits classCallCheck and Terser drops the unused
    // constructor name, leaving an anonymous function as the first argument.
    let input = r#"
function t(t,r){for(var e=0;e<r.length;e++){var n=r[e];n.enumerable=n.enumerable||!1,n.configurable=!0,"value"in n&&(n.writable=!0),Object.defineProperty(t,n.key,n)}}
function r(r,e,n){return e&&t(r.prototype,e),n&&t(r,n),Object.defineProperty(r,"prototype",{writable:!1}),r}
var e=r(function(){},[{key:"m",value:function(){return 1}}]);
export function run(){return(new e).m()}
"#;
    let output = apply_pipeline(input);
    assert!(
        output.contains("(function() {}"),
        "createClass argument must stay constructible:\n{output}"
    );
    assert!(
        !output.contains("(() => {}"),
        "createClass argument must not become an arrow:\n{output}"
    );
}

#[test]
fn immediate_call_argument_remains_constructible() {
    for input in [
        "(function(Ctor) { return new Ctor(); })(function() {});",
        "((Ctor) => { return new Ctor(); })(function() {});",
        "(function(Ctor) { var Alias = Ctor; return new Alias(); })(function() {});",
        "(function(Ctor) { return new Ctor(); })(ready ? function() {} : fallback);",
    ] {
        let output = apply(input);
        assert!(output.contains("function()"), "{output}");
    }
    let input = "export function create(Base, args) { return (function(ctor, values, Temporary) { Temporary.prototype = ctor.prototype; var instance = new Temporary(); var result = ctor.apply(instance, values); return Object(result) === result ? result : instance; })(Base, args, function() {}); }";
    let output = apply_pipeline(input);
    assert!(output.contains("function()"), "{output}");
}

#[test]
fn immediate_call_constructor_argument_does_not_freeze_sibling_callbacks() {
    let input = "(function(Ctor, callback) { callback(); return new Ctor(); })(function() {}, function() { return 1; });";
    let output = apply(input);
    assert_eq!(output.matches("function()").count(), 1, "{output}");
    assert!(output.contains("return 1"), "{output}");
    assert!(output.contains("=>"), "{output}");
}

#[test]
fn immediate_call_constructor_pairing_respects_shadowed_parameters() {
    let input = "(function(Ctor) { function nested(Ctor) { return new Ctor(); } return Ctor(); })(function() { return 1; });";
    let output = apply(input);
    assert!(!output.contains("function()"), "{output}");
    assert!(output.contains("function nested(Ctor)"), "{output}");
}

#[test]
fn exported_iife_return_stays_constructible() {
    // The export is the IIFE result, which is the returned binding.
    let input = r#"
export let Name;
Name = (function() {
    let ctor;
    ctor = function() {};
    const mapped = items.map(function(value) {
        return value;
    });
    use(mapped);
    return ctor;
})();
"#;
    let expected = r#"
export let Name;
Name = (()=>{
    let ctor;
    ctor = function() {};
    const mapped = items.map((value)=>{
        return value;
    });
    use(mapped);
    return ctor;
})();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn constructed_iife_return_stays_constructible_without_export() {
    let input = r#"
let ctor;
ctor = (function() {
    let inner;
    inner = function() {};
    return inner;
})();
ctor.instance = new ctor();
"#;
    let expected = r#"
let ctor;
ctor = (()=>{
    let inner;
    inner = function() {};
    return inner;
})();
ctor.instance = new ctor();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn arrow_iife_expression_body_return_stays_constructible() {
    let input = r#"
let ctor = function() {};
export let Name;
Name = (()=>ctor)();
Name.other = (()=>(0, ctor))();
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn iife_with_arguments_return_stays_constructible() {
    let input = r#"
export let Name;
Name = (function(tag) {
    let ctor;
    ctor = function() {};
    use(tag);
    return ctor;
})(1);
"#;
    let expected = r#"
export let Name;
Name = ((tag)=>{
    let ctor;
    ctor = function() {};
    use(tag);
    return ctor;
})(1);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn sequence_return_ident_stays_constructible() {
    let input = r#"
export let Name;
Name = (function() {
    let ctor;
    ctor = function() {};
    return helper(), ctor;
})();
"#;
    let expected = r#"
export let Name;
Name = (()=>{
    let ctor;
    ctor = function() {};
    return helper(), ctor;
})();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn unconstructed_iife_return_still_converts() {
    let input = r#"
const Name = (function() {
    let ctor;
    ctor = function() {};
    return ctor;
})();
Name();
"#;
    let expected = r#"
const Name = (()=>{
    let ctor;
    ctor = ()=>{};
    return ctor;
})();
Name();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn nested_function_return_does_not_alias_outer_iife() {
    let input = r#"
export let Name;
Name = (function() {
    let ctor;
    ctor = function() {};
    function nested() {
        return other;
    }
    let other = function() {
        return 1;
    };
    use(nested);
    return ctor;
})();
"#;
    let output = apply(input);
    assert!(output.contains("ctor = function() {}"), "{output}");
    assert!(output.contains("other = ()=>"), "{output}");
}

#[test]
fn iife_return_shadow_same_short_name_does_not_freeze_inner() {
    // Binding identity is (sym, ctxt). The inner `ctor` is not the returned one.
    let input = r#"
export let Name;
Name = (function() {
    let ctor;
    ctor = function() {
        const ctor = function() {
            return 1;
        };
        return ctor;
    };
    return ctor;
})();
"#;
    let output = apply(input);
    assert!(
        output.contains("ctor = function() {"),
        "outer returned function must stay constructible:\n{output}"
    );
    assert!(
        output.contains("const ctor = ()=>"),
        "inner shadow must still convert:\n{output}"
    );
}

#[test]
fn every_iife_return_stays_constructible() {
    // `new Name()` can construct any value the IIFE returns, so every branch
    // and every return is a source, as for a conditional outside an IIFE.
    for returns in [
        "return flag ? left : right;",
        "if (flag) return left;\n    return right;",
        "return left || right;",
        "if (flag) return;\n    return flag2 ? left : right;",
    ] {
        let input = format!(
            r#"
export let Name;
Name = (function() {{
    let left = function() {{
        return 1;
    }};
    let right = function() {{
        return 2;
    }};
    {returns}
}})();
"#
        );
        let output = apply(&input);
        assert!(output.contains("left = function()"), "{output}");
        assert!(output.contains("right = function()"), "{output}");
    }
}

#[test]
fn indirect_iife_callee_return_stays_constructible() {
    for call in ["(0, function() {\n", "(function() {\n"] {
        for suffix in ["})();", "}).call(this);", "}).apply(this, []);"] {
            if call.starts_with("(0") && suffix != "})();" {
                continue;
            }
            let input = format!(
                "export let Name;\nName = {call}    let ctor;\n    ctor = function() {{}};\n    return ctor;\n{suffix}\n"
            );
            let output = apply(&input);
            assert!(output.contains("ctor = function()"), "{output}");
        }
    }
}

#[test]
fn member_and_nested_iife_return_stays_constructible() {
    let input = r#"
export let Name;
Name = (function() {
    const api = {};
    api.Ctor = function() {};
    return api.Ctor;
})();
export let Other;
Other = (function() {
    return (function() {
        let ctor;
        ctor = function() {};
        return ctor;
    })();
})();
"#;
    let output = apply(input);
    assert!(output.contains("api.Ctor = function()"), "{output}");
    assert!(output.contains("ctor = function()"), "{output}");
}

#[test]
fn async_and_generator_iife_return_still_converts() {
    for input in [
        r#"
export let Name;
Name = (async function() {
    let ctor;
    ctor = function() {};
    return ctor;
})();
"#,
        r#"
export let Name;
Name = (function*() {
    let ctor;
    ctor = function() {};
    return ctor;
})();
"#,
        r#"
export let Name;
Name = (async ()=>{
    let ctor;
    ctor = function() {};
    return ctor;
})();
"#,
    ] {
        let output = apply(input);
        assert!(
            output.contains("ctor = ()=>"),
            "async/generator IIFE must not freeze the inner function:\n{output}"
        );
    }
}

#[test]
fn iife_directly_returned_function_stays_constructible() {
    // The returned function has no binding for the analysis to mark, so the
    // converter must protect the IIFE's own return positions.
    let input = r#"
export let Name;
Name = (function() {
    return function() {};
})();
"#;
    let expected = r#"
export let Name;
Name = (()=>{
    return function() {};
})();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn iife_every_directly_returned_function_stays_constructible() {
    for (call, returns) in [
        (
            "(function() {\n",
            "return flag ? function() {} : function() {};",
        ),
        (
            "(function() {\n",
            "if (flag) return function() {};\n    return function() {};",
        ),
        ("(0, function() {\n", "return function() {};"),
        ("(function() {\n", "return function() {};"),
    ] {
        let suffix = if call.starts_with("(0") {
            "})();"
        } else {
            "}).call(this);"
        };
        let input = format!("export let Name;\nName = {call}    {returns}\n{suffix}\n");
        let output = apply(&input);
        assert!(!output.contains("return ()=>"), "{output}");
        assert!(!output.contains("? ()=>"), "{output}");
        assert!(!output.contains(": ()=>"), "{output}");
    }
}

#[test]
fn arrow_iife_expression_body_function_stays_constructible() {
    let input = r#"
export let Name;
Name = (()=>function() {})();
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn iife_nested_function_returns_still_convert() {
    // Only the IIFE's own returns are its result. A nested function's return
    // and an unconstructed IIFE still convert.
    let input = r#"
export let Name;
Name = (function() {
    const make = function() {
        return function() {
            return 1;
        };
    };
    use(make);
    return function() {};
})();
const plain = (function() {
    return function() {};
})();
plain();
"#;
    let expected = r#"
export let Name;
Name = (()=>{
    const make = ()=>{
        return ()=>{
            return 1;
        };
    };
    use(make);
    return function() {};
})();
const plain = (()=>{
    return ()=>{};
})();
plain();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn async_iife_returned_function_still_converts() {
    let input = r#"
export let Name;
Name = (async function() {
    return function() {};
})();
"#;
    let output = apply(input);
    assert!(output.contains("return ()=>{}"), "{output}");
}

#[test]
fn iife_returned_base_keeps_prototype_through_pipeline() {
    // `Base.prototype.hello = ...` throws on an arrow, which has no prototype.
    let input = r#"
var Base = (function() {
    return function() {};
})();
Base.prototype.hello = function() {
    return 1;
};
export { Base };
"#;
    let output = apply_pipeline(input);
    assert!(output.contains("Base = function()"), "{output}");
}

#[test]
fn iife_protection_stops_at_the_returned_function_body() {
    // The returned function stays constructible; what it returns is not the
    // IIFE's result.
    let input = r#"
export let Name;
Name = (function() {
    return function() {
        return function() {
            return 1;
        };
    };
})();
"#;
    let expected = r#"
export let Name;
Name = (()=>{
    return function() {
        return ()=>{
            return 1;
        };
    };
})();
"#;
    assert_eq_normalized(&apply(input), expected);
}

/// An anonymous function passed to a constructor-sensitive parameter of a
/// same-module function declaration stays an ordinary function.
fn assert_keeps_anonymous_function(output: &str) {
    assert!(
        output.contains("function()"),
        "anonymous function expression must stay constructible:\n{output}"
    );
    assert!(
        !output.contains("()=>{}") && !output.contains("() => {}"),
        "constructor argument must not become an empty arrow:\n{output}"
    );
}

#[test]
fn declared_extends_argument_stays_function() {
    let input = r#"
function declare(Base) {
    class Child extends Base {}
    return Child;
}
declare(function() {});
"#;
    let output = apply(input);
    assert_keeps_anonymous_function(&output);
    let piped = apply_pipeline(input);
    assert_keeps_anonymous_function(&piped);
}

#[test]
fn lowered_mixin_extends_argument_stays_function() {
    // producer typescript@5.9.3 (target ES5, module CommonJS) then
    // terser@5.51.2 defaults: `declare(class {})` becomes
    // `declare(function(){})`, and UnEs6Class turns the `__extends` IIFE in
    // `declare` back into `class n extends t` before ArrowFunction runs.
    let input = r#"
"use strict";var __extends=this&&this.__extends||function(){var t=function(n,e){return t=Object.setPrototypeOf||{__proto__:[]}instanceof Array&&function(t,n){t.__proto__=n}||function(t,n){for(var e in n)Object.prototype.hasOwnProperty.call(n,e)&&(t[e]=n[e])},t(n,e)};return function(n,e){if("function"!=typeof e&&null!==e)throw new TypeError("Class extends value "+String(e)+" is not a constructor or null");function r(){this.constructor=n}t(n,e),n.prototype=null===e?Object.create(e):(r.prototype=e.prototype,new r)}}();function declare(t){var n=function(t){function n(){return null!==t&&t.apply(this,arguments)||this}return __extends(n,t),n.prototype.kind=function(){return"child"},n}(t);return n}Object.defineProperty(exports,"__esModule",{value:!0}),exports.Other=void 0,exports.Other=declare(function(){});
"#;
    let expected = r#"
function declare(t) {
    class n extends t {
        kind() {
            return "child";
        }
    }
    return n;
}
export const Other = declare(function() {});
"#;
    let output = apply_pipeline(input);
    let tail = &output[output.find("function declare").expect("declare is kept")..];
    assert_eq_normalized(tail, expected);
}

#[test]
fn declared_extends_argument_called_before_declaration_stays_function() {
    // Function declarations are hoisted. The table is built before rewriting,
    // so the call may appear above the declaration.
    let input = r#"
declare(function() {});
function declare(Base) {
    class Child extends Base {}
    return Child;
}
"#;
    assert_keeps_anonymous_function(&apply(input));
}

#[test]
fn declared_extends_paren_and_sequence_callee_stay_function() {
    for input in [
        r#"
function declare(Base) {
    class Child extends Base {}
    return Child;
}
declare((function() {}));
"#,
        r#"
function declare(Base) {
    class Child extends Base {}
    return Child;
}
(0, declare)(function() {});
"#,
        r#"
function declare(Base) {
    class Child extends Base {}
    return Child;
}
declare(ready ? function() {} : other);
"#,
    ] {
        assert_keeps_anonymous_function(&apply(input));
    }
}

#[test]
fn declared_new_argument_stays_function() {
    // The same table: a parameter that is `new`'d keeps its argument too.
    let input = r#"
function declare(Base) {
    return new Base();
}
declare(function() {});
"#;
    assert_keeps_anonymous_function(&apply(input));
}

#[test]
fn declared_call_shifts_arguments_past_this() {
    let input = r#"
function declare(Base) {
    class Child extends Base {}
    return Child;
}
declare.call(function() {
    return 1;
}, function() {});
"#;
    let output = apply(input);
    assert!(
        output.contains("function() {}") || output.contains("function(){}"),
        "argument after this must stay a function:\n{output}"
    );
    assert!(
        output.contains("()=>") || output.contains("() =>"),
        "this argument is not a constructor parameter:\n{output}"
    );
}

#[test]
fn declared_sensitive_parameter_keeps_only_its_argument() {
    let input = r#"
function declare(callback, Base) {
    callback();
    class Child extends Base {}
    return Child;
}
declare(function() {
    return 1;
}, function() {
    return items.map(function(value) {
        return value;
    });
});
"#;
    let output = apply(input);
    assert!(
        output.contains("function() {") || output.contains("function(){"),
        "extends argument must stay a function:\n{output}"
    );
    assert!(
        output.contains("()=>"),
        "sibling and nested callbacks must still become arrows:\n{output}"
    );
    assert!(
        output.contains("(value)=>") || output.contains("(value) =>"),
        "nested callback inside the kept function must still become an arrow:\n{output}"
    );
}

#[test]
fn declared_argument_before_spread_stays_function() {
    let input = r#"
function declare(Base) {
    class Child extends Base {}
    return Child;
}
declare(function() {}, ...rest);
"#;
    assert_keeps_anonymous_function(&apply(input));
}

#[test]
fn exported_declared_extends_argument_stays_function() {
    let input = r#"
export function declare(Base) {
    class Child extends Base {}
    return Child;
}
declare(function() {});
"#;
    assert_keeps_anonymous_function(&apply(input));
    let input = r#"
export default function declare(Base) {
    class Child extends Base {}
    return Child;
}
declare(function() {});
"#;
    assert_keeps_anonymous_function(&apply(input));
}

#[test]
fn property_bag_empty_function_still_converts() {
    let input = r#"
const bag = function() {};
bag.KEY = 0;
use(bag);
"#;
    let output = apply(input);
    assert!(output.contains("()=>{}"), "{output}");
    assert!(!output.contains("function()"), "{output}");
}

#[test]
fn called_parameter_argument_still_converts() {
    let input = r#"
function declare(callback) {
    return callback();
}
declare(function() {
    return 1;
});
"#;
    let output = apply(input);
    assert!(output.contains("()=>"), "{output}");
    assert!(!output.contains("function()"), "{output}");
}

#[test]
fn unresolved_callee_argument_still_converts() {
    let input = r#"
unknown(function() {
    return 1;
});
"#;
    let output = apply(input);
    assert!(output.contains("()=>"), "{output}");
    assert!(!output.contains("function()"), "{output}");
}

#[test]
fn shadowed_declared_parameter_does_not_protect_outer_argument() {
    // An inner function or parameter with the same short name is a different
    // `(sym, ctxt)`. The outer call's argument may still become an arrow.
    let input = r#"
function declare(callback) {
    function declare(Base) {
        class Child extends Base {}
        return Child;
    }
    return callback();
}
declare(function() {
    return 1;
});
"#;
    let output = apply(input);
    assert!(
        output.contains("function declare(Base)"),
        "inner declaration stays:\n{output}"
    );
    assert!(
        output.contains("()=>"),
        "outer argument must still become an arrow:\n{output}"
    );
    let input = r#"
function declare(Base) {
    function nested(Base) {
        class Child extends Base {}
        return Child;
    }
    return Base();
}
declare(function() {
    return 1;
});
"#;
    let output = apply(input);
    assert!(output.contains("()=>"), "{output}");
    assert!(!output.contains("function()"), "{output}");
}

#[test]
fn destructured_extends_parameter_does_not_freeze_argument() {
    let input = r#"
function declare({ Base }) {
    class Child extends Base {}
    return Child;
}
declare(function() {
    return 1;
});
"#;
    let output = apply(input);
    assert!(output.contains("()=>"), "{output}");
    assert!(!output.contains("function()"), "{output}");
}

#[test]
fn declared_async_argument_still_converts() {
    let input = r#"
function declare(Base) {
    class Child extends Base {}
    return Child;
}
declare(async function() {
    return 1;
});
"#;
    let output = apply(input);
    assert!(output.contains("async ()=>"), "{output}");
    assert!(!output.contains("async function"), "{output}");
}

#[test]
fn declared_argument_count_mismatch_pairs_only_sensitive_slot() {
    let fewer = r#"
function declare(callback, Base) {
    class Child extends Base {}
    return Child;
}
declare(function() {
    return 1;
});
"#;
    let fewer_out = apply(fewer);
    assert!(fewer_out.contains("()=>"), "{fewer_out}");
    assert!(!fewer_out.contains("function()"), "{fewer_out}");

    let extra = r#"
function declare(callback, Base) {
    class Child extends Base {}
    return Child;
}
declare(function() {
    return 1;
}, function() {}, function() {
    return 2;
});
"#;
    let output = apply(extra);
    assert!(
        output.contains("function()"),
        "the extends argument stays a function:\n{output}"
    );
    assert!(
        output.matches("()=>").count() >= 2,
        "the sibling and the extra callback still become arrows:\n{output}"
    );
}
