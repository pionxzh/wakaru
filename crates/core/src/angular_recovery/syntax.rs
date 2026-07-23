use swc_core::atoms::{Atom, Wtf8Atom};
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{Expr, Ident, Lit, MemberProp, PropName};

pub(super) type BindingKey = (Atom, SyntaxContext);

pub(super) fn binding_key(ident: &Ident) -> BindingKey {
    (ident.sym.clone(), ident.ctxt)
}

pub(super) fn prop_name(name: &PropName) -> Option<String> {
    match name {
        PropName::Ident(ident) => Some(ident.sym.to_string()),
        PropName::Str(string) => Some(wtf8_to_string(&string.value)),
        _ => None,
    }
}

pub(super) fn member_prop_name(prop: &MemberProp) -> Option<Atom> {
    match prop {
        MemberProp::Ident(ident) => Some(ident.sym.clone()),
        MemberProp::Computed(computed) => string_lit(computed.expr.as_ref()).map(Atom::from),
        MemberProp::PrivateName(_) => None,
    }
}

pub(super) fn string_lit(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(Lit::Str(string)) => Some(wtf8_to_string(&string.value)),
        Expr::Tpl(template) if template.exprs.is_empty() && template.quasis.len() == 1 => template
            .quasis
            .first()
            .and_then(|quasi| quasi.cooked.as_ref())
            .map(wtf8_to_string),
        _ => None,
    }
}

pub(super) fn wtf8_to_string(value: &Wtf8Atom) -> String {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string_lossy().into_owned())
}
