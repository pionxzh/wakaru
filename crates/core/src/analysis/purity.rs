//! Conservative purity checks for top-level module items.
//!
//! "Pure" means executing the item has no observable effect, so skipping it
//! (dead-module elimination) or running it at import time instead of source
//! position (scope-hoist splitting) is unobservable. Anything not provably
//! inert — calls, `new`, member access (getters), class declarations
//! (decorators, static blocks, effectful heritage) — counts as impure.

use swc_core::ecma::ast::{Decl, DefaultDecl, Expr};

pub(crate) fn is_pure_decl(decl: &Decl) -> bool {
    match decl {
        Decl::Fn(_) => true,
        Decl::Class(_) => false,
        Decl::Var(var) => var
            .decls
            .iter()
            .all(|d| d.init.as_ref().is_none_or(|init| is_pure_init(init))),
        Decl::TsInterface(_) | Decl::TsTypeAlias(_) | Decl::TsEnum(_) | Decl::TsModule(_) => true,
        _ => false,
    }
}

pub(crate) fn is_pure_default_decl(decl: &DefaultDecl) -> bool {
    matches!(decl, DefaultDecl::Fn(_) | DefaultDecl::TsInterfaceDecl(_))
}

pub(crate) fn is_pure_init(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Fn(_) | Expr::Arrow(_) | Expr::Lit(_) | Expr::Ident(_)
    )
}
