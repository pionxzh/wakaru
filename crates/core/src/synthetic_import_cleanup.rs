//! Preserve evaluation while removing unused bindings from synthesized imports.
//!
//! `UnEsm` turns `var unused = require("./provider")` into a default import.
//! Transform-only DCE intentionally preserves bindings that were already dead
//! at the phase barrier, but retaining the guessed default specifier can leave
//! an invalid edge when the provider has no default export. The original
//! `require` still evaluated the provider, so this pass removes only the dead
//! synthesized binding and keeps the declaration as a side-effect import.
//! The import tags say which specifiers came from a require; a specifier
//! the input wrote stays, unless `UnEsm` merged it into such an import.

use swc_core::ecma::ast::{ImportSpecifier, Module, ModuleDecl, ModuleItem};

use crate::analysis::binding_id;
use crate::analysis::binding_uses::BindingUseIndex;
use crate::require_edges::ImportTags;

pub(crate) fn downgrade_unused_synthetic_imports(module: &mut Module, import_tags: &ImportTags) {
    let local = |specifier: &ImportSpecifier| match specifier {
        ImportSpecifier::Default(default) => binding_id(&default.local),
        ImportSpecifier::Namespace(namespace) => binding_id(&namespace.local),
        ImportSpecifier::Named(named) => binding_id(&named.local),
    };
    if import_tags.is_empty() {
        return;
    }
    let has_synthetic_import = module.body.iter().any(|item| {
        matches!(item, ModuleItem::ModuleDecl(ModuleDecl::Import(import))
            if !import.type_only
                && import.specifiers.iter().any(|specifier| import_tags.is_synthesized(&local(specifier))))
    });
    if !has_synthetic_import {
        return;
    }

    let uses = BindingUseIndex::collect(module);
    for item in &mut module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        if import.type_only {
            continue;
        }
        import.specifiers.retain(|specifier| {
            let local = local(specifier);
            !import_tags.is_synthesized(&local) || uses.use_count(&local) > 0
        });
    }
}
