use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrayLit, BinExpr, BinaryOp, CallExpr, Callee, Expr, ExprOrSpread, Ident, Lit, Number, Str,
    UnaryExpr, UnaryOp,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnTypeConstructor;

impl VisitMut for UnTypeConstructor {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        match expr {
            // +x → Number(x) — only when x is an Ident
            Expr::Unary(UnaryExpr {
                op: UnaryOp::Plus,
                arg,
                ..
            }) if matches!(**arg, Expr::Ident(_)) => {
                let arg = std::mem::replace(
                    arg,
                    Box::new(Expr::Lit(Lit::Num(Number {
                        span: DUMMY_SP,
                        value: 0.0,
                        raw: None,
                    }))),
                );
                *expr = make_call("Number", arg);
            }

            // x + "" → String(x)  OR  "str" + "" → "str"
            Expr::Bin(BinExpr {
                op: BinaryOp::Add,
                left,
                right,
                ..
            }) if is_empty_string(right) => {
                if is_string_lit(left) {
                    // "str" + "" → "str"  (simplify to just left)
                    let left = std::mem::replace(
                        left,
                        Box::new(Expr::Lit(Lit::Num(Number {
                            span: DUMMY_SP,
                            value: 0.0,
                            raw: None,
                        }))),
                    );
                    *expr = *left;
                } else {
                    let left = std::mem::replace(
                        left,
                        Box::new(Expr::Lit(Lit::Num(Number {
                            span: DUMMY_SP,
                            value: 0.0,
                            raw: None,
                        }))),
                    );
                    *expr = make_call("String", left);
                }
            }

            // [,,,] → Array(n) — all-holes array with n > 0
            Expr::Array(ArrayLit { elems, .. }) if is_all_holes(elems) && !elems.is_empty() => {
                let n = elems.len();
                *expr = make_call(
                    "Array",
                    Box::new(Expr::Lit(Lit::Num(Number {
                        span: DUMMY_SP,
                        value: n as f64,
                        raw: None,
                    }))),
                );
            }

            _ => {}
        }
    }
}

fn make_call(name: &str, arg: Box<Expr>) -> Expr {
    Expr::Call(CallExpr {
        span: DUMMY_SP,
        ctxt: Default::default(),
        callee: Callee::Expr(Box::new(Expr::Ident(Ident::new_no_ctxt(
            name.into(),
            DUMMY_SP,
        )))),
        args: vec![ExprOrSpread {
            spread: None,
            expr: arg,
        }],
        type_args: None,
    })
}

fn is_empty_string(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(Lit::Str(Str { value, .. })) if value.is_empty())
}

fn is_string_lit(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(Lit::Str(_)))
}

fn is_all_holes(elems: &[Option<ExprOrSpread>]) -> bool {
    elems.iter().all(|e| e.is_none())
}
