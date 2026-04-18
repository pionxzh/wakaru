mod common;

use common::{assert_eq_normalized, render_pipeline};

fn apply(input: &str) -> String {
    render_pipeline(input)
}

#[test]
fn iife_single_char_params_renamed_to_longer_ident_args() {
    let input = r#"
(function(i, s, o, g, r, a, m) {
  i['GoogleAnalyticsObject'] = r;
  i[r] = i[r] || function() { (i[r].q = i[r].q||[]).push(arguments) }
  i[r].l = 1 * new Date();
  a = s.createElement(o);
  m = s.getElementsByTagName(o)[0];
  a.async = 1;
  a.src = g;
  m.parentNode.insertBefore(a, m);
})(window, document, 'script', 'https://www.google-analytics.com/analytics.js', 'ga');
"#;
    // The single-char ident params (i, s) rename to non-shadowing aliases.
    // The nested function's `arguments` binding does not make the outer IIFE's
    // arg list observable, so literal args can still become const declarations.
    let expected = r#"
((window_1, document_1, a, m) => {
  const O = 'script';
  const g = 'https://www.google-analytics.com/analytics.js';
  const r = 'ga';
  window_1['GoogleAnalyticsObject'] = r;
  window_1[r] = window_1[r] || function() { (window_1[r].q = window_1[r].q||[]).push(arguments) }
  window_1[r].l = 1 * new Date();
  a = document_1.createElement(O);
  m = document_1.getElementsByTagName(O)[0];
  a.async = 1;
  a.src = g;
  m.parentNode.insertBefore(a, m);
})(window, document);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_literal_args_extracted_to_const_when_no_arguments_usage() {
    let input = r#"
!function(i, s, o, g, r, a, m) {
  i['GoogleAnalyticsObject'] = r;
  i[r].l = 1 * new Date();
  a = s.createElement(o);
  m = s.getElementsByTagName(o)[0];
  a.async = 1;
  a.src = g;
  m.parentNode.insertBefore(a, m);
}(window, document, 'script', 'https://www.google-analytics.com/analytics.js', 'ga');
"#;
    // i, s rename; o, g, r literals become const decls; a, m have no args.
    let expected = r#"
!((window_1, document_1, a, m) => {
  const O = 'script';
  const g = 'https://www.google-analytics.com/analytics.js';
  const r = 'ga';
  window_1['GoogleAnalyticsObject'] = r;
  window_1[r].l = 1 * new Date();
  a = document_1.createElement(O);
  m = document_1.getElementsByTagName(O)[0];
  a.async = 1;
  a.src = g;
  m.parentNode.insertBefore(a, m);
})(window, document);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_param_with_longer_name_not_touched() {
    let input = r#"
((win, s, a) => {
  win['GoogleAnalyticsObject'] = 'ga';
  a = s.createElement('script');
  a.src = 'url';
})(window, document);
"#;
    // `win` is multi-char so it's left alone; `s` renames to `document_1`; `a`
    // has no arg so it's untouched too.
    let expected = r#"
((win, document_1, a) => {
  win['GoogleAnalyticsObject'] = 'ga';
  a = document_1.createElement('script');
  a.src = 'url';
})(window, document);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_arg_with_shorter_name_not_renamed() {
    let input = r#"
(function(i, s, a) {
  i['GoogleAnalyticsObject'] = 'ga';
  a = s.createElement('script');
  a.src = 'url';
})(w, document);
"#;
    // arg `w` is single-char so we leave param `i` alone; `s` renames to
    // `document_1`; `a` has no arg.
    let expected = r#"
((i, document_1, a) => {
  i['GoogleAnalyticsObject'] = 'ga';
  a = document_1.createElement('script');
  a.src = 'url';
})(w, document);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

/// When the arg's name collides with a binding in the IIFE body's function
/// scope, inlining is unsafe; substituting body refs would clash with the
/// existing `const path`. Fall back to a renamed param with a `_N` suffix so
/// the body still has two distinct bindings.
#[test]
fn iife_param_rename_synthesizes_suffix_when_arg_name_collides_with_body_binding() {
    let input = r#"
const path = "outer";
const value = 1;
((e, t) => {
  const path = "inner";
  return e + t + path;
})(path, value);
"#;
    // - `e`: arg `path` collides with body's `const path`: suffix-rename to `path_1`.
    // - `t`: arg `value` is also kept as a call-time snapshot: suffix-rename to `value_1`.
    let expected = r#"
const path = "outer";
const value = 1;
((path_1, value_1) => {
  const path = "inner";
  return path_1 + value_1 + path;
})(path, value);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

/// Identifier args stay as params so the IIFE keeps a call-time snapshot of
/// the argument binding. The nested arrow's same-named `e` parameter is
/// untouched because RenameIdent matches by `(sym, ctxt)`, not `sym` alone.
#[test]
fn iife_param_renamed_to_arg_alias_when_no_collision_or_mutation() {
    let input = r#"
const path = "abc";
const value = 1;
((e, t) => {
  const inner = (e) => e * 2;
  return inner(e) + t;
})(path, value);
"#;
    let expected = r#"
const path = "abc";
const value = 1;
((path_1, value_1) => {
  const inner = (e) => e * 2;
  return inner(path_1) + value_1;
})(path, value);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_identifier_arg_keeps_snapshot_before_later_reassignment() {
    let input = r#"
let path = "abc";
((e) => {
  return use(e);
})(path);
path = "def";
"#;
    let expected = r#"
let path = "abc";
((path_1) => use(path_1))(path);
path = "def";
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_identifier_arg_keeps_snapshot_for_returned_closure() {
    let input = r#"
let path = "abc";
const read = ((e) => {
  return () => e;
})(path);
path = "def";
"#;
    let expected = r#"
let path = "abc";
const read = ((path_1) => () => path_1)(path);
path = "def";
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

/// When the param is assigned in the body, inlining would mutate the outer
/// binding instead of the local copy, so we suffix-rename to keep the
/// local-mutation semantics without producing the redundant `(path)(path)`
/// shadowing artifact.
#[test]
fn iife_assigned_param_uses_suffix_rename_to_avoid_outer_shadow() {
    let input = r#"
const path = "outer";
((e) => {
  e = e + "/extra";
  return e;
})(path);
"#;
    // The pipeline rewrites `path_1 + "/extra"` to a template literal.
    let expected = r#"
const path = "outer";
((path_1) => {
  path_1 = `${path_1}/extra`;
  return path_1;
})(path);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

/// When a normal-function IIFE reads its own `arguments`, removing params or
/// call args changes observable runtime behavior. We can still rename the
/// params to clearer non-conflicting names, but the argument list must remain
/// positionally intact.
#[test]
fn iife_own_arguments_preserves_params_and_args() {
    let input = r#"
function d() {}
const path = "p";
const value = "v";
const event = {};
(function(e, t, n, r) {
  d.apply(this, arguments);
  return [e, t, n, r];
})(path, value, undefined, event);
"#;
    let expected = r#"
function d() {}
const path = "p";
const value = "v";
const event = {};
(function(path_1, value_1, n, event_1) {
  d.apply(this, arguments);
  return [path_1, value_1, n, event_1];
})(path, value, undefined, event);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

/// Babel's inline `_inherits` helper is recognized by the class rule from its
/// original two-param/two-arg IIFE shape. UnIife must not inline the superclass
/// arg before that rule can discover it.
#[test]
fn iife_preserves_inline_inherits_shape_for_class_rule() {
    let input = r#"
function Ctor() {}
const Super = function() {};
((e, t) => {
  e.prototype = Object.create(t && t.prototype, {
    constructor: {
      value: e,
      enumerable: false,
      writable: true,
      configurable: true
    }
  });
  t && (Object.setPrototypeOf ? Object.setPrototypeOf(e, t) : e.__proto__ = t);
})(Ctor, Super);
"#;
    let expected = r#"
function Ctor() {}
const Super = () => {};
((e, t) => {
  e.prototype = Object.create(t && t.prototype, {
    constructor: {
      value: e,
      enumerable: false,
      writable: true,
      configurable: true
    }
  });
  t && (Object.setPrototypeOf ? Object.setPrototypeOf(e, t) : e.__proto__ = t);
})(Ctor, Super);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
