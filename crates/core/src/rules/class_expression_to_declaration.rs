use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    Class, ClassDecl, Decl, Expr, Ident, Module, ModuleDecl, ModuleItem, Pat, Stmt, VarDecl,
    VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_uses::BindingUseIndex;

use super::eval_utils::{js_source_mentions_binding, DirectEvalAnalyzer};

/// Promotes `const X = class { ... }` to `class X { ... }`.
///
/// Only anonymous class expressions with an unobservably immutable outer
/// binding are promoted. Named expressions keep a distinct inner self-binding,
/// while outer-binding references during class evaluation and writes hidden in
/// direct eval can distinguish a `const` binding from a class declaration.
pub struct ClassExpressionToDeclaration;

impl VisitMut for ClassExpressionToDeclaration {
    fn visit_mut_module(&mut self, module: &mut Module) {
        if !contains_promotion_candidate(module) {
            return;
        }

        let uses = BindingUseIndex::collect(module);
        let direct_eval = analyze_direct_eval(module);
        module.visit_mut_children_with(&mut ClassExpressionToDeclarationVisitor {
            uses: &uses,
            direct_eval: &direct_eval,
        });
    }
}

struct ClassExpressionToDeclarationVisitor<'a> {
    uses: &'a BindingUseIndex,
    direct_eval: &'a DirectEvalAnalyzer,
}

impl VisitMut for ClassExpressionToDeclarationVisitor<'_> {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        promote_in_module_items(items, self.uses, self.direct_eval);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        promote_in_stmts(stmts, self.uses, self.direct_eval);
    }
}

fn promote_in_module_items(
    items: &mut Vec<ModuleItem>,
    uses: &BindingUseIndex,
    direct_eval: &DirectEvalAnalyzer,
) {
    for item in items.iter_mut() {
        let class_decl = match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
                try_promote_var_decl(var_decl, uses, direct_eval)
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                if let Decl::Var(var_decl) = &export_decl.decl {
                    try_promote_var_decl(var_decl, uses, direct_eval)
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

fn promote_in_stmts(
    stmts: &mut Vec<Stmt>,
    uses: &BindingUseIndex,
    direct_eval: &DirectEvalAnalyzer,
) {
    for stmt in stmts.iter_mut() {
        let Stmt::Decl(Decl::Var(var_decl)) = stmt else {
            continue;
        };
        if let Some(class_decl) = try_promote_var_decl(var_decl, uses, direct_eval) {
            *stmt = Stmt::Decl(Decl::Class(class_decl));
        }
    }
}

fn contains_promotion_candidate(module: &Module) -> bool {
    #[derive(Default)]
    struct Finder {
        found: bool,
    }

    impl Visit for Finder {
        fn visit_var_decl(&mut self, var_decl: &VarDecl) {
            if self.found {
                return;
            }
            if is_promotion_candidate(var_decl) {
                self.found = true;
                return;
            }
            var_decl.visit_children_with(self);
        }
    }

    let mut finder = Finder::default();
    module.visit_with(&mut finder);
    finder.found
}

fn is_promotion_candidate(var_decl: &VarDecl) -> bool {
    if var_decl.kind != VarDeclKind::Const || var_decl.decls.len() != 1 {
        return false;
    }

    let declarator = &var_decl.decls[0];
    matches!(&declarator.name, Pat::Ident(_))
        && matches!(declarator.init.as_deref(), Some(Expr::Class(class)) if class.ident.is_none())
}

fn try_promote_var_decl(
    var_decl: &VarDecl,
    uses: &BindingUseIndex,
    direct_eval: &DirectEvalAnalyzer,
) -> Option<ClassDecl> {
    if !is_promotion_candidate(var_decl) {
        return None;
    }

    let declarator = &var_decl.decls[0];
    let Pat::Ident(binding_ident) = &declarator.name else {
        return None;
    };
    let Expr::Class(class_expr) = declarator.init.as_deref()? else {
        return None;
    };

    let binding = (binding_ident.sym.clone(), binding_ident.ctxt);
    if uses.has_direct_write(&binding)
        || class_references_binding(&class_expr.class, &binding_ident.sym, binding_ident.ctxt)
        || direct_eval_may_observe_binding(direct_eval, &binding_ident.sym)
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

fn analyze_direct_eval<T>(node: &T) -> DirectEvalAnalyzer
where
    T: VisitWith<DirectEvalAnalyzer> + ?Sized,
{
    let mut analyzer = DirectEvalAnalyzer::default();
    node.visit_with(&mut analyzer);
    analyzer
}

fn direct_eval_may_observe_binding(analyzer: &DirectEvalAnalyzer, binding: &Atom) -> bool {
    analyzer.unknown_direct_eval
        || analyzer
            .known_direct_eval_sources
            .iter()
            .any(|source| js_source_mentions_binding(source, binding))
}

fn class_references_binding(class: &Class, sym: &Atom, ctxt: SyntaxContext) -> bool {
    struct Finder<'a> {
        sym: &'a Atom,
        ctxt: SyntaxContext,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym == *self.sym && ident.ctxt == self.ctxt {
                self.found = true;
            }
        }
    }

    let mut finder = Finder {
        sym,
        ctxt,
        found: false,
    };
    class.visit_with(&mut finder);
    finder.found
}
