use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    BinExpr, BinaryOp, Decl, Expr, MemberExpr, MemberProp, Module, ModuleItem, Pat, Stmt,
};
use swc_core::ecma::visit::VisitMut;

use super::rename_utils::{rename_bindings_in_module, BindingRename};

/// Detect TypeScript helper declarations like:
/// ```js
/// const V = this && this.__awaiter || ((U, B, G, Y) => { ... });
/// const Z = this && this.__generator || ((U, B) => { ... });
/// ```
/// Rename local aliases to canonical names so downstream rules (UnAsyncAwait)
/// can match them, then remove the helper declarations.
pub struct UnTsHelpers;

impl VisitMut for UnTsHelpers {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let mut renames: Vec<BindingRename> = Vec::new();
        let mut helper_names: HashSet<Atom> = HashSet::new();

        for item in &module.body {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = item else {
                continue;
            };
            for decl in &var_decl.decls {
                let Pat::Ident(binding) = &decl.name else {
                    continue;
                };
                let Some(init) = &decl.init else {
                    continue;
                };
                if let Some(helper_name) = extract_ts_helper_name(init) {
                    let local_name = &binding.id.sym;
                    helper_names.insert(helper_name.clone());
                    if *local_name != helper_name {
                        renames.push(BindingRename {
                            old: (local_name.clone(), binding.id.ctxt),
                            new: helper_name,
                        });
                    }
                }
            }
        }

        if helper_names.is_empty() {
            return;
        }

        // Scope-aware rename using shared BindingRenamer
        if !renames.is_empty() {
            rename_bindings_in_module(module, &renames);
        }

        // Remove the helper declarations (now renamed to canonical names)
        module.body.retain_mut(|item| {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = item else {
                return true;
            };
            var_decl.decls.retain(|decl| {
                let Pat::Ident(binding) = &decl.name else {
                    return true;
                };
                if !helper_names.contains(&binding.id.sym) {
                    return true;
                }
                decl.init
                    .as_deref()
                    .and_then(extract_ts_helper_name)
                    .is_none_or(|helper_name| helper_name != binding.id.sym)
            });
            !var_decl.decls.is_empty()
        });
    }
}

/// Extract the canonical helper name from `this && this.__helperName || (...)`.
fn extract_ts_helper_name(expr: &Expr) -> Option<Atom> {
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalOr,
        left,
        ..
    }) = expr
    else {
        return None;
    };

    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalAnd,
        left: and_left,
        right: and_right,
        ..
    }) = &**left
    else {
        return None;
    };

    if !matches!(&**and_left, Expr::This(_)) {
        return None;
    }

    let Expr::Member(MemberExpr {
        obj,
        prop: MemberProp::Ident(prop_name),
        ..
    }) = &**and_right
    else {
        return None;
    };
    if !matches!(&**obj, Expr::This(_)) {
        return None;
    }

    let name = &prop_name.sym;
    match name.as_ref() {
        "__awaiter" | "__generator" | "__assign" | "__rest" | "__extends" | "__importDefault"
        | "__importStar" | "__createBinding" | "__setModuleDefault" => Some(name.clone()),
        _ => None,
    }
}
