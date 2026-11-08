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

// Bound constructs. `new (F.bind(t, ...args))()` constructs the same object
// as `new F(...args)`: [[Construct]] of a bound function ignores the bound
// `this` and passes the target as `new.target`.

#[test]
fn bound_construct_of_a_proven_list_becomes_a_spread_construct() {
    // producer typescript@5.9.3 ES5, after UnToConsumableArray turned
    // `__spreadArray([void 0], xs, false)` into `[void 0, ...xs]`.
    let input = r#"
function make(C, xs) {
    return new (C.bind.apply(C, [void 0, ...xs]))();
}
function lead(C, xs) {
    return new (C.bind.apply(C, [void 0, 0, ...xs]))();
}
function readProp(C, xs) {
    return new (C.bind.apply(C, [void 0, ...xs]))().args;
}
"#;
    let expected = r#"
function make(C, xs) {
    return new C(...xs);
}
function lead(C, xs) {
    return new C(0, ...xs);
}
function readProp(C, xs) {
    return new C(...xs).args;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn bound_construct_accepts_every_effect_free_bound_this() {
    let input = r#"
a = new (C.bind(undefined, ...xs))();
b = new (C.bind(void 0, x))();
c = new (C.bind(null, ...xs, y))();
"#;
    let expected = r#"
a = new C(...xs);
b = new C(x);
c = new C(...xs, y);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn function_prototype_bind_apply_construct_becomes_a_spread_construct() {
    // producer babel-core@6.26.3 es2015, after the concat rules made
    // `[null].concat(_toConsumableArray(xs))` an array literal; producer
    // google-closure-compiler@20260629.0.0 omits the outer `()`.
    let input = r#"
a = new (Function.prototype.bind.apply(C, [null, ...xs]))();
b = new (Function.prototype.bind.apply(C, [null, 0, ...xs]));
c = (new (Function.prototype.bind.apply(box.inner.Ctor, [null, ...xs]))).args;
d = new (Function.bind.apply(C, [null, ...xs]))();
"#;
    let expected = r#"
a = new C(...xs);
b = new C(0, ...xs);
c = new box.inner.Ctor(...xs).args;
d = new C(...xs);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn memoized_bind_receiver_drops_an_isolated_temp() {
    // producer typescript@5.9.3 ES5 memoizes a member constructor.
    let input = r#"
function make(box, xs) {
    var _a;
    return new ((_a = box.inner.Ctor).bind.apply(_a, [void 0, ...xs]))();
}
"#;
    let expected = r#"
function make(box, xs) {
    return new box.inner.Ctor(...xs);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn memoized_bind_receiver_keeps_a_temp_with_another_use() {
    let input = r#"
function make(box, xs) {
    var _a;
    var made = new ((_a = box.inner.Ctor).bind.apply(_a, [void 0, ...xs]))();
    return [made, _a];
}
"#;
    let expected = r#"
function make(box, xs) {
    var _a;
    var made = new (_a = box.inner.Ctor)(...xs);
    return [made, _a];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn bound_construct_keeps_an_observable_bound_this() {
    // The construct ignores the bound `this`, but evaluating it can have an
    // effect; only `null`, `void 0` and `undefined` are dropped.
    let input = r#"
a = new (C.bind(ctx, ...xs))();
b = new (C.bind(getThis(), ...xs))();
c = new (Function.prototype.bind.apply(C, [ctx, ...xs]))();
d = new (C.bind(...xs))();
e = new (Function.prototype.bind.apply(C, [...xs]))();
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn bound_construct_keeps_outer_arguments() {
    let input = r#"
a = new (C.bind(undefined, ...xs))(extra);
b = new (Function.prototype.bind.apply(C, [null, ...xs]))(extra);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn bind_apply_with_a_different_target_is_kept() {
    // `C.bind.apply(D, …)` binds `D` with `C`'s `bind`: the `apply` rewrite
    // keeps it, so no bound construct appears.
    let input = r#"
function make(C, D, xs) {
    return new (C.bind.apply(D, [void 0, ...xs]))();
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn function_bind_apply_construct_needs_the_global_function() {
    let input = r#"
function make(Function, C, xs) {
    return new (Function.prototype.bind.apply(C, [null, ...xs]))();
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn function_bind_apply_construct_skips_a_module_with_dynamic_scope() {
    let input = r#"
with (scope) {
    a = new (Function.prototype.bind.apply(C, [null, ...xs]))();
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn function_bind_apply_construct_keeps_a_list_with_holes() {
    let input = r#"
a = new (Function.prototype.bind.apply(C, [null, , x]))();
b = new (Function.prototype.bind.apply(C, [, x]))();
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn bound_construct_of_an_unknown_list_follows_the_apply_level() {
    // `apply_arguments_are_arrays`: an unknown list never reaches a bound
    // construct at `standard`. At `aggressive` the `apply` rewrite turns it
    // into `C.bind(...args)`, whose bound `this` is unknown.
    let input = r#"
function make(C, args) {
    return new (C.bind.apply(C, args))();
}
"#;
    assert_eq_normalized(&apply(input), input);
    assert_eq_normalized(
        &aggressive(input),
        r#"
function make(C, args) {
    return new (C.bind(...args))();
}
"#,
    );
}

#[test]
fn bound_construct_is_kept_at_minimal() {
    let input = r#"
a = new (C.bind(undefined, ...xs))();
b = new (Function.prototype.bind.apply(C, [null, ...xs]))();
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), input);
}

#[test]
fn bound_construct_parenthesizes_a_call_constructor() {
    let input = r#"
a = new (make().bind(undefined, ...xs))();
b = new (Function.prototype.bind.apply(a.b().c, [null, ...xs]))();
"#;
    let expected = r#"
a = new (make())(...xs);
b = new (a.b()).c(...xs);
"#;
    assert_eq_normalized(&apply(input), expected);
}

// Lowered spread constructs through the whole pipeline. Each input is the
// producer's output for `new C(...xs)` and its variants, trimmed to the
// functions shown.

#[test]
fn babel_six_spread_construct_recovers_through_the_pipeline() {
    // producer babel-core@6.26.3 {"presets":["es2015"]}
    let input = r#"
"use strict";

Object.defineProperty(exports, "__esModule", {
  value: true
});
exports.make = make;
exports.makeLead = makeLead;
exports.makeMember = makeMember;
exports.makeRest = makeRest;

function _toConsumableArray(arr) { if (Array.isArray(arr)) { for (var i = 0, arr2 = Array(arr.length); i < arr.length; i++) { arr2[i] = arr[i]; } return arr2; } else { return Array.from(arr); } }

function make(C, xs) {
  return new (Function.prototype.bind.apply(C, [null].concat(_toConsumableArray(xs))))();
}
function makeLead(C, xs) {
  return new (Function.prototype.bind.apply(C, [null].concat([0], _toConsumableArray(xs))))();
}
function makeMember(box, xs) {
  return new (Function.prototype.bind.apply(box.inner.Ctor, [null].concat(_toConsumableArray(xs))))();
}
function makeRest(C) {
  for (var _len = arguments.length, rest = Array(_len > 1 ? _len - 1 : 0), _key = 1; _key < _len; _key++) {
    rest[_key - 1] = arguments[_key];
  }

  return new (Function.prototype.bind.apply(C, [null].concat(rest)))();
}
"#;
    let expected = r#"
export { make };
export { makeLead };
export { makeMember };
export { makeRest };
function make(C, xs) {
    return new C(...xs);
}
function makeLead(C, xs) {
    return new C(0, ...xs);
}
function makeMember(box, xs) {
    return new box.inner.Ctor(...xs);
}
function makeRest(C, ...rest) {
    return new C(...rest);
}
"#;
    assert_eq_normalized(
        &common::render_with_level(input, RewriteLevel::Standard),
        expected,
    );
}

#[test]
fn typescript_spread_construct_recovers_through_the_pipeline() {
    // producer typescript@5.9.3 {"module":"CommonJS","target":"ES5","esModuleInterop":true}
    let input = r#"
"use strict";
var __spreadArray = (this && this.__spreadArray) || function (to, from, pack) {
    if (pack || arguments.length === 2) for (var i = 0, l = from.length, ar; i < l; i++) {
        if (ar || !(i in from)) {
            if (!ar) ar = Array.prototype.slice.call(from, 0, i);
            ar[i] = from[i];
        }
    }
    return to.concat(ar || Array.prototype.slice.call(from));
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.make = make;
exports.makeTail = makeTail;
exports.makeMember = makeMember;
exports.makeRest = makeRest;
exports.readProp = readProp;
function make(C, xs) {
    return new (C.bind.apply(C, __spreadArray([void 0], xs, false)))();
}
function makeTail(C, xs, b) {
    return new (C.bind.apply(C, __spreadArray(__spreadArray([void 0], xs, false), [b], false)))();
}
function makeMember(box, xs) {
    var _a;
    return new ((_a = box.inner.Ctor).bind.apply(_a, __spreadArray([void 0], xs, false)))();
}
function makeRest(C) {
    var rest = [];
    for (var _i = 1; _i < arguments.length; _i++) {
        rest[_i - 1] = arguments[_i];
    }
    return new (C.bind.apply(C, __spreadArray([void 0], rest, false)))();
}
function readProp(C, xs) {
    return new (C.bind.apply(C, __spreadArray([void 0], xs, false)))().args;
}
"#;
    let expected = r#"
export { make };
export { makeTail };
export { makeMember };
export { makeRest };
export { readProp };
function make(C, xs) {
    return new C(...xs);
}
function makeTail(C, xs, b) {
    return new C(...xs, b);
}
function makeMember(box, xs) {
    return new box.inner.Ctor(...xs);
}
function makeRest(C, ...rest) {
    return new C(...rest);
}
function readProp(C, xs) {
    return new C(...xs).args;
}
"#;
    assert_eq_normalized(
        &common::render_with_level(input, RewriteLevel::Standard),
        expected,
    );
}

#[test]
fn typescript_downlevel_iteration_spread_construct_recovers_through_the_pipeline() {
    // producer typescript@5.9.3 {"module":"CommonJS","target":"ES5","downlevelIteration":true,"esModuleInterop":true}
    let input = r#"
"use strict";
var __read = (this && this.__read) || function (o, n) {
    var m = typeof Symbol === "function" && o[Symbol.iterator];
    if (!m) return o;
    var i = m.call(o), r, ar = [], e;
    try {
        while ((n === void 0 || n-- > 0) && !(r = i.next()).done) ar.push(r.value);
    }
    catch (error) { e = { error: error }; }
    finally {
        try {
            if (r && !r.done && (m = i["return"])) m.call(i);
        }
        finally { if (e) throw e.error; }
    }
    return ar;
};
var __spreadArray = (this && this.__spreadArray) || function (to, from, pack) {
    if (pack || arguments.length === 2) for (var i = 0, l = from.length, ar; i < l; i++) {
        if (ar || !(i in from)) {
            if (!ar) ar = Array.prototype.slice.call(from, 0, i);
            ar[i] = from[i];
        }
    }
    return to.concat(ar || Array.prototype.slice.call(from));
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.makeLead = makeLead;
exports.makeFmt = makeFmt;
function makeLead(C, xs) {
    return new (C.bind.apply(C, __spreadArray([void 0, 0], __read(xs), false)))();
}
function makeFmt(t) {
    var _a;
    return new ((_a = Intl.NumberFormat).bind.apply(_a, __spreadArray([void 0], __read(t), false)))();
}
"#;
    let expected = r#"
export { makeLead };
export { makeFmt };
function makeLead(C, xs) {
    return new C(0, ...xs);
}
function makeFmt(t) {
    return new Intl.NumberFormat(...t);
}
"#;
    assert_eq_normalized(
        &common::render_with_level(input, RewriteLevel::Standard),
        expected,
    );
}

#[test]
fn closure_spread_construct_recovers_through_the_pipeline() {
    // producer @babel/core@7.28.5 {"plugins":["@babel/plugin-transform-modules-commonjs"]}
    // then google-closure-compiler@20260629.0.0 {"compilation_level":"SIMPLE","language_out":"ECMASCRIPT5"}
    let input = r#"var $jscomp=$jscomp||{};$jscomp.scope={};$jscomp.arrayIteratorImpl=function(a){var b=0;return function(){return b<a.length?{done:!1,value:a[b++]}:{done:!0}}};$jscomp.arrayIterator=function(a){return{next:$jscomp.arrayIteratorImpl(a)}};$jscomp.makeIterator=function(a){var b=typeof Symbol!="undefined"&&Symbol.iterator&&a[Symbol.iterator];if(b)return b.call(a);if(typeof a.length=="number")return $jscomp.arrayIterator(a);throw Error(String(a)+" is not an iterable or ArrayLike");};
$jscomp.arrayFromIterator=function(a){for(var b,c=[];!(b=a.next()).done;)c.push(b.value);return c};$jscomp.arrayFromIterable=function(a){return a instanceof Array?a:$jscomp.arrayFromIterator($jscomp.makeIterator(a))};Object.defineProperty(exports,"__esModule",{value:!0});exports.make=make;exports.makeLead=makeLead;exports.makeMember=makeMember;exports.readProp=readProp;
function make(a,b){return new (Function.prototype.bind.apply(a,[null].concat((0,$jscomp.arrayFromIterable)(b))))}function makeLead(a,b){return new (Function.prototype.bind.apply(a,[null,0].concat((0,$jscomp.arrayFromIterable)(b))))}
function makeMember(a,b){return new (Function.prototype.bind.apply(a.inner.Ctor,[null].concat((0,$jscomp.arrayFromIterable)(b))))}
function readProp(a,b){return(new (Function.prototype.bind.apply(a,[null].concat((0,$jscomp.arrayFromIterable)(b))))).args};
"#;
    let output = common::render_with_level(input, RewriteLevel::Standard);
    for present in [
        "return new a(...b);",
        "return new a(0, ...b);",
        "return new a.inner.Ctor(...b);",
        "return new a(...b).args;",
    ] {
        assert!(output.contains(present), "missing `{present}`:\n{output}");
    }
    assert!(!output.contains("bind"), "{output}");
}

#[test]
fn unproven_concat_spread_construct_recovers_only_at_aggressive() {
    // shape: hypothetical, per `concat_arguments_are_arrays` (TypeScript
    // before 3.6 targeting ES5 without downlevelIteration lowered spread to
    // `[void 0].concat(xs)`; not reproduced with a pinned producer here).
    // Whether `xs` reads the same through concat and spread is the concat
    // rule's question, so the construct follows its level.
    let input = r#"
function make(C, xs) {
    return new (C.bind.apply(C, [void 0].concat(xs)))();
}
function makeNull(C, xs) {
    return new (Function.prototype.bind.apply(C, [null].concat(xs)))();
}
export { make, makeNull };
"#;
    let standard = common::render_with_level(input, RewriteLevel::Standard);
    assert!(standard.contains(".concat(xs)"), "{standard}");
    assert!(!standard.contains("new C("), "{standard}");
    assert_eq_normalized(
        &common::render_with_level(input, RewriteLevel::Aggressive),
        r#"
function make(C, xs) {
    return new C(...xs);
}
function makeNull(C, xs) {
    return new C(...xs);
}
export { make, makeNull };
"#,
    );
}

// Babel 7's and swc's `_construct(C, list)` helper.

/// producer @babel/core@7.29.7 {"presets":[["@babel/preset-env",{"targets":{"ie":"11"},"modules":false}]]}
const BABEL_CONSTRUCT: &str = r#"
function _construct(t, e, r) { if (_isNativeReflectConstruct()) return Reflect.construct.apply(null, arguments); var o = [null]; o.push.apply(o, e); var p = new (t.bind.apply(t, o))(); return r && _setPrototypeOf(p, r.prototype), p; }
function _setPrototypeOf(t, e) { return _setPrototypeOf = Object.setPrototypeOf ? Object.setPrototypeOf.bind() : function (t, e) { return t.__proto__ = e, t; }, _setPrototypeOf(t, e); }
function _isNativeReflectConstruct() { try { var t = !Boolean.prototype.valueOf.call(Reflect.construct(Boolean, [], function () {})); } catch (t) {} return (_isNativeReflectConstruct = function _isNativeReflectConstruct() { return !!t; })(); }
"#;

/// producer @swc/core@1.16.2 {"jsc":{"target":"es5"},"module":{"type":"commonjs"}}
const SWC_CONSTRUCT: &str = r#"
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
function _set_prototype_of(o, p) {
    _set_prototype_of = Object.setPrototypeOf || function setPrototypeOf(o, p) {
        o.__proto__ = p;
        return o;
    };
    return _set_prototype_of(o, p);
}
"#;

#[test]
fn babel_construct_helper_with_two_arguments_becomes_a_spread_construct() {
    let input = format!(
        "{BABEL_CONSTRUCT}\nexport function make(C, xs) {{ return _construct(C, [...xs]); }}\nexport function lead(C, xs) {{ return _construct(C, [0, ...xs]); }}\n"
    );
    let expected = r#"
export function make(C, xs) {
    return new C(...xs);
}
export function lead(C, xs) {
    return new C(0, ...xs);
}
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn swc_construct_helper_with_two_arguments_becomes_a_spread_construct() {
    let input = format!(
        "{SWC_CONSTRUCT}\nfunction make(C, ...rest) {{ return _construct(C, rest); }}\nexport {{ make }};\n"
    );
    let expected = r#"
function make(C, ...rest) {
    return new C(...rest);
}
export { make };
"#;
    assert_eq_normalized(&apply(&input), expected);
}

#[test]
fn construct_helper_keeps_the_three_argument_native_super_call() {
    // `_wrapNativeSuper` constructs the parent with another prototype.
    let input = format!(
        "{BABEL_CONSTRUCT}\nexport function wrap(t) {{ return function Wrapper() {{ return _construct(t, arguments, _getPrototypeOf(this).constructor); }}; }}\nexport function make(C, xs) {{ return _construct(C, [...xs]); }}\n"
    );
    let output = apply(&input);
    assert!(
        output.contains("_construct(t, arguments, _getPrototypeOf(this).constructor)"),
        "{output}"
    );
    assert!(output.contains("return new C(...xs);"), "{output}");
    assert!(output.contains("function _construct(t, e, r)"), "{output}");
    assert!(
        output.contains("function _isNativeReflectConstruct()"),
        "{output}"
    );
}

#[test]
fn construct_helper_with_an_unknown_list_follows_the_apply_level() {
    // producer @babel/core@7.29.7 preset-env ie11 loose passes the operand
    // as is; `Reflect.construct` reads it as an array-like.
    let input =
        format!("{BABEL_CONSTRUCT}\nexport function make(C, xs) {{ return _construct(C, xs); }}\n");
    let standard = apply(&input);
    assert!(standard.contains("return _construct(C, xs);"), "{standard}");
    assert!(
        standard.contains("function _construct(t, e, r)"),
        "{standard}"
    );
    assert_eq_normalized(
        &aggressive(&input),
        r#"
export function make(C, xs) {
    return new C(...xs);
}
"#,
    );
}

#[test]
fn construct_helper_lookalikes_keep_their_calls() {
    let lookalikes = [
        // the native check is not a `Reflect.construct` probe
        BABEL_CONSTRUCT.replace(
            "Reflect.construct(Boolean, [], function () {})",
            "track(Boolean)",
        ),
        // a local `Reflect`
        format!("var Reflect = custom;\n{BABEL_CONSTRUCT}"),
        // the fallback does not copy the arguments
        BABEL_CONSTRUCT.replace("o.push.apply(o, e); ", ""),
        // the fallback binds another constructor
        BABEL_CONSTRUCT.replace("t.bind.apply(t, o)", "t.bind.apply(other, o)"),
        // the native branch forwards other arguments
        BABEL_CONSTRUCT.replace(
            "Reflect.construct.apply(null, arguments)",
            "Reflect.construct.apply(null, extra)",
        ),
    ];
    for helper in lookalikes {
        let input =
            format!("{helper}\nexport function make(C, xs) {{ return _construct(C, [...xs]); }}\n");
        let output = apply(&input);
        assert!(output.contains("_construct(C, ["), "{helper}\n{output}");
    }
}

#[test]
fn written_or_redeclared_construct_helper_keeps_its_calls() {
    let written = format!(
        "{BABEL_CONSTRUCT}\nexport function swap(f) {{ _construct = f; }}\nexport function make(C, xs) {{ return _construct(C, [...xs]); }}\n"
    );
    let redeclared = format!(
        "{SWC_CONSTRUCT}\nvar _construct = custom;\nexport function make(C, xs) {{ return _construct(C, [...xs]); }}\n"
    );
    for input in [written, redeclared] {
        let output = apply(&input);
        assert!(output.contains("_construct(C, ["), "{input}\n{output}");
    }
}

#[test]
fn babel_seven_construct_helper_recovers_through_the_pipeline() {
    // producer @babel/core@7.29.7 {"presets":[["@babel/preset-env",{"targets":{"ie":"11"},"modules":false}]]}
    let input = r#"
function _construct(t, e, r) { if (_isNativeReflectConstruct()) return Reflect.construct.apply(null, arguments); var o = [null]; o.push.apply(o, e); var p = new (t.bind.apply(t, o))(); return r && _setPrototypeOf(p, r.prototype), p; }
function _setPrototypeOf(t, e) { return _setPrototypeOf = Object.setPrototypeOf ? Object.setPrototypeOf.bind() : function (t, e) { return t.__proto__ = e, t; }, _setPrototypeOf(t, e); }
function _isNativeReflectConstruct() { try { var t = !Boolean.prototype.valueOf.call(Reflect.construct(Boolean, [], function () {})); } catch (t) {} return (_isNativeReflectConstruct = function _isNativeReflectConstruct() { return !!t; })(); }
function _toConsumableArray(r) { return _arrayWithoutHoles(r) || _iterableToArray(r) || _unsupportedIterableToArray(r) || _nonIterableSpread(); }
function _nonIterableSpread() { throw new TypeError("Invalid attempt to spread non-iterable instance.\nIn order to be iterable, non-array objects must have a [Symbol.iterator]() method."); }
function _unsupportedIterableToArray(r, a) { if (r) { if ("string" == typeof r) return _arrayLikeToArray(r, a); var t = {}.toString.call(r).slice(8, -1); return "Object" === t && r.constructor && (t = r.constructor.name), "Map" === t || "Set" === t ? Array.from(r) : "Arguments" === t || /^(?:Ui|I)nt(?:8|16|32)(?:Clamped)?Array$/.test(t) ? _arrayLikeToArray(r, a) : void 0; } }
function _iterableToArray(r) { if ("undefined" != typeof Symbol && null != r[Symbol.iterator] || null != r["@@iterator"]) return Array.from(r); }
function _arrayWithoutHoles(r) { if (Array.isArray(r)) return _arrayLikeToArray(r); }
function _arrayLikeToArray(r, a) { (null == a || a > r.length) && (a = r.length); for (var e = 0, n = Array(a); e < a; e++) n[e] = r[e]; return n; }
export function make(C, xs) {
  return _construct(C, _toConsumableArray(xs));
}
export function makeLead(C, xs) {
  return _construct(C, [0].concat(_toConsumableArray(xs)));
}
export function makeMember(box, xs) {
  return _construct(box.inner.Ctor, _toConsumableArray(xs));
}
export function makeRest(C) {
  for (var _len = arguments.length, rest = new Array(_len > 1 ? _len - 1 : 0), _key = 1; _key < _len; _key++) {
    rest[_key - 1] = arguments[_key];
  }
  return _construct(C, rest);
}
"#;
    let expected = r#"
export function make(C, xs) {
    return new C(...xs);
}
export function makeLead(C, xs) {
    return new C(0, ...xs);
}
export function makeMember(box, xs) {
    return new box.inner.Ctor(...xs);
}
export function makeRest(C, ...rest) {
    return new C(...rest);
}
"#;
    assert_eq_normalized(
        &common::render_with_level(input, RewriteLevel::Standard),
        expected,
    );
}

#[test]
fn babel_seven_loose_construct_helper_follows_the_apply_level() {
    // producer @babel/core@7.29.7 {"presets":[["@babel/preset-env",{"targets":{"ie":"11"},"modules":false,"loose":true}]]}
    let input = r#"
function _construct(t, e, r) { if (_isNativeReflectConstruct()) return Reflect.construct.apply(null, arguments); var o = [null]; o.push.apply(o, e); var p = new (t.bind.apply(t, o))(); return r && _setPrototypeOf(p, r.prototype), p; }
function _setPrototypeOf(t, e) { return _setPrototypeOf = Object.setPrototypeOf ? Object.setPrototypeOf.bind() : function (t, e) { return t.__proto__ = e, t; }, _setPrototypeOf(t, e); }
function _isNativeReflectConstruct() { try { var t = !Boolean.prototype.valueOf.call(Reflect.construct(Boolean, [], function () {})); } catch (t) {} return (_isNativeReflectConstruct = function _isNativeReflectConstruct() { return !!t; })(); }
export function make(C, xs) {
  return _construct(C, xs);
}
export function makeLead(C, xs) {
  return _construct(C, [0].concat(xs));
}
"#;
    let standard = common::render_with_level(input, RewriteLevel::Standard);
    assert!(standard.contains("return _construct(C, xs);"), "{standard}");
    assert!(standard.contains("return new C(...[\n"), "{standard}");
    assert_eq_normalized(
        &common::render_with_level(input, RewriteLevel::Aggressive),
        r#"
export function make(C, xs) {
    return new C(...xs);
}
export function makeLead(C, xs) {
    return new C(0, ...xs);
}
"#,
    );
}

#[test]
fn swc_construct_helper_recovers_through_the_pipeline() {
    // producer @swc/core@1.16.2 {"jsc":{"target":"es5"},"module":{"type":"commonjs"}},
    // trimmed to the construct helpers.
    let input = format!(
        r#""use strict";
Object.defineProperty(exports, "__esModule", {{
    value: true
}});
exports.makeRest = makeRest;
{SWC_CONSTRUCT}
function makeRest(C) {{
    for(var _len = arguments.length, rest = new Array(_len > 1 ? _len - 1 : 0), _key = 1; _key < _len; _key++){{
        rest[_key - 1] = arguments[_key];
    }}
    return _construct(C, rest);
}}
"#
    );
    let expected = r#"
export { makeRest };
function makeRest(C, ...rest) {
    return new C(...rest);
}
"#;
    assert_eq_normalized(
        &common::render_with_level(&input, RewriteLevel::Standard),
        expected,
    );
}

// `Array.prototype.slice.call(x)` builds a plain Array when `x` is the
// `arguments` object or a proven Array.

#[test]
fn slice_copy_of_arguments_converts_at_standard() {
    // wild-observed: hand-written ES5 forwarding.
    let input = r#"
function f() {
    fn.apply(void 0, Array.prototype.slice.call(arguments));
    fn.apply(void 0, Array.prototype.slice.call(arguments, 0));
    fn.apply(void 0, [].slice.call(arguments));
    obj.fn.apply(obj, Array.prototype.slice.call(arguments, 1));
    obj.fn.apply(obj, [].slice.call(arguments, 1, end()));
}
"#;
    let expected = r#"
function f() {
    fn(...arguments);
    fn(...arguments);
    fn(...arguments);
    obj.fn(...Array.prototype.slice.call(arguments, 1));
    obj.fn(...[].slice.call(arguments, 1, end()));
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn slice_copy_of_a_proven_array_converts_at_standard() {
    let input = r#"
function f(...xs) {
    g.apply(void 0, Array.prototype.slice.call(xs, 1));
}
"#;
    let expected = r#"
function f(...xs) {
    g(...Array.prototype.slice.call(xs, 1));
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn slice_copy_of_written_arguments_keeps_apply() {
    let input = r#"
function f(x) {
    arguments = x;
    fn.apply(void 0, Array.prototype.slice.call(arguments));
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn slice_copy_with_a_shadowed_array_keeps_apply() {
    let input = r#"
function f(Array) {
    fn.apply(void 0, Array.prototype.slice.call(arguments));
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn slice_copy_skips_a_module_with_dynamic_scope() {
    let input = r#"
function f(o) {
    with (o) {
        obj.fn.apply(obj, [].slice.call(arguments, 1));
    }
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn slice_copy_of_an_unknown_operand_follows_the_apply_level() {
    // wild-observed: `[].slice.call(u)` of any array-like. An Array subclass
    // instance builds its slice through its own species.
    let input = r#"
function f(u) {
    fn.apply(void 0, [].slice.call(u));
}
"#;
    assert_eq_normalized(&apply(input), input);
    assert_eq_normalized(
        &aggressive(input),
        r#"
function f(u) {
    fn(...[].slice.call(u));
}
"#,
    );
}

#[test]
fn slice_lookalikes_keep_apply() {
    let input = r#"
function f() {
    fn.apply(void 0, Array.prototype.slice.apply(arguments));
    fn.apply(void 0, Array.prototype["slice"].call(arguments));
    fn.apply(void 0, Array.prototype.slice.call?.(arguments));
    fn.apply(void 0, Array.prototype.slice.call(...arguments));
    fn.apply(void 0, Array.prototype.slice.call());
    fn.apply(void 0, list.slice.call(arguments));
    fn.apply(void 0, Array.slice.call(arguments));
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn slice_copy_keeps_each_functions_own_arguments() {
    // An arrow reads the enclosing function's `arguments`; a nested function
    // has its own. The rewrite leaves `arguments` where it was.
    let input = r#"
function outer() {
    const viaArrow = () => fn.apply(void 0, [].slice.call(arguments));
    const viaFunction = function() {
        return fn.apply(void 0, Array.prototype.slice.call(arguments, 1));
    };
    return [viaArrow, viaFunction];
}
"#;
    let expected = r#"
function outer() {
    const viaArrow = () => fn(...arguments);
    const viaFunction = function() {
        return fn(...Array.prototype.slice.call(arguments, 1));
    };
    return [viaArrow, viaFunction];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn concat_of_fresh_arrays_spreads_in_place() {
    // producer babel-core@6.26.3 {"presets":["es2015"]}: `fn(x, ...arguments)`.
    let input = r#"
function f(x) {
    fn.apply(undefined, [x].concat(Array.prototype.slice.call(arguments)));
    fn.apply(undefined, [x].concat([1], Array.prototype.slice.call(arguments, 1)));
}
"#;
    let expected = r#"
function f(x) {
    fn(x, ...arguments);
    fn(x, 1, ...Array.prototype.slice.call(arguments, 1));
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn concat_of_a_proven_binding_stays_one_spread() {
    // Code holding `xs` may have set its `Symbol.isConcatSpreadable`; the
    // concat result is still a proven Array.
    let input = r#"
function f(...xs) {
    fn.apply(void 0, [a].concat(xs));
}
"#;
    let expected = r#"
function f(...xs) {
    fn(...[a].concat(xs));
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn bound_construct_of_a_concat_slice_copy_becomes_a_spread_construct() {
    let input = r#"
function f(C) {
    return new (Function.prototype.bind.apply(C, [null].concat([0], Array.prototype.slice.call(arguments, 1))))();
}
function g(C, u) {
    return new (Function.prototype.bind.apply(C, [null].concat([].slice.call(u))))();
}
"#;
    let expected = r#"
function f(C) {
    return new C(0, ...Array.prototype.slice.call(arguments, 1));
}
function g(C, u) {
    return new (Function.prototype.bind.apply(C, [null].concat([].slice.call(u))))();
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn babel_six_arguments_spread_recovers_through_the_pipeline() {
    // producer babel-core@6.26.3 {"presets":["es2015"]}
    let input = r#"
"use strict";

Object.defineProperty(exports, "__esModule", {
  value: true
});
exports.make = make;
exports.makeLead = makeLead;
exports.call = call;
function make(C) {
  return function () {
    return new (Function.prototype.bind.apply(C, [null].concat(Array.prototype.slice.call(arguments))))();
  };
}
function makeLead(C) {
  return function () {
    return new (Function.prototype.bind.apply(C, [null].concat([0], Array.prototype.slice.call(arguments))))();
  };
}
function call(fn, x) {
  return fn.apply(undefined, [x].concat(Array.prototype.slice.call(arguments)));
}
"#;
    let expected = r#"
export { make };
export { makeLead };
export { call };
function make(C) {
    return function() {
        return new C(...arguments);
    };
}
function makeLead(C) {
    return function() {
        return new C(0, ...arguments);
    };
}
function call(fn, x) {
    return fn(x, ...arguments);
}
"#;
    assert_eq_normalized(
        &common::render_with_level(input, RewriteLevel::Standard),
        expected,
    );
}

#[test]
fn babel_seven_arguments_spread_recovers_through_the_pipeline() {
    // producer @babel/core@7.29.7 {"presets":[["@babel/preset-env",{"targets":{"ie":"11"},"modules":false}]]}
    let input = format!(
        r#"{BABEL_CONSTRUCT}
export function make(C) {{
  return function () {{
    return _construct(C, Array.prototype.slice.call(arguments));
  }};
}}
export function makeLead(C) {{
  return function () {{
    return _construct(C, [0].concat(Array.prototype.slice.call(arguments)));
  }};
}}
export function call(fn, x) {{
  return fn.apply(void 0, [x].concat(Array.prototype.slice.call(arguments)));
}}
"#
    );
    let expected = r#"
export function make(C) {
    return function() {
        return new C(...arguments);
    };
}
export function makeLead(C) {
    return function() {
        return new C(0, ...arguments);
    };
}
export function call(fn, x) {
    return fn(x, ...arguments);
}
"#;
    assert_eq_normalized(
        &common::render_with_level(&input, RewriteLevel::Standard),
        expected,
    );
}

#[test]
fn swc_arguments_spread_recovers_through_the_pipeline() {
    // producer @swc/core@1.16.2 {"jsc":{"target":"es5"},"module":{"type":"commonjs"}},
    // trimmed to the construct helpers.
    let input = format!(
        r#""use strict";
Object.defineProperty(exports, "__esModule", {{
    value: true
}});
exports.make = make;
exports.makeLead = makeLead;
{SWC_CONSTRUCT}
function make(C) {{
    return function() {{
        return _construct(C, Array.prototype.slice.call(arguments));
    }};
}}
function makeLead(C) {{
    return function() {{
        return _construct(C, [
            0
        ].concat(Array.prototype.slice.call(arguments)));
    }};
}}
"#
    );
    let expected = r#"
export { make };
export { makeLead };
function make(C) {
    return function() {
        return new C(...arguments);
    };
}
function makeLead(C) {
    return function() {
        return new C(0, ...arguments);
    };
}
"#;
    assert_eq_normalized(
        &common::render_with_level(&input, RewriteLevel::Standard),
        expected,
    );
}
