use swc_core::atoms::Wtf8Atom;
use swc_core::common::Mark;
use swc_core::ecma::ast::{Expr, Ident, Lit, MemberProp, Str, UnaryExpr, UnaryOp};

/// The string value of a string literal, whichever quote it is printed with.
///
/// Some minifiers (the oxc printer behind Rolldown, for one) print every
/// string literal as a template literal with no substitutions: `"id"` comes
/// out as `` `id` ``. At runtime the two are the same string, so a rule that
/// *reads* a string literal (a helper's key list, a `typeof` comparand, a
/// name) should accept both. Returns the value of a `Lit::Str`, or the cooked
/// value of a template with exactly one quasi and no expressions. A tagged
/// template is a different node (`Expr::TaggedTpl`) and never matches, and a
/// template whose segment has no cooked value (an invalid escape) returns
/// `None`.
///
/// Use this only where the string's value is read. A template is never a
/// directive (`` `use strict` `` as a statement does nothing), and code that
/// keeps or emits the `Str` node itself still needs a real string literal.
pub fn string_lit_value(expr: &Expr) -> Option<&Wtf8Atom> {
    match expr {
        Expr::Lit(Lit::Str(s)) => Some(&s.value),
        Expr::Tpl(tpl) if tpl.exprs.is_empty() && tpl.quasis.len() == 1 => {
            tpl.quasis[0].cooked.as_ref()
        }
        _ => None,
    }
}

/// [`string_lit_value`] as a `Str` node, for callers that carry the literal
/// along (a name candidate, an element tag). A template comes back as a plain
/// string literal with no `raw`, so the printer quotes it afresh.
pub fn as_string_lit(expr: &Expr) -> Option<Str> {
    match expr {
        Expr::Lit(Lit::Str(s)) => Some(s.clone()),
        Expr::Tpl(tpl) => Some(Str {
            span: tpl.span,
            value: string_lit_value(expr)?.clone(),
            raw: None,
        }),
        _ => None,
    }
}

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
