use swc_core::atoms::Atom;
use swc_core::ecma::ast::{CallExpr, Callee, Expr, IdentName, MemberExpr, MemberProp};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::un_nullish_coalescing::is_undefined;

/// Convert `.then(null, errorHandler)` to `.catch(errorHandler)`.
///
/// These are semantically identical but `.catch()` is far more readable.
pub struct UnThenCatch;

impl VisitMut for UnThenCatch {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Member(MemberExpr {
            obj,
            prop: MemberProp::Ident(prop_name),
            ..
        }) = &**callee
        else {
            return;
        };

        if prop_name.sym.as_ref() != "then" || call.args.len() != 2 {
            return;
        }

        let first_arg = &call.args[0];
        if !is_null_or_undefined(&first_arg.expr) || first_arg.spread.is_some() {
            return;
        }

        let second_arg = call.args[1].clone();
        if second_arg.spread.is_some() {
            return;
        }

        // Rewrite: obj.then(null, handler) → obj.catch(handler)
        let new_callee = Expr::Member(MemberExpr {
            span: Default::default(),
            obj: obj.clone(),
            prop: MemberProp::Ident(IdentName::new(Atom::from("catch"), Default::default())),
        });

        *expr = Expr::Call(CallExpr {
            span: call.span,
            ctxt: call.ctxt,
            callee: Callee::Expr(Box::new(new_callee)),
            args: vec![second_arg],
            type_args: None,
        });
    }
}

fn is_null_or_undefined(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(swc_core::ecma::ast::Lit::Null(_))) || is_undefined(expr)
}
