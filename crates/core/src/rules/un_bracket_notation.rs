use swc_core::common::Spanned;
use swc_core::ecma::ast::{Expr, Ident, IdentName, Lit, MemberExpr, MemberProp, Number, PropName};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnBracketNotation;

impl VisitMut for UnBracketNotation {
    fn visit_mut_member_expr(&mut self, member: &mut MemberExpr) {
        member.visit_mut_children_with(self);

        let MemberProp::Computed(computed) = &mut member.prop else {
            return;
        };
        let Expr::Lit(Lit::Str(str_lit)) = &*computed.expr else {
            return;
        };

        let value = str_lit.value.to_string_lossy().into_owned();

        if let Some(num) = parse_normalized_decimal(&value) {
            member.prop = MemberProp::Computed(swc_core::ecma::ast::ComputedPropName {
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
            member.prop = MemberProp::Ident(IdentName::new(value.into(), str_lit.span));
        }
    }

    fn visit_mut_prop_name(&mut self, name: &mut PropName) {
        name.visit_mut_children_with(self);

        let (value, is_computed) = match name {
            PropName::Computed(computed) => {
                let Expr::Lit(Lit::Str(str_lit)) = &*computed.expr else {
                    return;
                };
                (str_lit.value.to_string_lossy().into_owned(), true)
            }
            PropName::Str(s) => (s.value.to_string_lossy().into_owned(), false),
            _ => return,
        };

        if is_computed && matches!(value.as_str(), "__proto__" | "constructor") {
            return;
        }

        if let Some(num) = parse_normalized_decimal(&value) {
            *name = PropName::Num(Number {
                span: name.span(),
                value: num,
                raw: None,
            });
            return;
        }

        if Ident::verify_symbol(&value).is_ok() {
            *name = PropName::Ident(IdentName::new(value.into(), name.span()));
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
