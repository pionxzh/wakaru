use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{BinExpr, BinaryOp, Callee, Expr, MemberProp};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

/// Converts `Math.pow(a, b)` → `a ** b`.
pub struct Exponent;

impl VisitMut for Exponent {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else {
            return;
        };

        // Must have exactly 2 args with no spread
        if call.args.len() != 2 || call.args[0].spread.is_some() || call.args[1].spread.is_some() {
            return;
        }

        // Callee must be `Math.pow`
        let Callee::Expr(callee_expr) = &call.callee else {
            return;
        };
        let Expr::Member(member) = callee_expr.as_ref() else {
            return;
        };
        let Expr::Ident(obj_ident) = member.obj.as_ref() else {
            return;
        };
        if obj_ident.sym != "Math" {
            return;
        }
        let MemberProp::Ident(prop_ident) = &member.prop else {
            return;
        };
        if prop_ident.sym != "pow" {
            return;
        }

        // Take ownership and build the ** expression
        let Expr::Call(mut call_owned) = std::mem::replace(expr, Expr::Invalid(Default::default()))
        else {
            unreachable!()
        };
        let b = *call_owned.args.pop().unwrap().expr;
        let a = *call_owned.args.pop().unwrap().expr;

        *expr = Expr::Bin(BinExpr {
            span: DUMMY_SP,
            op: BinaryOp::Exp,
            left: Box::new(a),
            right: Box::new(b),
        });
    }
}
