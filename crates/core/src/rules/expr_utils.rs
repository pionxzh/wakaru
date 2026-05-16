use swc_core::common::Mark;
use swc_core::ecma::ast::{Expr, Ident, Lit, MemberProp, UnaryExpr, UnaryOp};

pub fn exprs_structurally_equal(a: &Expr, b: &Expr) -> bool {
    match (a, b) {
        (Expr::Ident(ai), Expr::Ident(bi)) => ai.sym == bi.sym && ai.ctxt == bi.ctxt,
        (Expr::This(_), Expr::This(_)) => true,
        (Expr::Member(am), Expr::Member(bm)) => {
            exprs_structurally_equal(&am.obj, &bm.obj) && member_props_equal(&am.prop, &bm.prop)
        }
        (Expr::Array(aa), Expr::Array(ab)) => aa.elems.is_empty() && ab.elems.is_empty(),
        (Expr::Lit(la), Expr::Lit(lb)) => lits_equal(la, lb),
        _ => false,
    }
}

fn member_props_equal(a: &MemberProp, b: &MemberProp) -> bool {
    match (a, b) {
        (MemberProp::Ident(ai), MemberProp::Ident(bi)) => ai.sym == bi.sym,
        (MemberProp::Computed(ac), MemberProp::Computed(bc)) => {
            exprs_structurally_equal(&ac.expr, &bc.expr)
        }
        _ => false,
    }
}

fn lits_equal(a: &Lit, b: &Lit) -> bool {
    match (a, b) {
        (Lit::Str(a), Lit::Str(b)) => a.value == b.value,
        (Lit::Num(a), Lit::Num(b)) => a.value == b.value,
        (Lit::Bool(a), Lit::Bool(b)) => a.value == b.value,
        (Lit::Null(_), Lit::Null(_)) => true,
        _ => false,
    }
}

pub fn is_unresolved_undefined(expr: &Expr, unresolved_mark: Mark) -> bool {
    match expr {
        Expr::Ident(id) if is_unresolved_ident(id, "undefined", unresolved_mark) => true,
        Expr::Unary(UnaryExpr {
            op: UnaryOp::Void,
            arg,
            ..
        }) => matches!(&**arg, Expr::Lit(Lit::Num(n)) if n.value == 0.0),
        _ => false,
    }
}

pub fn is_unresolved_ident(id: &Ident, name: &str, unresolved_mark: Mark) -> bool {
    id.sym.as_ref() == name && id.ctxt.outer() == unresolved_mark
}
