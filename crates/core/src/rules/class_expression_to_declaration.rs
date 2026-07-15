use swc_core::ecma::ast::{
    ClassDecl, Decl, Expr, Ident, Module, ModuleDecl, ModuleItem, Pat, Stmt, VarDecl, VarDeclKind,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

/// Promotes `const X = class { ... }` to `class X { ... }`.
///
/// Anonymous class expressions and named expressions whose internal name is
/// already `X` preserve the observable class name and self-binding. A named
/// expression with a different internal name must stay an expression: changing
/// that name affects `Class.name`, static initialization, and the binding
/// captured by heritage expressions and methods.
pub struct ClassExpressionToDeclaration;

impl VisitMut for ClassExpressionToDeclaration {
    fn visit_mut_module(&mut self, module: &mut Module) {
        module.visit_mut_children_with(&mut ClassExpressionToDeclarationVisitor);
    }
}

struct ClassExpressionToDeclarationVisitor;

impl VisitMut for ClassExpressionToDeclarationVisitor {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        promote_in_module_items(items);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        promote_in_stmts(stmts);
    }
}

fn promote_in_module_items(items: &mut Vec<ModuleItem>) {
    for item in items.iter_mut() {
        let class_decl = match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => try_promote_var_decl(var_decl),
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                if let Decl::Var(var_decl) = &export_decl.decl {
                    try_promote_var_decl(var_decl)
                } else {
                    None
                }
            }
            _ => None,
        };

        let Some(class_decl) = class_decl else {
            continue;
        };
        match item {
            ModuleItem::Stmt(_) => {
                *item = ModuleItem::Stmt(Stmt::Decl(Decl::Class(class_decl)));
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                export_decl.decl = Decl::Class(class_decl);
            }
            _ => {}
        }
    }
}

fn promote_in_stmts(stmts: &mut Vec<Stmt>) {
    for stmt in stmts.iter_mut() {
        let Stmt::Decl(Decl::Var(var_decl)) = stmt else {
            continue;
        };
        if let Some(class_decl) = try_promote_var_decl(var_decl) {
            *stmt = Stmt::Decl(Decl::Class(class_decl));
        }
    }
}

fn try_promote_var_decl(var_decl: &VarDecl) -> Option<ClassDecl> {
    if var_decl.kind != VarDeclKind::Const || var_decl.decls.len() != 1 {
        return None;
    }

    let declarator = &var_decl.decls[0];
    let Pat::Ident(binding_ident) = &declarator.name else {
        return None;
    };
    let Expr::Class(class_expr) = declarator.init.as_deref()? else {
        return None;
    };

    if class_expr
        .ident
        .as_ref()
        .is_some_and(|internal| internal.sym != binding_ident.sym)
    {
        return None;
    }

    Some(ClassDecl {
        ident: Ident {
            span: binding_ident.span,
            ctxt: binding_ident.ctxt,
            sym: binding_ident.sym.clone(),
            optional: false,
        },
        declare: false,
        class: class_expr.class.clone(),
    })
}
