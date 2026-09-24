mod common;

use common::{assert_eq_normalized, render_rule};
use swc_core::common::Mark;
use swc_core::ecma::ast::Module;
use swc_core::ecma::visit::{VisitMut, VisitMutWith};
use wakaru_core::rules::{RewriteLevel, SimplifySequence, UnCurlyBraces};

fn apply(input: &str) -> String {
    render_rule(input, SimplifySequence::new)
}

fn apply_minimal(input: &str) -> String {
    render_rule(input, |unresolved_mark| {
        SimplifySequence::new_with_level(unresolved_mark, RewriteLevel::Minimal)
    })
}

fn apply_after_curly_braces(input: &str) -> String {
    struct CurlyThenSimplify {
        unresolved_mark: Mark,
    }

    impl VisitMut for CurlyThenSimplify {
        fn visit_mut_module(&mut self, module: &mut Module) {
            module.visit_mut_with(&mut UnCurlyBraces);
            module.visit_mut_with(&mut SimplifySequence::new(self.unresolved_mark));
        }
    }

    render_rule(input, |unresolved_mark| CurlyThenSimplify {
        unresolved_mark,
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

#[test]
fn splits_bare_yield_sequence_after_curly_braces() {
    let input = r#"
function* fib(limit) {
  let current = 0;
  let next = 1;
  while (current < limit)
    yield current, [current, next] = [next, current + next];
}
"#;
    let expected = r#"
function* fib(limit) {
  let current = 0;
  let next = 1;
  while (current < limit) {
    yield current;
    [current, next] = [next, current + next];
  }
}
"#;
    let output = apply_after_curly_braces(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_parenthesized_yield_sequence() {
    let input = r#"
function* fib(limit) {
  let current = 0;
  let next = 1;
  while (current < limit) {
    yield (current, [current, next] = [next, current + next]);
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
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
fn splits_swc_single_call_for_initializer() {
    // Produced by @swc/core minify with compress.sequences=true, mangle=false.
    let input = r#"
function run() {
  for (setup(); i < n; i++) work(i);
}
"#;
    let expected = r#"
function run() {
  setup();
  for (; i < n; i++) work(i);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_single_call_for_initializer_inside_conditional_branch() {
    let input = r#"
function run(flag) {
  if (flag) for (setup(); i < n; i++) work(i);
}
"#;
    let expected = r#"
function run(flag) {
  if (flag) {
    setup();
    for (; i < n; i++) work(i);
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_single_call_for_initializer_before_later_lexical_binding() {
    let input = r#"
{
  for (setup(later); false;) {}
  let later = 1;
}
"#;
    let expected = r#"
{
  setup(later);
  for (; false;) {}
  let later = 1;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_split_member_call_for_initializer() {
    let input = r#"
function run(iterator) {
  for (iterator.start(); !iterator.done(); ) work(iterator.value);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_split_directive_like_for_initializer() {
    let input = r#"
function run(flag) {
  for ("use strict"; flag; flag = false) work();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_split_tdz_sensitive_identifier_for_initializer() {
    let input = r#"
{
  for (later; false;) {}
  let later = 1;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
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
fn preserves_import_binding_reads_nested_in_void_expressions() {
    let input = r#"
import { x as y } from './self.js';
assert.throws(ReferenceError, function() {
  void y;
});
void (0, y);
void [y];
export const x = 23;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_tdz_read_nested_in_void_expression() {
    let input = r#"
{
  void x;
  let x;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_tdz_read_in_nested_block_before_outer_declaration() {
    let input = r#"
{
  {
    void x;
  }
  let x;
}
let initialized;
{
  void initialized;
}
"#;
    let expected = r#"
{
  {
    void x;
  }
  let x;
}
let initialized;
{
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_outer_lexical_read_in_hoisted_function_called_before_initialization() {
    let input = r#"
f();
let x;
function f() {
  void x;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn function_like_bodies_conservatively_preserve_outer_lexical_reads() {
    let input = r#"
let outer;
const arrow = () => {
  void outer;
};
const expression = function() {
  void outer;
};
const object = {
  method() {
    void outer;
  },
  get value() {
    void outer;
    return 1;
  },
  set value(next) {
    void outer;
  }
};
class C {
  constructor() {
    void outer;
  }
  method() {
    void outer;
  }
  get value() {
    void outer;
    return 1;
  }
  set value(next) {
    void outer;
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn function_body_still_drops_initialized_shadowing_lexical_read() {
    let input = r#"
let value;
function f() {
  let value;
  void value;
}
"#;
    let expected = r#"
let value;
function f() {
  let value;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn drops_safe_void_literal_no_op_statement() {
    let input = r#"
void 0;
void 1;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, "");
}

#[test]
fn drops_stable_builtin_member_reads_but_keeps_computed_key_reads() {
    let input = r#"
void Math.min;
void Math[missing];
"#;
    let expected = r#"
void Math[missing];
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn drops_void_wrapped_closures_without_executing_their_reads() {
    let input = r#"
let helper;
void (() => helper);
void function() { helper; };
"#;
    let expected = r#"
let helper;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_require_binding_read_statement_for_later_esm_recovery() {
    let input = r#"
var a = require("./dep.js");
a;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_new_expression_statement_with_spread_argument() {
    let input = r#"
var iter = {};
assert.throws(Test262Error, function() {
  new function() {}(...iter);
});
"#;
    let output = apply_minimal(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_nested_new_on_async_function_expression() {
    // `new` on a non-constructible callee throws a TypeError even when the
    // enclosing expression's value is unused. SimplifySequence relies on
    // swc_ecma_utils `may_have_side_effects` for this (fixed in 35.0.2); these
    // tests are the tripwire if a later swc_ecma_utils treats the callee as
    // pure again.
    let input = r#"
[new async function() {}];
"#;
    assert_eq_normalized(&apply(input), input);
    assert_eq_normalized(&apply_minimal(input), input);
}

#[test]
fn preserves_nested_new_on_generator_function_expression() {
    let input = r#"
[new function*() {}];
"#;
    assert_eq_normalized(&apply(input), input);
    assert_eq_normalized(&apply_minimal(input), input);
}

#[test]
fn preserves_deeply_nested_new_on_non_constructible_callee() {
    let input = r#"
[[new (async function*() {})]];
"#;
    let expected = r#"
[[new async function*() {}]];
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn drops_nested_new_on_empty_plain_function_expression() {
    // Control: `new` on an empty plain function expression stays droppable.
    let input = r#"
[new function() {}];
"#;
    let output = apply(input);
    assert_eq_normalized(&output, "");
}

#[test]
fn preserves_elision_only_array_assignment_pattern() {
    // `[,] = f()` advances the iterator once, so the statement is observable
    // even though it binds nothing. swc_ecma_parser 45.1.2 keeps the trailing
    // elision in assignment patterns (earlier versions parsed it as `[] = f()`);
    // this pins that the pattern reaches the rule intact and survives it.
    let input = r#"
[,] = f();
[, ,] = f();
[a, ,] = f();
"#;
    let expected = r#"
[,] = f();
[, ,] = f();
[a, ,] = f();
"#;
    assert_eq_normalized(&apply(input), expected);
    assert_eq_normalized(&apply_minimal(input), expected);
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
fn preserves_bigint_operator_throw_statements() {
    let input = r#"
1n + 1;
1n / 0n;
1n % 0n;
1n >>> 1n;
+1n;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_in_and_instanceof_throw_statements() {
    let input = r#"
"x" in true;
true instanceof true;
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

// Babel loose `for-of` packs Map entry unpack into the inner for-init:
// `for (var a, r = n.value, s = (r[0], r[1]), l = helper(s); !(a = l()).done;)`
// Sequence prefixes must not run before earlier declarators in the same list.

#[test]
fn for_var_init_sequence_prefix_does_not_read_earlier_declarator_before_init() {
    let input = r#"
function walk(n, helper) {
  for (var a, r = n.value, s = (r[0], r[1]), l = helper(s); !(a = l()).done;);
}
"#;
    let expected = r#"
function walk(n, helper) {
  var a, r = n.value;
  r[0];
  for (var s = r[1], l = helper(s); !(a = l()).done;);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn for_var_init_independent_sequence_prefix_keeps_prior_initializer_order() {
    // A call can observe any earlier `var` through effects or a closure even
    // when its argument AST does not directly reference that binding.
    let input = r#"
function run(log) {
  for (var a = log("init"), b = (log("prefix"), 1); false;);
}
"#;
    let expected = r#"
function run(log) {
  var a = log("init");
  log("prefix");
  for (var b = 1; false;);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn for_let_init_independent_later_prefix_keeps_prior_initializer_order() {
    // A lexical declarator cannot be flushed out of the loop, so keep the
    // later sequence intact rather than moving its effects before `a`.
    let input = r#"
function run(log) {
  for (let a = log("init"), b = (log("prefix"), 1); false;);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn for_let_init_sequence_prefix_that_reads_earlier_decl_stays_unsplit() {
    // Lexical for-init bindings cannot be hoisted out of the loop.
    let input = r#"
for (let r = n.value, s = (r[0], r[1]); r < 10; r++) {}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn for_const_init_sequence_prefix_that_reads_earlier_decl_stays_unsplit() {
    // Same fail-closed as `let`: do not pull const declarators out of the `for`.
    let input = r#"
for (const r = n.value, s = (r[0], r[1]); false; ) {}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn for_let_init_sequence_prefix_self_reference_stays_in_tdz() {
    let input = r#"
let x = 0;
for (let x = (x, 1); false;);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn for_const_init_sequence_prefix_later_reference_stays_in_tdz() {
    let input = r#"
const later = 0;
for (const x = (later, 1), later = 2; false;);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn for_let_init_shadowed_header_name_does_not_trigger_tdz_guard() {
    // Resolver identity distinguishes the IIFE parameter from the loop binding.
    let input = r#"
for (let x = ((function(x) { use(x); })(0), 1); false;);
"#;
    let expected = r#"
(function(x) { use(x); })(0);
for (let x = 1; false;);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn for_var_init_function_iife_prefix_keeps_expression_context() {
    // Lifted function-callee IIFE must stay an expression, not `function(){}()`.
    let input = r#"
function run() {
  for (var x = 0, y = ((function () {})(), 1); false;);
}
"#;
    let expected = r#"
function run() {
  var x = 0;
  (function() {})();
  for (var y = 1; false;);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn for_var_init_object_headed_prefix_keeps_expression_context() {
    // Lifted object-headed call chains must keep expression context.
    let input = r#"
function run(k, h) {
  for (var x = 0, y = ({ [k()]: h }[k()](), 1); false;);
}
"#;
    let expected = r#"
function run(k, h) {
  var x = 0;
  ({ [k()]: h })[k()]();
  for (var y = 1; false;);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn for_var_init_string_prefix_does_not_become_directive() {
    // A leading string statement can become a directive; leave this init unsplit.
    let input = r#"
function run() {
  for (var x = ("use strict", 1); false;);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn keeps_math_random_call_split_from_sequence() {
    // Math.random() advances the global PRNG even when its value is unused.
    // Splitting the comma list must not drop that call, and must not reorder
    // the later push.
    let input = r#"
function refresh() {
  before(), Math.round(2 * Math.random()), this.items.push(1);
}
"#;
    let expected = r#"
function refresh() {
  before();
  Math.round(2 * Math.random());
  this.items.push(1);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
    assert!(!output.contains("import "));
    assert!(!output.contains("export "));
}

#[test]
fn keeps_bare_math_random_statement() {
    let input = r#"
Math.random();
void Math.random();
Math.random?.();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn keeps_global_math_random_when_inner_binding_shadows_math() {
    // Binding identity is (sym, ctxt). A parameter or inner `var Math` is not
    // the global PRNG, and must not be rewritten. The unresolved calls stay.
    let input = r#"
function f(Math) {
  Math.random();
}
function outer() {
  Math.random();
  function inner() {
    var Math = {
      random() {
        return 0;
      }
    };
    Math.random();
  }
}
Math.random();
"#;
    let expected = r#"
function f(Math) {
  Math.random();
}
function outer() {
  Math.random();
  function inner() {
    var Math = {
      random () {
        return 0;
      }
    };
    Math.random();
  }
}
Math.random();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn drops_math_round_of_literal_and_non_calls() {
    // Other Math methods, and a reference that does not call Math.random,
    // stay removable. Directives stay.
    let input = r#"
"use strict";
Math.round(2);
void 0;
void Math.min;
void Math.random;
Math.round(Math.random);
return 1;
"#;
    let expected = r#"
"use strict";
return 1;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn drops_unevaluated_math_random_inside_closure() {
    // The arrow and the function expression are not invoked, so evaluating
    // the statement does not sample.
    let input = r#"
void (() => Math.random());
void function() {
  Math.random();
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, "");
}

#[test]
fn keeps_math_random_inside_array_object_and_pure_new() {
    // SWC recurses into these positions and still treats Math.random() as pure.
    // The member form is written without parens. The printer adds them;
    // the object is still the member's object, not a paren-wrapped no-op.
    let input = r#"
[Math.random()];
void [Math.random()];
Math.round([Math.random()]);
Math.max(...[Math.random()]);
[{ a: Math.random() }];
void { a: Math.random() };
void { [Math.random()]: 1 };
void { a: Math.random() }.a;
void new function() {}(Math.random());
"#;
    let expected = r#"
[Math.random()];
void [Math.random()];
Math.round([Math.random()]);
Math.max(...[Math.random()]);
[{ a: Math.random() }];
void { a: Math.random() };
void { [Math.random()]: 1 };
void ({ a: Math.random() }).a;
void new function() {}(Math.random());
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_unresolved_math_random_alias_and_computed_call_unchanged() {
    // An alias and a computed key are not the unresolved `Math.random` member.
    // Leave them as calls; do not invent a binding or rewrite the callee.
    let input = r#"
M.random();
Math["random"]();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn keeps_identifier_new_and_object_literal_statements() {
    let input = r#"
missing;
new Foo();
({ a: 1 });
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}
