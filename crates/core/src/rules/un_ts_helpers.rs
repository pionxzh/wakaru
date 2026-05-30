use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{Decl, Module, ModuleItem, Pat, Stmt};
use swc_core::ecma::visit::VisitMut;

use super::babel_helper_utils::{BindingKey, LocalHelperContext};
use super::helper_matcher::{remove_var_declarators_by_binding, var_declarator_binding_key};
use super::rename_utils::{rename_bindings_in_module, BindingRename};

/// Detect TypeScript helper declarations like:
/// ```js
/// const V = this && this.__awaiter || ((U, B, G, Y) => { ... });
/// const Z = this && this.__generator || ((U, B) => { ... });
/// ```
/// Rename local aliases to canonical names so downstream rules (UnAsyncAwait)
/// can match them, then remove the helper declarations.
pub struct UnTsHelpers;

impl UnTsHelpers {
    pub(crate) fn run_with_helpers(
        module: &mut Module,
        local_helpers: &LocalHelperContext,
    ) -> bool {
        run_un_ts_helpers(module, local_helpers)
    }
}

impl VisitMut for UnTsHelpers {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_ts_helpers(module, &local_helpers);
    }
}

fn run_un_ts_helpers(module: &mut Module, local_helpers: &LocalHelperContext) -> bool {
    let inline_helpers = local_helpers.inline_ts_helpers();
    if inline_helpers.is_empty() {
        return false;
    }

    let mut renames: Vec<BindingRename> = Vec::new();
    let mut removable_helpers: HashSet<BindingKey> = HashSet::new();

    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = item else {
            continue;
        };
        for decl in &var_decl.decls {
            let Some(key) = var_declarator_binding_key(decl) else {
                continue;
            };
            let Some(kind) = inline_helpers.get(&key) else {
                continue;
            };
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };

            let canonical_name = Atom::from(kind.canonical_name());
            if binding.id.sym != canonical_name {
                renames.push(BindingRename {
                    old: (binding.id.sym.clone(), binding.id.ctxt),
                    new: canonical_name.clone(),
                });
            }
            removable_helpers.insert((canonical_name, binding.id.ctxt));
        }
    }

    if removable_helpers.is_empty() {
        return false;
    }

    if !renames.is_empty() {
        rename_bindings_in_module(module, &renames);
    }

    remove_var_declarators_by_binding(&mut module.body, &removable_helpers);
    true
}
