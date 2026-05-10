use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{ArrayLit, CallExpr, Callee, Expr, ExprOrSpread, MemberProp};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

/// Converts `[x].concat(arr)` → `[x, ...arr]`.
///
/// Handles:
/// - `[a].concat(b)` → `[a, ...b]`
/// - `[a, b].concat(c)` → `[a, b, ...c]`
/// - `[a].concat(b, c)` → `[a, ...b, ...c]`
/// - `[a].concat([b, c])` → `[a, b, c]`
/// - `[].concat(a)` → `[...a]`
///
/// Only transforms when the receiver is an **array literal** — variable
/// receivers like `arr.concat(other)` are left as-is since `concat` may
/// be overridden or the receiver may not be a plain array.
pub struct UnArrayConcatSpread;

impl VisitMut for UnArrayConcatSpread {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };

        if let Some(new_arr) = try_simplify_array_concat(call) {
            *expr = Expr::Array(new_arr);
        }
    }
}

/// Try to convert `[elems].concat(args...)` into a single array literal.
fn try_simplify_array_concat(call: &CallExpr) -> Option<ArrayLit> {
    // Callee must be member expression: something.concat
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };

    // Property must be `concat`
    let MemberProp::Ident(prop) = &member.prop else {
        return None;
    };
    if prop.sym.as_ref() != "concat" {
        return None;
    }

    // Receiver must be an array literal
    let Expr::Array(receiver_arr) = member.obj.as_ref() else {
        return None;
    };

    // Must have at least one argument
    if call.args.is_empty() {
        return None;
    }

    // Don't transform if any argument uses spread syntax.
    // `[].concat(...arr)` flattens sub-arrays via concat's built-in behavior,
    // but `[...arr]` does not — so the transformation would change semantics.
    if call.args.iter().any(|a| a.spread.is_some()) {
        return None;
    }

    // Build the new array: start with receiver elements
    let mut elems: Vec<Option<ExprOrSpread>> = receiver_arr.elems.clone();

    // Add each concat argument
    for arg in &call.args {
        match arg.expr.as_ref() {
            // Array literal arg: flatten its elements
            Expr::Array(arr) => {
                elems.extend(arr.elems.iter().cloned());
            }
            // Non-array arg: add as spread
            _ => {
                elems.push(Some(ExprOrSpread {
                    spread: Some(DUMMY_SP),
                    expr: arg.expr.clone(),
                }));
            }
        }
    }

    Some(ArrayLit {
        span: DUMMY_SP,
        elems,
    })
}
