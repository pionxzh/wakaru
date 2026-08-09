//! Preserve evaluation while removing unused bindings from synthesized imports.
//!
//! `UnEsm` turns `var unused = require("./provider")` into a default import.
//! Transform-only DCE intentionally preserves bindings that were already dead
//! at the phase barrier, but retaining the guessed default specifier can leave
//! an invalid edge when the provider has no default export. The original
//! `require` still evaluated the provider, so this pass removes only the dead
//! synthesized binding and keeps the declaration as a side-effect import.

use swc_core::ecma::ast::{ImportSpecifier, Module, ModuleDecl, ModuleItem};

use crate::analysis::binding_id;
use crate::analysis::binding_uses::BindingUseIndex;

pub(crate) fn downgrade_unused_synthetic_imports(module: &mut Module) {
    let has_synthetic_import = module.body.iter().any(|item| {
        matches!(item, ModuleItem::ModuleDecl(ModuleDecl::Import(import))
            if import.span.is_dummy() && !import.type_only && !import.specifiers.is_empty())
    });
    if !has_synthetic_import {
        return;
    }

    let uses = BindingUseIndex::collect(module);
    for item in &mut module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        if !import.span.is_dummy() || import.type_only {
            continue;
        }
        import.specifiers.retain(|specifier| {
            let local = match specifier {
                ImportSpecifier::Default(default) => &default.local,
                ImportSpecifier::Namespace(namespace) => &namespace.local,
                ImportSpecifier::Named(named) => &named.local,
            };
            uses.use_count(&binding_id(local)) > 0
        });
    }
}
