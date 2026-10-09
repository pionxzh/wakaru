//! TypeScript's `importHelpers` interop calls whose helper is bundled as its
//! own module:
//!
//! ```js
//! const tslib_1 = require("./tslib.js");
//! const widget_js_1 = tslib_1.__importDefault(require("./widget.js"));
//! const util = tslib_1.__importStar(require("./util.js"));
//! ```
//!
//! `UnEsm` unwraps these calls when the helper is local or comes from
//! `require("tslib")`. A bundler that includes tslib as a module leaves only
//! a relative require, which Phase 1 cannot prove to be tslib, so the module
//! becomes ESM around a helper call that still wraps a `require`. In Phase 2
//! the helper is proven by the provider's TypeScript helper export facts,
//! and [`run_cross_module_interop_imports`] turns each top-level binding of
//! such a call into an import of the wrapped module.
//!
//! The import form follows the wrapped module's facts, because the helpers
//! return different values for a provider marked `__esModule` and for an
//! unmarked one:
//!
//! - `__importDefault`: a marked provider's `.default` is its default export;
//!   an unmarked provider's `.default` is the whole required value. Only
//!   `.default` reads are accepted, and each becomes the binding.
//! - `__importStar`: a marked provider is returned as is, so the binding is
//!   its namespace. An unmarked provider is copied, with `default` set to the
//!   whole required value: member reads stay, `.default` becomes the binding,
//!   and the copy itself must not escape.
//!
//! The whole required value of an unmarked provider is its default export
//! when that comes from `module.exports = value` (the module refers to
//! `module`), and its namespace when it has named exports and no default.
//! Anything else keeps the call.

use crate::collections::{HashMap, HashSet};

use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, Callee, Decl, Expr, Ident, ImportDecl, ImportDefaultSpecifier,
    ImportSpecifier, ImportStarAsSpecifier, Lit, MemberExpr, Module, ModuleDecl, ModuleItem,
    OptChainBase, OptChainExpr, SimpleAssignTarget, Stmt, Str, UnaryExpr, UnaryOp, UpdateExpr,
    VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::cross_module_helper_refs::{
    collect_cross_module_helper_refs, collect_cross_module_ts_helper_refs,
    cross_module_member_helper_kind, cross_module_ts_member_helper, CrossModuleTsHelperRefs,
};
use super::helper_matcher::{binding_key, static_member_prop_name, BindingKey};
use super::transpiler_helper_utils::TranspilerHelperKind;
use crate::facts::{ExportKind, ModuleFacts, ModuleFactsMap, TypeScriptHelperKind};
use crate::utils::paren::strip_parens;

/// Turns top-level `x = helper(require("./p"))` bindings into imports of
/// `./p`, where `helper` is `__importDefault` or `__importStar` imported from
/// another module of the bundle and proven by its helper export facts.
pub(crate) fn run_cross_module_interop_imports(
    module: &mut Module,
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
    unresolved_mark: Mark,
) {
    // tslib's `__importDefault` has Babel's `interopRequireDefault` body, so
    // its facts are the semantic helper kind; `__importStar` is a raw
    // TypeScript helper fact. Both channels are read for both helpers.
    let semantic_refs =
        collect_cross_module_helper_refs(module, module_facts, current_filename, |kind| {
            matches!(
                kind,
                TranspilerHelperKind::InteropRequireDefault
                    | TranspilerHelperKind::InteropRequireWildcard
            )
        });
    let ts_default_refs = collect_cross_module_ts_helper_refs(
        module,
        module_facts,
        current_filename,
        TypeScriptHelperKind::ImportDefault,
    );
    let ts_star_refs = collect_cross_module_ts_helper_refs(
        module,
        module_facts,
        current_filename,
        TypeScriptHelperKind::ImportStar,
    );
    if semantic_refs.direct.is_empty()
        && semantic_refs.namespaces.is_empty()
        && is_empty(&ts_default_refs)
        && is_empty(&ts_star_refs)
    {
        return;
    }
    // The rewrite removes a binding's declaration and renames its `.default`
    // reads; a `with` statement or a direct eval could still reach it by name
    // (docs/rewrite-assumptions.md, dynamic-scope skip).
    if super::eval_utils::has_dynamic_scope_construct(module) {
        return;
    }

    let helper_kind = |callee: &Expr| {
        let callee = strip_parens(callee);
        let semantic = match callee {
            Expr::Ident(helper) => semantic_refs.direct.get(&binding_key(helper)).copied(),
            callee => cross_module_member_helper_kind(callee, &semantic_refs.namespaces),
        };
        let is_ts = |refs: &CrossModuleTsHelperRefs| match callee {
            Expr::Ident(helper) => refs.direct.contains(&binding_key(helper)),
            callee => cross_module_ts_member_helper(callee, &refs.namespaces),
        };
        match semantic {
            Some(TranspilerHelperKind::InteropRequireDefault) => Some(Interop::Default),
            Some(TranspilerHelperKind::InteropRequireWildcard) => Some(Interop::Star),
            _ if is_ts(&ts_default_refs) => Some(Interop::Default),
            _ if is_ts(&ts_star_refs) => Some(Interop::Star),
            _ => None,
        }
    };

    let mut candidates: HashMap<BindingKey, Candidate> = HashMap::default();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for declarator in &var.decls {
            let Some((local, interop, source)) =
                interop_require_binding(declarator, &helper_kind, unresolved_mark)
            else {
                continue;
            };
            let Some(provider) =
                module_facts.get_from(current_filename, source.value.as_str().unwrap_or(""))
            else {
                continue;
            };
            let Some(plan) = import_plan(interop, provider) else {
                continue;
            };
            candidates.insert(
                binding_key(local),
                Candidate {
                    local: local.clone(),
                    source: source.clone(),
                    plan,
                },
            );
        }
    }
    if candidates.is_empty() {
        return;
    }

    let mut uses = UseCollector {
        candidates: &candidates,
        uses: HashMap::default(),
        declarations: HashMap::default(),
    };
    module.visit_with(&mut uses);
    candidates.retain(|key, candidate| {
        uses.declarations.get(key) == Some(&1)
            && uses
                .uses
                .get(key)
                .is_none_or(|uses| candidate.plan.accepts(uses))
    });
    if candidates.is_empty() {
        return;
    }

    let rewrite_default: HashSet<BindingKey> = candidates
        .iter()
        .filter(|(_, candidate)| candidate.plan.default_is_binding)
        .map(|(key, _)| key.clone())
        .collect();
    let mut imports = Vec::with_capacity(candidates.len());
    let mut body = Vec::with_capacity(module.body.len());
    for item in std::mem::take(&mut module.body) {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(mut var))) = item else {
            body.push(item);
            continue;
        };
        var.decls.retain(|declarator| {
            let Some(candidate) = declarator
                .name
                .as_ident()
                .and_then(|binding| candidates.get(&binding_key(&binding.id)))
            else {
                return true;
            };
            imports.push(candidate.import());
            false
        });
        if !var.decls.is_empty() {
            body.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))));
        }
    }
    // Imports are hoisted either way; keep them with the module's others.
    let insert_at = body
        .iter()
        .rposition(|item| matches!(item, ModuleItem::ModuleDecl(ModuleDecl::Import(_))))
        .map_or(0, |index| index + 1);
    body.splice(insert_at..insert_at, imports);
    module.body = body;

    if !rewrite_default.is_empty() {
        module.visit_mut_with(&mut DefaultReadRewriter {
            bindings: &rewrite_default,
        });
    }
}

fn is_empty(refs: &CrossModuleTsHelperRefs) -> bool {
    refs.direct.is_empty() && refs.namespaces.is_empty()
}

#[derive(Clone, Copy)]
enum Interop {
    Default,
    Star,
}

/// `local = helper(require("source"))` with a literal source and the
/// unresolved `require`.
fn interop_require_binding<'a>(
    declarator: &'a VarDeclarator,
    helper_kind: &impl Fn(&Expr) -> Option<Interop>,
    unresolved_mark: Mark,
) -> Option<(&'a Ident, Interop, &'a Str)> {
    let local = &declarator.name.as_ident()?.id;
    let Expr::Call(call) = strip_parens(declarator.init.as_deref()?) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let interop = helper_kind(callee)?;
    let [arg] = call.args.as_slice() else {
        return None;
    };
    if arg.spread.is_some() {
        return None;
    }
    let Expr::Call(require) = strip_parens(&arg.expr) else {
        return None;
    };
    let Callee::Expr(require_callee) = &require.callee else {
        return None;
    };
    let Expr::Ident(require_ident) = require_callee.as_ref() else {
        return None;
    };
    if require_ident.sym != "require" || require_ident.ctxt.outer() != unresolved_mark {
        return None;
    }
    let [source] = require.args.as_slice() else {
        return None;
    };
    if source.spread.is_some() {
        return None;
    }
    let Expr::Lit(Lit::Str(source)) = source.expr.as_ref() else {
        return None;
    };
    Some((local, interop, source))
}

/// How a binding of the helper result is written as an import, and which of
/// its uses keep their meaning.
#[derive(Clone, Copy)]
struct ImportPlan {
    namespace: bool,
    /// `.default` reads become the binding itself.
    default_is_binding: bool,
    /// Member reads other than `.default` are allowed.
    member_reads: bool,
    /// Uses of the binding as a value are allowed.
    value_uses: bool,
}

impl ImportPlan {
    fn accepts(&self, uses: &Uses) -> bool {
        !uses.write && (!uses.member_read || self.member_reads) && (!uses.value || self.value_uses)
    }
}

fn import_plan(interop: Interop, provider: &ModuleFacts) -> Option<ImportPlan> {
    let has_default = provider
        .exports
        .iter()
        .any(|export| export.kind == ExportKind::Default);
    let has_named = provider.has_export_all
        || provider
            .exports
            .iter()
            .any(|export| export.kind == ExportKind::Named);
    let plan = |namespace, default_is_binding, member_reads, value_uses| ImportPlan {
        namespace,
        default_is_binding,
        member_reads,
        value_uses,
    };
    if provider.marks_es_module {
        return match interop {
            Interop::Default => has_default.then(|| plan(false, true, false, false)),
            Interop::Star => Some(plan(true, false, true, true)),
        };
    }
    // The whole required value of an unmarked provider. A default export of
    // a module that never refers to `module` comes from `exports.default`
    // (or an ESM source), one property of that value: not proven either way.
    let whole_is_namespace = if has_default && !provider.require_returns_exports_object {
        false
    } else if !has_default && has_named {
        true
    } else {
        return None;
    };
    Some(match interop {
        Interop::Default => plan(whole_is_namespace, true, false, false),
        Interop::Star => plan(whole_is_namespace, true, true, false),
    })
}

struct Candidate {
    local: Ident,
    source: Str,
    plan: ImportPlan,
}

impl Candidate {
    fn import(&self) -> ModuleItem {
        let specifier = if self.plan.namespace {
            ImportSpecifier::Namespace(ImportStarAsSpecifier {
                span: DUMMY_SP,
                local: self.local.clone(),
            })
        } else {
            ImportSpecifier::Default(ImportDefaultSpecifier {
                span: DUMMY_SP,
                local: self.local.clone(),
            })
        };
        ModuleItem::ModuleDecl(ModuleDecl::Import(ImportDecl {
            span: DUMMY_SP,
            specifiers: vec![specifier],
            src: Box::new(Str {
                span: DUMMY_SP,
                value: self.source.value.clone(),
                raw: None,
            }),
            type_only: false,
            with: None,
            phase: Default::default(),
        }))
    }
}

#[derive(Default)]
struct Uses {
    write: bool,
    member_read: bool,
    value: bool,
}

struct UseCollector<'a> {
    candidates: &'a HashMap<BindingKey, Candidate>,
    uses: HashMap<BindingKey, Uses>,
    declarations: HashMap<BindingKey, usize>,
}

impl UseCollector<'_> {
    fn candidate(&self, expr: &Expr) -> Option<BindingKey> {
        let Expr::Ident(id) = expr else { return None };
        let key = binding_key(id);
        self.candidates.contains_key(&key).then_some(key)
    }

    fn record(&mut self, key: BindingKey, mark: impl FnOnce(&mut Uses)) {
        mark(self.uses.entry(key).or_default());
    }

    /// Marks a write to a candidate, or to a member of one.
    fn record_write(&mut self, expr: &Expr) {
        let target = match expr {
            Expr::Member(member) => member.obj.as_ref(),
            expr => expr,
        };
        if let Some(key) = self.candidate(target) {
            self.record(key, |uses| uses.write = true);
        }
    }
}

impl Visit for UseCollector<'_> {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let Some(binding) = declarator.name.as_ident() {
            let key = binding_key(&binding.id);
            if self.candidates.contains_key(&key) {
                *self.declarations.entry(key).or_default() += 1;
                declarator.init.visit_with(self);
                return;
            }
        }
        declarator.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if let Some(key) = self.candidate(&member.obj) {
            match static_member_prop_name(&member.prop) {
                Some("default") => {}
                Some(_) => self.record(key, |uses| uses.member_read = true),
                None => self.record(key, |uses| uses.value = true),
            }
            member.prop.visit_with(self);
            return;
        }
        member.visit_children_with(self);
    }

    fn visit_opt_chain_expr(&mut self, chain: &OptChainExpr) {
        // `x?.default` cannot become a plain binding read in place.
        if let OptChainBase::Member(member) = chain.base.as_ref() {
            if let Some(key) = self.candidate(&member.obj) {
                self.record(key, |uses| uses.value = true);
            }
        }
        chain.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
                self.record_write(&Expr::Ident(binding.id.clone()));
            }
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
                self.record_write(&member.obj);
            }
            AssignTarget::Pat(_) => {
                let mut writes = PatternWrites {
                    candidates: self.candidates,
                    found: Vec::new(),
                };
                assign.left.visit_with(&mut writes);
                for key in writes.found {
                    self.record(key, |uses| uses.write = true);
                }
            }
            _ => {}
        }
        assign.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        self.record_write(&update.arg);
        update.visit_children_with(self);
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        if unary.op == UnaryOp::Delete {
            self.record_write(&unary.arg);
        }
        unary.visit_children_with(self);
    }

    fn visit_ident(&mut self, id: &Ident) {
        let key = binding_key(id);
        if self.candidates.contains_key(&key) {
            self.record(key, |uses| uses.value = true);
        }
    }
}

/// Candidates that appear anywhere in a destructuring assignment target.
struct PatternWrites<'a> {
    candidates: &'a HashMap<BindingKey, Candidate>,
    found: Vec<BindingKey>,
}

impl Visit for PatternWrites<'_> {
    fn visit_ident(&mut self, id: &Ident) {
        let key = binding_key(id);
        if self.candidates.contains_key(&key) {
            self.found.push(key);
        }
    }
}

/// Rewrites `x.default` to `x` for the given bindings.
struct DefaultReadRewriter<'a> {
    bindings: &'a HashSet<BindingKey>,
}

impl VisitMut for DefaultReadRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Member(member) = expr {
            if let Expr::Ident(obj) = member.obj.as_ref() {
                if self.bindings.contains(&binding_key(obj))
                    && static_member_prop_name(&member.prop) == Some("default")
                {
                    *expr = Expr::Ident(obj.clone());
                    return;
                }
            }
        }
        expr.visit_mut_children_with(self);
    }
}
