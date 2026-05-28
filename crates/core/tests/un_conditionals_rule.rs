mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::{
    UnConditionals, UnConditionalsAssignmentOnly, UnConditionalsExprStmtOnly,
};

fn apply(input: &str) -> String {
    render_rule(input, |_| UnConditionals)
}

fn apply_assignment_only(input: &str) -> String {
    render_rule(input, |_| UnConditionalsAssignmentOnly)
}

fn apply_expr_stmt_only(input: &str) -> String {
    render_rule(input, |_| UnConditionalsExprStmtOnly)
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
fn assignment_only_converts_short_circuit_assignments_without_call_statements() {
    let input = r#"
x && call();
x || call();
x && (value = null);
x || (value = 1);
x && (a = 1, b = 2);
x && (call(), value = 3);
"#;
    let expected = r#"
x && call();
x || call();

if (x) {
  value = null;
}

if (!x) {
  value = 1;
}

if (x) {
  a = 1;
  b = 2;
}

x && (call(), value = 3);
"#;
    let output = apply_assignment_only(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn expr_stmt_only_converts_standalone_ternary_but_not_returns_or_logicals() {
    let input = r#"
condition ? (changed = true, notify()) : (changed = true, count++, count && (state = next));
flag && call();
function choose() {
  return condition ? first() : second();
}
"#;
    let expected = r#"
if (condition) {
  changed = true;
  notify();
} else {
  changed = true;
  count++;
  if (count) {
    state = next;
  }
}

flag && call();

function choose() {
  return condition ? first() : second();
}
"#;
    let output = apply_expr_stmt_only(input);
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
fn strict_return_ternary_chain_to_switch() {
    let input = r#"
function fn(kind) {
  return kind === "bar" ? bar() : kind === "baz" ? baz() : kind === "qux" ? qux() : quux();
}
"#;
    let expected = r#"
function fn(kind) {
  switch (kind) {
    case "bar":
      return bar();
    case "baz":
      return baz();
    case "qux":
      return qux();
    default:
      return quux();
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn strict_statement_ternary_chain_to_switch() {
    let input = r#"
kind === "bar" ? bar() : kind === "baz" ? baz() : kind === "qux" ? qux() : quux();
"#;
    let expected = r#"
switch (kind) {
  case "bar":
    bar();
    break;
  case "baz":
    baz();
    break;
  case "qux":
    qux();
    break;
  default:
    quux();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn switch_case_sequence_body_expands_without_block_wrapper() {
    let input = r#"
kind === "bar" ? (console.log(1), alert(2)) : kind === "baz" ? baz() : quux();
"#;
    let expected = r#"
switch (kind) {
  case "bar":
    console.log(1);
    alert(2);
    break;
  case "baz":
    baz();
    break;
  default:
    quux();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn switch_case_body_expands_nested_ternary() {
    let input = r#"
kind === "bar" ? (flag ? bar() : baz()) : kind === "qux" ? qux() : quux();
"#;
    let expected = r#"
switch (kind) {
  case "bar":
    if (flag) {
      bar();
    } else {
      baz();
    }
    break;
  case "qux":
    qux();
    break;
  default:
    quux();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn strict_literal_left_ternary_chain_to_switch() {
    let input = r#"
function fn(kind) {
  return "bar" === kind ? bar() : "baz" === kind ? baz() : quux();
}
"#;
    let expected = r#"
function fn(kind) {
  switch (kind) {
    case "bar":
      return bar();
    case "baz":
      return baz();
    default:
      return quux();
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn loose_equality_ternary_chain_stays_if_return_chain() {
    let input = r#"
function fn(kind) {
  return kind == "bar" ? bar() : kind == "baz" ? baz() : quux();
}
"#;
    let expected = r#"
function fn(kind) {
  if (kind == "bar") {
    return bar();
  }

  if (kind == "baz") {
    return baz();
  }

  return quux();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn member_discriminant_ternary_chain_stays_if_return_chain() {
    let input = r#"
function fn(obj) {
  return obj.kind === "bar" ? bar() : obj.kind === "baz" ? baz() : quux();
}
"#;
    let expected = r#"
function fn(obj) {
  if (obj.kind === "bar") {
    return bar();
  }

  if (obj.kind === "baz") {
    return baz();
  }

  return quux();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn mixed_discriminant_ternary_chain_stays_if_return_chain() {
    let input = r#"
function fn(kind, other) {
  return kind === "bar" ? bar() : other === "baz" ? baz() : quux();
}
"#;
    let expected = r#"
function fn(kind, other) {
  if (kind === "bar") {
    return bar();
  }

  if (other === "baz") {
    return baz();
  }

  return quux();
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
fn return_nested_consequent_ternary_split() {
    let input = r#"
function fn(match, Component, props, render, children) {
  return Component ? match ? createElement(Component, props) : null : render ? match ? render(props) : null : children;
}
"#;
    let expected = r#"
function fn(match, Component, props, render, children) {
  if (Component) {
    if (match) {
      return createElement(Component, props);
    }

    return null;
  }

  if (render) {
    if (match) {
      return render(props);
    }

    return null;
  }

  return children;
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

#[test]
fn sequence_branch_conditionals_are_converted_recursively() {
    let input = r#"
a ? (b ? c() : d || (e(), f = 5), g()) : h();
"#;
    let expected = r#"
if (a) {
  if (b) {
    c();
  } else if (!d) {
    e();
    f = 5;
  }
  g();
} else {
  h();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
