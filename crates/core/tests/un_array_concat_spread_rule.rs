mod common;
use common::{assert_eq_normalized, render, render_rule, render_with_level};
use wakaru_core::{
    rules::{UnArrayConcatSpread, UnArrayConcatSpreadRest},
    RewriteLevel,
};

fn apply_rule_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |_| UnArrayConcatSpread::new_with_level(level))
}

fn apply_rest_proof(input: &str) -> String {
    render_rule(input, |unresolved_mark| {
        UnArrayConcatSpreadRest::new(unresolved_mark, RewriteLevel::Standard)
    })
}

#[test]
fn preserves_unknown_concat_single_element_at_standard() {
    let input = r#"
const x = [a].concat(b);
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn preserves_unknown_concat_after_multiple_elements_at_standard() {
    let input = r#"
const x = [a, b].concat(c);
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn preserves_multiple_unknown_concat_args_at_standard() {
    let input = r#"
const x = [a].concat(b, c);
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn simplifies_concat_with_array_literal_arg() {
    let input = r#"
const x = [a].concat([b, c]);
"#;
    let expected = r#"
const x = [a, b, c];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_empty_array_concat_with_unknown_arg_at_standard() {
    let input = r#"
const x = [].concat(a);
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn simplifies_spread_over_concat_pattern() {
    // Babel class ctor leftover: e.call(...[this].concat(args)).
    // Unknown concat args stay concat; UnSpreadArrayLiteral only inlines
    // ...[array-literal], so the outer spread is left as-is.
    let input = r#"
const x = e.call(...[this].concat(args));
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn preserves_variable_concat() {
    // Don't transform x.concat(y) where x is not an array literal
    let input = r#"
const x = arr.concat(other);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn minimal_preserves_concat_with_unknown_argument() {
    let input = r#"
const x = [this].concat(args);
"#;
    let output = apply_rule_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn minimal_flattens_concat_with_array_literal_argument() {
    let input = r#"
const x = [a].concat([b, c]);
"#;
    let expected = r#"
const x = [a, b, c];
"#;
    let output = apply_rule_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, expected);
}

#[test]
fn unknown_concat_argument_stays_concat_at_standard() {
    // concat only spreads Arrays (or @@isConcatSpreadable). An identifier is
    // not an array proof, so Standard must keep the call.
    let input = r#"
const x = [].concat(a);
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn aggressive_assumes_concat_arguments_are_arrays() {
    // `concat_arguments_are_arrays`: retain the generated-code heuristic for
    // Babel loose / iterableIsArray output only at Aggressive.
    let input = r#"
const x = [b].concat(a);
const y = [].concat(a, c);
"#;
    let expected = r#"
const x = [b, ...a];
const y = [...a, ...c];
"#;
    let output = apply_rule_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn proven_rest_copy_argument_becomes_spread_at_standard() {
    let input = r#"
function forward() {
  for (var len = arguments.length, args = Array(len), index = 0; index < len; index++) {
    args[index] = arguments[index];
  }
  return call(...[head].concat(args, [tail]));
}
"#;
    let expected = r#"
function forward() {
  for (var len = arguments.length, args = Array(len), index = 0; index < len; index++) {
    args[index] = arguments[index];
  }
  return call(...[head, ...args, tail]);
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), expected);
}

#[test]
fn proven_typescript_rest_copy_argument_becomes_spread_at_standard() {
    let input = r#"
function forward(first) {
  var args = [];
  for (var index = 1; index < arguments.length; index++) {
    args[index - 1] = arguments[index];
  }
  return call(...[first].concat(args, [tail]));
}
"#;
    let expected = r#"
function forward(first) {
  var args = [];
  for (var index = 1; index < arguments.length; index++) {
    args[index - 1] = arguments[index];
  }
  return call(...[first, ...args, tail]);
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), expected);
}

#[test]
fn captured_rest_copy_argument_becomes_spread_at_standard() {
    let input = r#"
function forward() {
  for (var len = arguments.length, args = Array(len), index = 0; index < len; index++) {
    args[index] = arguments[index];
  }
  return function () {
    return call(...[head].concat(args, [tail]));
  };
}
"#;
    let expected = r#"
function forward() {
  for (var len = arguments.length, args = Array(len), index = 0; index < len; index++) {
    args[index] = arguments[index];
  }
  return function () {
    return call(...[head, ...args, tail]);
  };
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), expected);
}

#[test]
fn existing_rest_parameter_is_array_proof_at_standard() {
    let input = r#"
function forward(...args) {
  return call(...[head].concat(args, [tail]));
}
"#;
    let expected = r#"
function forward(...args) {
  return call(...[head, ...args, tail]);
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), expected);
}

#[test]
fn reassigned_rest_copy_is_not_array_proof() {
    let input = r#"
function forward() {
  for (var len = arguments.length, args = Array(len), index = 0; index < len; index++) {
    args[index] = arguments[index];
  }
  args = fallback;
  return call(...[head].concat(args));
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn concat_spreadability_mutation_blocks_rest_copy_proof() {
    let input = r#"
function forward() {
  for (var len = arguments.length, args = Array(len), index = 0; index < len; index++) {
    args[index] = arguments[index];
  }
  args[Symbol.isConcatSpreadable] = false;
  return call(...[head].concat(args));
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn escaped_rest_copy_is_not_array_proof() {
    let input = r#"
function forward() {
  for (var len = arguments.length, args = Array(len), index = 0; index < len; index++) {
    args[index] = arguments[index];
  }
  observe(args);
  return call(...[head].concat(args));
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn shadowed_array_constructor_is_not_rest_copy_proof() {
    let input = r#"
function forward(Array) {
  for (var len = arguments.length, args = Array(len), index = 0; index < len; index++) {
    args[index] = arguments[index];
  }
  return call(...[head].concat(args));
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn pipeline_recovers_class_with_proven_rest_copy_super_concat() {
    let input = r#"
var Foo = ((Base_1) => {
  function Foo() {
    for (var len = arguments.length, args = Array(len), index = 0; index < len; index++) {
      args[index] = arguments[index];
    }
    return Base_1.call.apply(Base_1, [this].concat(args));
  }
  ((Child, Base) => {
    Child.prototype = Object.create(Base && Base.prototype, {
      constructor: { value: Child, enumerable: false, writable: true, configurable: true }
    });
    Base && (Object.setPrototypeOf ? Object.setPrototypeOf(Child, Base) : Child.__proto__ = Base);
  })(Foo, Base_1);
  return Foo;
})(Base);
"#;
    let expected = r#"
class Foo extends Base {
  constructor(...args) {
    super(...args);
  }
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unknown_this_concat_args_stays_concat_at_standard() {
    let input = r#"
const x = [this].concat(args);
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn concat_map_iterator_argument_stays_concat() {
    // MapIterator is not IsConcatSpreadable. Keep concat; do not assert .length.
    let input = r#"
const x = [].concat(m.values());
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn concat_set_argument_stays_concat() {
    let input = r#"
const x = [].concat(new Set([1, 2]));
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn concat_string_argument_is_not_spread() {
    // [].concat("ab") is ["ab"]; [..."ab"] is ["a","b"].
    let input = r#"
const x = [].concat("ab");
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn concat_number_argument_is_not_spread() {
    // [].concat(1) is [1]; [...1] throws.
    let input = r#"
const x = [].concat(1);
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn aggressive_keeps_visibly_non_array_arguments_as_elements() {
    // `[].concat(128, zeros)` appends 128; `[...128]` throws.
    let input = r#"
const a = [].concat(128, zeros);
const b = [x].concat("ab", `t${y}`, -1, n * 2, !flag, typeof v);
const c = [].concat({ id: 1 }, function() {}, () => 0, items);
"#;
    let expected = r#"
const a = [128, ...zeros];
const b = [x, "ab", `t${y}`, -1, n * 2, !flag, typeof v];
const c = [{ id: 1 }, function() {}, () => 0, ...items];
"#;
    let output = apply_rule_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn aggressive_keeps_concat_for_object_literals_that_may_be_spreadable() {
    // A computed key or a `__proto__` entry can make the object
    // concat-spreadable, but spreading it would still throw.
    let input = r#"
const b = [].concat({ [k]: 1 });
const c = [].concat({ __proto__: p }, items);
"#;
    let output = apply_rule_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, input);
}

#[test]
fn aggressive_keeps_concat_with_arguments_object() {
    // `concat` appends the array-like `arguments` object as one element;
    // spreading it would copy its entries instead.
    let input = r#"
function f() {
    return [].concat(arguments);
}
function g() {
    return [0].concat(arguments, [1]);
}
"#;
    let output = apply_rule_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, input);
}

#[test]
fn aggressive_keeps_single_unproven_argument_of_empty_concat() {
    // `[].concat(x)` is also the castArray idiom: a scalar `x` becomes a
    // one-element array, and `[...x]` would iterate a string or throw.
    let input = r#"
function toArray(v) {
    return [].concat(v);
}
const messages = [].concat(rule.message);
const boundaries = [].concat(x || y);
"#;
    let output = apply_rule_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, input);
}

#[test]
fn aggressive_still_flattens_empty_concat_of_array_literal() {
    let input = r#"
const a = [].concat([x, y]);
const b = [].concat(1);
"#;
    let expected = r#"
const a = [x, y];
const b = [1];
"#;
    let output = apply_rule_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn aggressive_spreads_logical_expression_arguments() {
    // Either operand can be an array, so the assumption still applies.
    let input = r#"
const a = [0].concat(x || y);
"#;
    let expected = r#"
const a = [0, ...x || y];
"#;
    let output = apply_rule_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn concat_arguments_object_stays_concat() {
    // ES6 concat does not spread arguments; [...arguments] does.
    let input = r#"
function f() {
  const x = [].concat(arguments);
}
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn concat_spread_argument_is_not_rewritten() {
    // [].concat(...arr) flattens nested arrays; [...arr] does not.
    let input = r#"
const x = [].concat(...arr);
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn mixed_unknown_and_array_literal_keeps_concat() {
    // One unknown argument fail-closes the whole call; do not half-flatten.
    let input = r#"
const x = [a].concat(b, [c]);
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn ident_named_arr_is_not_array_proof() {
    // A name, or an earlier `= []`, is not proof by itself: `fill` may replace
    // or mutate the array. Only use analysis over every reference proves it.
    let input = r#"
const arr = [];
fill(arr);
const x = [].concat(arr);
"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn local_array_literal_binding_becomes_spread_at_standard() {
    let input = r#"
function f() {
  var parts = [a, b];
  return [head].concat(parts, [tail]);
}
"#;
    let expected = r#"
function f() {
  var parts = [a, b];
  return [head, ...parts, tail];
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), expected);
}

#[test]
fn module_array_literal_bindings_become_spreads_at_standard() {
    let input = r#"
const first = [a];
const second = [b, c];
const all = [].concat(first, second, [d]);
"#;
    let expected = r#"
const first = [a];
const second = [b, c];
const all = [...first, ...second, d];
"#;
    assert_eq_normalized(&apply_rest_proof(input), expected);
}

#[test]
fn proven_array_receiver_becomes_spread_at_standard() {
    let input = r#"
const base = [a, b];
const x = base.concat([c]);
const y = base.concat(base);
"#;
    let expected = r#"
const base = [a, b];
const x = [...base, c];
const y = [...base, ...base];
"#;
    assert_eq_normalized(&apply_rest_proof(input), expected);
}

#[test]
fn array_literal_arrow_call_becomes_spread_at_standard() {
    let input = r#"
function config() {
  const sizes = () => [small, large];
  const units = function () {
    return [px, rem];
  };
  return {
    a: [].concat(sizes(), [auto]),
    b: [].concat(units(), sizes()),
    c: sizes(),
  };
}
"#;
    let expected = r#"
function config() {
  const sizes = () => [small, large];
  const units = function () {
    return [px, rem];
  };
  return {
    a: [...sizes(), auto],
    b: [...units(), ...sizes()],
    c: sizes(),
  };
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), expected);
}

#[test]
fn array_literal_function_declaration_call_becomes_spread_at_standard() {
    let input = r#"
const x = [].concat(sizes(), [auto]);
function sizes() {
  return [small, large];
}
"#;
    let expected = r#"
const x = [...sizes(), auto];
function sizes() {
  return [small, large];
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), expected);
}

#[test]
fn mutated_array_literal_binding_stays_concat() {
    let input = r#"
function f() {
  var parts = [a];
  parts.push(b);
  return [head].concat(parts);
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn escaped_array_literal_binding_stays_concat() {
    let input = r#"
const parts = [a];
observe(parts);
const x = [head].concat(parts);
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn reassigned_array_literal_binding_stays_concat() {
    let input = r#"
let parts = [a];
parts = other;
const x = [head].concat(parts);
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn holey_array_literal_binding_stays_concat() {
    // concat keeps the hole; spread reads it as undefined.
    let input = r#"
const parts = [a, , b];
const x = [head].concat(parts);
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn object_literal_binding_stays_concat() {
    // concat appends an object as one element; spread would throw.
    let input = r#"
const mode = { begin: a };
const x = [].concat(mode, [b]);
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn var_array_read_by_hoisted_function_before_init_stays_concat() {
    // g() runs before `parts` is assigned: concat yields [undefined], spread throws.
    let input = r#"
function f() {
  g();
  var parts = [a];
  function g() {
    return [].concat(parts);
  }
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn array_literal_binding_passed_to_unknown_concat_stays_concat() {
    // other.concat may be user code that keeps a reference to parts.
    let input = r#"
const parts = [a];
const x = other.concat(parts);
const y = [].concat(parts);
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn array_literal_binding_passed_to_unproven_receiver_stays_concat() {
    let input = r#"
const parts = [a];
const base = [b];
base.push(c);
const x = base.concat(parts);
const y = [].concat(parts);
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn mapped_parameter_array_binding_stays_concat() {
    // In sloppy mode, arguments[0] aliases the redeclared parameter.
    let input = r#"
function f(parts) {
  var parts = [a];
  arguments[0] = other;
  return [].concat(parts);
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn async_arrow_call_stays_concat() {
    let input = r#"
const load = async () => [a];
const x = [].concat(load());
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn reassigned_array_factory_stays_concat() {
    let input = r#"
var sizes = () => [a];
sizes = other;
const x = [].concat(sizes());
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn array_factory_with_extra_statements_stays_concat() {
    let input = r#"
const sizes = function () {
  log();
  return [a];
};
const x = [].concat(sizes());
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn array_literal_binding_with_eval_stays_concat() {
    let input = r#"
function f() {
  var parts = [a];
  eval(code);
  return [].concat(parts);
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn chained_proven_concat_becomes_one_spread_array_at_standard() {
    let input = r#"
const a = [x];
const b = [y];
const c = [z];
const all = a.concat(b).concat(c);
"#;
    let expected = r#"
const a = [x];
const b = [y];
const c = [z];
const all = [...a, ...b, ...c];
"#;
    assert_eq_normalized(&apply_rest_proof(input), expected);
}

#[test]
fn closure_array_from_iterable_becomes_spread_at_standard() {
    // Closure lowers `[0, ...xs, 1]` through `$jscomp.arrayFromIterable`.
    let input = r#"
var $jscomp = $jscomp || {};
function f(xs, ys) {
    return [0].concat($jscomp.arrayFromIterable(xs), [1], $jscomp.arrayFromIterable(ys));
}
"#;
    let expected = r#"
var $jscomp = $jscomp || {};
function f(xs, ys) {
    return [0, ...xs, 1, ...ys];
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), expected);
}

#[test]
fn closure_array_from_iterable_on_unresolved_runtime_becomes_spread() {
    let input = r#"
const x = [a].concat($jscomp.arrayFromIterable(xs));
"#;
    let expected = r#"
const x = [a, ...xs];
"#;
    assert_eq_normalized(&apply_rest_proof(input), expected);
}

#[test]
fn closure_array_from_iterable_spread_element_is_unwrapped() {
    let input = r#"
const x = [a, ...$jscomp.arrayFromIterable(xs)];
f(...$jscomp.arrayFromIterable(ys));
"#;
    let expected = r#"
const x = [a, ...xs];
f(...ys);
"#;
    assert_eq_normalized(&apply_rest_proof(input), expected);
}

#[test]
fn local_jscomp_binding_is_not_closure_runtime() {
    let input = r#"
function f($jscomp, xs) {
    return [0].concat($jscomp.arrayFromIterable(xs));
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn minimal_keeps_closure_array_from_iterable_concat() {
    let input = r#"
const x = [a].concat($jscomp.arrayFromIterable(xs));
"#;
    let output = render_rule(input, |unresolved_mark| {
        UnArrayConcatSpreadRest::new(unresolved_mark, RewriteLevel::Minimal)
    });
    assert_eq_normalized(&output, input);
}

#[test]
fn pipeline_recovers_closure_array_spread() {
    let input = r#"
var $jscomp = $jscomp || {};
function arrSpread(a) {
    return [0].concat($jscomp.arrayFromIterable(a));
}
"#;
    for level in [RewriteLevel::Standard, RewriteLevel::Aggressive] {
        let output = crate::common::render_with_level(input, level);
        assert_eq_normalized(
            &output,
            r#"
var $jscomp = $jscomp || {};
function arrSpread(a) {
    return [0, ...a];
}
"#,
        );
    }
}

#[test]
fn renamed_closure_array_from_iterable_becomes_spread_at_standard() {
    // producer google-closure-compiler@20260629.0.0
    // --compilation_level=ADVANCED --language_out=ECMASCRIPT5 inlines
    // arrayFromIterator into the renamed helper.
    let input = r#"
function r(a) {
    return a[Symbol.iterator]();
}
function u(a) {
    if (!(a instanceof Array)) {
        a = r(a);
        for (var e, h = []; !(e = a.next()).done;) h.push(e.value);
        a = h;
    }
    return a;
}
function B(a, e) {
    return [a].concat(u(e));
}
function D(a) {
    return [].concat(u(a));
}
"#;
    let expected = r#"
function r(a) {
    return a[Symbol.iterator]();
}
function u(a) {
    if (!(a instanceof Array)) {
        a = r(a);
        for (var e, h = []; !(e = a.next()).done;) h.push(e.value);
        a = h;
    }
    return a;
}
function B(a, e) {
    return [a, ...u(e)];
}
function D(a) {
    return [...u(a)];
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), expected);
}

#[test]
fn renamed_closure_array_from_iterable_with_iterator_helper_becomes_spread() {
    // wild-observed: the helper keeps a call to the renamed
    // arrayFromIterator instead of inlining it.
    let input = r#"
var ya = function(a) {
    for (var b, c = []; !(b = a.next()).done;) c.push(b.value);
    return c;
}, w = function(a) {
    return a instanceof Array ? a : ya(m(a));
};
const x = [].concat(w(xs), [1]);
"#;
    let expected = r#"
var ya = function(a) {
    for (var b, c = []; !(b = a.next()).done;) c.push(b.value);
    return c;
}, w = function(a) {
    return a instanceof Array ? a : ya(m(a));
};
const x = [...w(xs), 1];
"#;
    assert_eq_normalized(&apply_rest_proof(input), expected);
}

#[test]
fn renamed_closure_array_from_iterable_split_by_un_conditionals_becomes_spread() {
    // UnConditionals runs first and splits the helper's ternary return.
    let input = r#"
var ya = function(a) {
    var b;
    var c = [];
    while (!(b = a.next()).done) c.push(b.value);
    return c;
};
var w = function(a) {
    if (a instanceof Array) {
        return a;
    }
    return ya(m(a));
};
const x = [].concat(w(xs), [1]);
"#;
    let expected = r#"
var ya = function(a) {
    var b;
    var c = [];
    while (!(b = a.next()).done) c.push(b.value);
    return c;
};
var w = function(a) {
    if (a instanceof Array) {
        return a;
    }
    return ya(m(a));
};
const x = [...w(xs), 1];
"#;
    assert_eq_normalized(&apply_rest_proof(input), expected);
}

#[test]
fn renamed_closure_array_from_iterable_becomes_spread_in_the_pipeline() {
    // wild-observed helper outline; the whole pipeline runs UnConditionals
    // before the proof.
    let input = r#"
var ya = function(a) {
    for (var b, c = []; !(b = a.next()).done;) c.push(b.value);
    return c;
}, w = function(a) {
    return a instanceof Array ? a : ya(m(a));
};
export const x = [].concat(w(xs), [1]);
export const y = [].concat(w(xs));
"#;
    for level in [RewriteLevel::Standard, RewriteLevel::Aggressive] {
        let output = render_with_level(input, level);
        let tail = output.split("export const x").nth(1).unwrap_or_default();
        assert_eq_normalized(
            &format!("export const x{tail}"),
            "export const x = [...w(xs), 1];\nexport const y = [...w(xs)];",
        );
    }
}

#[test]
fn array_builder_that_returns_before_its_declaration_is_not_proof() {
    let input = r#"
var ya = function(a) {
    if (a) return c;
    var b, c = [];
    return c;
};
var w = function(a) {
    return a instanceof Array ? a : ya(a);
};
const x = [].concat(w(xs));
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn cast_array_helper_is_not_array_from_iterable() {
    // Returns its argument when it is not an Array.
    let input = r#"
function u(a) {
    if (!(a instanceof Array)) {
        var h = [];
        h.push(a);
    }
    return a;
}
var w = function(a) {
    return a instanceof Array ? a : wrap(a);
};
const x = [].concat(u(v), w(v));
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn reassigned_renamed_array_from_iterable_stays_concat() {
    let input = r#"
function u(a) {
    if (!(a instanceof Array)) {
        var h = [];
        a = h;
    }
    return a;
}
u = other;
const x = [].concat(u(v));
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn renamed_array_from_iterable_needs_the_global_array() {
    let input = r#"
function f(Array) {
    function u(a) {
        if (!(a instanceof Array)) {
            var h = [];
            a = h;
        }
        return a;
    }
    return [].concat(u(v));
}
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}

#[test]
fn renamed_array_from_iterable_with_early_return_stays_concat() {
    let input = r#"
function u(a) {
    if (!(a instanceof Array)) {
        if (a == null) return a;
        var h = [];
        a = h;
    }
    return a;
}
const x = [].concat(u(v));
"#;
    assert_eq_normalized(&apply_rest_proof(input), input);
}
