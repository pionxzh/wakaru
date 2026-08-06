pub(crate) mod binding_uses;
pub(crate) mod purity;

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::Ident;

/// Canonical binding identity: symbol name plus hygiene context.
///
/// This is the one definition; `rules::decl_utils`, `rules::helper_matcher`
/// (as `BindingKey`), `rules::rename_utils`, `analysis::binding_uses`, and
/// `unpacker` re-export it.
pub(crate) type BindingId = (Atom, SyntaxContext);

pub(crate) fn binding_id(ident: &Ident) -> BindingId {
    (ident.sym.clone(), ident.ctxt)
}

pub(crate) fn ident_matches_binding(ident: &Ident, binding: &BindingId) -> bool {
    ident.sym == binding.0 && ident.ctxt == binding.1
}
