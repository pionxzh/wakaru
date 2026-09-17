mod common;

use common::{assert_eq_normalized, render_pipeline, render_rule};
use wakaru_core::{rules::UnIife, RewriteLevel};

fn apply(input: &str) -> String {
    render_pipeline(input)
}

fn apply_rule(input: &str) -> String {
    apply_rule_with_level(input, RewriteLevel::Standard)
}

fn apply_rule_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |_| UnIife::new(level))
}

#[test]
fn preserves_async_expression_body_iife() {
    let input = "const promise = (async () => await first() + await second())();";
    let output = apply_rule_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn iife_single_char_params_renamed_to_longer_ident_args() {
    let input = r#"
(function(i, s, o, g, r, a, m) {
  i.GoogleAnalyticsObject = r;
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
  window_1.GoogleAnalyticsObject = r;
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
  i.GoogleAnalyticsObject = r;
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
  window_1.GoogleAnalyticsObject = r;
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
fn iife_mutated_literal_arg_extracts_to_let() {
    let input = r#"
(function (a, b) {
    for (let i = 0; i < 12; i++) {
        a += " ";
    };

    console.log("a length: ", a.length);
})(" ", 4);
"#;
    let expected = r#"
(() => {
  let a = " ";
  const b = 4;
  for (let i = 0; i < 12; i++) {
    a += " ";
  }
  ;
  console.log("a length: ", a.length);
})();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_literal_arg_rewrite_marks_written_param_as_let() {
    let input = r#"
((a, b) => {
  a++;
  return a + b;
})(1, 2);
"#;
    let expected = r#"
(() => {
  let a = 1;
  const b = 2;
  a++;
  return a + b;
})();
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_literal_arg_written_by_for_of_extracts_to_let() {
    let input = r#"
((a) => {
  for (a of items) {
    use(a);
  }
})(0);
"#;
    let expected = r#"
(() => {
  let a = 0;
  for (a of items) {
    use(a);
  }
})();
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_literal_arg_written_by_for_in_extracts_to_let() {
    let input = r#"
((a) => {
  for (a in object) {
    use(a);
  }
})("");
"#;
    let expected = r#"
(() => {
  let a = "";
  for (a in object) {
    use(a);
  }
})();
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_literal_arg_written_by_for_of_destructuring_extracts_to_let() {
    let input = r#"
((a) => {
  for ({ value: a } of items) {
    use(a);
  }
})(0);
"#;
    let expected = r#"
(() => {
  let a = 0;
  for ({ value: a } of items) {
    use(a);
  }
})();
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_literal_arg_written_by_for_await_of_extracts_to_let() {
    let input = r#"
(async (a) => {
  for await (a of items) {
    use(a);
  }
})(0);
"#;
    let expected = r#"
(async () => {
  let a = 0;
  for await (a of items) {
    use(a);
  }
})();
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_literal_arg_written_by_parenthesized_for_of_extracts_to_let() {
    let input = r#"
((a) => {
  for ((a) of items) {
    use(a);
  }
})(0);
"#;
    let expected = r#"
(() => {
  let a = 0;
  for (a of items) {
    use(a);
  }
})();
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_literal_arg_written_by_parenthesized_for_in_extracts_to_let() {
    let input = r#"
((a) => {
  for ((a) in object) {
    use(a);
  }
})("");
"#;
    let expected = r#"
(() => {
  let a = "";
  for (a in object) {
    use(a);
  }
})();
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_literal_arg_rewrite_marks_parenthesized_update_as_let() {
    let input = r#"
((a) => {
  (a)++;
  return a;
})(1);
"#;
    let expected = r#"
(() => {
  let a = 1;
  a++;
  return a;
})();
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_literal_arg_with_same_binding_var_redeclaration_is_preserved() {
    let input = r#"
((a) => {
  var a = 2;
  use(a);
})(0);
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn function_iife_literal_arg_with_same_binding_var_redeclaration_is_preserved() {
    let input = r#"
(function (a) {
  var a;
  use(a);
})(0);
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn iife_literal_arg_with_same_binding_function_redeclaration_is_preserved() {
    let input = r#"
((a) => {
  function a() {}
  use(a);
})(0);
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn iife_duplicate_params_with_literal_args_are_preserved() {
    let input = r#"
(function (a, a) {
  use(a);
})(1, 2);
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn iife_duplicate_params_with_ident_args_are_not_renamed() {
    let input = r#"
(function (a, a) {
  use(a);
})(first, second);
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn iife_direct_eval_mentioning_param_blocks_rewrites() {
    let input = r#"
(function (c, e) {
  eval("c = 9; use(e)");
  use(c, e);
})(0, target);
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn iife_direct_eval_with_unknown_source_blocks_all_rewrites() {
    let input = r#"
(function (c, e) {
  eval(code);
  use(c, e);
})(0, target);
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn iife_direct_eval_not_mentioning_param_allows_rewrites() {
    let input = r#"
((a) => {
  eval("g()");
  use(a);
})(1);
"#;
    let expected = r#"
(() => {
  const a = 1;
  eval("g()");
  use(a);
})();
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_direct_eval_in_nested_function_blocks_literal_extraction() {
    let input = r#"
((a) => {
  function inner() {
    eval("a = 2");
  }
  inner();
  use(a);
})(1);
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn iife_indirect_eval_does_not_block_literal_extraction() {
    let input = r#"
((a) => {
  (0, eval)("a");
  use(a);
})(1);
"#;
    let expected = r#"
(() => {
  const a = 1;
  (0, eval)("a");
  use(a);
})();
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn function_iife_eval_arguments_preserves_mapped_param_and_arg() {
    let input = r#"
(function (c) {
  eval("arguments[0] = 9");
  use(c);
})(0);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn function_iife_unknown_eval_preserves_function_shape_through_pipeline() {
    let input = r#"
(function (c) {
  eval(code);
  use(c);
})(0);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn function_iife_eval_mentioning_candidate_name_blocks_rename() {
    let input = r#"
(function (a) {
  eval("target_1 = 9");
  use(a);
})(target);
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn arrow_iife_eval_arguments_allows_literal_extraction() {
    let input = r#"
((a) => {
  eval("arguments[0] = 9");
  use(a);
})(0);
"#;
    let expected = r#"
(() => {
  const a = 0;
  eval("arguments[0] = 9");
  use(a);
})();
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn iife_param_with_longer_name_not_touched() {
    let input = r#"
((win, s, a) => {
  win.GoogleAnalyticsObject = 'ga';
  a = s.createElement('script');
  a.src = 'url';
})(window, document);
"#;
    // `win` is multi-char so it's left alone; `s` renames to `document_1`; `a`
    // has no arg so it's untouched too.
    let expected = r#"
((win, document_1, a) => {
  win.GoogleAnalyticsObject = 'ga';
  a = document_1.createElement('script');
  a.src = 'url';
})(window, document);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_does_not_rewrite_iife_params_or_literal_args() {
    let input = r#"
((i, s, o) => {
  return s.createElement(o);
})(window, document, 'script');
"#;
    let output = apply_rule_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn iife_arg_with_shorter_name_not_renamed() {
    let input = r#"
(function(i, s, a) {
  i.GoogleAnalyticsObject = 'ga';
  a = s.createElement('script');
  a.src = 'url';
})(w, document);
"#;
    // arg `w` is single-char so we leave param `i` alone; `s` renames to
    // `document_1`; `a` has no arg.
    let expected = r#"
((i, document_1, a) => {
  i.GoogleAnalyticsObject = 'ga';
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

// ============================================================
// `.call(thisArg, args...)` on an arrow IIFE
// ============================================================

#[test]
fn iife_dot_call_on_arrow_strips_this_arg() {
    // Arrow functions ignore `.call()`'s thisArg (their `this` is lexical),
    // so `.call(this, a, b)` is equivalent to `(a, b)` and can be stripped.
    let input = r#"((a, b) => { f(a, b); }).call(this, x, y);"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, r#"((a, b) => { f(a, b); })(x, y);"#);
}

#[test]
fn minimal_still_strips_dot_call_on_arrow() {
    let input = r#"((a) => { f(a); }).call(this, x);"#;
    let output = apply_rule_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, r#"((a) => { f(a); })(x);"#);
}

#[test]
fn iife_dot_call_on_arrow_with_null_this_arg_stripped() {
    // The thisArg value doesn't matter for arrows — strip regardless.
    let input = r#"((a) => { f(a); }).call(null, x);"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, r#"((a) => { f(a); })(x);"#);
}

#[test]
fn iife_dot_call_on_function_preserved() {
    // A plain `function` may reference its own `this` via `.call`'s thisArg.
    // UnIife must not rewrite this — `ArrowFunction` is responsible for proving
    // `this`/`arguments` are unused before the `.call` can be stripped (by UnIife2).
    let input = r#"(function(a) { this.x = a; }).call(obj, 1);"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn iife_dot_call_with_spread_this_arg_preserved() {
    // Spread in the thisArg slot means subsequent args don't line up with
    // params. Leave it alone.
    let input = r#"((a) => { f(a); }).call(...args);"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn iife_dot_apply_preserved() {
    // `.apply` takes an array; positional arg rewriting doesn't fit. Only
    // `.call` is handled.
    let input = r#"((a) => { f(a); }).apply(this, [1]);"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn iife_dot_call_module_21_pipeline_strips_wrapper() {
    // Module-21 style: a `function` IIFE with no `this` usage wrapped in
    // `.call(this, ...)` for global polyfill injection. After the full
    // pipeline, `ArrowFunction` converts fn→arrow and `UnIife2` strips the
    // now-dead `.call(this, ...)`.
    let input = r#"
(function(e, r) {
    var o = g(e, r);
    exports.a = o;
}).call(this, globalPoly, amdPoly(module));
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

// --- spread arguments break the positional argument/parameter pairing ---

#[test]
fn literal_after_spread_argument_is_not_paired_with_a_parameter() {
    // `...xs` has a runtime length: `3` initializes `c` when `xs` has two
    // elements, not `b`. Pairing it with `b` returned `[1, 2]` for `[1, 3]`.
    let input = r#"
function run(xs) {
  return (function(a, b, c) { return [a, c]; })(...xs, 3);
}
"#;
    // Only the printer's parenthesization differs from the input.
    let expected = r#"
function run(xs) {
  return function(a, b, c) {
    return [a, c];
  }(...xs, 3);
}
"#;
    assert_eq_normalized(&apply_rule(input), expected);
}

#[test]
fn identifier_after_spread_argument_is_not_renamed_into_a_parameter() {
    let input = r#"
function run(xs, value) {
  return (function(a, b) { return [a, b]; })(...xs, value);
}
"#;
    let expected = r#"
function run(xs, value) {
  return function(a, b) {
    return [a, b];
  }(...xs, value);
}
"#;
    assert_eq_normalized(&apply_rule(input), expected);
}

#[test]
fn literal_before_spread_argument_is_still_extracted() {
    // Positions before the spread are exact; only the tail is unknown.
    let input = r#"
function run(xs) {
  return (function(a, b, c) { return [a, b, c]; })(1, ...xs);
}
"#;
    let expected = r#"
function run(xs) {
  return function(b, c) {
    const a = 1;
    return [a, b, c];
  }(...xs);
}
"#;
    assert_eq_normalized(&apply_rule(input), expected);
}

#[test]
fn renamed_param_stays_visible_to_a_later_param_default() {
    // The default `r = t` reads a sibling parameter. Renaming `t` to
    // `locale_1` must rewrite that read too, or the default dangles.
    let input = r#"
((e, t, r = t, n) => {
  if (!a(e)) return e;
  return t !== r && n != null ? u(e, n) : e;
})(href, locale, g, prefix);
"#;
    let expected = r#"
((href_1, locale_1, r = locale_1, prefix_1) => {
  if (!a(href_1)) return href_1;
  return locale_1 !== r && prefix_1 != null ? u(href_1, prefix_1) : href_1;
})(href, locale, g, prefix);
"#;
    assert_eq_normalized(&apply_rule(input), expected);
}

#[test]
fn literal_param_read_by_a_later_param_default_stays_a_param() {
    // Extracting `e` into a body `const` would leave the default `t = e`
    // reading an undeclared name: parameter defaults cannot see body bindings.
    let input = r#"
(function(e, t = e) {
  use(e, t);
})(1, x);
"#;
    assert_eq_normalized(&apply_rule(input), input);
}

#[test]
fn parameter_rename_preserves_shorthand_keys_in_defaults_and_body() {
    for input in [
        "((e, t = {e}) => { use(t.e, {e}); })(source);",
        "(function(e, t = {e}) { use(t.e, {e}); })(source);",
    ] {
        let expected = input
            .replace("(e, t = {e})", "(source_1, t = {e: source_1})")
            .replace("use(t.e, {e})", "use(t.e, {e: source_1})");
        assert_eq_normalized(&apply_rule(input), &expected);
    }
}

#[test]
fn parameter_rename_avoids_bindings_inside_default_functions() {
    for input in [
        "((e, t = (source_1) => e) => { use(t(2)); })(source);",
        "(function(e, t = function(source_1) { return e; }) { use(t(2)); })(source);",
    ] {
        let expected = input
            .replace("(e, t =", "(source_2, t =")
            .replace("=> e)", "=> source_2)")
            .replace("return e;", "return source_2;");
        assert_eq_normalized(&apply_rule(input), &expected);
    }
}

#[test]
fn with_statement_keeps_iife_params_module_wide() {
    // A `with` anywhere in the module blocks param renames and literal
    // extraction (docs/rewrite-assumptions.md, dynamic-scope skip); the eval
    // side is already handled per IIFE body.
    let input = r#"
(function(e) {
  use(e);
})(1);
with (scope) { observe(); }
"#;
    assert_eq_normalized(&apply_rule(input), input);
}

#[test]
fn named_fn_expr_reenter_call_keeps_literal_param() {
    // A named function expression used as an IIFE may still be invoked again
    // with a different argument. Baking the IIFE literal into `const a = 0`
    // would make later `o(next)` drop that argument.
    let input = r#"
(function o(a) {
  if (a < 2) {
    return o(a + 1);
  }
  return a;
})(0);
"#;
    assert_eq_normalized(&apply_rule(input), input);
}

#[test]
fn named_fn_expr_reenter_new_keeps_literal_param() {
    let input = r#"
(function o(a) {
  return new o(a + 1);
})(0);
"#;
    assert_eq_normalized(&apply_rule(input), input);
}

#[test]
fn named_fn_expr_reenter_dot_call_keeps_literal_param() {
    let input = r#"
(function o(a) {
  return o.call(null, a + 1);
})(0);
"#;
    assert_eq_normalized(&apply_rule(input), input);
}

#[test]
fn named_fn_expr_escape_keeps_literal_param() {
    let input = r#"
(function o(a) {
  later(o);
  return a;
})(0);
"#;
    assert_eq_normalized(&apply_rule(input), input);
}

#[test]
fn named_fn_expr_eval_mentioning_name_keeps_literal_param() {
    // Known eval source can invoke the function name even though the body has
    // no identifier use of that binding.
    let input = r#"
(function o(a) {
  eval("o(1)");
  use(a);
})(0);
"#;
    assert_eq_normalized(&apply_rule(input), input);
}

#[test]
fn named_fn_expr_typeof_name_keeps_literal_param() {
    // Any live use of the function-name binding is fail-closed: `typeof o`
    // does not prove later code cannot observe a fresh call through eval or
    // an aliased binding we did not reconstruct.
    let input = r#"
(function o(a) {
  use(typeof o, a);
})(0);
"#;
    assert_eq_normalized(&apply_rule(input), input);
}

#[test]
fn unused_named_fn_expr_still_extracts_literal_param() {
    // A name kept only for stack traces is not a re-entry. Extraction stays.
    let input = r#"
(function o(a) {
  use(a);
})(0);
"#;
    let expected = r#"
(function o() {
  const a = 0;
  use(a);
})();
"#;
    assert_eq_normalized(&apply_rule(input), expected);
}

#[test]
fn shadowed_named_fn_expr_still_extracts_literal_param() {
    // Inner `var o` is a different binding after resolver. The function name
    // is unused, so the IIFE literal is still a single-invocation snapshot.
    let input = r#"
(function o(a) {
  var o = helper;
  use(a, o);
})(0);
"#;
    let expected = r#"
(function o() {
  const a = 0;
  var o = helper;
  use(a, o);
})();
"#;
    assert_eq_normalized(&apply_rule(input), expected);
}

#[test]
fn nested_param_shadowing_named_fn_expr_still_extracts_literal_param() {
    // Nested `function inner(o)` is not the IIFE name. Compare binding
    // identity, not the printed short name.
    let input = r#"
(function o(a) {
  function inner(o) {
    o(1);
  }
  use(a);
})(0);
"#;
    let expected = r#"
(function o() {
  const a = 0;
  function inner(o) {
    o(1);
  }
  use(a);
})();
"#;
    assert_eq_normalized(&apply_rule(input), expected);
}

#[test]
fn named_fn_expr_reenter_still_renames_ident_arg() {
    // Ident args stay parameters. Recursion must still receive the renamed
    // binding; do not skip the rename path just because the name re-enters.
    let input = r#"
(function o(a) {
  return o(next);
})(start);
"#;
    let expected = r#"
(function o(start_1) {
  return o(next);
})(start);
"#;
    assert_eq_normalized(&apply_rule(input), expected);
}

#[test]
fn paren_named_fn_expr_reenter_keeps_literal_param() {
    let input = r#"
(function o(a) {
  return o(next);
})(0);
"#;
    assert_eq_normalized(&apply_rule(input), input);
}

#[test]
fn named_fn_expr_reenter_keeps_mutated_literal_param() {
    let input = r#"
(function o(a) {
  a += 1;
  return o(a);
})(0);
"#;
    assert_eq_normalized(&apply_rule(input), input);
}

#[test]
fn named_fn_expr_reenter_from_param_default_keeps_literal_param() {
    // The name binding is visible in parameter initializers. `b = o` can
    // invoke the function again after the IIFE snapshot; extracting `a`
    // would freeze it and shift later arguments onto `b`.
    let input = r#"
(function o(a, b = o) {
  return a === 0 ? b(1) : a;
})(0);
"#;
    assert_eq_normalized(&apply_rule(input), input);
}

#[test]
fn named_fn_expr_eval_in_param_default_keeps_literal_param() {
    // Known eval in a default can invoke the name even when the body never
    // mentions it.
    let input = r#"
(function o(a, b = eval("o")) {
  use(a, b);
})(0);
"#;
    assert_eq_normalized(&apply_rule(input), input);
}
