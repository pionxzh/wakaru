//! The plan unpack builds at barrier 0, where every module has reached
//! `UnEsm` but none has run it, from the require edges of all modules.
//!
//! Requiring a CommonJS module returns its `exports` object, and a consumer
//! may write to it. An ES module has no such object: its namespace is not
//! extensible and its properties cannot be assigned from outside. So a
//! provider that a consumer in the input writes to stays CommonJS, and
//! `UnEsm` reads that decision here instead of guessing alone.
//!
//! A provider counts as written when an edge to it has a write in its uses
//! ([`RequireUses::writes`]): a member assignment, update, or `delete`, the
//! target of an `Object` method that writes, or the argument of a function
//! of the consumer's module that writes that parameter. A value passed to a
//! callee in another module or to an unknown one does not count; a provider
//! recovered as a namespace that meets one gets the `commonjs_exports_escape`
//! warning after the fact barrier instead.
//!
//! One written provider may still become ESM: one whose only export is a
//! single top-level `module.exports = v` ([`exports_only_whole_value`]) that
//! `UnEsm` turns into `export default v` (not a `v` rooted at a `require`,
//! not an esbuild namespace `UnEsm` lowers to named exports), when every
//! write lands on `v` itself. Its ES form is `export default v`.
//! A consumer that becomes ESM writes through its default import, which
//! holds `v`; one that stays CommonJS reads `require(p).default` after the
//! fact barrier (`run_kept_require_default_reads`), which is `v` too. A
//! write counts as landing on `v` when the consumer is CommonJS in the
//! input (an ES module or webpack harmony consumer writes through a
//! namespace import), reads the value without an interop helper or an
//! `__esModule` test, and names neither `default` nor `__esModule`
//! ([`RequireUses::interop_key_write`]). The decision reads only what both
//! sides show before `UnEsm`, never the format a consumer ends up in. Such a
//! provider still counts as written: it keeps no proven helpers, and if
//! `UnEsm` leaves it CommonJS for another reason, the late ESM recovery
//! keeps it so (`keep_commonjs` in the rule pipeline options): a module only
//! that recovery converts misses the provider repairs on its own imports.
//!
//! A provider whose required value was never a writable CommonJS `exports`
//! object is left alone: a webpack harmony module (its exports are getters
//! `require.d` defined), and a module the input already wrote as ESM.
//!
//! The plan also lists, for each consumer, the sources that name a module
//! whose only export is one `module.exports = v`, written or not
//! ([`InterfacePlan::whole_value_sources`]). `require(source)` returns `v`,
//! the module's default export when `UnEsm` converts it and its
//! `module.exports` when it does not, so `UnEsm` reads `require(source).name`
//! through the default import instead of importing `name`, which such a
//! module never exports.
//!
//! The plan also carries the interop helpers each provider exports
//! ([`ProviderHelpers`]), so that `UnEsm` proves a helper a consumer reaches
//! through `require("./m.js")` as it proves one reached through
//! `require("tslib")`, but per proven export instead of by name. A written
//! provider's helpers are dropped: a consumer may have replaced one.
//!
//! A CommonJS passthrough, a module whose whole body is
//! `module.exports = require("x")` ([`passthrough_source`]), returns from
//! `require` what requiring `x` returns: the same object. So a write through
//! a passthrough is a write on the module at the end of its chain, and
//! counts there as a direct write does; so is a write through a module that
//! may return one of several required values ([`exports_from_requires`]), on
//! each of them. And the plan names, for each
//! consumer, the module at the end of every passthrough chain it requires
//! ([`InterfacePlan::passthrough_sources`]); `UnEsm` reads that module
//! instead: its run rewrites the consumer's `require("./pass")` to `require`
//! of that module first ([`rewrite_passthrough_requires`]). The answers
//! above for a source that names a passthrough are the answers for that
//! module, under the rewritten specifier. The passthrough itself stays
//! CommonJS.

use swc_core::atoms::Atom;
use swc_core::common::Mark;
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BinaryOp, CallExpr, Callee, Expr, Ident, Lit, MemberExpr,
    MemberProp, Module, ModuleItem, SimpleAssignTarget, Stmt, Str,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::collections::{HashMap, HashSet};
use crate::facts::{canonical_module_key, resolve_module_key};
use crate::facts::{collect_commonjs_value_is_default, collect_marks_es_module};
use crate::module_path::relative_import_specifier;
use crate::provider_helpers::ProviderHelpers;
use crate::require_edges::{
    require_call_source, CalleeExport, Interop, RequireEdge, RequireEdges, RequireUses,
};
use crate::rules::expr_utils::is_unresolved_ident;
use crate::rules::transpiler_helper_utils::TranspilerHelperKind;
use crate::utils::paren::strip_parens;

/// One module's input to the plan.
pub(crate) struct PlanModule<'a> {
    pub(crate) filename: &'a str,
    pub(crate) edges: &'a RequireEdges,
    /// Requiring the module returned its CommonJS `exports` object: the
    /// module has no import or export declaration and is not a webpack
    /// harmony module.
    pub(crate) exports_object_writable: bool,
    /// The interop helpers the module exports, read before `UnEsm`.
    pub(crate) helpers: Option<&'a ProviderHelpers>,
    /// The module's only export is one top-level `module.exports = v`
    /// ([`exports_only_whole_value`]).
    pub(crate) exports_whole_value: bool,
    /// The module is a CommonJS passthrough of this source
    /// ([`passthrough_source`]).
    pub(crate) passthrough: Option<Atom>,
    /// The sources whose required value may become the module's
    /// `module.exports` ([`exports_from_requires`]).
    pub(crate) exports_from_requires: Vec<Atom>,
}

/// The sources `s` of the `require("s")` calls whose value may become the
/// module's `module.exports`, read on the AST `UnEsm` will see: the right
/// side of any `module.exports = …` with `module` free, or a branch of a
/// conditional, a logical operator, or the last element of a sequence
/// there. A passthrough's source is one; so is each source of
/// `module.exports = c ? require("a") : require("b")` or of an
/// `if`/`else` that assigns either.
pub(crate) fn exports_from_requires(module: &Module, unresolved_mark: Mark) -> Vec<Atom> {
    struct Assignments {
        unresolved_mark: Mark,
        sources: Vec<Atom>,
    }
    impl Assignments {
        fn add(&mut self, expr: &Expr) {
            match strip_parens(expr) {
                Expr::Call(call) => {
                    if let Some(source) = require_call_source(call, self.unresolved_mark) {
                        if !self.sources.contains(&source) {
                            self.sources.push(source);
                        }
                    }
                }
                Expr::Cond(cond) => {
                    self.add(&cond.cons);
                    self.add(&cond.alt);
                }
                Expr::Bin(bin)
                    if matches!(
                        bin.op,
                        BinaryOp::LogicalOr | BinaryOp::LogicalAnd | BinaryOp::NullishCoalescing
                    ) =>
                {
                    self.add(&bin.left);
                    self.add(&bin.right);
                }
                Expr::Seq(seq) => {
                    if let Some(last) = seq.exprs.last() {
                        self.add(last);
                    }
                }
                Expr::Assign(assign) if assign.op == AssignOp::Assign => self.add(&assign.right),
                _ => {}
            }
        }
    }
    impl Visit for Assignments {
        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if assign.op == AssignOp::Assign
                && matches!(&assign.left, AssignTarget::Simple(SimpleAssignTarget::Member(target))
                    if matches!(strip_parens(&target.obj), Expr::Ident(object)
                        if is_unresolved_ident(object, "module", self.unresolved_mark))
                        && matches!(&target.prop, MemberProp::Ident(prop) if prop.sym == "exports"))
            {
                self.add(&assign.right);
            }
            assign.visit_children_with(self);
        }
    }
    let mut assignments = Assignments {
        unresolved_mark,
        sources: Vec::new(),
    };
    module.visit_with(&mut assignments);
    assignments.sources
}

/// The source `x` of a CommonJS passthrough: a module whose body, read on
/// the AST `UnEsm` will see, is directives and one statement
/// `module.exports = require("x")`, with `module` and `require` free. A
/// numeric module id (`require(17)`, a module outside the bundle) is no
/// source. Requiring such a module returns what requiring `x` returns.
///
/// The caller also checks that requiring the module returns its CommonJS
/// value: it is neither ESM nor a webpack harmony module.
pub(crate) fn passthrough_source(module: &Module, unresolved_mark: Mark) -> Option<Atom> {
    let mut statements = module.body.iter().skip_while(|item| {
        matches!(item, ModuleItem::Stmt(Stmt::Expr(statement))
            if matches!(statement.expr.as_ref(), Expr::Lit(Lit::Str(_))))
    });
    let (Some(ModuleItem::Stmt(Stmt::Expr(statement))), None) =
        (statements.next(), statements.next())
    else {
        return None;
    };
    let Expr::Assign(assign) = strip_parens(&statement.expr) else {
        return None;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assign.left else {
        return None;
    };
    if assign.op != AssignOp::Assign
        || !matches!(strip_parens(&target.obj), Expr::Ident(object)
            if is_unresolved_ident(object, "module", unresolved_mark))
        || !matches!(&target.prop, MemberProp::Ident(prop) if prop.sym == "exports")
    {
        return None;
    }
    let Expr::Call(call) = strip_parens(&assign.right) else {
        return None;
    };
    require_call_source(call, unresolved_mark)
}

/// Rewrites, in a consumer, every `require("s")` whose source `sources`
/// maps to another specifier, keeping the call and its span: the edge key
/// of the call stays the same. The rewritten specifier is no key of
/// `sources`, so running this again changes nothing. Returns whether a call
/// changed.
pub(crate) fn rewrite_passthrough_requires(
    module: &mut Module,
    unresolved_mark: Mark,
    sources: &HashMap<String, String>,
) -> bool {
    struct Rewriter<'a> {
        unresolved_mark: Mark,
        sources: &'a HashMap<String, String>,
        changed: bool,
    }
    impl VisitMut for Rewriter<'_> {
        fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
            call.visit_mut_children_with(self);
            let Some(source) = require_call_source(call, self.unresolved_mark) else {
                return;
            };
            let Some(target) = self.sources.get(source.as_ref()) else {
                return;
            };
            if let Expr::Lit(Lit::Str(literal)) = call.args[0].expr.as_mut() {
                *literal = Str {
                    span: literal.span,
                    value: target.as_str().into(),
                    raw: None,
                };
                self.changed = true;
            }
        }
    }
    if sources.is_empty() {
        return false;
    }
    let mut rewriter = Rewriter {
        unresolved_mark,
        sources,
        changed: false,
    };
    module.visit_mut_with(&mut rewriter);
    rewriter.changed
}

/// Where a chain of passthroughs ends.
#[derive(Debug, Clone)]
enum PassthroughEnd {
    /// A module of the input that is no passthrough, by canonical key.
    Module(String),
    /// A source outside the input: a bare package name or a `node:`
    /// builtin, as the last passthrough wrote it.
    External(Atom),
}

/// Follows the passthrough `start` (a canonical key in `passthroughs`) to
/// the first module that is no passthrough. `None` for a chain that meets
/// itself again (a cycle) and for a relative source that names no module of
/// the input.
fn passthrough_end(
    start: &str,
    passthroughs: &HashMap<String, (&str, Atom)>,
    keys: &HashSet<String>,
) -> Option<PassthroughEnd> {
    let mut seen: HashSet<&str> = HashSet::default();
    let mut current = passthroughs.get_key_value(start)?.0.as_str();
    loop {
        if !seen.insert(current) {
            return None;
        }
        let (filename, target) = passthroughs.get(current)?;
        match resolve_module_key(|key| keys.contains(key), Some(filename), target) {
            Some(next) => match passthroughs.get_key_value(&next) {
                Some((next, _)) => current = next.as_str(),
                None => return Some(PassthroughEnd::Module(next)),
            },
            None => {
                return (!target.starts_with('.') && !target.starts_with('/'))
                    .then(|| PassthroughEnd::External(target.clone()));
            }
        }
    }
}

/// Whether the module's only export is the value of one top-level
/// `module.exports = v`, read on the input before any rule: the statement
/// runs on every evaluation of the body and is the module's only
/// `module.exports` assignment ([`collect_commonjs_value_is_default`]), no
/// other `module.exports` member is read or written, the free `exports` is
/// never referenced (no `exports.x` write, no `exports` passed to a
/// function), and the module does not mark itself `__esModule`. `UnEsm`
/// turns such a module into `export default v` alone.
///
/// A `v` rooted at a `require` call is not such a value: `require(s)`
/// itself, a member chain on it, or a call of an identifier whose first
/// argument is `require(s)` (`interopDefault(require(s))`). `UnEsm` turns
/// the passthrough into a re-export, or keeps it CommonJS, and a wrapped
/// call into an import of the required module. Phase 1a also rejects a
/// module `UnEsm` lowers to the names of an esbuild `__toCommonJS`
/// namespace (`lowers_to_commonjs_namespace`), which this AST read cannot
/// tell from another call.
pub(crate) fn exports_only_whole_value(module: &Module, unresolved_mark: Mark) -> bool {
    struct Surface {
        unresolved_mark: Mark,
        module_exports: usize,
        exports: usize,
        rooted_at_require: bool,
    }
    impl Visit for Surface {
        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assign.left {
                if matches!(strip_parens(&target.obj), Expr::Ident(object)
                    if is_unresolved_ident(object, "module", self.unresolved_mark))
                    && matches!(&target.prop, MemberProp::Ident(prop) if prop.sym == "exports")
                    && is_rooted_at_require(&assign.right, self.unresolved_mark)
                {
                    self.rooted_at_require = true;
                }
            }
            assign.visit_children_with(self);
        }

        fn visit_member_expr(&mut self, member: &MemberExpr) {
            if matches!(strip_parens(&member.obj), Expr::Ident(object)
                if is_unresolved_ident(object, "module", self.unresolved_mark))
                && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "exports")
            {
                self.module_exports += 1;
            }
            member.visit_children_with(self);
        }

        fn visit_ident(&mut self, ident: &Ident) {
            if is_unresolved_ident(ident, "exports", self.unresolved_mark) {
                self.exports += 1;
            }
        }
    }
    if !collect_commonjs_value_is_default(module, unresolved_mark)
        || collect_marks_es_module(module, unresolved_mark)
    {
        return false;
    }
    let mut surface = Surface {
        unresolved_mark,
        module_exports: 0,
        exports: 0,
        rooted_at_require: false,
    };
    module.visit_with(&mut surface);
    surface.module_exports == 1 && surface.exports == 0 && !surface.rooted_at_require
}

/// `require(s)`, a member chain on it, or a call of an identifier whose
/// first argument is `require(s)`.
pub(crate) fn is_rooted_at_require(expr: &Expr, unresolved_mark: Mark) -> bool {
    match strip_parens(expr) {
        Expr::Member(member) => is_rooted_at_require(&member.obj, unresolved_mark),
        Expr::Call(call) if require_call_source(call, unresolved_mark).is_some() => true,
        Expr::Call(call) => {
            matches!(&call.callee, Callee::Expr(callee) if matches!(strip_parens(callee), Expr::Ident(_)))
                && call.args.first().is_some_and(|arg| {
                    arg.spread.is_none()
                        && matches!(strip_parens(&arg.expr), Expr::Call(inner)
                            if require_call_source(inner, unresolved_mark).is_some())
                })
        }
        _ => false,
    }
}

/// Whether `edge`'s writes land on the value `require` returned: the
/// consumer is CommonJS in the input, reads the value without an interop
/// helper or an `__esModule` test, and names neither `default` nor
/// `__esModule` in a write.
fn writes_the_value_itself(
    edge: &RequireEdge,
    consumer_writable: bool,
    uses: &RequireUses,
) -> bool {
    consumer_writable
        && edge.interop == Interop::None
        && !edge.esmodule_tested
        && !uses.interop_key_write
}

/// The decisions barrier 0 makes for every module of an unpack.
#[derive(Debug, Default)]
pub(crate) struct InterfacePlan {
    /// Providers a consumer writes to, by canonical key, with the first
    /// consumer that writes to each.
    written: HashMap<String, String>,
    /// The written providers that stay CommonJS: all but a whole-value
    /// provider whose every write lands on that value.
    kept: HashSet<String>,
    /// Modules whose required value was not a writable `exports` object.
    closed: HashSet<String>,
    /// The interop helpers of each provider no consumer writes to, by
    /// canonical key.
    helpers: HashMap<String, ProviderHelpers>,
    /// For each consumer, by canonical key: the sources it requires that
    /// resolve to a provider in `helpers`, with that provider's key.
    helper_sources: HashMap<String, Vec<(Atom, String)>>,
    /// For each consumer, by canonical key: the sources it requires that
    /// resolve to another module whose only export is one
    /// `module.exports = v` ([`PlanModule::exports_whole_value`]).
    whole_value_sources: HashMap<String, Vec<Atom>>,
    /// For each consumer, by canonical key: the sources it requires that
    /// name a passthrough, with the specifier of the module the chain ends
    /// at.
    passthrough_sources: HashMap<String, Vec<(Atom, Atom)>>,
}

impl InterfacePlan {
    pub(crate) fn build<'a>(modules: impl IntoIterator<Item = PlanModule<'a>>) -> Self {
        let modules: Vec<PlanModule<'a>> = modules.into_iter().collect();
        let keys: HashSet<String> = modules
            .iter()
            .map(|module| canonical_module_key(module.filename))
            .collect();
        let closed: HashSet<String> = modules
            .iter()
            .filter(|module| !module.exports_object_writable)
            .map(|module| canonical_module_key(module.filename))
            .collect();
        let whole_value: HashSet<String> = modules
            .iter()
            .filter(|module| module.exports_whole_value)
            .map(|module| canonical_module_key(module.filename))
            .collect();
        let passthroughs: HashMap<String, (&str, Atom)> = modules
            .iter()
            .filter_map(|module| {
                let target = module.passthrough.clone()?;
                Some((
                    canonical_module_key(module.filename),
                    (module.filename, target),
                ))
            })
            .collect();
        let exports_from_requires: HashMap<String, (&str, &[Atom])> = modules
            .iter()
            .filter(|module| !module.exports_from_requires.is_empty())
            .map(|module| {
                (
                    canonical_module_key(module.filename),
                    (module.filename, module.exports_from_requires.as_slice()),
                )
            })
            .collect();
        // Where each passthrough chain ends.
        let passthrough_ends: HashMap<&str, PassthroughEnd> = passthroughs
            .keys()
            .filter_map(|key| Some((key.as_str(), passthrough_end(key, &passthroughs, &keys)?)))
            .collect();
        // Each consumer's requires of a passthrough, with the specifier of
        // the module the chain ends at. A passthrough's own require is left
        // alone, and so is a chain that ends at the consumer itself.
        let mut passthrough_sources: HashMap<String, Vec<(Atom, Atom)>> = HashMap::default();
        if !passthrough_ends.is_empty() {
            for module in &modules {
                let consumer = canonical_module_key(module.filename);
                if passthroughs.contains_key(&consumer) {
                    continue;
                }
                let mut sources: Vec<(Atom, Atom)> = Vec::new();
                for edge in module.edges.iter().chain(module.edges.unkeyed()) {
                    if sources.iter().any(|(source, _)| source == &edge.source) {
                        continue;
                    }
                    let Some(end) = resolve_module_key(
                        |key| keys.contains(key),
                        Some(module.filename),
                        &edge.source,
                    )
                    .and_then(|provider| passthrough_ends.get(provider.as_str())) else {
                        continue;
                    };
                    let specifier = match end {
                        PassthroughEnd::Module(target) if *target == consumer => continue,
                        PassthroughEnd::Module(target) => {
                            Atom::from(relative_import_specifier(module.filename, target))
                        }
                        PassthroughEnd::External(target) => target.clone(),
                    };
                    sources.push((edge.source.clone(), specifier));
                }
                if !sources.is_empty() {
                    passthrough_sources.insert(consumer, sources);
                }
            }
        }
        // The source `UnEsm` will read for an edge, and the module of the
        // input it names: through a passthrough, the module its chain ends
        // at, under the rewritten specifier.
        let provider_of = |module: &PlanModule<'_>, source: &Atom| -> (Atom, Option<String>) {
            let aliased = passthrough_sources
                .get(&canonical_module_key(module.filename))
                .and_then(|sources| sources.iter().find(|(original, _)| original == source));
            let source = aliased.map_or(source, |(_, specifier)| specifier).clone();
            let provider =
                resolve_module_key(|key| keys.contains(key), Some(module.filename), &source);
            (source, provider)
        };
        let mut written: HashMap<String, String> = HashMap::default();
        let mut kept: HashSet<String> = HashSet::default();
        for module in &modules {
            for edge in module.edges.iter() {
                let Some(uses) = edge.uses.as_ref().filter(|uses| uses.writes()) else {
                    continue;
                };
                let Some(provider) = resolve_module_key(
                    |key| keys.contains(key),
                    Some(module.filename),
                    &edge.source,
                ) else {
                    continue;
                };
                // A write through a module whose `module.exports` may be
                // another module's required value (a passthrough, or a
                // conditional one) may land on that value: it counts on every
                // module such a chain reaches.
                let mut reached: Vec<String> = vec![provider];
                let mut index = 0;
                while let Some(provider) = reached.get(index).cloned() {
                    index += 1;
                    if let Some((filename, sources)) = exports_from_requires.get(&provider) {
                        for source in sources.iter() {
                            if let Some(next) =
                                resolve_module_key(|key| keys.contains(key), Some(filename), source)
                            {
                                if !reached.contains(&next) {
                                    reached.push(next);
                                }
                            }
                        }
                    }
                    if closed.contains(&provider) {
                        continue;
                    }
                    if !whole_value.contains(&provider)
                        || !writes_the_value_itself(edge, module.exports_object_writable, uses)
                    {
                        kept.insert(provider.clone());
                    }
                    written
                        .entry(provider)
                        .or_insert_with(|| module.filename.to_string());
                }
            }
        }
        let helpers: HashMap<String, ProviderHelpers> = modules
            .iter()
            .filter_map(|module| {
                let key = canonical_module_key(module.filename);
                (!written.contains_key(&key))
                    .then(|| module.helpers.map(|helpers| (key, helpers.clone())))
                    .flatten()
            })
            .collect();
        let mut helper_sources: HashMap<String, Vec<(Atom, String)>> = HashMap::default();
        if !helpers.is_empty() {
            for module in &modules {
                let mut sources: Vec<(Atom, String)> = Vec::new();
                for edge in module.edges.iter().chain(module.edges.unkeyed()) {
                    let (source, Some(provider)) = provider_of(module, &edge.source) else {
                        continue;
                    };
                    if sources.iter().any(|(listed, _)| *listed == source) {
                        continue;
                    }
                    if helpers.contains_key(&provider) {
                        sources.push((source, provider));
                    }
                }
                if !sources.is_empty() {
                    helper_sources.insert(canonical_module_key(module.filename), sources);
                }
            }
        }
        let mut whole_value_sources: HashMap<String, Vec<Atom>> = HashMap::default();
        if !whole_value.is_empty() {
            for module in &modules {
                let consumer = canonical_module_key(module.filename);
                let mut sources: Vec<Atom> = Vec::new();
                for edge in module.edges.iter().chain(module.edges.unkeyed()) {
                    let (source, Some(provider)) = provider_of(module, &edge.source) else {
                        continue;
                    };
                    if sources.contains(&source) {
                        continue;
                    }
                    if provider != consumer && whole_value.contains(&provider) {
                        sources.push(source);
                    }
                }
                if !sources.is_empty() {
                    whole_value_sources.insert(consumer, sources);
                }
            }
        }
        Self {
            written,
            kept,
            closed,
            helpers,
            helper_sources,
            whole_value_sources,
            passthrough_sources,
        }
    }

    /// The sources `filename` requires that name a CommonJS passthrough,
    /// each with the specifier of the module its chain ends at: the
    /// relative path of a module of the input, or the source outside the
    /// input (`"fs"`, `"node:crypto"`) as the last passthrough wrote it.
    /// Requiring the passthrough returns what requiring that module
    /// returns. A chain that meets itself again or ends at `filename`, a
    /// relative source no module of the input matches, and a passthrough's
    /// own require are left out.
    pub(crate) fn passthrough_sources(&self, filename: &str) -> HashMap<String, String> {
        self.passthrough_sources
            .get(&canonical_module_key(filename))
            .into_iter()
            .flatten()
            .map(|(source, specifier)| (source.to_string(), specifier.to_string()))
            .collect()
    }

    /// Whether `UnEsm` keeps the module CommonJS because a consumer in the
    /// input writes to its `exports` object.
    pub(crate) fn keeps_commonjs(&self, filename: &str) -> bool {
        self.kept.contains(&canonical_module_key(filename))
    }

    /// Whether a consumer in the input writes to the module's required
    /// value, whether or not that keeps it CommonJS.
    pub(crate) fn is_written(&self, filename: &str) -> bool {
        self.written.contains_key(&canonical_module_key(filename))
    }

    /// The first consumer that writes to the module, when one does.
    pub(crate) fn written_by(&self, filename: &str) -> Option<&str> {
        self.written
            .get(&canonical_module_key(filename))
            .map(String::as_str)
    }

    /// The interop helpers of the providers `filename` requires, by the
    /// source it names each with.
    pub(crate) fn helper_sources(&self, filename: &str) -> HashMap<String, ProviderHelpers> {
        self.helper_sources
            .get(&canonical_module_key(filename))
            .into_iter()
            .flatten()
            .filter_map(|(source, provider)| {
                Some((source.to_string(), self.helpers.get(provider)?.clone()))
            })
            .collect()
    }

    /// The sources `filename` requires that name another module whose only
    /// export is one `module.exports = v`: `require(source)` returns `v`,
    /// which `UnEsm` makes that module's default export when it converts
    /// it, and which stays its `module.exports` when it does not.
    pub(crate) fn whole_value_sources(&self, filename: &str) -> HashSet<String> {
        self.whole_value_sources
            .get(&canonical_module_key(filename))
            .into_iter()
            .flatten()
            .map(|source| source.to_string())
            .collect()
    }

    /// The interop helpers the module exports, when it is not written.
    pub(crate) fn helper_exports(&self, filename: &str) -> Option<&ProviderHelpers> {
        self.helpers.get(&canonical_module_key(filename))
    }

    /// The interop helper `filename` reaches as `export` of what it
    /// requires from `source`.
    pub(crate) fn helper_kind(
        &self,
        filename: &str,
        source: &str,
        export: &CalleeExport,
    ) -> Option<TranspilerHelperKind> {
        let (_, provider) = self
            .helper_sources
            .get(&canonical_module_key(filename))?
            .iter()
            .find(|(required, _)| required.as_ref() == source)?;
        self.helpers.get(provider)?.kind_of(export)
    }

    /// Whether requiring the module returned a writable CommonJS `exports`
    /// object.
    pub(crate) fn exports_object_writable(&self, filename: &str) -> bool {
        !self.closed.contains(&canonical_module_key(filename))
    }
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
    use swc_core::ecma::parser::{parse_file_as_module, EsSyntax, Syntax};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::VisitMutWith;

    use super::*;
    use crate::rules::lowers_to_commonjs_namespace;
    use crate::rules::transpiler_helper_utils::LocalHelperContext;

    /// Parses and resolves `source`, then reads it with `read`.
    fn with_module<T>(source: &str, read: impl FnOnce(&Module, Mark) -> T) -> T {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let file = cm.new_source_file(FileName::Anon.into(), source.to_string());
            let mut module = parse_file_as_module(
                &file,
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                None,
                &mut Vec::new(),
            )
            .expect("test source parses");
            let mark = Mark::new();
            module.visit_mut_with(&mut resolver(mark, Mark::new(), false));
            read(&module, mark)
        })
    }

    fn edges(source: &str) -> RequireEdges {
        with_module(source, |module, mark| {
            let mut edges = RequireEdges::collect(module, mark);
            edges.add_uses(
                module,
                mark,
                &LocalHelperContext::collect_with_mark(module, mark),
            );
            edges
        })
    }

    fn whole_value(source: &str) -> bool {
        with_module(source, exports_only_whole_value)
    }

    /// Whether `UnEsm` lowers `source` to the names of an esbuild namespace,
    /// read as Phase 1a reads it: after the rules before `UnEsm`.
    fn lowers_to_namespace(source: &str) -> bool {
        use crate::rules::{apply_rules, PipelineMode, RulePipelineOptions};
        with_module(source, |module, mark| {
            let mut module = module.clone();
            apply_rules(
                &mut module,
                mark,
                RulePipelineOptions::before("UnEsm").with_mode(PipelineMode::UnpackPhase1),
            );
            lowers_to_commonjs_namespace(&module, mark)
        })
    }

    /// The plan for one consumer `src/consumer.js` beside the providers
    /// `src/<name>.js`; `closed` names the providers whose required value
    /// was not a writable `exports` object.
    fn plan(consumer: &str, providers: &[&str], closed: &[&str]) -> InterfacePlan {
        plan_with(consumer, true, providers, closed, &[])
    }

    /// [`plan`] with the consumer's own `exports` writable or not, and
    /// `whole` naming the providers whose only export is one
    /// `module.exports = v`.
    fn plan_with(
        consumer: &str,
        consumer_writable: bool,
        providers: &[&str],
        closed: &[&str],
        whole: &[&str],
    ) -> InterfacePlan {
        let consumer_edges = edges(consumer);
        let empty = RequireEdges::default();
        let names: Vec<String> = providers
            .iter()
            .map(|name| format!("src/{name}.js"))
            .collect();
        let mut modules = vec![PlanModule {
            filename: "src/consumer.js",
            edges: &consumer_edges,
            exports_object_writable: consumer_writable,
            helpers: None,
            exports_whole_value: false,
            passthrough: None,
            exports_from_requires: Vec::new(),
        }];
        for (name, filename) in providers.iter().zip(&names) {
            modules.push(PlanModule {
                filename,
                edges: &empty,
                exports_object_writable: !closed.contains(name),
                helpers: None,
                exports_whole_value: whole.contains(name),
                passthrough: None,
                exports_from_requires: Vec::new(),
            });
        }
        InterfacePlan::build(modules)
    }

    #[test]
    fn each_write_form_keeps_its_provider_commonjs() {
        let plan = plan(
            r#"var a = require("./assigned.js");
a.f = 1;
require("./inlined").g = 2;
var u = require("./updated.js");
if (x) { u.count++; }
delete require("./deleted.js").gone;
Object.assign(require("./assign-target.js"), extra);
Object.defineProperty(require("./defined.js"), "k", { get: k });
function install(m) { m.f = 1; }
install(require("./installed.js"));
var r = require("./read.js");
use(r.f);"#,
            &[
                "assigned",
                "inlined",
                "updated",
                "deleted",
                "assign-target",
                "defined",
                "installed",
                "read",
            ],
            &[],
        );
        for name in [
            "assigned",
            "inlined",
            "updated",
            "deleted",
            "assign-target",
            "defined",
            "installed",
        ] {
            let filename = format!("src/{name}.js");
            assert!(plan.keeps_commonjs(&filename), "{name}");
            assert_eq!(plan.written_by(&filename), Some("src/consumer.js"));
        }
        assert!(!plan.keeps_commonjs("src/read.js"));
    }

    #[test]
    fn escapes_that_prove_no_write_leave_the_provider_free() {
        let plan = plan(
            r#"function describe(m) { return Object.keys(m); }
describe(require("./local-reader.js"));
Object.keys(require("./builtin-reader.js"));
var installer = require("./installer.js");
installer.install(require("./cross-module.js"));
register(require("./unknown.js"));
__exportStar(require("./reexported.js"), exports);"#,
            &[
                "local-reader",
                "builtin-reader",
                "installer",
                "cross-module",
                "unknown",
                "reexported",
            ],
            &[],
        );
        for name in [
            "local-reader",
            "builtin-reader",
            "installer",
            "cross-module",
            "unknown",
            "reexported",
        ] {
            assert!(!plan.keeps_commonjs(&format!("src/{name}.js")), "{name}");
        }
    }

    #[test]
    fn a_provider_whose_exports_were_never_writable_is_left_alone() {
        let plan = plan(
            r#"require("./harmony.js").f = 1;
require("./plain.js").f = 1;"#,
            &["harmony", "plain"],
            &["harmony"],
        );
        assert!(!plan.keeps_commonjs("src/harmony.js"));
        assert!(!plan.exports_object_writable("src/harmony.js"));
        assert!(plan.keeps_commonjs("src/plain.js"));
        assert!(plan.exports_object_writable("src/plain.js"));
    }

    fn provider_helpers(source: &str) -> ProviderHelpers {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let file = cm.new_source_file(FileName::Anon.into(), source.to_string());
            let mut module = parse_file_as_module(
                &file,
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                None,
                &mut Vec::new(),
            )
            .expect("test source parses");
            let mark = Mark::new();
            module.visit_mut_with(&mut resolver(mark, Mark::new(), false));
            ProviderHelpers::collect(&module, mark).expect("the provider exports a helper")
        })
    }

    #[test]
    fn a_written_provider_carries_no_helpers() {
        let helpers = provider_helpers(
            "module.exports = function (e) { return e && e.__esModule ? e : { default: e }; };",
        );
        let consumer = edges(
            r#"var h = require("./helper.js");
var w = require("./written.js");
w.f = 1;
h(require("./other.js"));"#,
        );
        let empty = RequireEdges::default();
        let plan = InterfacePlan::build([
            PlanModule {
                filename: "src/consumer.js",
                edges: &consumer,
                exports_object_writable: true,
                helpers: None,
                exports_whole_value: false,
                passthrough: None,
                exports_from_requires: Vec::new(),
            },
            PlanModule {
                filename: "src/helper.js",
                edges: &empty,
                exports_object_writable: true,
                helpers: Some(&helpers),
                exports_whole_value: false,
                passthrough: None,
                exports_from_requires: Vec::new(),
            },
            PlanModule {
                filename: "src/written.js",
                edges: &empty,
                exports_object_writable: true,
                helpers: Some(&helpers),
                exports_whole_value: false,
                passthrough: None,
                exports_from_requires: Vec::new(),
            },
        ]);
        assert!(plan.helper_exports("src/helper.js").is_some());
        assert!(plan.helper_exports("src/written.js").is_none());
        let sources = plan.helper_sources("src/consumer.js");
        assert_eq!(
            sources.keys().map(String::as_str).collect::<Vec<_>>(),
            ["./helper.js"]
        );
        assert!(plan.helper_sources("src/helper.js").is_empty());
        assert_eq!(
            plan.helper_kind("src/consumer.js", "./helper.js", &CalleeExport::Value),
            Some(TranspilerHelperKind::InteropRequireDefault)
        );
        assert_eq!(
            plan.helper_kind(
                "src/consumer.js",
                "./helper.js",
                &CalleeExport::Named("default".into())
            ),
            None
        );
        assert_eq!(
            plan.helper_kind("src/consumer.js", "./written.js", &CalleeExport::Value),
            None
        );
    }

    #[test]
    fn only_a_single_top_level_module_exports_value_is_a_whole_value_export() {
        // An empty registry object; core-js 2's `_global.js` (wild-observed,
        // webpack-bundled); core-js 3's `internals/shared-store.js` as a
        // minifier joins it (wild-observed, core-js@3.46.0); and an object
        // filled through its alias.
        for source in [
            "module.exports = {};",
            r#"var global = module.exports = typeof window != "undefined" && window.Math == Math ? window : typeof self != "undefined" && self.Math == Math ? self : Function("return this")();
if (typeof __g == "number") __g = global;"#,
            r#""use strict";
var n = require("./module-111.js"), o = require("./module-222.js"), i = require("./module-333.js"), c = "__core-js_shared__", a = module.exports = o[c] || i(c, {});
(a.versions || (a.versions = [])).push({ version: "3.46.0", mode: n ? "pure" : "global" });"#,
            "var o = module.exports = {}; o.f = function () {};",
        ] {
            assert!(whole_value(source), "{source}");
        }
        for source in [
            "exports.f = 1;",
            "module.exports = v; module.exports.x = 1;",
            "module.exports = v; Object.defineProperty(module.exports, \"__esModule\", { value: true });",
            "Object.defineProperty(exports, \"__esModule\", { value: true }); module.exports = v;",
            "module.exports = v; install(exports);",
            "if (c) module.exports = v;",
            "module.exports = v; module.exports = w;",
            "module.exports = v; module.exports.default = module.exports;",
        ] {
            assert!(!whole_value(source), "{source}");
        }
    }

    #[test]
    fn a_value_rooted_at_a_require_is_not_a_whole_value_export() {
        // `UnEsm` turns a passthrough into a re-export or keeps it
        // CommonJS, and a wrapped require into an import of that module.
        for source in [
            r#"module.exports = require("./a");"#,
            r#"module.exports = (require("./a"));"#,
            r#"module.exports = require("./a").f;"#,
            r#"module.exports = require("./a").f.g;"#,
            r#"var o = module.exports = require("./a");"#,
            r#"function i(e) { return e && e.__esModule ? e : { default: e }; }
module.exports = i(require("./a"));"#,
        ] {
            assert!(!whole_value(source), "{source}");
        }
        // A member callee, or a require that is not the first argument, is
        // a value of its own.
        for source in [
            r#"module.exports = Object.assign({}, require("./a"));"#,
            r#"module.exports = f(1, require("./a"));"#,
            r#"module.exports = require("./a")();"#,
            "module.exports = class {};",
            "module.exports = mk();",
        ] {
            assert!(whole_value(source), "{source}");
        }
    }

    /// esbuild's CommonJS output of an ES module (producer esbuild
    /// `--format=cjs`), with `entries` in its `__export` call.
    fn esbuild_namespace(entries: &str, tail: &str) -> String {
        format!(
            r#"var __defProp = Object.defineProperty;
var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
var __getOwnPropNames = Object.getOwnPropertyNames;
var __hasOwnProp = Object.prototype.hasOwnProperty;
var __export = (target, all) => {{
  for (var name in all)
    __defProp(target, name, {{ get: all[name], enumerable: true }});
}};
var __copyProps = (to, from, except, desc) => {{
  if (from && typeof from === "object" || typeof from === "function") {{
    for (let key of __getOwnPropNames(from))
      if (!__hasOwnProp.call(to, key) && key !== except)
        __defProp(to, key, {{ get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable }});
  }}
  return to;
}};
var __toCommonJS = (mod) => __copyProps(__defProp({{}}, "__esModule", {{ value: true }}), mod);
var mod_exports = {{}};
__export(mod_exports, {{ {entries} }});
module.exports = __toCommonJS(mod_exports);
function foo() {{ return 1; }}
{tail}"#
        )
    }

    #[test]
    fn an_esbuild_namespace_is_not_a_whole_value_export_at_the_barrier() {
        // The AST read on the input sees one `module.exports = call()`;
        // Phase 1a also asks whether `UnEsm` lowers it to the namespace's
        // names, the default getter's value included.
        for source in [
            esbuild_namespace("foo: () => foo", ""),
            esbuild_namespace(
                "foo: () => foo, default: () => mod_default",
                "var mod_default = { other: 1 };",
            ),
        ] {
            assert!(whole_value(&source), "{source}");
            assert!(lowers_to_namespace(&source), "{source}");
        }
        for source in [
            "module.exports = {};",
            "module.exports = class K {};",
            "module.exports = mk();",
            r#"module.exports = typeof window != "undefined" ? window : Function("return this")();"#,
        ] {
            assert!(!lowers_to_namespace(source), "{source}");
        }
    }

    #[test]
    fn a_whole_value_provider_written_on_its_value_is_not_kept() {
        let plan = plan_with(
            r#"var r = require("./registry.js");
r[key] = true;
r.flag = 1;
r.count++;
delete r.gone;
Object.assign(r, extra);
function install(m) { m.extra = 1; }
install(require("./registry.js"));
require("./registry.js").inline = 2;
var n = require("./named.js");
n.f = 1;"#,
            true,
            &["registry", "named"],
            &[],
            &["registry"],
        );
        assert!(!plan.keeps_commonjs("src/registry.js"));
        assert!(plan.is_written("src/registry.js"));
        assert_eq!(plan.written_by("src/registry.js"), Some("src/consumer.js"));
        // A provider that also has named exports keeps its `exports`
        // object, the only one a write can land on.
        assert!(plan.keeps_commonjs("src/named.js"));
    }

    #[test]
    fn a_write_that_may_miss_the_whole_value_keeps_its_provider() {
        let plan = plan_with(
            r#"var d = require("./default-key.js");
d.default = replacement;
Object.defineProperty(require("./marker.js"), "__esModule", { value: true });
function mark(m) { m.__esModule = true; }
mark(require("./marked-by-callee.js"));
function w(e) {
    if (e && e.__esModule) return e;
    var n = {};
    if (e != null) for (var k in e) if (Object.prototype.hasOwnProperty.call(e, k)) n[k] = e[k];
    n.default = e;
    return n;
}
var copy = w(require("./wildcard.js"));
copy.flag = 1;
var t = require("./tested.js");
if (!t.__esModule) t.flag = 1;"#,
            true,
            &[
                "default-key",
                "marker",
                "marked-by-callee",
                "wildcard",
                "tested",
            ],
            &[],
            &[
                "default-key",
                "marker",
                "marked-by-callee",
                "wildcard",
                "tested",
            ],
        );
        for name in [
            "default-key",
            "marker",
            "marked-by-callee",
            "wildcard",
            "tested",
        ] {
            assert!(plan.keeps_commonjs(&format!("src/{name}.js")), "{name}");
        }
    }

    #[test]
    fn a_write_from_an_es_module_consumer_keeps_a_whole_value_provider() {
        // A harmony consumer's require stands for a namespace import.
        let plan = plan_with(
            r#"var r = require("./registry.js");
r.flag = 1;"#,
            false,
            &["registry"],
            &[],
            &["registry"],
        );
        assert!(plan.keeps_commonjs("src/registry.js"));
    }

    #[test]
    fn a_written_whole_value_provider_carries_no_helpers() {
        let helpers = provider_helpers(
            "module.exports = function (e) { return e && e.__esModule ? e : { default: e }; };",
        );
        let consumer = edges(
            r#"var h = require("./helper.js");
h.patched = true;"#,
        );
        let empty = RequireEdges::default();
        let plan = InterfacePlan::build([
            PlanModule {
                filename: "src/consumer.js",
                edges: &consumer,
                exports_object_writable: true,
                helpers: None,
                exports_whole_value: false,
                passthrough: None,
                exports_from_requires: Vec::new(),
            },
            PlanModule {
                filename: "src/helper.js",
                edges: &empty,
                exports_object_writable: true,
                helpers: Some(&helpers),
                exports_whole_value: true,
                passthrough: None,
                exports_from_requires: Vec::new(),
            },
        ]);
        assert!(!plan.keeps_commonjs("src/helper.js"));
        assert!(plan.helper_exports("src/helper.js").is_none());
        assert!(plan.helper_sources("src/consumer.js").is_empty());
    }

    #[test]
    fn whole_value_sources_name_the_whole_value_providers_a_module_requires() {
        // The consumer's own only export is a whole value too: a
        // self-require is not a provider of it.
        let consumer = edges(
            r#"var a = require("./whole.js").a;
var again = require("./whole.js").again;
var b = require("./named.js").b;
var self = require("./consumer.js").self;
var d = require("lodash").d;
module.exports = make();"#,
        );
        let empty = RequireEdges::default();
        let module = |filename, edges, exports_whole_value| PlanModule {
            filename,
            edges,
            exports_object_writable: true,
            helpers: None,
            exports_whole_value,
            passthrough: None,
            exports_from_requires: Vec::new(),
        };
        let plan = InterfacePlan::build([
            module("src/consumer.js", &consumer, true),
            module("src/whole.js", &empty, true),
            module("src/named.js", &empty, false),
        ]);
        assert_eq!(
            plan.whole_value_sources("src/consumer.js"),
            HashSet::from_iter(["./whole.js".to_string()])
        );
        assert!(plan.whole_value_sources("src/whole.js").is_empty());
    }

    fn passthrough(source: &str) -> Option<Atom> {
        with_module(source, passthrough_source)
    }

    #[test]
    fn only_a_lone_module_exports_of_a_require_is_a_passthrough() {
        // The react-is and use-sync-external-store entry shape
        // (wild-observed), as the extractor leaves it.
        assert_eq!(
            passthrough(r#"module.exports = require("./module-100.js");"#).as_deref(),
            Some("./module-100.js")
        );
        assert_eq!(
            passthrough(r#""use strict"; "client"; module.exports = (require("node:crypto"));"#)
                .as_deref(),
            Some("node:crypto")
        );
        for source in [
            r#"module.exports = require("./a"); exports.x = 1;"#,
            r#"exports.x = 1; module.exports = require("./a");"#,
            r#"module.exports = require("./a").f;"#,
            r#"module.exports = require("./a")();"#,
            r#"if (c) module.exports = require("./a");"#,
            r#"module.exports = c ? require("./a") : require("./b");"#,
            r#"exports.a = require("./a");"#,
            r#"module.exports = require(17);"#,
            // A local `require` needs a declaration, an extra statement.
            r#"var require = load; module.exports = require("./a");"#,
            r#"module.exports += require("./a");"#,
            r#"var t = module.exports = require("./a");"#,
            // A webpack harmony module, and a module the input wrote as ESM.
            r#"require.r(exports); module.exports = require("./a");"#,
            r#"export default require("./a");"#,
        ] {
            assert_eq!(passthrough(source), None, "{source}");
        }
    }

    /// The plan for `src/consumer.js` beside the modules `src/<name>.js`,
    /// each with its passthrough source, if any, and whether its only
    /// export is one `module.exports = v`.
    fn plan_with_passthroughs(
        consumer: &str,
        modules: &[(&str, Option<&str>, bool)],
    ) -> InterfacePlan {
        let consumer_edges = edges(consumer);
        let module_edges: Vec<(String, RequireEdges)> = modules
            .iter()
            .map(|(name, passthrough, _)| {
                (
                    format!("src/{name}.js"),
                    passthrough
                        .map(|source| edges(&format!(r#"module.exports = require("{source}");"#)))
                        .unwrap_or_default(),
                )
            })
            .collect();
        let mut plan_modules = vec![PlanModule {
            filename: "src/consumer.js",
            edges: &consumer_edges,
            exports_object_writable: true,
            helpers: None,
            exports_whole_value: false,
            passthrough: None,
            exports_from_requires: Vec::new(),
        }];
        for ((_, passthrough, whole), (filename, edges)) in modules.iter().zip(&module_edges) {
            plan_modules.push(PlanModule {
                filename,
                edges,
                exports_object_writable: true,
                helpers: None,
                exports_whole_value: *whole,
                passthrough: passthrough.map(Atom::from),
                exports_from_requires: passthrough
                    .iter()
                    .map(|source| Atom::from(*source))
                    .collect(),
            });
        }
        InterfacePlan::build(plan_modules)
    }

    #[test]
    fn a_write_through_a_passthrough_counts_on_the_module_its_chain_ends_at() {
        let plan = plan_with_passthroughs(
            r#"var p = require("./pass.js");
p.f = 1;
require("./outer.js").g = 2;
require("./whole-pass.js").k = 3;
require("./cycle-a.js").h = 4;
require("./external.js").i = 5;"#,
            &[
                ("named", None, false),
                ("pass", Some("./named.js"), false),
                ("chained", None, false),
                ("inner", Some("./chained.js"), false),
                ("outer", Some("./inner.js"), false),
                ("whole", None, true),
                ("whole-pass", Some("./whole.js"), false),
                ("cycle-a", Some("./cycle-b.js"), false),
                ("cycle-b", Some("./cycle-a.js"), false),
                ("external", Some("lodash"), false),
            ],
        );
        for name in ["named", "chained"] {
            let filename = format!("src/{name}.js");
            assert!(plan.keeps_commonjs(&filename), "{name}");
            assert_eq!(plan.written_by(&filename), Some("src/consumer.js"));
        }
        // The passthrough itself is written too; it stays CommonJS anyway.
        assert!(plan.is_written("src/pass.js"));
        // A write through a passthrough lands on a whole value as a direct
        // write does.
        assert!(plan.is_written("src/whole.js"));
        assert!(!plan.keeps_commonjs("src/whole.js"));
        // Every module a chain reaches is written, a cycle's members and an
        // intermediate passthrough included; each stays CommonJS anyway.
        for name in ["cycle-a", "cycle-b", "inner"] {
            assert!(plan.is_written(&format!("src/{name}.js")), "{name}");
        }
    }

    #[test]
    fn a_module_exports_assigned_a_required_value_lists_its_source() {
        let sources = |source: &str| {
            with_module(source, exports_from_requires)
                .iter()
                .map(Atom::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(sources(r#"module.exports = require("./a");"#), ["./a"]);
        assert_eq!(
            sources(
                r#"if (globalThis.PROD) module.exports = require("./a"); else module.exports = require("./b");"#
            ),
            ["./a", "./b"]
        );
        assert_eq!(
            sources(
                r#"module.exports = c ? require("./a") : (x, require("./b")) || require("./c");"#
            ),
            ["./a", "./b", "./c"]
        );
        assert_eq!(
            sources(r#"var t = module.exports = require("./a"); t.more = 1;"#),
            ["./a"]
        );
        for source in [
            r#"module.exports = require("./a").f;"#,
            r#"module.exports = wrap(require("./a"));"#,
            r#"exports.a = require("./a");"#,
            r#"module.exports = require(17);"#,
        ] {
            assert!(sources(source).is_empty(), "{source}");
        }
    }

    #[test]
    fn a_write_through_a_module_that_may_return_a_required_value_counts_on_each() {
        // `if (c) module.exports = require("./a"); else module.exports =
        // require("./b");`: a write through the module may land on either.
        let consumer = edges(
            r#"var p = require("./cond.js");
p.f = 1;"#,
        );
        let empty = RequireEdges::default();
        let module = |filename, edges, exports_from_requires: &[&str]| PlanModule {
            filename,
            edges,
            exports_object_writable: true,
            helpers: None,
            exports_whole_value: false,
            passthrough: None,
            exports_from_requires: exports_from_requires
                .iter()
                .map(|s| Atom::from(*s))
                .collect(),
        };
        let plan = InterfacePlan::build([
            module("src/consumer.js", &consumer, &[]),
            module("src/cond.js", &empty, &["./a.js", "./b.js"]),
            module("src/a.js", &empty, &[]),
            module("src/b.js", &empty, &[]),
            module("src/c.js", &empty, &[]),
        ]);
        for name in ["cond", "a", "b"] {
            assert!(plan.keeps_commonjs(&format!("src/{name}.js")), "{name}");
        }
        assert!(!plan.is_written("src/c.js"));
        // No alias: the module is no passthrough.
        assert!(plan.passthrough_sources("src/consumer.js").is_empty());
    }

    #[test]
    fn a_require_of_a_passthrough_names_the_module_its_chain_ends_at() {
        let plan = plan_with_passthroughs(
            r#"var a = require("./pass.js");
var b = require("./outer");
var c = require("./cycle-a.js");
var d = require("./to-consumer.js");
var e = require("./fs-pass.js");
var f = require("./builtin-pass.js");
var g = require("./missing-pass.js");
var h = require("./named.js");
function later() { return require("./pass.js"); }"#,
            &[
                ("named", None, false),
                ("pass", Some("./named.js"), false),
                ("deep/chained", None, false),
                ("deep/inner", Some("./chained.js"), false),
                ("outer", Some("./deep/inner.js"), false),
                ("cycle-a", Some("./cycle-b.js"), false),
                ("cycle-b", Some("./cycle-a.js"), false),
                ("to-consumer", Some("./consumer.js"), false),
                ("fs-pass", Some("fs"), false),
                ("builtin-pass", Some("node:crypto"), false),
                ("missing-pass", Some("./missing.js"), false),
            ],
        );
        let sources = plan.passthrough_sources("src/consumer.js");
        let mut listed: Vec<(&str, &str)> = sources
            .iter()
            .map(|(source, target)| (source.as_str(), target.as_str()))
            .collect();
        listed.sort_unstable();
        assert_eq!(
            listed,
            [
                ("./builtin-pass.js", "node:crypto"),
                ("./fs-pass.js", "fs"),
                ("./outer", "./deep/chained.js"),
                ("./pass.js", "./named.js"),
            ]
        );
        // A passthrough's own require is not retargeted.
        assert!(plan.passthrough_sources("src/outer.js").is_empty());
        assert!(plan.passthrough_sources("src/deep/inner.js").is_empty());
    }

    #[test]
    fn whole_value_and_helper_sources_name_the_rewritten_specifier() {
        let helpers = provider_helpers(
            "module.exports = function (e) { return e && e.__esModule ? e : { default: e }; };",
        );
        let consumer = edges(
            r#"var h = require("./helper-pass.js");
var w = require("./whole-pass.js").size;"#,
        );
        let pass = |source: &str| edges(&format!(r#"module.exports = require("{source}");"#));
        let (helper_pass, whole_pass) = (pass("./helper.js"), pass("./whole.js"));
        let empty = RequireEdges::default();
        let module = |filename, edges, helpers, whole, passthrough: Option<&str>| PlanModule {
            filename,
            edges,
            exports_object_writable: true,
            helpers,
            exports_whole_value: whole,
            passthrough: passthrough.map(Atom::from),
            exports_from_requires: passthrough
                .iter()
                .map(|source| Atom::from(*source))
                .collect(),
        };
        let plan = InterfacePlan::build([
            module("src/consumer.js", &consumer, None, false, None),
            module("src/helper.js", &empty, Some(&helpers), false, None),
            module(
                "src/helper-pass.js",
                &helper_pass,
                None,
                false,
                Some("./helper.js"),
            ),
            module("src/whole.js", &empty, None, true, None),
            module(
                "src/whole-pass.js",
                &whole_pass,
                None,
                false,
                Some("./whole.js"),
            ),
        ]);
        assert_eq!(
            plan.helper_sources("src/consumer.js")
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["./helper.js"]
        );
        assert_eq!(
            plan.helper_kind("src/consumer.js", "./helper.js", &CalleeExport::Value),
            Some(TranspilerHelperKind::InteropRequireDefault)
        );
        assert_eq!(
            plan.whole_value_sources("src/consumer.js"),
            HashSet::from_iter(["./whole.js".to_string()])
        );
    }

    #[test]
    fn rewriting_a_passthrough_require_keeps_the_call_and_runs_once() {
        let sources = HashMap::from_iter([("./pass.js".to_string(), "./named.js".to_string())]);
        let printed = GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let file = cm.new_source_file(
                FileName::Anon.into(),
                r#"var a = require("./pass.js");
function later() { return require("./pass.js").f; }
require.e(1).then(() => require("./pass.js"));
var b = other("./pass.js");"#
                    .to_string(),
            );
            let mut module = parse_file_as_module(
                &file,
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                None,
                &mut Vec::new(),
            )
            .expect("test source parses");
            let mark = Mark::new();
            module.visit_mut_with(&mut resolver(mark, Mark::new(), false));
            let spans = |module: &Module| {
                RequireEdges::collect(module, mark)
                    .iter()
                    .map(|edge| edge.call)
                    .collect::<Vec<_>>()
            };
            let before = spans(&module);
            assert!(rewrite_passthrough_requires(&mut module, mark, &sources));
            assert_eq!(spans(&module), before);
            assert!(!rewrite_passthrough_requires(&mut module, mark, &sources));
            crate::unpacker::emit_esm::emit_module_raw(&module, cm).expect("prints")
        });
        assert_eq!(
            printed.matches(r#"require("./named.js")"#).count(),
            3,
            "{printed}"
        );
        assert!(printed.contains(r#"other("./pass.js")"#), "{printed}");
    }

    #[test]
    fn a_source_outside_the_input_is_not_planned() {
        let plan = plan(r#"require("lodash").f = 1;"#, &[], &[]);
        assert!(plan.written.is_empty());
    }
}
