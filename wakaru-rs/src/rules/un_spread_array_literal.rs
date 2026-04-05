use swc_core::ecma::ast::{CallExpr, Expr, ExprOrSpread, NewExpr};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

/// Inlines spread-over-array-literal in function call arguments.
///
/// `fn(...[a, b, ...c])` → `fn(a, b, ...c)`
///
/// When a spread argument is an array literal, we can inline each element
/// as a separate argument, preserving inner spreads. This eliminates the
/// unnecessary intermediate array allocation.
pub struct UnSpreadArrayLiteral;

impl VisitMut for UnSpreadArrayLiteral {
    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        call.visit_mut_children_with(self);
        inline_spread_array_args(&mut call.args);
    }

    fn visit_mut_new_expr(&mut self, new_expr: &mut NewExpr) {
        new_expr.visit_mut_children_with(self);
        if let Some(args) = &mut new_expr.args {
            inline_spread_array_args(args);
        }
    }
}

/// Walk the args list. For each `...[]` spread of an array literal,
/// inline the array's elements directly as individual arguments.
fn inline_spread_array_args(args: &mut Vec<ExprOrSpread>) {
    let mut needs_inline = false;
    for arg in args.iter() {
        if arg.spread.is_some() {
            if matches!(arg.expr.as_ref(), Expr::Array(_)) {
                needs_inline = true;
                break;
            }
        }
    }

    if !needs_inline {
        return;
    }

    let old = std::mem::take(args);
    for arg in old {
        if arg.spread.is_some() {
            if let Expr::Array(arr) = *arg.expr {
                // Inline each element of the array literal
                for elem in arr.elems {
                    match elem {
                        Some(eos) => args.push(eos),
                        // Holes in array literals are rare but possible
                        // — skip them since they can't be function args
                        None => {}
                    }
                }
                continue;
            } else {
                // Not an array literal — keep the spread as-is
                args.push(ExprOrSpread {
                    spread: arg.spread,
                    expr: arg.expr,
                });
                continue;
            }
        }
        args.push(arg);
    }
}
