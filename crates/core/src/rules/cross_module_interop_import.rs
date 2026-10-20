//! TypeScript's `importHelpers` interop calls whose helper is bundled as its
//! own module:
//!
//! ```js
//! const tslib_1 = require("./tslib.js");
//! const widget_js_1 = tslib_1.__importDefault(require("./widget.js"));
//! const util = tslib_1.__importStar(require("./util.js"));
//! ```
//!
//! `UnEsm` unwraps these calls when the helper is local, comes from
//! `require("tslib")`, or is an export barrier 0 proved in the module that
//! bundles it ([`crate::provider_helpers`]). Barrier 0 does not read every
//! helper module: one the input wrote as ESM (tslib's ES module build passed
//! as another input), one whose helper export only the late ESM recovery
//! exposes, or one whose export storage it cannot map. The consumer then
//! becomes ESM around a helper call that still wraps a `require`. In Phase 2
//! the helper is proven by the provider's helper export facts, and
//! [`run_cross_module_interop_imports`] turns each top-level binding of such
//! a call into an import of the wrapped module.
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
    cross_module_member_helper_kind, cross_module_ts_member_helper, CrossModuleHelperRefs,
    CrossModuleTsHelperRefs,
};
use super::decl_utils::{fresh_binding_ident, fresh_specifier_name};
use super::helper_matcher::{binding_key, static_member_prop_name, BindingKey};
use super::rename_utils::{collect_module_names, collect_unresolved_reference_names};
use super::transpiler_helper_utils::TranspilerHelperKind;
use crate::facts::{ExportKind, ModuleFacts, ModuleFactsMap, TypeScriptHelperKind};
use crate::kept_require::is_kept_require_call;
use crate::provider_interface::{ProviderInterface, RequiredValueImport};
use crate::utils::paren::{strip_parens, strip_parens_mut};

/// Turns top-level `x = helper(require("./p"))` bindings into imports of
/// `./p`, where `helper` is `__importDefault` or `__importStar` imported from
/// another module of the bundle and proven by its helper export facts.
pub(crate) fn run_cross_module_interop_imports(
    module: &mut Module,
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
    unresolved_mark: Mark,
) {
    let helpers = InteropHelperRefs::collect(module, module_facts, current_filename);
    if helpers.is_empty() {
        return;
    }
    // The rewrite removes a binding's declaration and renames its `.default`
    // reads; a `with` statement or a direct eval could still reach it by name
    // (docs/rewrite-assumptions.md, dynamic-scope skip).
    if super::eval_utils::has_dynamic_scope_construct(module) {
        return;
    }
    let helper_kind = |callee: &Expr| helpers.kind(callee);

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

/// The imported bindings of a module that its providers' facts prove to be
/// a default or wildcard interop helper.
struct InteropHelperRefs {
    semantic: CrossModuleHelperRefs,
    ts_default: CrossModuleTsHelperRefs,
    ts_star: CrossModuleTsHelperRefs,
}

impl InteropHelperRefs {
    fn collect(
        module: &Module,
        module_facts: &ModuleFactsMap,
        current_filename: Option<&str>,
    ) -> Self {
        // tslib's `__importDefault` has Babel's `interopRequireDefault` body,
        // so its facts are the semantic helper kind; `__importStar` is a raw
        // TypeScript helper fact. Both channels are read for both helpers.
        Self {
            semantic: collect_cross_module_helper_refs(
                module,
                module_facts,
                current_filename,
                |kind| {
                    matches!(
                        kind,
                        TranspilerHelperKind::InteropRequireDefault
                            | TranspilerHelperKind::InteropRequireWildcard
                    )
                },
            ),
            ts_default: collect_cross_module_ts_helper_refs(
                module,
                module_facts,
                current_filename,
                TypeScriptHelperKind::ImportDefault,
            ),
            ts_star: collect_cross_module_ts_helper_refs(
                module,
                module_facts,
                current_filename,
                TypeScriptHelperKind::ImportStar,
            ),
        }
    }

    fn is_empty(&self) -> bool {
        let ts_empty =
            |refs: &CrossModuleTsHelperRefs| refs.direct.is_empty() && refs.namespaces.is_empty();
        self.semantic.direct.is_empty()
            && self.semantic.namespaces.is_empty()
            && ts_empty(&self.ts_default)
            && ts_empty(&self.ts_star)
    }

    fn kind(&self, callee: &Expr) -> Option<Interop> {
        let callee = strip_parens(callee);
        let semantic = match callee {
            Expr::Ident(helper) => self.semantic.direct.get(&binding_key(helper)).copied(),
            callee => cross_module_member_helper_kind(callee, &self.semantic.namespaces),
        };
        let is_ts = |refs: &CrossModuleTsHelperRefs| match callee {
            Expr::Ident(helper) => refs.direct.contains(&binding_key(helper)),
            callee => cross_module_ts_member_helper(callee, &refs.namespaces),
        };
        match semantic {
            Some(TranspilerHelperKind::InteropRequireDefault) => Some(Interop::Default),
            Some(TranspilerHelperKind::InteropRequireWildcard) => Some(Interop::Star),
            _ if is_ts(&self.ts_default) => Some(Interop::Default),
            _ if is_ts(&self.ts_star) => Some(Interop::Star),
            _ => None,
        }
    }
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
    // An unmarked provider that stayed CommonJS: the default interop's
    // `.default` is its `module.exports`, which is what a default import of
    // a CommonJS module reads.
    if provider.stays_commonjs {
        return match interop {
            Interop::Default => Some(plan(false, true, false, false)),
            Interop::Star => None,
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

/// After [`run_cross_module_interop_imports`], in an ES module: the require
/// argument of each top-level `var x = h(require("./p"))` or
/// `var x = ns.h(require("./p"))` that pass left (the facts do not prove the
/// imported `h` an interop helper, or the binding has a use its import plan
/// rejects) becomes an import of the value the require returned:
/// `import p from "./p"; var x = h(p);`.
///
/// A provider that marked itself `__esModule` and became an ES module is the
/// exception: its namespace stands for the exports object but has no marker.
/// A proven interop helper returned such a provider unchanged, so the call
/// goes and `x` is the namespace; for any other function the marker may
/// matter, so the require stays.
///
/// With bundled helpers proven at barrier 0, the call this stage meets is
/// either a function that is no interop helper (a CommonJS function another
/// module exports, `var x = wrap(require("./p"))`) or a helper that neither
/// barrier 0 nor the facts prove.
///
/// `UnEsm` made the module ESM on the expectation that this stage takes the
/// require (`kept_require::blocking_require_calls`), and ESM has no
/// `require`. The call runs as before; only its provider loads with the
/// other imports, which `UnEsm` allowed by leaving the require for a later
/// stage only before the first hoisting barrier. The import form follows the
/// provider's facts as for any kept require
/// ([`ProviderInterface::required_value_import`]); a provider outside the
/// input is a default import, as `UnEsm` turns `var x = require("pkg")`
/// into one. A provider whose value no import holds keeps the require,
/// which the `commonjs_require_unrecovered` warning reports.
pub(crate) fn import_unproven_helper_requires(
    module: &mut Module,
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
    unresolved_mark: Mark,
) {
    if !module
        .body
        .iter()
        .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
    {
        return;
    }
    let mut imported = HashSet::default();
    for item in &module.body {
        if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item {
            imported.extend(import.specifiers.iter().map(|specifier| match specifier {
                ImportSpecifier::Named(named) => binding_key(&named.local),
                ImportSpecifier::Default(default) => binding_key(&default.local),
                ImportSpecifier::Namespace(namespace) => binding_key(&namespace.local),
            }));
        }
    }
    if imported.is_empty() {
        return;
    }
    let is_imported_helper = |callee: &Expr| {
        let helper = match strip_parens(callee) {
            Expr::Ident(id) => id,
            Expr::Member(member) => match strip_parens(&member.obj) {
                Expr::Ident(id) => id,
                _ => return false,
            },
            _ => return false,
        };
        imported.contains(&binding_key(helper))
    };

    let helpers = InteropHelperRefs::collect(module, module_facts, current_filename);
    let mut used_names = collect_module_names(module);
    used_names.extend(collect_unresolved_reference_names(module, unresolved_mark));
    let mut imports = Vec::new();
    for item in &mut module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for declarator in &mut var.decls {
            let Some(init) = declarator.init.as_mut() else {
                continue;
            };
            let Expr::Call(call) = strip_parens_mut(init) else {
                continue;
            };
            let Callee::Expr(callee) = &call.callee else {
                continue;
            };
            if !is_imported_helper(callee) {
                continue;
            }
            let Some(first) = call.args.first_mut() else {
                continue;
            };
            if first.spread.is_some() {
                continue;
            }
            let Expr::Call(require) = strip_parens(&first.expr) else {
                continue;
            };
            if !is_kept_require_call(require, unresolved_mark) {
                continue;
            }
            let [source] = require.args.as_slice() else {
                continue;
            };
            let Expr::Lit(Lit::Str(source)) = strip_parens(&source.expr) else {
                continue;
            };
            let specifier = source.value.as_str().unwrap_or("");
            let (namespace, drop_helper) = match module_facts.get_from(current_filename, specifier)
            {
                None => (false, false),
                Some(provider) => {
                    let provider = ProviderInterface::of(provider);
                    match provider.required_value_import() {
                        Some(RequiredValueImport::Default) => (false, false),
                        Some(RequiredValueImport::Namespace) if !provider.marks_es_module() => {
                            (true, false)
                        }
                        Some(RequiredValueImport::Namespace) if helpers.kind(callee).is_some() => {
                            (true, true)
                        }
                        _ => continue,
                    }
                }
            };
            let local =
                fresh_binding_ident(fresh_specifier_name(specifier, &mut used_names), DUMMY_SP);
            let source = source.clone();
            if drop_helper {
                **init = Expr::Ident(local.clone());
            } else {
                *first.expr = Expr::Ident(local.clone());
            }
            imports.push(
                Candidate {
                    local,
                    source,
                    plan: ImportPlan {
                        namespace,
                        default_is_binding: false,
                        member_reads: false,
                        value_uses: false,
                    },
                }
                .import(),
            );
        }
    }
    if imports.is_empty() {
        return;
    }
    let insert_at = module
        .body
        .iter()
        .rposition(|item| matches!(item, ModuleItem::ModuleDecl(ModuleDecl::Import(_))))
        .map_or(0, |index| index + 1);
    module.body.splice(insert_at..insert_at, imports);
}
