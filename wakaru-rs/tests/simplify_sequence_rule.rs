mod common;

use common::{assert_eq_normalized, render};

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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_for_in_sequence_expression() {
    let input = r#"
for (let x in (a(), b(), c())) {
  console.log(x);
}
"#;
    let expected = r#"
a();
b();
for (let x in c()) {
  console.log(x);
}
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
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
    let output = render(input);
    assert_eq_normalized(&output, expected);
}
