mod common;

use common::{assert_eq_normalized, render_pipeline, render_rule};
use wakaru_core::rules::ArrowFunction;

fn apply(input: &str) -> String {
    render_rule(input, |_| ArrowFunction)
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
export const fn = function() { return arguments[0]; };
"#;
    let expected = r#"
export const fn = (...args) => args[0];
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
