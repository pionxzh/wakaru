//! Re-export consolidation: redirect imports from passthrough modules to the
//! actual target module.
//!
//! A passthrough module has the shape `module.exports = require("./X.js")`
//! (or `export default require("./X.js")`) with no other statements: its value
//! is what `require` of the target returned when the target was CommonJS. The
//! import is rewritten to the import of the target that holds that value
//! (`kept_require::required_value_import`):
//!
//! - The default import, for a target that stays CommonJS (its
//!   `module.exports`) or that was `module.exports = v` and now only exports
//!   `v` as its default. Only the source changes:
//!   `import x from "./passthrough.js"` → `import x from "./target.js"`.
//! - The namespace import, for an ES module target. When the binding is only
//!   read through members (`x.foo`, or destructuring with fixed keys),
//!   `import x from "./passthrough.js"` → `import * as x from "./target.js"`.
//!   The resulting namespace imports can then be further decomposed into
//!   named imports by the namespace decomposition pass.
//!
//! A `node:` builtin outside the input takes the namespace import. Any other
//! target without facts (a package outside the input), or one whose value no
//! import holds, leaves its importers alone.

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, AssignTargetPat, Expr, Ident, ImportDecl, ImportSpecifier,
    ImportStarAsSpecifier, MemberExpr, MemberProp, Module, ModuleDecl, ModuleItem, ObjectPat,
    ObjectPatProp, PropName, SimpleAssignTarget, Str, UnaryExpr, UnaryOp, UpdateExpr,
    VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::facts::ModuleFactsMap;
use crate::kept_require::{required_value_import, RequiredValueImport};
use crate::module_path::relative_import_specifier;
use crate::utils::paren::strip_parens;

/// The module a passthrough chain ends at, and the import of it that holds
/// the passthrough's value.
enum PassthroughTarget {
    Default(Atom),
    Namespace(Atom),
}

pub fn run_reexport_consolidation(
    module: &mut Module,
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
) {
    if module_facts.is_empty() {
        return;
    }

    let redirects: Vec<(usize, PassthroughTarget)> = module
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
            let target = resolve_passthrough(source_str, module_facts, current_filename)?;
            if matches!(target, PassthroughTarget::Default(_)) {
                return Some((idx, target));
            }

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
        let target = match target {
            PassthroughTarget::Default(target) => target,
            PassthroughTarget::Namespace(target) => {
                let local_ident = default_spec.local.clone();
                import.specifiers = vec![ImportSpecifier::Namespace(ImportStarAsSpecifier {
                    span: DUMMY_SP,
                    local: Ident::new(local_ident.sym, DUMMY_SP, local_ident.ctxt),
                })];
                target
            }
        };
        *import.src = Str::from(target.as_ref());
    }
}

fn resolve_passthrough(
    source: &str,
    facts: &ModuleFactsMap,
    current_filename: Option<&str>,
) -> Option<PassthroughTarget> {
    let original_importer = current_filename?;
    let mut current = facts.resolve_key_from(current_filename, source)?;
    let mut seen = crate::collections::HashSet::default();

    loop {
        if !seen.insert(current.clone()) {
            return None;
        }
        let module_facts = facts.get(&current)?;
        let target = module_facts.passthrough_target.as_ref()?;
        let target_str = target.as_ref();

        let Some(target_key) = facts.resolve_key_from(Some(&current), target_str) else {
            // A `node:` builtin's namespace holds every export of its
            // `require` value. Any other target outside the input has an
            // unknown format: a bare name can be a bundled package.
            return target_str
                .starts_with("node:")
                .then(|| PassthroughTarget::Namespace(target.clone()));
        };
        let target_facts = facts.get(&target_key)?;
        if target_facts.passthrough_target.is_some() {
            current = target_key;
            continue;
        }
        let specifier = Atom::from(relative_import_specifier(original_importer, &target_key));
        return Some(match required_value_import(target_facts)? {
            RequiredValueImport::Default => PassthroughTarget::Default(specifier),
            RequiredValueImport::Namespace => PassthroughTarget::Namespace(specifier),
        });
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

    fn is_target_expr(&self, expr: &Expr) -> bool {
        matches!(strip_parens(expr), Expr::Ident(ident) if self.is_target(ident))
    }
}

/// `{ a, b: c, d = 1 }`: every key a fixed name, no rest element. Reading
/// such a pattern from an object reads those members, as `x.a` does.
fn is_fixed_key_object_pat(pat: &ObjectPat) -> bool {
    pat.props.iter().all(|prop| match prop {
        ObjectPatProp::KeyValue(prop) => !matches!(prop.key, PropName::Computed(_)),
        ObjectPatProp::Assign(_) => true,
        ObjectPatProp::Rest(_) => false,
    })
}

impl Visit for MemberOnlyAnalyzer<'_> {
    fn visit_import_decl(&mut self, import: &ImportDecl) {
        self.in_import_decl = true;
        import.visit_children_with(self);
        self.in_import_decl = false;
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Some(pat), Some(init)) = (declarator.name.as_object(), declarator.init.as_deref()) {
            if self.is_target_expr(init) && is_fixed_key_object_pat(pat) {
                pat.visit_with(self);
                return;
            }
        }
        declarator.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if let AssignTarget::Pat(AssignTargetPat::Object(pat)) = &assign.left {
            if assign.op == AssignOp::Assign
                && self.is_target_expr(&assign.right)
                && is_fixed_key_object_pat(pat)
            {
                pat.visit_with(self);
                return;
            }
        }
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

    fn visit_prop_name(&mut self, prop: &swc_core::ecma::ast::PropName) {
        if let swc_core::ecma::ast::PropName::Computed(computed) = prop {
            computed.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(prop) = prop {
            prop.visit_with(self);
        }
    }
}
