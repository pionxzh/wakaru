mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::{RewriteLevel, SimplifySequence};

fn apply(input: &str) -> String {
    render_rule(input, SimplifySequence::new)
}

fn apply_minimal(input: &str) -> String {
    render_rule(input, |unresolved_mark| {
        SimplifySequence::new_with_level(unresolved_mark, RewriteLevel::Minimal)
    })
}

#[test]
fn splits_top_level_sequence_expression_statement() {
    let input = r#"
a(), b(), c()
"#;
    let expected = r#"
a();
b();
c();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_parenthesized_top_level_sequence_expression_statement() {
    let input = r#"
(a(), b(), c())
"#;
    let expected = r#"
a();
b();
c();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_split_while_condition_but_splits_body_sequence_statement() {
    let input = r#"
while (a(), b(), c()) {
  d(), e()
}
"#;
    let expected = r#"
while (a(), b(), c()) {
  d();
  e();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_return_sequence_expression() {
    let input = r#"
if(a) return b(), c();
else return d = 1, e = 2, f = 3;
"#;
    let expected = r#"
if (a) {
  b();
  return c();
} else {
  d = 1;
  e = 2;
  return f = 3;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_switch_discriminant_sequence_expression() {
    let input = r#"
switch (a(), b(), c()) {
  case 1:
    d(), e()
}
"#;
    let expected = r#"
a();
b();
switch (c()) {
  case 1:
    d();
    e();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_throw_sequence_expression() {
    let input = r#"
if(e !== null) throw a(), e
"#;
    let expected = r#"
if (e !== null) {
  a();
  throw e;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

// ---------- Current Focus: new tests ----------

#[test]
fn splits_variable_declaration_sequence_expression() {
    let input = r#"
const x = (a(), b(), c())
"#;
    let expected = r#"
a();
b();
const x = c();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_sequence_around_anonymous_function_decl_init() {
    let input = r#"
let x = (0, function() {});
"#;
    let output = apply_minimal(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_sequence_around_anonymous_class_decl_init() {
    let input = r#"
let x = (0, class {});
"#;
    let output = apply_minimal(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_splits_sequence_around_anonymous_function_decl_init() {
    let input = r#"
let x = (setup(), function() {});
"#;
    let expected = r#"
setup();
let x = function() {};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_variable_declaration_sequence_expression_advanced() {
    let input = r#"
const x = (a(), b(), c()), y = 3, z = (d(), e())
"#;
    let expected = r#"
a();
b();
const x = c();
const y = 3;
d();
const z = e();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_for_init_sequence_expression_basic() {
    let input = r#"
for (a(), b(); c(); d(), e()) {
  f(), g()
}
"#;
    let expected = r#"
a();
b();
for (; c(); d(), e()) {
  f();
  g();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_for_init_keeps_assignment_as_init() {
    let input = r#"
for (foo(), bar(), x = 5; false;);
"#;
    let expected = r#"
foo();
bar();
for (x = 5; false;);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_for_init_sequence_with_var_decl() {
    let input = r#"
for (let x = (a(), b(), c()), y = 1; x < 10; x++) {
  d(), e()
}
"#;
    let expected = r#"
a();
b();
for (let x = c(), y = 1; x < 10; x++) {
  d();
  e();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_for_in_sequence_expression() {
    let input = r#"
for (var x in (a(), b(), c())) {
  console.log(x);
}
"#;
    let expected = r#"
a();
b();
for (var x in c()) {
  console.log(x);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_lexical_for_in_sequence_expression() {
    let input = r#"
for (let x in (a(), b(), c())) {
  console.log(x);
}
"#;
    let expected = r#"
for (let x in a(), b(), c()) {
  console.log(x);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_for_of_sequence_expression() {
    let input = r#"
for (var x of (a(), b(), c())) {
  console.log(x);
}
"#;
    let expected = r#"
a();
b();
for (var x of c()) {
  console.log(x);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_lexical_for_of_sequence_expression() {
    let input = r#"
for (let x of (a(), b(), c())) {
  console.log(x);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn drops_pure_literal_no_op_statements() {
    // Numeric, boolean, and null literals as statements are dead code
    let input = r#"
a(), 0, b();
0;
false;
null;
"use strict";
"#;
    let expected = r#"
a();
b();
"use strict";
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_identifier_read_statements() {
    let input = r#"
missing;
(value);
"#;
    let expected = r#"
missing;
value;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_tdz_identifier_read_before_lexical_declaration() {
    let input = r#"
{
  x;
  let x;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_typeof_resolved_binding_read() {
    // `typeof` can throw for lexical bindings while they are in TDZ. Even when
    // the expression looks like a no-op, dropping it can remove an observable
    // ReferenceError from a closure created in a for-of TDZ environment.
    let input = r#"
let x;
function probe() {
  typeof x;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_this_read_statement() {
    let input = r#"
class C extends Base {
  constructor() {
    (() => {
      this;
    })();
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn drops_safe_identifier_read_statements() {
    let input = r#"
undefined;
let value;
value;
"#;
    let expected = r#"
let value;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_import_binding_read_statement_inside_function() {
    let input = r#"
import { x as y } from './self.js';
assert.throws(ReferenceError, function() {
  y;
});
export const x = 23;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_object_literal_computed_key_coercion() {
    let input = r#"
({
  get [badKey]() {}
});
({
  set [badKey](_) {}
});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_object_literal_shorthand_lookup() {
    let input = r#"
({ unresolvable });
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_binary_coercion_no_op_statement() {
    let input = r#"
var badKey = Object.create(null);
function probe() {
  badKey + "";
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_function_expression_statement() {
    // A function expression as a statement should not be removed even though
    // it's technically side-effect-free (issue #150: webcrack output wrapper)
    let input = r#"
(function anonymous(arg) {
  (function () {
    var foo = 1;
    console.log(foo);
  })();
})
"#;
    let expected = r#"
(function anonymous(arg) {
  (function () {
    var foo = 1;
    console.log(foo);
  })();
})
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_arrow_function_expression_statement() {
    let input = r#"
() => { console.log(1); };
doSomething();
"#;
    let expected = r#"
() => { console.log(1); };
doSomething();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_class_expression_statement() {
    let input = r#"
(class {
  static [name] = value;
});
doSomething();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn splits_assignment_member_pattern() {
    let input = r#"
(a = b())['c'] = d;
(a = v).b = c;
"#;
    let expected = r#"
a = b();
a['c'] = d;
a = v;
a.b = c;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
