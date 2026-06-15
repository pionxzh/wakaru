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
var _app_info;
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
