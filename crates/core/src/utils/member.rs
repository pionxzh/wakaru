use swc_core::atoms::Atom;
use swc_core::ecma::ast::{Expr, Lit, MemberProp};

/// The property name of a static member access: `.name` or `["name"]`.
pub(crate) fn static_member_name(prop: &MemberProp) -> Option<Atom> {
    match prop {
        MemberProp::Ident(ident) => Some(ident.sym.clone()),
        MemberProp::Computed(computed) => match computed.expr.as_ref() {
            Expr::Lit(Lit::Str(value)) => value.value.as_str().map(Atom::from),
            _ => None,
        },
        MemberProp::PrivateName(_) => None,
    }
}
