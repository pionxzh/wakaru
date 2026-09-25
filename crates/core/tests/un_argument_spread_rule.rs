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

#[test]
fn converts_apply_with_undefined_to_spread() {
    let input = r#"
fn.apply(undefined, args);
"#;
    let expected = r#"
fn(...args);
"#;
    let output = apply(input);
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
fn converts_apply_with_null_to_spread() {
    let input = r#"
fn.apply(null, args);
"#;
    let expected = r#"
fn(...args);
"#;
    let output = apply(input);
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
    let output = apply(input);
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
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn converts_obj_method_apply_with_array_expression() {
    let input = r#"
obj.fn.apply(obj, [1, 2, 3]);
"#;
    let expected = r#"
obj.fn(...[1, 2, 3]);
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
const out = app_info.build(...[prefix, ...items, tail]);
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
    let output = apply(input);
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
    let output = apply(input);
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
    assert_eq_normalized(&apply(input), expected);
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
    assert_eq_normalized(&apply(input), expected);
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
    assert_eq_normalized(&apply(input), expected);
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
    assert_eq_normalized(&apply(input), expected);
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
    assert_eq_normalized(&apply(input), expected);
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
    assert_eq_normalized(&apply(input), expected);
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
    assert_eq_normalized(&apply(input), expected);
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
    assert_eq_normalized(&apply(input), expected);
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
    assert_eq_normalized(&apply(input), expected);
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
    assert_eq_normalized(&apply(input), expected);
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
    assert_eq_normalized(&apply(input), expected);
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
    assert_eq_normalized(&apply(input), expected);
}
