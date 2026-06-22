use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    ClassDecl, Decl, Expr, Ident, ModuleDecl, ModuleItem, Pat, Stmt, VarDecl, VarDeclKind,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use crate::js_names::is_likely_generated_alias;

use super::rename_utils::{rename_bindings, rename_bindings_in_module, BindingRename};

/// Promotes `const X = class { ... }` to `class X { ... }`.
///
/// Safe when the binding is `const` with a single class-expression initializer.
/// Both `const` and `class` declarations have TDZ semantics, so the promotion
/// preserves runtime behavior.
///
/// When the class expression has a more readable internal name than the binding
/// (e.g. `const d = class Logger { ... }`), the declaration uses the internal
/// name and all references to the binding are renamed module-wide.
pub struct ClassExpressionToDeclaration;

impl VisitMut for ClassExpressionToDeclaration {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        promote_in_module_items(items);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        promote_in_stmts(stmts);
    }
}

struct PromotionResult {
    class_decl: ClassDecl,
    binding_rename: Option<BindingRename>,
}

fn promote_in_module_items(items: &mut Vec<ModuleItem>) {
    let scope_names = collect_scope_names_from_items(items);
    let mut module_renames: Vec<BindingRename> = Vec::new();

    for item in items.iter_mut() {
        let result = match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
                try_promote_var_decl(var_decl, false, &scope_names)
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                if let Decl::Var(var_decl) = &export_decl.decl {
                    try_promote_var_decl(var_decl, true, &scope_names)
                } else {
                    None
                }
            }
            _ => None,
        };

        if let Some(result) = result {
            if let Some(rename) = result.binding_rename {
                module_renames.push(rename);
            }
            match item {
                ModuleItem::Stmt(_) => {
                    *item = ModuleItem::Stmt(Stmt::Decl(Decl::Class(result.class_decl)));
                }
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                    export_decl.decl = Decl::Class(result.class_decl);
                }
                _ => {}
            }
        }
    }

    if !module_renames.is_empty() {
        let mut module_wrapper = swc_core::ecma::ast::Module {
            span: swc_core::common::DUMMY_SP,
            body: std::mem::take(items),
            shebang: None,
        };
        rename_bindings_in_module(&mut module_wrapper, &module_renames);
        *items = module_wrapper.body;
    }
}

fn promote_in_stmts(stmts: &mut Vec<Stmt>) {
    let scope_names = collect_scope_names_from_stmts(stmts);
    let mut renames: Vec<BindingRename> = Vec::new();

    for stmt in stmts.iter_mut() {
        if let Stmt::Decl(Decl::Var(var_decl)) = stmt {
            if let Some(result) = try_promote_var_decl(var_decl, false, &scope_names) {
                if let Some(rename) = result.binding_rename {
                    renames.push(rename);
                }
                *stmt = Stmt::Decl(Decl::Class(result.class_decl));
            }
        }
    }

    if !renames.is_empty() {
        for stmt in stmts.iter_mut() {
            rename_bindings(stmt, &renames);
        }
    }
}

fn collect_scope_names_from_items(items: &[ModuleItem]) -> HashSet<Atom> {
    let mut names = HashSet::new();
    for item in items {
        match item {
            ModuleItem::Stmt(Stmt::Decl(decl)) => {
                collect_decl_binding_names(decl, &mut names);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                collect_decl_binding_names(&export_decl.decl, &mut names);
            }
            _ => {}
        }
    }
    names
}

fn collect_scope_names_from_stmts(stmts: &[Stmt]) -> HashSet<Atom> {
    let mut names = HashSet::new();
    for stmt in stmts {
        if let Stmt::Decl(decl) = stmt {
            collect_decl_binding_names(decl, &mut names);
        }
    }
    names
}

fn collect_decl_binding_names(decl: &Decl, names: &mut HashSet<Atom>) {
    match decl {
        Decl::Var(var) => {
            for d in &var.decls {
                if let Pat::Ident(id) = &d.name {
                    names.insert(id.sym.clone());
                }
            }
        }
        Decl::Fn(f) => {
            names.insert(f.ident.sym.clone());
        }
        Decl::Class(c) => {
            names.insert(c.ident.sym.clone());
        }
        _ => {}
    }
}

fn try_promote_var_decl(
    var_decl: &VarDecl,
    is_exported: bool,
    scope_names: &HashSet<Atom>,
) -> Option<PromotionResult> {
    if var_decl.kind != VarDeclKind::Const {
        return None;
    }
    if var_decl.decls.len() != 1 {
        return None;
    }

    let declarator = &var_decl.decls[0];

    let binding_ident = match &declarator.name {
        Pat::Ident(binding) => binding,
        _ => return None,
    };

    let init = declarator.init.as_ref()?;

    let class_expr = match init.as_ref() {
        Expr::Class(class_expr) => class_expr,
        _ => return None,
    };

    let (chosen_name, binding_rename) = choose_name(
        binding_ident,
        class_expr.ident.as_ref(),
        is_exported,
        scope_names,
    );

    let mut class = class_expr.class.as_ref().clone();

    if let Some(internal) = &class_expr.ident {
        if internal.sym != chosen_name {
            let renames = [BindingRename {
                old: (internal.sym.clone(), internal.ctxt),
                new: chosen_name.clone(),
            }];
            rename_bindings(&mut class, &renames);
        }
    }

    Some(PromotionResult {
        class_decl: ClassDecl {
            ident: Ident {
                span: binding_ident.span,
                ctxt: binding_ident.ctxt,
                sym: chosen_name,
                optional: false,
            },
            declare: false,
            class: Box::new(class),
        },
        binding_rename,
    })
}

/// Pick the better name between the const binding and the class expression's
/// internal name. When they differ and the class name is more readable,
/// returns a `BindingRename` to rewrite external references.
fn choose_name(
    binding: &swc_core::ecma::ast::BindingIdent,
    class_ident: Option<&Ident>,
    is_exported: bool,
    scope_names: &HashSet<Atom>,
) -> (Atom, Option<BindingRename>) {
    let binding_name = &binding.sym;

    let Some(class_id) = class_ident else {
        return (binding_name.clone(), None);
    };

    if &class_id.sym == binding_name {
        return (binding_name.clone(), None);
    }

    if is_exported {
        return (binding_name.clone(), None);
    }

    let binding_generated = is_likely_generated_alias(binding_name.as_ref());
    let class_generated = is_likely_generated_alias(class_id.sym.as_ref());

    if binding_generated && !class_generated {
        if scope_names.contains(&class_id.sym) {
            return (binding_name.clone(), None);
        }
        let rename = BindingRename {
            old: (binding_name.clone(), binding.ctxt),
            new: class_id.sym.clone(),
        };
        (class_id.sym.clone(), Some(rename))
    } else {
        (binding_name.clone(), None)
    }
}
