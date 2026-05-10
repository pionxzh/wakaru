mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnConditionals;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnConditionals)
}

#[test]
fn simple_ternary_to_if_else() {
    let input = r#"
x ? a() : b()
"#;
    let expected = r#"
if (x) {
  a();
} else {
  b();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn ternary_not_at_top_level_stays() {
    let input = r#"
obj[foo] = cond ? 10 : 20;
cond ? obj[bar] = 10 : obj[bar] = 20;
"#;
    let expected = r#"
obj[foo] = cond ? 10 : 20;

if (cond) {
  obj[bar] = 10;
} else {
  obj[bar] = 20;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn logical_and_or_converted_nullish_stays() {
    let input = r#"
x && a();
x || b();
x ?? c();

!x && a();
!x || b();
!x ?? c();
"#;
    let expected = r#"
if (x) {
  a();
}

if (!x) {
  b();
}

x ?? c();

if (!x) {
  a();
}

if (x) {
  b();
}

!x ?? c();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn nested_ternary_to_else_if_chain() {
    let input = r#"
a ? b() : c ? d() : e() ? g ? h() : i() : j()
"#;
    let expected = r#"
if (a) {
  b();
} else if (c) {
  d();
} else if (e()) {
  if (g) {
    h();
  } else {
    i();
  }
} else {
  j();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn return_ternary_split_into_if_return_chain() {
    let input = r#"
function fn() {
  return 2 == e ? foo() : 3 == f ? bar() : 4 == g ? baz() : fail(e)
}
"#;
    let expected = r#"
function fn() {
  if (2 == e) {
    return foo();
  }

  if (3 == f) {
    return bar();
  }

  if (4 == g) {
    return baz();
  }

  return fail(e);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn return_simple_ternary_split() {
    let input = r#"
function fn() {
  return a ? b() : c ? d() : e()
}
"#;
    let expected = r#"
function fn() {
  if (a) {
    return b();
  }

  if (c) {
    return d();
  }

  return e();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn ternary_with_sequence_branches_converted() {
    // cond ? (a(), b()) : (c(), d()) → if (cond) { a(); b(); } else { c(); d(); }
    // Common webpack pattern for conditional side-effect blocks
    let input = r#"
arguments.length > 1 ? (check(a), check(b)) : (check(c), x = 0);
"#;
    let expected = r#"
if (arguments.length > 1) {
    check(a);
    check(b);
} else {
    check(c);
    x = 0;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
