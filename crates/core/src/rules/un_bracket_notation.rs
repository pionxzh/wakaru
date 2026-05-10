use swc_core::ecma::ast::{Expr, Ident, IdentName, Lit, MemberExpr, MemberProp, Number};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnBracketNotation;

impl VisitMut for UnBracketNotation {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Member(MemberExpr { prop, .. }) = expr else {
            return;
        };
        let MemberProp::Computed(computed) = prop else {
            return;
        };
        let Expr::Lit(Lit::Str(str_lit)) = &*computed.expr else {
            return;
        };

        let value = str_lit.value.to_string_lossy().into_owned();

        if let Some(num) = parse_normalized_decimal(&value) {
            *prop = MemberProp::Computed(swc_core::ecma::ast::ComputedPropName {
                span: computed.span,
                expr: Box::new(Expr::Lit(Lit::Num(Number {
                    span: str_lit.span,
                    value: num,
                    raw: None,
                }))),
            });
            return;
        }

        if Ident::verify_symbol(&value).is_ok() {
            *prop = MemberProp::Ident(IdentName::new(value.into(), str_lit.span));
        }
    }
}

fn parse_normalized_decimal(value: &str) -> Option<f64> {
    if !is_plain_decimal(value) {
        return None;
    }

    let parsed = value.parse::<f64>().ok()?;
    if parsed.to_string() == value {
        Some(parsed)
    } else {
        None
    }
}

fn is_plain_decimal(value: &str) -> bool {
    let mut parts = value.split('.');
    let left = parts.next().unwrap_or_default();
    let right = parts.next();

    if parts.next().is_some() {
        return false;
    }
    if left.is_empty() || !left.chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }

    match right {
        None => true,
        Some(r) => !r.is_empty() && r.chars().all(|ch| ch.is_ascii_digit()),
    }
}
