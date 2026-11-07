mod common;

use common::{assert_eq_normalized, render_rule};
use swc_core::common::Mark;
use swc_core::ecma::ast::{CallExpr, Callee, Expr, MemberProp, Module};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};
use wakaru_core::{rules::UnArgumentSpread, RewriteLevel};

fn apply(input: &str) -> String {
    apply_with_level(input, RewriteLevel::Standard)
}

fn apply_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |unresolved_mark| {
        UnArgumentSpread::new(unresolved_mark, level)
    })
}

// Receiver and temp tests below pass an unknown argument list, which converts
// only at `aggressive` (`apply_arguments_are_arrays`); the receiver logic does
// not depend on the level. Argument-list proofs at `standard` are tested at
// the end of the file.

#[test]
fn converts_apply_with_undefined_to_spread_at_aggressive() {
    let input = r#"
fn.apply(undefined, args);
"#;
    let expected = r#"
fn(...args);
"#;
    let output = aggressive(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_does_not_convert_apply_with_undefined_to_spread() {
    let input = r#"
fn.apply(undefined, args);
"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_convert_apply_with_shadowed_undefined() {
    let input = r#"
function wrapper(undefined) {
  fn.apply(undefined, args);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn converts_apply_with_null_to_spread_at_aggressive() {
    let input = r#"
fn.apply(null, args);
"#;
    let expected = r#"
fn(...args);
"#;
    let output = aggressive(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn converts_obj_method_apply_with_same_obj_to_spread() {
    let input = r#"
obj.fn.apply(obj, someArray);
"#;
    let expected = r#"
obj.fn(...someArray);
"#;
    let output = aggressive(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_convert_member_apply_with_same_name_different_this_binding() {
    let input = r#"
obj.fn.apply(obj, args);
"#;
    let output = render_rule(input, |unresolved_mark| {
        MismatchThisArgBindingThenUnArgumentSpread { unresolved_mark }
    });
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_convert_apply_with_different_this() {
    // obj.fn.apply(otherObj, ...) — not converted because thisArg != obj
    let input = r#"
fn.apply(obj, someArray);
"#;
    let expected = r#"
fn.apply(obj, someArray);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

struct MismatchThisArgBindingThenUnArgumentSpread {
    unresolved_mark: Mark,
}

impl VisitMut for MismatchThisArgBindingThenUnArgumentSpread {
    fn visit_mut_module(&mut self, module: &mut Module) {
        module.visit_mut_with(&mut MismatchApplyThisArgBinding);
        module.visit_mut_with(&mut UnArgumentSpread::new(
            self.unresolved_mark,
            RewriteLevel::Standard,
        ));
    }
}

struct MismatchApplyThisArgBinding;

impl VisitMut for MismatchApplyThisArgBinding {
    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        call.visit_mut_children_with(self);

        if !is_apply_callee(&call.callee) {
            return;
        }

        let Some(first_arg) = call.args.get_mut(0) else {
            return;
        };

        if let Expr::Ident(ident) = first_arg.expr.as_mut() {
            if ident.sym.as_ref() == "obj" {
                ident.ctxt = ident.ctxt.apply_mark(Mark::new());
            }
        }
    }
}

fn is_apply_callee(callee: &Callee) -> bool {
    match callee {
        Callee::Expr(expr) => matches!(
            expr.as_ref(),
            Expr::Member(member)
                if matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "apply")
        ),
        _ => false,
    }
}

#[test]
fn does_not_convert_member_apply_with_null_this() {
    // obj.fn.apply(null, ...) — not converted because it changes `this` from
    // undefined to obj. The proper fix is namespace import decomposition
    // (obj.fn → fn), after which Pattern 1 handles it.
    let input = r#"
obj.fn.apply(null, someArray);
"#;
    let expected = r#"
obj.fn.apply(null, someArray);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn converts_this_method_apply_with_this_to_spread() {
    let input = r#"
function foo() {
  this.fn.apply(this, someArray);
}
"#;
    let expected = r#"
function foo() {
  this.fn(...someArray);
}
"#;
    let output = aggressive(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn converts_obj_method_apply_with_array_expression() {
    let input = r#"
obj.fn.apply(obj, [1, 2, 3]);
"#;
    let expected = r#"
obj.fn(1, 2, 3);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn converts_memoized_method_apply_with_same_receiver_temp() {
    let input = r#"
var _app_info;
const out = (_app_info = app_info).build.apply(_app_info, [prefix, ...items, tail]);
"#;
    let expected = r#"
const out = app_info.build(prefix, ...items, tail);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn converts_split_memoized_method_apply_with_same_receiver_temp() {
    let input = r#"
async function collect(output, item) {
  let method;
  let receiver;
  method = (receiver = output).push;
  method.apply(receiver, [await fetch_item(item.id)]);
}
"#;
    let expected = r#"
async function collect(output, item) {
  output.push(await fetch_item(item.id));
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn converts_split_memoized_method_apply_with_direct_receiver() {
    let input = r#"
function collect(output, args) {
  let method;
  method = output.push;
  method.apply(output, args);
}
"#;
    let expected = r#"
function collect(output, args) {
  output.push(...args);
}
"#;
    let output = aggressive(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_split_memoized_method_apply_with_member_chain_receiver() {
    // `root.child` is read once by the method assignment and once by the
    // `apply` thisArg; the converted `root.child.push(...args)` reads it
    // once, so a getter's evaluation count would change. Babel memoizes
    // member receivers into a temp, so this direct form is not producer
    // output — preserve it, as the bare form does.
    let input = r#"
function collect(root, args) {
  let method;
  method = root.child.push;
  method.apply(root.child, args);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_split_memoized_method_apply_with_call_receiver() {
    // The receiver is evaluated twice by the input; converting would drop
    // one `make()` call.
    let input = r#"
function collect(args) {
  let method;
  method = make().push;
  method.apply(make(), args);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_unmatched_statements_between_owned_split_apply_rewrites() {
    let input = r#"
function collect(first, second, args) {
  let firstMethod;
  let secondMethod;
  const config = {
    nested: { values: [1, 2, 3] },
    read() { return this.nested.values; }
  };
  firstMethod = first.push;
  firstMethod.apply(first, args);
  observe(config);
  secondMethod = second.push;
  secondMethod.apply(second, args);
}
"#;
    let expected = r#"
function collect(first, second, args) {
  const config = {
    nested: { values: [1, 2, 3] },
    read() { return this.nested.values; }
  };
  first.push(...args);
  observe(config);
  second.push(...args);
}
"#;
    let output = aggressive(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_split_memoized_method_apply_when_temps_are_used_later() {
    let input = r#"
function collect(output, args) {
  let method;
  let receiver;
  method = (receiver = output).push;
  method.apply(receiver, args);
  observe(method, receiver);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_split_memoized_method_apply_without_local_temp_decls() {
    let input = r#"
function collect(output, args) {
  method = output.push;
  method.apply(output, args);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_memoized_method_apply_with_different_receiver_temp() {
    let input = r#"
var _app_info;
const out = (_app_info = app_info).build.apply(other_info, [prefix, ...items, tail]);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn keeps_assignment_when_receiver_is_read_after_spread() {
    let input = r#"
let t;
(t = get()).push.apply(t, items);
t.length;
recover(t);
"#;
    let expected = r#"
let t;
(t = get()).push(...items);
t.length;
recover(t);
"#;
    assert_eq_normalized(&aggressive(input), expected);
}

#[test]
fn keeps_assignment_when_receiver_is_read_inside_arguments() {
    // The callee object runs before the arguments, so `l` there is `list[i]`.
    let input = r#"
let l;
(l = list[i]).run.apply(l, [head, l].concat(rest));
"#;
    let expected = r#"
let l;
(l = list[i]).run(...[head, l].concat(rest));
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn keeps_assignment_when_comma_value_is_the_receiver() {
    // The sequence stays an expression and yields the assigned object.
    let input = r#"
function init(t) {
  return (t = superGet()).init.apply(t, arguments), t;
}
"#;
    let expected = r#"
function init(t) {
  return (t = superGet()).init(...arguments), t;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn preserves_memoized_apply_with_compound_assignment() {
    let input = r#"
(t += get()).push.apply(t, items);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_apply_when_call_receiver_would_be_evaluated_twice() {
    // The input calls `make()` twice; the spread form would call it once.
    let input = r#"
make().push.apply(make(), args);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_split_assignment_when_only_receiver_temp_is_used_later() {
    let input = r#"
function collect(output, args) {
  let method;
  let receiver;
  method = (receiver = output).push;
  method.apply(receiver, args);
  observe(receiver);
}
"#;
    let expected = r#"
function collect(output, args) {
  let receiver;
  (receiver = output).push(...args);
  observe(receiver);
}
"#;
    assert_eq_normalized(&aggressive(input), expected);
}

#[test]
fn preserves_split_memoized_apply_when_method_temp_is_read_in_arguments() {
    // `method` in the arguments reads the assignment the rewrite deletes.
    let input = r#"
function collect(output) {
  let method;
  let receiver;
  method = (receiver = output).push;
  method.apply(receiver, [method]);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_split_memoized_apply_when_method_and_receiver_are_the_same_binding() {
    // The outer write makes `method` the function, so `thisArg` is not `output`.
    let input = r#"
function collect(output, args) {
  let method;
  method = (method = output).push;
  method.apply(method, args);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_a_method_temp_write_inside_the_computed_key() {
    // The key's write to `method` stays in the kept member, so its declaration
    // stays too; nothing reads `method` afterwards.
    let input = r#"
function collect(output, args) {
  let method;
  let receiver;
  method = (receiver = output)[method = "push"];
  method.apply(receiver, args);
}
"#;
    let expected = r#"
function collect(output, args) {
  let method;
  output[method = "push"](...args);
}
"#;
    assert_eq_normalized(&aggressive(input), expected);
}

#[test]
fn drops_isolated_memoized_receiver_temp() {
    let input = r#"
function add(items) {
  var _this$list;
  (_this$list = this.list).push.apply(_this$list, items);
}
"#;
    let expected = r#"
function add(items) {
  this.list.push(...items);
}
"#;
    assert_eq_normalized(&aggressive(input), expected);
}

#[test]
fn keeps_memoized_receiver_assignments_when_the_temp_is_reused() {
    // Each rewrite must account for every use of `_a`; with two sites neither
    // holds all of them, so both keep the assignment.
    let input = r#"
function add(x, y) {
  var _a;
  (_a = this.a).push.apply(_a, x);
  (_a = this.b).push.apply(_a, y);
}
"#;
    let expected = r#"
function add(x, y) {
  var _a;
  (_a = this.a).push(...x);
  (_a = this.b).push(...y);
}
"#;
    assert_eq_normalized(&aggressive(input), expected);
}

#[test]
fn keeps_reused_memoized_receiver_assignments_when_one_read_escapes() {
    let input = r#"
function add(x, y) {
  var _a;
  (_a = this.a).push.apply(_a, x);
  (_a = this.b).push.apply(_a, y);
  return _a;
}
"#;
    let expected = r#"
function add(x, y) {
  var _a;
  (_a = this.a).push(...x);
  (_a = this.b).push(...y);
  return _a;
}
"#;
    assert_eq_normalized(&aggressive(input), expected);
}

#[test]
fn keeps_assignment_when_receiver_temp_is_a_parameter() {
    // Sloppy-mode `arguments` aliases the parameter, so its write is observable.
    let input = r#"
function add(t, items) {
  (t = get()).push.apply(t, items);
  return arguments[0];
}
"#;
    let expected = r#"
function add(t, items) {
  (t = get()).push(...items);
  return arguments[0];
}
"#;
    assert_eq_normalized(&aggressive(input), expected);
}

#[test]
fn keeps_assignment_when_let_temp_is_in_its_tdz() {
    // The write throws before `let t` runs; dropping it would hide that.
    let input = r#"
function add(items) {
  (t = get()).push.apply(t, items);
  let t;
}
"#;
    let expected = r#"
function add(items) {
  (t = get()).push(...items);
  let t;
}
"#;
    assert_eq_normalized(&aggressive(input), expected);
}

#[test]
fn keeps_only_the_eval_visible_declaration_of_a_dropped_temp() {
    // Dynamic Scope Limits: an isolated compiler temp does not bail on direct
    // `eval`, but its declaration stays for eval to resolve.
    let input = r#"
function add(items) {
  var t;
  (t = get()).push.apply(t, items);
  return eval("t");
}
"#;
    let expected = r#"
function add(items) {
  var t;
  get().push(...items);
  return eval("t");
}
"#;
    assert_eq_normalized(&aggressive(input), expected);
}

#[test]
fn keeps_split_receiver_assignment_when_an_earlier_closure_reads_it() {
    let input = r#"
function collect(output, args) {
  let method;
  let receiver;
  const peek = () => receiver;
  method = (receiver = output).push;
  method.apply(receiver, args);
  return peek();
}
"#;
    let expected = r#"
function collect(output, args) {
  let receiver;
  const peek = () => receiver;
  (receiver = output).push(...args);
  return peek();
}
"#;
    assert_eq_normalized(&aggressive(input), expected);
}

#[test]
fn preserves_split_memoized_apply_when_an_earlier_closure_reads_method_temp() {
    let input = r#"
function collect(output, args) {
  let method;
  let receiver;
  const peek = () => method;
  method = (receiver = output).push;
  method.apply(receiver, args);
  return peek();
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_apply_with_member_chain_receiver() {
    // `root.child` is read twice by the input (callee chain and thisArg) and
    // would be read once by the converted output, changing a getter's
    // evaluation count. Babel memoizes member receivers, so the bare form
    // with a member-chain receiver is not producer output — preserve it.
    let input = r#"
root.child.method.apply(root.child, [1, 2]);
"#;
    let expected = r#"
root.child.method.apply(root.child, [1, 2]);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn keeps_assignment_when_receiver_temp_is_exported() {
    // Importers read the live `t` binding, so its write is observable.
    let input = r#"
export var t;
(t = get()).push.apply(t, items);
"#;
    let expected = r#"
export var t;
(t = get()).push(...items);
"#;
    assert_eq_normalized(&aggressive(input), expected);
}

#[test]
fn converts_split_memoized_apply_with_temps_declared_in_an_outer_block() {
    let input = r#"
function collect(output, args) {
  var method, receiver;
  if (ready) {
    method = (receiver = output).push;
    method.apply(receiver, args);
  }
}
"#;
    let expected = r#"
function collect(output, args) {
  if (ready) {
    output.push(...args);
  }
}
"#;
    assert_eq_normalized(&aggressive(input), expected);
}

#[test]
fn keeps_unbound_apply_in_module_with_with_statement() {
    // `fn(...args)` would pass `o` as `this` when `fn` resolves through it.
    let input = r#"
with (o) {
    g(fn.apply(null, args));
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_unbound_apply_in_module_with_direct_eval() {
    let input = r#"
function f(s, fn, args) {
    eval(s);
    return fn.apply(undefined, args);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn recovers_receiver_apply_in_module_with_direct_eval() {
    let input = r#"
eval(s);
obj.fn.apply(obj, args);
"#;
    let expected = r#"
eval(s);
obj.fn(...args);
"#;
    assert_eq_normalized(&aggressive(input), expected);
}

// `apply` reads `length` and indexes of any array-like and treats
// `null`/`undefined` as no arguments; spread iterates and throws on both. At
// `standard` only proven argument lists convert (`apply_arguments_are_arrays`
// covers the rest at `aggressive`).

fn aggressive(input: &str) -> String {
    apply_with_level(input, RewriteLevel::Aggressive)
}

#[test]
fn unknown_argument_list_keeps_apply_at_standard() {
    let input = r#"
fn.apply(undefined, args);
fn.apply(void 0, args);
obj.fn.apply(obj, args);
this.fn.apply(this, args);
const out = (_t = get()).fn.apply(_t, args);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn unknown_argument_list_converts_at_aggressive() {
    let input = r#"
fn.apply(void 0, args);
obj.fn.apply(obj, args);
"#;
    let expected = r#"
fn(...args);
obj.fn(...args);
"#;
    assert_eq_normalized(&aggressive(input), expected);
}

#[test]
fn split_memoized_apply_with_unknown_arguments_keeps_apply_at_standard() {
    let input = r#"
let _method, _receiver;
_method = (_receiver = output).push;
_method.apply(_receiver, items);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn null_receiver_keeps_apply_at_standard() {
    // A strict callee sees `this === null` through `apply` and `undefined` in
    // a bare call.
    let input = r#"
fn.apply(null, [1]);
"#;
    assert_eq_normalized(&apply(input), input);
    let expected = r#"
fn(1);
"#;
    assert_eq_normalized(&aggressive(input), expected);
}

#[test]
fn array_literal_argument_list_converts_at_standard() {
    let input = r#"
fn.apply(void 0, [a, b]);
obj.fn.apply(obj, [a, ...rest]);
"#;
    let expected = r#"
fn(a, b);
obj.fn(a, ...rest);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn rest_parameter_argument_list_converts_at_standard() {
    let input = r#"
function f(...args) {
    return g.apply(void 0, args);
}
const h = (...args) => g.apply(void 0, args);
"#;
    let expected = r#"
function f(...args) {
    return g(...args);
}
const h = (...args) => g(...args);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reassigned_rest_parameter_keeps_apply() {
    let input = r#"
function f(...args) {
    args = null;
    return g.apply(void 0, args);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn arguments_object_converts_at_standard() {
    // producer babel-core@6.26.3 {"presets":["es2015"]}:
    // `target(...args)` for a rest `args` becomes `target.apply(undefined, arguments)`.
    let input = r#"
function f() {
    return target.apply(undefined, arguments);
}
"#;
    let expected = r#"
function f() {
    return target(...arguments);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn assigned_arguments_keeps_apply() {
    let input = r#"
function f() {
    arguments = null;
    return target.apply(undefined, arguments);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn declared_arguments_binding_keeps_apply() {
    let input = r#"
function f(arguments) {
    return target.apply(undefined, arguments);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn typescript_arguments_copy_converts_at_standard() {
    // producer typescript@5.9.3 {"module":"CommonJS","target":"ES5"} (same in
    // typescript@3.8.3): a rest parameter forwarded by spread.
    let input = r#"
function forward() {
    var args = [];
    for (var _i = 0; _i < arguments.length; _i++) {
        args[_i] = arguments[_i];
    }
    return target.apply(void 0, args);
}
function forwardMethod(obj) {
    var rest = [];
    for (var _i = 1; _i < arguments.length; _i++) {
        rest[_i - 1] = arguments[_i];
    }
    return obj.method.apply(obj, rest);
}
"#;
    let expected = r#"
function forward() {
    var args = [];
    for (var _i = 0; _i < arguments.length; _i++) {
        args[_i] = arguments[_i];
    }
    return target(...args);
}
function forwardMethod(obj) {
    var rest = [];
    for (var _i = 1; _i < arguments.length; _i++) {
        rest[_i - 1] = arguments[_i];
    }
    return obj.method(...rest);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn babel_arguments_copy_loop_converts_at_standard() {
    // producer babel-core@6.26.3 {"presets":["es2015"]}: the rest copy loop,
    // then a `.concat` on an array literal.
    let input = r#"
function prefix(a) {
    for (var _len = arguments.length, rest = Array(_len > 1 ? _len - 1 : 0), _key = 1; _key < _len; _key++) {
        rest[_key - 1] = arguments[_key];
    }
    obj.method.apply(obj, rest);
    return target.apply(undefined, [a].concat(rest));
}
"#;
    let expected = r#"
function prefix(a) {
    for (var _len = arguments.length, rest = Array(_len > 1 ? _len - 1 : 0), _key = 1; _key < _len; _key++) {
        rest[_key - 1] = arguments[_key];
    }
    obj.method(...rest);
    return target(...[a].concat(rest));
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn local_array_binding_converts_after_its_declaration() {
    // producer typescript@5.9.3 {"module":"CommonJS","target":"ES5"}:
    // `const xs = [1, 2]; target(...xs)`.
    let input = r#"
function fromLocal() {
    var xs = [1, 2];
    return target.apply(void 0, xs);
}
function fromConst() {
    const xs = [1, 2];
    return target.apply(void 0, xs);
}
"#;
    let expected = r#"
function fromLocal() {
    var xs = [1, 2];
    return target(...xs);
}
function fromConst() {
    const xs = [1, 2];
    return target(...xs);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn var_array_read_before_its_declaration_keeps_apply() {
    // The `var` is still `undefined` at the first call, which `apply` accepts
    // and spread does not.
    let input = r#"
function f() {
    target.apply(void 0, xs);
    var xs = [1, 2];
    return target.apply(void 0, xs);
}
"#;
    let expected = r#"
function f() {
    target.apply(void 0, xs);
    var xs = [1, 2];
    return target(...xs);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn var_array_read_in_hoisted_function_declaration_keeps_apply() {
    // `h` is hoisted and may run before `xs` is assigned.
    let input = r#"
function f() {
    h();
    var xs = [1, 2];
    function h() {
        return target.apply(void 0, xs);
    }
    const k = () => target.apply(void 0, xs);
    return k;
}
"#;
    let expected = r#"
function f() {
    h();
    var xs = [1, 2];
    function h() {
        return target.apply(void 0, xs);
    }
    const k = () => target(...xs);
    return k;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reassigned_or_redeclared_array_binding_keeps_apply() {
    let input = r#"
function f(other) {
    var xs = [1, 2];
    xs = other;
    return target.apply(void 0, xs);
}
function g(other) {
    var ys = [1, 2];
    var ys = other;
    return target.apply(void 0, ys);
}
function h(other) {
    let zs = [1, 2];
    zs = other;
    return target.apply(void 0, zs);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn shadowed_array_constructor_keeps_apply() {
    let input = r#"
function f(Array) {
    var xs = new Array(2);
    return target.apply(void 0, xs);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn module_level_var_array_converts_after_its_declaration() {
    let input = r#"
var xs = [1, 2];
target.apply(void 0, xs);
"#;
    let expected = r#"
var xs = [1, 2];
target(...xs);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn pipeline_keeps_apply_semantics_for_unproven_argument_lists() {
    // `null` receiver for a strict callee, an `undefined` argument list, and a
    // non-iterable array-like: each behaves differently as a spread call.
    let input = r#"
"use strict";
function isNullThis() { return this === null; }
function count() { return arguments.length; }
var empty;
var arrayLike = { length: 2 };
console.log(isNullThis.apply(null, [1]));
console.log(count.apply(null, empty));
console.log(count.apply(null, arrayLike));
"#;
    let output = common::render(input);
    assert!(output.contains("isNullThis.apply(null, ["), "{output}");
    assert!(output.contains("count.apply(null, empty)"), "{output}");
    assert!(output.contains("count.apply(null, arrayLike)"), "{output}");
}

#[test]
fn pipeline_recovers_typescript_es5_rest_forwarding() {
    // producer typescript@5.9.3 {"module":"CommonJS","target":"ES5"}:
    // `function forward(...args) { return target(...args); }` and
    // `function fromLocal() { const xs = [1, 2]; return target(...xs); }`.
    let input = r#"
function forward() {
    var args = [];
    for (var _i = 0; _i < arguments.length; _i++) {
        args[_i] = arguments[_i];
    }
    return target.apply(void 0, args);
}
function fromLocal() {
    var xs = [1, 2];
    return target.apply(void 0, xs);
}
"#;
    let expected = r#"
function forward(...args) {
    return target(...args);
}
function fromLocal() {
    const xs = [1, 2];
    return target(...xs);
}
"#;
    assert_eq_normalized(&common::render(input), expected);
}

#[test]
fn pipeline_recovers_babel6_rest_forwarding() {
    // producer babel-core@6.26.3 {"presets":["es2015"]}:
    // `function forward(...args) { return target(...args); }` and
    // `function prefix(a, ...rest) { return target(a, ...rest); }`.
    let input = r#"
function forward() {
    return target.apply(undefined, arguments);
}
function prefix(a) {
    for (var _len = arguments.length, rest = Array(_len > 1 ? _len - 1 : 0), _key = 1; _key < _len; _key++) {
        rest[_key - 1] = arguments[_key];
    }
    return target.apply(undefined, [a].concat(rest));
}
"#;
    let output = common::render(input);
    assert!(output.contains("return target(...arguments);"), "{output}");
    assert!(output.contains("function prefix(a, ...rest)"), "{output}");
    assert!(!output.contains(".apply("), "{output}");
}

#[test]
fn array_method_result_of_a_proven_array_converts() {
    let input = r#"
function applyAll(...fns) {
    var chain = fns.map(function (f) { return f(api); });
    return compose.apply(undefined, chain);
}
function sliced(...xs) {
    return target.apply(void 0, xs.filter(Boolean).slice(1));
}
"#;
    let expected = r#"
function applyAll(...fns) {
    var chain = fns.map(function (f) { return f(api); });
    return compose(...chain);
}
function sliced(...xs) {
    return target(...xs.filter(Boolean).slice(1));
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn array_method_result_of_an_unproven_receiver_keeps_apply() {
    // A non-Array receiver can define its own `map` returning anything.
    let input = r#"
function f(list) {
    var chain = list.map(g);
    target.apply(void 0, chain);
    return target.apply(void 0, list.slice(1));
}
function h() {
    var chain = arguments.map(g);
    return target.apply(void 0, chain);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn array_from_and_of_argument_lists_convert() {
    let input = r#"
nodes.push.apply(nodes, Array.from(list));
target.apply(void 0, Array.of(a, b));
"#;
    let expected = r#"
nodes.push(...Array.from(list));
target(...Array.of(a, b));
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn shadowed_array_from_keeps_apply() {
    let input = r#"
function f(Array) {
    return target.apply(void 0, Array.from(list));
}
"#;
    assert_eq_normalized(&apply(input), input);
}

// A spread-helper argument is producer evidence that the call was a spread
// call (`lowered_spread_call_source_semantics`): its receiver is a lowering
// artifact, so `null`, a duplicated member-chain receiver, and `this` on a
// call-result callee recover to the source call.

#[test]
fn closure_named_array_from_iterable_recovers_at_standard() {
    // producer google-closure-compiler@20260629.0.0 --compilation_level=SIMPLE
    // --language_out=ECMASCRIPT5 (the parenthesized form; 20240317.0.0 emits
    // the bare member call).
    let input = r#"
function plain(fn, xs) {
    return fn.apply(null, (0, $jscomp.arrayFromIterable)(xs));
}
function method(obj, xs) {
    return obj.m.apply(obj, $jscomp.arrayFromIterable(xs));
}
"#;
    let expected = r#"
function plain(fn, xs) {
    return fn(...xs);
}
function method(obj, xs) {
    return obj.m(...xs);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn closure_renamed_array_from_iterable_recovers_with_its_receivers() {
    // producer google-closure-compiler@20260629.0.0
    // --compilation_level=ADVANCED --language_out=ECMASCRIPT5: the helper is
    // renamed, and `box.inner.m(...xs)` reads `box.inner` twice.
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
function plain(g, a) {
    return g.apply(null, u(a));
}
function chain(b, a) {
    return b.a.g.apply(b.a, u(a));
}
function self(a) {
    return this.g.apply(this, u(a));
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
function plain(g, a) {
    return g(...u(a));
}
function chain(b, a) {
    return b.a.g(...u(a));
}
function self(a) {
    return this.g(...u(a));
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn recovered_spread_array_literal_is_evidence_for_the_receiver() {
    // producer @swc/core@1.16.2 {"jsc":{"target":"es5"}}:
    // `make()(...xs)` becomes `make().apply(this, _to_consumable_array(xs))`,
    // and UnToConsumableArray has turned the helper into `[...xs]`. Closure's
    // `[a].concat($jscomp.arrayFromIterable(xs))` becomes `[a, ...xs]`.
    let input = r#"
function callCallee(make, xs) {
    return make().apply(this, [...xs]);
}
function prefix(fn, xs) {
    return fn.apply(null, [0, ...xs]);
}
"#;
    let expected = r#"
function callCallee(make, xs) {
    return make()(...xs);
}
function prefix(fn, xs) {
    return fn(0, ...xs);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn receivers_without_spread_evidence_keep_apply() {
    // Hand-written code forwards `this` to a computed function and reads a
    // member-chain receiver twice; an array literal without a spread element
    // carries no evidence.
    let input = r#"
function forward(xs) {
    return this.getHandler().apply(this, [xs, 1]);
}
function chain(root) {
    return root.a.m.apply(root.a, [1, 2]);
}
function nullReceiver(fn) {
    return fn.apply(null, [1, 2]);
}
function callInReceiver(root, xs) {
    return root.a().m.apply(root.a(), [...xs]);
}
"#;
    assert_eq_normalized(&apply(input), input);
    assert_eq_normalized(
        &aggressive(input),
        r#"
function forward(xs) {
    return this.getHandler().apply(this, [xs, 1]);
}
function chain(root) {
    return root.a.m.apply(root.a, [1, 2]);
}
function nullReceiver(fn) {
    return fn(1, 2);
}
function callInReceiver(root, xs) {
    return root.a().m.apply(root.a(), [...xs]);
}
"#,
    );
}

#[test]
fn shadowed_jscomp_is_not_closure_evidence() {
    let input = r#"
function f($jscomp, fn, xs) {
    return fn.apply(null, $jscomp.arrayFromIterable(xs));
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn pipeline_recovers_closure_advanced_spread_calls() {
    // producer google-closure-compiler@20260629.0.0
    // --compilation_level=ADVANCED --language_out=ECMASCRIPT5 (helper `f` is
    // the renamed `$jscomp.arrayFromIterable`; the source called `fn(...xs)`,
    // `obj.a.m(...xs)`, `this.m(...xs)`, `make()(...xs)` and `fn(a, ...xs)`).
    let input = r#"
function d(a) {
  var b = 0;
  return function() {
    return b < a.length ? {done:!1, value:a[b++]} : {done:!0};
  };
}
function f(a) {
  if (!(a instanceof Array)) {
    var b = typeof Symbol != "undefined" && Symbol.iterator && a[Symbol.iterator];
    if (b) {
      a = b.call(a);
    } else {
      if (typeof a.length == "number") {
        a = {next:d(a)};
      } else {
        throw Error(String(a) + " is not an iterable or ArrayLike");
      }
    }
    for (var c = []; !(b = a.next()).done;) {
      c.push(b.value);
    }
    a = c;
  }
  return a;
}
globalThis.o = {u:function(a, b) {
  return a.apply(null, f(b));
}, j:function(a, b) {
  return a.a.g.apply(a.a, f(b));
}, A:function(a) {
  return this.g.apply(this, f(a));
}, i:function(a, b) {
  return a().apply(null, f(b));
}, prefix:function(a, b, c) {
  return a.apply(null, [b].concat(f(c)));
}};
"#;
    let output = common::render(input);
    assert!(output.contains("return a(...f(b));"), "{output}");
    assert!(output.contains("return a.a.g(...f(b));"), "{output}");
    assert!(output.contains("return this.g(...f(a));"), "{output}");
    assert!(output.contains("return a()(...f(b));"), "{output}");
    assert!(output.contains("return a(b, ...f(c));"), "{output}");
    assert!(!output.contains(".apply("), "{output}");
}

#[test]
fn pipeline_recovers_swc_call_result_spread_call() {
    // producer @swc/core@1.16.2 {"jsc":{"target":"es5"},"module":{"type":"commonjs"}}:
    // `make()(...xs)` becomes `make().apply(this, _to_consumable_array(xs))`.
    let input = r#"
function _array_like_to_array(arr, len) {
    if (len == null || len > arr.length) len = arr.length;
    for(var i = 0, arr2 = new Array(len); i < len; i++)arr2[i] = arr[i];
    return arr2;
}
function _array_without_holes(arr) {
    if (Array.isArray(arr)) return _array_like_to_array(arr);
}
function _construct(Parent, args, Class) {
    if (_is_native_reflect_construct()) _construct = Reflect.construct;
    else {
        _construct = function construct(Parent, args, Class) {
            var a = [
                null
            ];
            a.push.apply(a, args);
            var Constructor = Function.bind.apply(Parent, a);
            var instance = new Constructor();
            if (Class) _set_prototype_of(instance, Class.prototype);
            return instance;
        };
    }
    return _construct.apply(null, arguments);
}
function _is_native_reflect_construct() {
    try {
        var result = !Boolean.prototype.valueOf.call(Reflect.construct(Boolean, [], function() {}));
    } catch (_) {}
    return (_is_native_reflect_construct = function() {
        return !!result;
    })();
}
function _iterable_to_array(iter) {
    if (typeof Symbol !== "undefined" && iter[Symbol.iterator] != null || iter["@@iterator"] != null) {
        return Array.from(iter);
    }
}
function _non_iterable_spread() {
    throw new TypeError("Invalid attempt to spread non-iterable instance.\nIn order to be iterable, non-array objects must have a [Symbol.iterator]() method.");
}
function _set_prototype_of(o, p) {
    _set_prototype_of = Object.setPrototypeOf || function setPrototypeOf(o, p) {
        o.__proto__ = p;
        return o;
    };
    return _set_prototype_of(o, p);
}
function _to_consumable_array(arr) {
    return _array_without_holes(arr) || _iterable_to_array(arr) || _unsupported_iterable_to_array(arr) || _non_iterable_spread();
}
function _unsupported_iterable_to_array(o, minLen) {
    if (!o) return;
    if (typeof o === "string") return _array_like_to_array(o, minLen);
    var n = Object.prototype.toString.call(o).slice(8, -1);
    if (n === "Object" && o.constructor) n = o.constructor.name;
    if (n === "Map" || n === "Set") return Array.from(n);
    if (n === "Arguments" || /^(?:Ui|I)nt(?:8|16|32)(?:Clamped)?Array$/.test(n)) return _array_like_to_array(o, minLen);
}
function callCallee(make, xs) {
    return make().apply(this, _to_consumable_array(xs));
}
globalThis.out = { callCallee };
"#;
    let output = common::render(input);
    assert!(output.contains("return make()(...xs);"), "{output}");
    assert!(!output.contains("make().apply("), "{output}");
}
