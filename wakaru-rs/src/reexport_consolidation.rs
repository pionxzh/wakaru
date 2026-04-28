//! Re-export consolidation: redirect imports from passthrough modules to the
//! actual target module.
//!
//! A passthrough module has the shape `export default require("./X.js")` with
//! no other statements — it re-exports another module's namespace as its
//! default export. When the imported binding is only used via member access
//! (e.g. `x.foo`, `x.bar`), the import is rewritten:
//!   `import x from "./passthrough.js"` → `import * as x from "./target.js"`
//!
//! The resulting namespace imports can then be further decomposed into named
//! imports by the namespace decomposition pass.

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, Expr, Ident, ImportDecl, ImportSpecifier, ImportStarAsSpecifier,
    MemberExpr, MemberProp, ModuleDecl, ModuleItem, Module, SimpleAssignTarget, Str, UnaryExpr,
    UnaryOp, UpdateExpr,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::facts::ModuleFactsMap;

pub fn run_reexport_consolidation(module: &mut Module, module_facts: &ModuleFactsMap) {
    if module_facts.is_empty() {
        return;
    }

    let redirects: Vec<(usize, Atom)> = module
        .body
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| {
            let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
                return None;
            };
            if import.specifiers.len() != 1 {
                return None;
            }
            let ImportSpecifier::Default(default_spec) = &import.specifiers[0] else {
                return None;
            };

            let source_str = import.src.value.as_str().unwrap_or("");
            let target = resolve_passthrough(source_str, module_facts)?;

            let mut analyzer = MemberOnlyAnalyzer {
                target_sym: &default_spec.local.sym,
                target_ctxt: default_spec.local.ctxt,
                safe: true,
                in_import_decl: false,
            };
            module.visit_with(&mut analyzer);
            if !analyzer.safe {
                return None;
            }

            Some((idx, target))
        })
        .collect();

    for (idx, target) in redirects {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = &mut module.body[idx] else {
            continue;
        };
        let ImportSpecifier::Default(default_spec) = &import.specifiers[0] else {
            continue;
        };
        let local_ident = default_spec.local.clone();
        import.specifiers = vec![ImportSpecifier::Namespace(ImportStarAsSpecifier {
            span: DUMMY_SP,
            local: Ident::new(local_ident.sym, DUMMY_SP, local_ident.ctxt),
        })];
        import.src = Box::new(Str::from(target.as_ref()));
    }
}

fn resolve_passthrough(source: &str, facts: &ModuleFactsMap) -> Option<Atom> {
    let mut current = source.to_string();
    let mut seen = std::collections::HashSet::new();

    loop {
        if !seen.insert(current.clone()) {
            return None;
        }
        let module_facts = facts.get(&current)?;
        let target = module_facts.passthrough_target.as_ref()?;
        let target_str = target.as_ref();

        if let Some(target_facts) = facts.get(target_str) {
            if target_facts.passthrough_target.is_some() {
                current = target_str.to_string();
                continue;
            }
        }
        return Some(target.clone());
    }
}

struct MemberOnlyAnalyzer<'a> {
    target_sym: &'a Atom,
    target_ctxt: SyntaxContext,
    safe: bool,
    in_import_decl: bool,
}

impl MemberOnlyAnalyzer<'_> {
    fn is_target(&self, ident: &Ident) -> bool {
        ident.sym == *self.target_sym && ident.ctxt == self.target_ctxt
    }
}

impl Visit for MemberOnlyAnalyzer<'_> {
    fn visit_import_decl(&mut self, import: &ImportDecl) {
        self.in_import_decl = true;
        import.visit_children_with(self);
        self.in_import_decl = false;
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left {
            if matches!(member.obj.as_ref(), Expr::Ident(obj) if self.is_target(obj)) {
                self.safe = false;
                assign.right.visit_with(self);
                return;
            }
        }
        assign.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        if let Expr::Member(member) = update.arg.as_ref() {
            if matches!(member.obj.as_ref(), Expr::Ident(obj) if self.is_target(obj)) {
                self.safe = false;
                return;
            }
        }
        update.visit_children_with(self);
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        if unary.op == UnaryOp::Delete {
            if let Expr::Member(member) = unary.arg.as_ref() {
                if matches!(member.obj.as_ref(), Expr::Ident(obj) if self.is_target(obj)) {
                    self.safe = false;
                    return;
                }
            }
        }
        unary.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if let Expr::Ident(obj) = member.obj.as_ref() {
            if self.is_target(obj) {
                match &member.prop {
                    MemberProp::Ident(_) => return,
                    MemberProp::Computed(_) => {
                        self.safe = false;
                        return;
                    }
                    _ => {
                        self.safe = false;
                        return;
                    }
                }
            }
        }
        member.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if self.in_import_decl {
            return;
        }
        if self.is_target(ident) {
            self.safe = false;
        }
    }

    fn visit_prop_name(&mut self, _: &swc_core::ecma::ast::PropName) {}

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(prop) = prop {
            prop.visit_with(self);
        }
    }
}
