mod common;
use common::{assert_eq_normalized, render, render_rule};
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
const x = [].concat(a);
"#;
    let expected = r#"
const x = [...a];
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
    // Do not treat a name, or an earlier `= []`, as proof the concat arg is an array.
    let input = r#"
const arr = [];
const x = [].concat(arr);
"#;
    assert_eq_normalized(&render(input), input);
}
