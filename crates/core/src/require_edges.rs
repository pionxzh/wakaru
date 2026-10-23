//! Per-`require()` edge records: how one module requires another.
//!
//! An edge describes one `require("src")` call of a consumer: the source, the
//! form the call is written in, the interop around it, and how the required
//! value is used. It is keyed by the call's span, which survives the rule
//! range; a binding does not (`UnWebpackInterop` and `UnEsm` rename, and
//! `SmartRename` runs later).
//!
//! Edges are collected in two passes, and both observe an AST that exists at
//! that point:
//!
//! - [`RequireEdges::collect`] runs on the resolved input, before any rule.
//!   Rules before `UnEsm` erase the interop (`UnWebpackInterop` removes
//!   webpack's `.n` getter, `UnIndirectCall` removes `(0, x.f)()`), so on the
//!   AST `UnEsm` sees a default-interop read and a whole-value use look the
//!   same.
//! - [`RequireEdges::add_uses`] runs on the AST `UnEsm` sees, where the
//!   extractor has turned bundler loads into `require("./p")` and
//!   `SimplifySequence` has split comma sequences.
//!
//! `UnEsm` records which import specifier it made from which calls
//! ([`ImportTags`]); the passes after the fact barrier read that table to
//! tell an import synthesized from a require from one the input wrote.
//!
//! A call whose span is dummy, or shared with another call, cannot be found
//! again by span: it keeps what the first pass read, which the per-source
//! queries count, but has no key, so it gets no uses and no import tag. A
//! detector that builds its module AST itself (a prepared AST) gives a
//! `require()` it translates the span of the input call it replaces; calls
//! with no input call behind them (Turbopack's synthesized facade and loader
//! modules) stay dummy, and the webpack extractors inline
//! `require.n(require(x))` by cloning one call, so the clones share its
//! span.
//!

use crate::collections::{HashMap, HashSet};
use std::fmt;

use swc_core::atoms::Atom;
use swc_core::common::{BytePos, Mark, Span};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignTarget, BinExpr, BinaryOp, CallExpr, Callee, Class, Decl, Expr,
    ExprStmt, Function, ImportSpecifier, Lit, MemberExpr, MemberProp, Module, ModuleDecl,
    ModuleItem, NewExpr, ObjectPatProp, OptCall, OptChainBase, OptChainExpr, Pat, Prop, PropName,
    SimpleAssignTarget, Stmt, TaggedTpl, UnaryExpr, UnaryOp, UpdateExpr, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::analysis::{binding_id, BindingId};
use crate::rules::expr_utils::is_unresolved_ident;
use crate::rules::transpiler_helper_utils::{
    classify_inline_callable, top_level_required_bindings, LocalHelperContext, TranspilerHelperKind,
};
use crate::utils::paren::strip_parens;

/// The key of an edge: the `require()` call's span.
pub type EdgeKey = (BytePos, BytePos);

fn key_of(span: Span) -> EdgeKey {
    (span.lo, span.hi)
}

/// A call's identity within one immutable walk of the first pass, which
/// sees calls a span cannot tell apart.
type CallId = *const CallExpr;

fn call_id(call: &CallExpr) -> CallId {
    call
}

/// The interop a consumer applied to a required value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interop {
    None,
    /// webpack's `require.n`, or a default interop helper
    /// (`interopRequireDefault`, `__importDefault`).
    Default,
    /// A wildcard interop helper (`interopRequireWildcard`, `__importStar`).
    Wildcard,
}

/// How a `require()` call is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequireForm {
    /// `var x = require("src")`, one identifier declarator.
    Declarator { binding: Atom },
    /// `var { a, b: c } = require("src")`; the property names read.
    Destructure { names: Vec<Atom> },
    /// `require("src").name`; `None` for a computed member.
    Member { name: Option<Atom> },
    /// `var x = helper(require("src"))` with an interop helper, declared in
    /// the module or inlined at the call site (`inline`).
    HelperArg {
        helper: Interop,
        inline: bool,
        binding: Atom,
    },
    /// `require("src");` as a statement.
    Bare,
    /// Any other position.
    Inline,
}

/// How a required value is used, read on the AST `UnEsm` sees. Uses through
/// the binding a declarator gives the call count as uses of the call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequireUses {
    /// Names read as static members (`x.name`, `x?.name`), in first-use
    /// order. A receiver-sensitive call is listed in `member_calls` instead.
    pub member_reads: Vec<Atom>,
    /// A computed member read (`x[key]`).
    pub computed_read: bool,
    /// Names called as members (`x.name()`, `x.name?.()`, a tag
    /// `` x.name`…` ``): the call passes the value as `this`.
    pub member_calls: Vec<Atom>,
    /// The value is called or constructed (`x()`, `new x()`).
    pub called: bool,
    /// Names written as static members: an assignment (chained or
    /// compound), an update, or a `delete`.
    pub member_writes: Vec<Atom>,
    /// A computed member written.
    pub computed_write: bool,
    /// The value is the target of `Object.assign`, `Object.defineProperty`,
    /// `Object.defineProperties`, or `Object.setPrototypeOf`.
    pub object_api_write: bool,
    /// The value is an `Object.assign` source after the target.
    pub assign_source: bool,
    /// The value is passed to an interop helper or `require.n`.
    pub interop_arg: bool,
    /// The binding is assigned again.
    pub binding_write: bool,
    /// The value is used whole anywhere else: passed to a call, stored,
    /// returned, or aliased.
    pub escapes: bool,
    /// The value is passed to a function of this module that writes the
    /// parameter it arrives in ([`ArgumentCallee`]).
    pub callee_write: bool,
    /// A write names `default` or `__esModule`: a member write, a literal
    /// key of an `Object` or `Reflect` method that writes the value, or such
    /// a write by a function of this module to its parameter. A computed
    /// key does not count.
    pub interop_key_write: bool,
    /// The value is passed to a callee required from another module, in
    /// first-use order.
    pub passed_to_other_module: Vec<ModuleCallee>,
    /// The value is passed to a callee whose effect on it is unknown.
    pub passed_to_unknown: bool,
}

impl RequireUses {
    /// Whether the consumer writes to the required value: directly, or
    /// through a function of the module it passes the value to.
    pub fn writes(&self) -> bool {
        !self.member_writes.is_empty()
            || self.computed_write
            || self.object_api_write
            || self.callee_write
    }

    /// Whether the value reaches a callee that could write to it unseen:
    /// one in another module, or one nothing is known about.
    pub fn passed_to_unseen_callee(&self) -> bool {
        self.passed_to_unknown || !self.passed_to_other_module.is_empty()
    }
}

/// A callee required from another module, which a required value is
/// passed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleCallee {
    /// The source the callee is required from.
    pub source: Atom,
    /// Which part of that source's required value the callee is.
    pub export: CalleeExport,
}

/// The part of a required value a callee is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalleeExport {
    /// The value itself: `h(x)` with `var h = require("src")`, or
    /// `require("src")(x)`.
    Value,
    /// One static member of it: `ns.name(x)`, `require("src").name(x)`,
    /// or `h(x)` with `var h = require("src").name`.
    Named(Atom),
    /// A computed member, or a member of a member.
    Other,
}

/// What a callee does with a required value passed to it as an argument.
///
/// A same-module function counts as a writer when the call reaches it
/// directly (a function literal, or a binding declared once and never
/// written; TypeScript's guarded helper `this && this.__name || F` reaches
/// its fallbacks `F`), it does not read `arguments`, and its body writes the
/// parameter: a member assignment, update, or `delete`, or the target of
/// `Object.assign`, `Object.defineProperty`, `Object.defineProperties`, or
/// `Object.setPrototypeOf` (the conditions `ArrowFunction` uses for a
/// callee's parameters). Only that one level is followed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ArgumentCallee {
    /// An interop or re-export helper, a builtin that only reads, or a
    /// same-module function that does not write the parameter.
    Reads,
    /// A same-module function that writes the parameter; `interop_key`:
    /// a write names `default` or `__esModule`.
    Writes {
        interop_key: bool,
    },
    /// A same-module function binding, decided once the module is read.
    Local(BindingId),
    /// A callee required from another module.
    OtherModule(ModuleCallee),
    Unknown,
}

/// One `require("src")` call of a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequireEdge {
    /// The call's span, the edge key.
    pub call: Span,
    pub source: Atom,
    pub form: RequireForm,
    /// The call sits in a statement of the module body (not an `import` or
    /// `export` declaration), outside any nested statement, function, or
    /// class.
    pub top_level: bool,
    /// The interop applied to the call, or to the binding of a top-level
    /// declarator holding it.
    pub interop: Interop,
    /// `__esModule` is read on the call or on that binding.
    pub esmodule_tested: bool,
    /// Uses on the AST `UnEsm` sees; `None` when that AST does not hold the
    /// call exactly once.
    pub uses: Option<RequireUses>,
    /// The source the input wrote, when it names a CommonJS passthrough and
    /// `source` is the module the passthrough chain ends at
    /// ([`RequireEdges::alias_sources`]).
    pub via: Option<Atom>,
}

impl RequireEdge {
    pub fn key(&self) -> EdgeKey {
        key_of(self.call)
    }

    /// The call is a top-level `var x = require("src")`.
    fn is_top_level_declarator(&self) -> bool {
        self.top_level && matches!(self.form, RequireForm::Declarator { .. })
    }

    /// The call is a top-level `var x = wildcard(require("src"))`.
    fn is_top_level_wildcard_arg(&self) -> bool {
        self.top_level
            && matches!(
                self.form,
                RequireForm::HelperArg {
                    helper: Interop::Wildcard,
                    ..
                }
            )
    }

    fn reads_through_interop(&self) -> bool {
        self.interop != Interop::None || self.esmodule_tested
    }
}

/// The `require()` edges of one module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequireEdges {
    edges: Vec<RequireEdge>,
    /// Calls with no key: a dummy span, or a span shared with another call.
    /// They count in the per-source queries, but have no uses and cannot be
    /// looked up.
    unkeyed: Vec<RequireEdge>,
}

impl RequireEdges {
    /// Collects the edges of `module`, the resolved input before any rule:
    /// each call's form, top-level position, and interop.
    pub(crate) fn collect(module: &Module, unresolved_mark: Mark) -> Self {
        let local_helpers = LocalHelperContext::collect_with_mark(module, unresolved_mark);
        let forms = FormCollector::collect(module, unresolved_mark, &local_helpers);
        let mut collector = EdgeCollector {
            unresolved_mark,
            forms: &forms,
            edges: Vec::new(),
            stmt_depth: 0,
            fn_depth: 0,
            module_decl_depth: 0,
        };
        module.visit_with(&mut collector);
        let (calls, mut edges): (Vec<CallId>, Vec<RequireEdge>) =
            collector.edges.into_iter().unzip();

        // The interop reaches a top-level declarator's call through its
        // binding. A binding declared twice names the last declarator.
        let index_of: HashMap<CallId, usize> = calls
            .iter()
            .enumerate()
            .map(|(index, call)| (*call, index))
            .collect();
        let mut bindings: HashMap<BindingId, usize> = HashMap::default();
        for (binding, call) in &forms.top_level_bindings {
            if let Some(index) = index_of.get(call) {
                bindings.insert(binding.clone(), *index);
            }
        }
        let mut marks = InteropMarks {
            unresolved_mark,
            local_helpers: &local_helpers,
            bindings: &bindings,
            calls: &index_of,
            interop: Vec::new(),
            esmodule_tested: Vec::new(),
        };
        module.visit_with(&mut marks);
        for (index, interop) in marks.interop {
            // The first interop seen wins; a declarator's own helper is it.
            if edges[index].interop == Interop::None {
                edges[index].interop = interop;
            }
        }
        for index in marks.esmodule_tested {
            edges[index].esmodule_tested = true;
        }

        // A span shared by several calls cannot be told apart later, as a
        // dummy one cannot: such calls keep what this pass read, but no key.
        let mut counts: HashMap<EdgeKey, usize> = HashMap::default();
        for edge in &edges {
            *counts.entry(edge.key()).or_default() += 1;
        }
        let (edges, unkeyed) = edges
            .into_iter()
            .partition(|edge| !edge.call.is_dummy() && counts[&edge.key()] == 1);
        Self { edges, unkeyed }
    }

    /// Adds each edge's uses, read on `module` as it reaches `UnEsm`.
    pub(crate) fn add_uses(
        &mut self,
        module: &Module,
        unresolved_mark: Mark,
        local_helpers: &LocalHelperContext,
    ) {
        if self.edges.is_empty() {
            return;
        }
        let keys: HashSet<EdgeKey> = self.edges.iter().map(RequireEdge::key).collect();
        let mut sites = CallSites {
            unresolved_mark,
            keys: &keys,
            local_helpers,
            seen: HashMap::default(),
            bindings: HashMap::default(),
        };
        module.visit_with(&mut sites);
        let mut exports_values = ExportsValues {
            unresolved_mark,
            keys: &keys,
            found: Vec::new(),
        };
        module.visit_with(&mut exports_values);
        let sources: HashMap<EdgeKey, Atom> = self
            .edges
            .iter()
            .map(|edge| (edge.key(), edge.source.clone()))
            .collect();
        let mut uses = UseCollector {
            unresolved_mark,
            keys: &keys,
            local_helpers,
            bindings: &sites.bindings,
            exports_values: &exports_values.found,
            sources: &sources,
            uses: HashMap::default(),
            local_callees: Vec::new(),
            member_callees: member_callees(module, unresolved_mark),
            this_depth: 0,
        };
        module.visit_with(&mut uses);
        if !uses.local_callees.is_empty() {
            let writers = LocalWriters::collect(module, &uses.local_callees);
            for (key, callee, index) in std::mem::take(&mut uses.local_callees) {
                let entry = uses.uses.entry(key).or_default();
                match writers.writes(&callee, index) {
                    Some(write) if write.writes => {
                        entry.callee_write = true;
                        entry.interop_key_write |= write.interop_key;
                    }
                    Some(_) => {}
                    None => entry.passed_to_unknown = true,
                }
            }
        }
        for edge in &mut self.edges {
            let key = edge.key();
            edge.uses = (sites.seen.get(&key) == Some(&1))
                .then(|| uses.uses.remove(&key).unwrap_or_default());
        }
    }

    /// The keyed edges.
    pub fn iter(&self) -> impl Iterator<Item = &RequireEdge> {
        self.edges.iter()
    }

    /// The calls with no key ([`Self::get`] cannot find them).
    pub fn unkeyed(&self) -> &[RequireEdge] {
        &self.unkeyed
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty() && self.unkeyed.is_empty()
    }

    pub fn get(&self, key: EdgeKey) -> Option<&RequireEdge> {
        self.edges.iter().find(|edge| edge.key() == key)
    }

    /// Every call of `source`, keyed or not; `None` when it has none.
    fn of_source<'a>(&'a self, source: &'a str) -> Option<impl Iterator<Item = &'a RequireEdge>> {
        let mut all = self
            .edges
            .iter()
            .chain(&self.unkeyed)
            .filter(move |edge| edge.source.as_ref() == source)
            .peekable();
        all.peek().is_some().then_some(all)
    }

    /// Reads each edge of a source `sources` maps, and each callee required
    /// from one, as an edge of the specifier it maps to: the module a
    /// CommonJS passthrough chain ends at, which `UnEsm` read instead
    /// (`InterfacePlan::passthrough_sources`). The edge keeps the source the
    /// input wrote in `via`.
    pub(crate) fn alias_sources(&mut self, sources: &HashMap<String, String>) {
        if sources.is_empty() {
            return;
        }
        for edge in self.edges.iter_mut().chain(self.unkeyed.iter_mut()) {
            if let Some(target) = sources.get(edge.source.as_ref()) {
                edge.via = Some(std::mem::replace(&mut edge.source, target.as_str().into()));
            }
            for callee in edge
                .uses
                .iter_mut()
                .flat_map(|uses| uses.passed_to_other_module.iter_mut())
            {
                if let Some(target) = sources.get(callee.source.as_ref()) {
                    callee.source = target.as_str().into();
                }
            }
        }
    }

    /// Whether the module uses `source` as the plain required value: every
    /// call is a top-level `var x = require("src")`, and no call or binding
    /// of the source meets an interop or an `__esModule` read. The helper
    /// stage and `UnWebpackInterop` later replace an interop default with
    /// the plain binding, and `UnEsm` merges every binding of one source
    /// into one default import, so after the rule range the two meanings
    /// can no longer be told apart.
    pub fn uses_whole_value(&self, source: &str) -> bool {
        self.of_source(source).is_some_and(|mut edges| {
            edges.all(|edge| edge.is_top_level_declarator() && !edge.reads_through_interop())
        })
    }

    /// Whether the module uses `source` as the plain required value
    /// ([`Self::uses_whole_value`]) and reads nothing of it but its static
    /// `default` member: no other member, call, write, or escape, on the AST
    /// `UnEsm` sees.
    pub fn reads_only_default(&self, source: &str) -> bool {
        let only_default = RequireUses {
            member_reads: vec!["default".into()],
            ..RequireUses::default()
        };
        self.uses_whole_value(source)
            && self.of_source(source).is_some_and(|mut edges| {
                edges.all(|edge| edge.uses.as_ref() == Some(&only_default))
            })
    }

    /// Whether the module requires `source` only through a wildcard interop
    /// helper: every call is a top-level `var x = wildcard(require("src"))`,
    /// the helper declared or inlined at the call site. `UnEsm` unwraps the
    /// helper, so the binding reaches the provider repair as a default
    /// import; for a provider marked `__esModule` the helper returned the
    /// required value unchanged.
    pub fn uses_only_wildcard_interop(&self, source: &str) -> bool {
        self.of_source(source)
            .is_some_and(|mut edges| edges.all(RequireEdge::is_top_level_wildcard_arg))
    }

    /// Whether the module passes the value of `source` to `require.n` or an
    /// interop helper declared in the module, or tests it for `__esModule`,
    /// directly or through a top-level `var x = require("src")` binding.
    pub fn reads_through_interop(&self, source: &str) -> bool {
        self.of_source(source)
            .is_some_and(|mut edges| edges.any(RequireEdge::reads_through_interop))
    }
}

/// The import specifiers made from `require()` calls: each local binding,
/// after `UnEsm`'s own collision renames, and the keys of the calls it came
/// from. A pass after the fact barrier reads it to tell an import
/// synthesized from a require from one the input wrote, and a pass that
/// rebuilds such a local carries the entry over. A call without a key (a
/// dummy span) still tags its local, with no key.
///
/// It also lists the specifiers the input wrote that `UnEsm` merged into an
/// import it made from a require of the same source. The barrier passes
/// treat those as synthesized, as they did when they read the import's
/// dummy span: the provider they import is converted from CommonJS as well.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportTags {
    locals: HashMap<BindingId, Vec<EdgeKey>>,
    merged: HashSet<BindingId>,
}

impl ImportTags {
    /// Records that the import bound to `local` was made from the
    /// `require()` call at `call`.
    pub(crate) fn insert(&mut self, local: BindingId, call: Span) {
        let keys = self.locals.entry(local).or_default();
        let key = key_of(call);
        if !call.is_dummy() && !keys.contains(&key) {
            keys.push(key);
        }
    }

    /// Records that `local` was made from the same calls as `from`.
    pub(crate) fn carry(&mut self, from: &BindingId, local: BindingId) {
        let Some(keys) = self.locals.get(from).cloned() else {
            return;
        };
        let target = self.locals.entry(local).or_default();
        for key in keys {
            if !target.contains(&key) {
                target.push(key);
            }
        }
    }

    /// Records that the input wrote the specifier bound to `local`, and
    /// `UnEsm` merged it into an import it made from a require.
    pub(crate) fn insert_merged(&mut self, local: BindingId) {
        self.merged.insert(local);
    }

    /// Whether `local` is the binding of an import specifier made from a
    /// `require()` call.
    pub(crate) fn is_tagged(&self, local: &BindingId) -> bool {
        self.locals.contains_key(local)
    }

    /// Whether the barrier passes treat the specifier bound to `local` as
    /// one `UnEsm` synthesized: made from a require, or merged into such an
    /// import.
    pub(crate) fn is_synthesized(&self, local: &BindingId) -> bool {
        self.is_tagged(local) || self.merged.contains(local)
    }

    /// The keys of the calls the specifier bound to `local` came from.
    pub fn edges_of(&self, local: &BindingId) -> &[EdgeKey] {
        self.locals.get(local).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&BindingId) -> bool) {
        self.locals.retain(|local, _| keep(local));
        self.merged.retain(|local| keep(local));
    }

    pub fn is_empty(&self) -> bool {
        self.locals.is_empty() && self.merged.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&BindingId, &[EdgeKey])> {
        self.locals
            .iter()
            .map(|(local, keys)| (local, keys.as_slice()))
    }
}

/// The `require()` call each top-level declarator binding is initialized
/// from, directly or as the first argument of a call
/// (`var x = helper(require("src"))`): what a pass that turns such a
/// declarator into an import made the import from.
pub(crate) fn declarator_require_calls(
    module: &Module,
    unresolved_mark: Mark,
) -> HashMap<BindingId, Span> {
    let mut calls = HashMap::default();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for declarator in &var.decls {
            let (Pat::Ident(binding), Some(init)) = (&declarator.name, &declarator.init) else {
                continue;
            };
            let call = as_require_call(init, unresolved_mark).or_else(|| {
                let Expr::Call(wrapper) = strip_parens(init) else {
                    return None;
                };
                let first = wrapper.args.first().filter(|arg| arg.spread.is_none())?;
                as_require_call(&first.expr, unresolved_mark)
            });
            if let Some(call) = call {
                calls.insert(binding_id(&binding.id), call.span);
            }
        }
    }
    calls
}

/// Tags each specifier of a synthesized (dummy-span) import whose local was
/// a declarator binding of `declarator_requires`, collected before a pass
/// that turns such declarators into imports directly.
pub(crate) fn tag_declarator_imports(
    module: &Module,
    declarator_requires: &HashMap<BindingId, Span>,
    tags: &mut ImportTags,
) {
    if declarator_requires.is_empty() {
        return;
    }
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        if !import.span.is_dummy() {
            continue;
        }
        for specifier in &import.specifiers {
            let local = binding_id(match specifier {
                ImportSpecifier::Default(default) => &default.local,
                ImportSpecifier::Named(named) => &named.local,
                ImportSpecifier::Namespace(namespace) => &namespace.local,
            });
            if !tags.is_tagged(&local) {
                if let Some(call) = declarator_requires.get(&local) {
                    tags.insert(local, *call);
                }
            }
        }
    }
}

impl fmt::Display for Interop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::None => "none",
            Self::Default => "default",
            Self::Wildcard => "wildcard",
        })
    }
}

impl fmt::Display for RequireForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Declarator { binding } => write!(f, "declarator `{binding}`"),
            Self::Destructure { names } => write!(f, "destructure {{{}}}", join(names)),
            Self::Member { name: Some(name) } => write!(f, "member `.{name}`"),
            Self::Member { name: None } => write!(f, "computed member"),
            Self::HelperArg {
                helper,
                inline,
                binding,
            } => write!(
                f,
                "{helper} helper argument{} `{binding}`",
                if *inline { " (inlined helper)" } else { "" }
            ),
            Self::Bare => write!(f, "bare"),
            Self::Inline => write!(f, "inline"),
        }
    }
}

impl fmt::Display for RequireUses {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if !self.member_reads.is_empty() {
            parts.push(format!("reads {}", join(&self.member_reads)));
        }
        if self.computed_read {
            parts.push("computed read".to_string());
        }
        if !self.member_calls.is_empty() {
            parts.push(format!("member calls {}", join(&self.member_calls)));
        }
        if self.called {
            parts.push("called".to_string());
        }
        if !self.member_writes.is_empty() {
            parts.push(format!("writes {}", join(&self.member_writes)));
        }
        if self.computed_write {
            parts.push("computed write".to_string());
        }
        if self.object_api_write {
            parts.push("Object.* target".to_string());
        }
        if self.assign_source {
            parts.push("Object.assign source".to_string());
        }
        if self.interop_arg {
            parts.push("interop argument".to_string());
        }
        if self.binding_write {
            parts.push("binding reassigned".to_string());
        }
        if self.escapes {
            parts.push("escapes".to_string());
        }
        if self.callee_write {
            parts.push("written by a callee".to_string());
        }
        if self.interop_key_write && !self.member_writes.iter().any(|name| is_interop_key(name)) {
            parts.push("writes default or __esModule".to_string());
        }
        if !self.passed_to_other_module.is_empty() {
            let mut sources: Vec<Atom> = Vec::new();
            for callee in &self.passed_to_other_module {
                if !sources.contains(&callee.source) {
                    sources.push(callee.source.clone());
                }
            }
            parts.push(format!("passed to a callee from {}", join(&sources)));
        }
        if self.passed_to_unknown {
            parts.push("passed to an unknown callee".to_string());
        }
        if parts.is_empty() {
            f.write_str("unused")
        } else {
            f.write_str(&parts.join(", "))
        }
    }
}

fn join(names: &[Atom]) -> String {
    names
        .iter()
        .map(Atom::as_ref)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The source of `require("src")`: the free `require` with one string
/// literal argument.
pub(crate) fn require_call_source(call: &CallExpr, unresolved_mark: Mark) -> Option<Atom> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let [arg] = call.args.as_slice() else {
        return None;
    };
    if arg.spread.is_some()
        || !matches!(strip_parens(callee), Expr::Ident(id)
            if is_unresolved_ident(id, "require", unresolved_mark))
    {
        return None;
    }
    let Expr::Lit(Lit::Str(source)) = arg.expr.as_ref() else {
        return None;
    };
    source.value.as_str().map(Atom::from)
}

/// `expr` is a `require("src")` call, parentheses aside.
fn as_require_call(expr: &Expr, unresolved_mark: Mark) -> Option<&CallExpr> {
    match strip_parens(expr) {
        Expr::Call(call) if require_call_source(call, unresolved_mark).is_some() => Some(call),
        _ => None,
    }
}

/// `require.n`.
fn is_require_n(callee: &Expr, unresolved_mark: Mark) -> bool {
    matches!(strip_parens(callee), Expr::Member(member)
        if matches!(member.obj.as_ref(), Expr::Ident(id)
            if is_unresolved_ident(id, "require", unresolved_mark))
            && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "n"))
}

/// The interop a callee declared in the module (or `require.n`) applies.
fn declared_interop(callee: &Expr, local_helpers: &LocalHelperContext, mark: Mark) -> Interop {
    if is_require_n(callee, mark)
        || local_helpers.is_helper_callee(callee, TranspilerHelperKind::InteropRequireDefault)
    {
        Interop::Default
    } else if local_helpers.is_helper_callee(callee, TranspilerHelperKind::InteropRequireWildcard) {
        Interop::Wildcard
    } else {
        Interop::None
    }
}

fn static_member_name(prop: &MemberProp) -> Option<Atom> {
    match prop {
        MemberProp::Ident(ident) => Some(ident.sym.clone()),
        MemberProp::Computed(computed) => match computed.expr.as_ref() {
            Expr::Lit(Lit::Str(name)) => name.value.as_str().map(Atom::from),
            _ => None,
        },
        MemberProp::PrivateName(_) => None,
    }
}

/// The form of each call that is written in a recognized position, and the
/// top-level declarator bindings that hold a call.
#[derive(Default)]
struct FormCollector {
    forms: HashMap<CallId, RequireForm>,
    top_level_bindings: Vec<(BindingId, CallId)>,
}

impl FormCollector {
    fn collect(module: &Module, unresolved_mark: Mark, local_helpers: &LocalHelperContext) -> Self {
        let mut collector = FormVisitor {
            unresolved_mark,
            local_helpers,
            out: FormCollector::default(),
        };
        module.visit_with(&mut collector);
        for item in &module.body {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
                continue;
            };
            for declarator in &var.decls {
                let (Pat::Ident(binding), Some(init)) = (&declarator.name, &declarator.init) else {
                    continue;
                };
                if let Some(call) = as_require_call(init, unresolved_mark) {
                    collector
                        .out
                        .top_level_bindings
                        .push((binding_id(&binding.id), call_id(call)));
                }
            }
        }
        collector.out
    }
}

struct FormVisitor<'a> {
    unresolved_mark: Mark,
    local_helpers: &'a LocalHelperContext,
    out: FormCollector,
}

impl FormVisitor<'_> {
    fn record(&mut self, call: &CallExpr, form: RequireForm) {
        self.out.forms.entry(call_id(call)).or_insert(form);
    }
}

impl Visit for FormVisitor<'_> {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let Some(init) = &declarator.init {
            match &declarator.name {
                Pat::Ident(binding) => {
                    if let Some(call) = as_require_call(init, self.unresolved_mark) {
                        self.record(
                            call,
                            RequireForm::Declarator {
                                binding: binding.id.sym.clone(),
                            },
                        );
                    } else if let Expr::Call(wrapper) = strip_parens(init) {
                        if let (Callee::Expr(callee), [arg]) =
                            (&wrapper.callee, wrapper.args.as_slice())
                        {
                            if let (None, Some(call)) =
                                (arg.spread, as_require_call(&arg.expr, self.unresolved_mark))
                            {
                                let declared = declared_interop(
                                    callee,
                                    self.local_helpers,
                                    self.unresolved_mark,
                                );
                                let inline = match classify_inline_callable(strip_parens(callee)) {
                                    Some(TranspilerHelperKind::InteropRequireDefault) => {
                                        Interop::Default
                                    }
                                    Some(TranspilerHelperKind::InteropRequireWildcard) => {
                                        Interop::Wildcard
                                    }
                                    _ => Interop::None,
                                };
                                let (helper, inline) = if declared != Interop::None {
                                    (declared, false)
                                } else {
                                    (inline, true)
                                };
                                if helper != Interop::None {
                                    self.record(
                                        call,
                                        RequireForm::HelperArg {
                                            helper,
                                            inline,
                                            binding: binding.id.sym.clone(),
                                        },
                                    );
                                }
                            }
                        }
                    }
                }
                Pat::Object(object) => {
                    if let Some(call) = as_require_call(init, self.unresolved_mark) {
                        let names = object
                            .props
                            .iter()
                            .filter_map(|prop| match prop {
                                ObjectPatProp::KeyValue(kv) => match &kv.key {
                                    PropName::Ident(name) => Some(name.sym.clone()),
                                    PropName::Str(name) => name.value.as_str().map(Atom::from),
                                    _ => None,
                                },
                                ObjectPatProp::Assign(assign) => Some(assign.key.sym.clone()),
                                ObjectPatProp::Rest(_) => None,
                            })
                            .collect();
                        self.record(call, RequireForm::Destructure { names });
                    }
                }
                _ => {}
            }
        }
        declarator.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if let Some(call) = as_require_call(&member.obj, self.unresolved_mark) {
            self.record(
                call,
                RequireForm::Member {
                    name: static_member_name(&member.prop),
                },
            );
        }
        member.visit_children_with(self);
    }

    fn visit_expr_stmt(&mut self, stmt: &ExprStmt) {
        if let Some(call) = as_require_call(&stmt.expr, self.unresolved_mark) {
            self.record(call, RequireForm::Bare);
        }
        stmt.visit_children_with(self);
    }
}

/// Creates one edge per `require()` call, with its form and position.
struct EdgeCollector<'a> {
    unresolved_mark: Mark,
    forms: &'a FormCollector,
    edges: Vec<(CallId, RequireEdge)>,
    stmt_depth: usize,
    fn_depth: usize,
    module_decl_depth: usize,
}

impl Visit for EdgeCollector<'_> {
    fn visit_module_decl(&mut self, decl: &ModuleDecl) {
        self.module_decl_depth += 1;
        decl.visit_children_with(self);
        self.module_decl_depth -= 1;
    }

    fn visit_stmt(&mut self, stmt: &Stmt) {
        self.stmt_depth += 1;
        stmt.visit_children_with(self);
        self.stmt_depth -= 1;
    }

    fn visit_function(&mut self, function: &Function) {
        self.fn_depth += 1;
        function.visit_children_with(self);
        self.fn_depth -= 1;
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.fn_depth += 1;
        arrow.visit_children_with(self);
        self.fn_depth -= 1;
    }

    fn visit_class(&mut self, class: &Class) {
        self.fn_depth += 1;
        class.visit_children_with(self);
        self.fn_depth -= 1;
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some(source) = require_call_source(call, self.unresolved_mark) {
            self.edges.push((
                call_id(call),
                RequireEdge {
                    call: call.span,
                    source,
                    form: self
                        .forms
                        .forms
                        .get(&call_id(call))
                        .cloned()
                        .unwrap_or(RequireForm::Inline),
                    top_level: self.fn_depth == 0
                        && self.stmt_depth <= 1
                        && self.module_decl_depth == 0,
                    interop: Interop::None,
                    esmodule_tested: false,
                    uses: None,
                    via: None,
                },
            ));
        }
        call.visit_children_with(self);
    }
}

/// Finds the interop helpers and `__esModule` reads that take a call, or
/// the binding of a top-level declarator holding one.
struct InteropMarks<'a> {
    unresolved_mark: Mark,
    local_helpers: &'a LocalHelperContext,
    bindings: &'a HashMap<BindingId, usize>,
    calls: &'a HashMap<CallId, usize>,
    interop: Vec<(usize, Interop)>,
    esmodule_tested: Vec<usize>,
}

impl InteropMarks<'_> {
    fn edge_of(&self, expr: &Expr) -> Option<usize> {
        match strip_parens(expr) {
            Expr::Ident(ident) => self.bindings.get(&binding_id(ident)).copied(),
            Expr::Call(call) if require_call_source(call, self.unresolved_mark).is_some() => {
                self.calls.get(&call_id(call)).copied()
            }
            _ => None,
        }
    }
}

impl Visit for InteropMarks<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            let interop = declared_interop(callee, self.local_helpers, self.unresolved_mark);
            if interop != Interop::None {
                for arg in &call.args {
                    if let Some(index) = self.edge_of(&arg.expr) {
                        self.interop.push((index, interop));
                    }
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "__esModule") {
            if let Some(index) = self.edge_of(&member.obj) {
                self.esmodule_tested.push(index);
            }
        }
        member.visit_children_with(self);
    }
}

/// An interop helper declared in the module or inlined at the call site,
/// or `require.n`.
fn is_interop_callee(callee: &Expr, local_helpers: &LocalHelperContext, mark: Mark) -> bool {
    declared_interop(callee, local_helpers, mark) != Interop::None
        || matches!(
            classify_inline_callable(strip_parens(callee)),
            Some(
                TranspilerHelperKind::InteropRequireDefault
                    | TranspilerHelperKind::InteropRequireWildcard
            )
        )
}

/// The key of `expr`, a keyed call or an interop helper around one: the
/// initializer that gives a declarator's binding the required value.
fn defining_key(
    expr: &Expr,
    keys: &HashSet<EdgeKey>,
    local_helpers: &LocalHelperContext,
    mark: Mark,
) -> Option<EdgeKey> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    if require_call_source(call, mark).is_some() {
        let key = key_of(call.span);
        return keys.contains(&key).then_some(key);
    }
    let (Callee::Expr(callee), [arg]) = (&call.callee, call.args.as_slice()) else {
        return None;
    };
    if arg.spread.is_some() || !is_interop_callee(callee, local_helpers, mark) {
        return None;
    }
    let inner = as_require_call(&arg.expr, mark)?;
    let key = key_of(inner.span);
    keys.contains(&key).then_some(key)
}

/// Counts each keyed call on the AST `UnEsm` sees, and finds the binding a
/// declarator gives it, directly or through an interop helper.
struct CallSites<'a> {
    unresolved_mark: Mark,
    keys: &'a HashSet<EdgeKey>,
    local_helpers: &'a LocalHelperContext,
    seen: HashMap<EdgeKey, usize>,
    bindings: HashMap<BindingId, EdgeKey>,
}

impl CallSites<'_> {
    fn defining_key(&self, expr: &Expr) -> Option<EdgeKey> {
        defining_key(expr, self.keys, self.local_helpers, self.unresolved_mark)
    }
}

impl Visit for CallSites<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        let key = key_of(call.span);
        if self.keys.contains(&key) && require_call_source(call, self.unresolved_mark).is_some() {
            *self.seen.entry(key).or_default() += 1;
        }
        call.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Pat::Ident(binding), Some(init)) = (&declarator.name, &declarator.init) {
            // `var t = module.exports = require("src")` binds the value too.
            let init = match strip_parens(init) {
                Expr::Assign(assign)
                    if assign.op == swc_core::ecma::ast::AssignOp::Assign
                        && matches!(&assign.left,
                            AssignTarget::Simple(SimpleAssignTarget::Member(member))
                                if is_module_exports(member, self.unresolved_mark)) =>
                {
                    &assign.right
                }
                init => init,
            };
            if let Some(key) = self.defining_key(init) {
                self.bindings.insert(binding_id(&binding.id), key);
            }
        }
        declarator.visit_children_with(self);
    }
}

/// The free `module.exports`.
fn is_module_exports(member: &MemberExpr, unresolved_mark: Mark) -> bool {
    matches!(strip_parens(&member.obj), Expr::Ident(object)
        if is_unresolved_ident(object, "module", unresolved_mark))
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "exports")
}

/// The keyed calls whose value may become the module's `module.exports`:
/// the right side of any `module.exports = …` assignment, or a branch of a
/// conditional, a logical operator, or the last element of a sequence
/// there. A write to `module.exports` may land on any of them.
struct ExportsValues<'a> {
    unresolved_mark: Mark,
    keys: &'a HashSet<EdgeKey>,
    found: Vec<EdgeKey>,
}

impl ExportsValues<'_> {
    fn add(&mut self, expr: &Expr) {
        match strip_parens(expr) {
            Expr::Call(call) if require_call_source(call, self.unresolved_mark).is_some() => {
                let key = key_of(call.span);
                if self.keys.contains(&key) && !self.found.contains(&key) {
                    self.found.push(key);
                }
            }
            Expr::Cond(cond) => {
                self.add(&cond.cons);
                self.add(&cond.alt);
            }
            Expr::Bin(BinExpr {
                op: BinaryOp::LogicalOr | BinaryOp::LogicalAnd | BinaryOp::NullishCoalescing,
                left,
                right,
                ..
            }) => {
                self.add(left);
                self.add(right);
            }
            Expr::Seq(seq) => {
                if let Some(last) = seq.exprs.last() {
                    self.add(last);
                }
            }
            // `module.exports = t = require("src")`
            Expr::Assign(assign) if assign.op == swc_core::ecma::ast::AssignOp::Assign => {
                self.add(&assign.right)
            }
            _ => {}
        }
    }
}

impl Visit for ExportsValues<'_> {
    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if assign.op == swc_core::ecma::ast::AssignOp::Assign
            && matches!(&assign.left, AssignTarget::Simple(SimpleAssignTarget::Member(member))
                if is_module_exports(member, self.unresolved_mark))
        {
            self.add(&assign.right);
        }
        assign.visit_children_with(self);
    }
}

/// Records how each keyed call's value is used, at the call and through
/// the binding a declarator gives it.
struct UseCollector<'a> {
    unresolved_mark: Mark,
    keys: &'a HashSet<EdgeKey>,
    local_helpers: &'a LocalHelperContext,
    bindings: &'a HashMap<BindingId, EdgeKey>,
    /// The keyed calls whose value may become `module.exports`
    /// ([`ExportsValues`]): a write to a member of `module.exports`, or to
    /// `module.exports` through an `Object` method, writes each of them.
    exports_values: &'a [EdgeKey],
    /// Each keyed call's source.
    sources: &'a HashMap<EdgeKey, Atom>,
    uses: HashMap<EdgeKey, RequireUses>,
    /// Values passed to a same-module function binding: the edge, the
    /// callee, and the argument position.
    local_callees: Vec<(EdgeKey, BindingId, usize)>,
    /// The top-level `var h = require("src").name` bindings no statement
    /// writes, and the export each one holds.
    member_callees: HashMap<BindingId, ModuleCallee>,
    /// The non-arrow functions and classes around the node being visited;
    /// at zero, `this` is the module's own.
    this_depth: usize,
}

impl UseCollector<'_> {
    /// The edge whose value `expr` is: a keyed call, or a binding of one.
    fn value_of(&self, expr: &Expr) -> Option<EdgeKey> {
        match strip_parens(expr) {
            Expr::Ident(ident) => self.bindings.get(&binding_id(ident)).copied(),
            Expr::Call(call) => {
                let key = key_of(call.span);
                (self.keys.contains(&key)
                    && require_call_source(call, self.unresolved_mark).is_some())
                .then_some(key)
            }
            _ => None,
        }
    }

    fn uses(&mut self, key: EdgeKey) -> &mut RequireUses {
        self.uses.entry(key).or_default()
    }

    /// A member read or call of an edge's value; `false` when `member` is
    /// on something else.
    fn member_use(&mut self, member: &MemberExpr, call: bool) -> bool {
        let Some(key) = self.value_of(&member.obj) else {
            return false;
        };
        match static_member_name(&member.prop) {
            Some(name) => {
                let uses = self.uses(key);
                let names = if call {
                    &mut uses.member_calls
                } else {
                    &mut uses.member_reads
                };
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            None => self.uses(key).computed_read = true,
        }
        if let MemberProp::Computed(computed) = &member.prop {
            computed.visit_with(self);
        }
        true
    }

    /// The edges whose value `expr` is or may be: a keyed call, a binding
    /// of one, or `module.exports` when a keyed call's value may have
    /// become it.
    fn written_values(&self, expr: &Expr) -> Vec<EdgeKey> {
        if let Some(key) = self.value_of(expr) {
            return vec![key];
        }
        match strip_parens(expr) {
            Expr::Member(member) if is_module_exports(member, self.unresolved_mark) => {
                self.exports_values.to_vec()
            }
            _ => Vec::new(),
        }
    }

    /// A write to a member of an edge's value, or of `module.exports` that
    /// may hold one; `false` when `member` is on something else.
    fn member_write(&mut self, member: &MemberExpr) -> bool {
        let keys = self.written_values(&member.obj);
        if keys.is_empty() {
            return false;
        }
        let name = static_member_name(&member.prop);
        for key in keys {
            let uses = self.uses(key);
            match &name {
                Some(name) => {
                    uses.interop_key_write |= is_interop_key(name);
                    if !uses.member_writes.contains(name) {
                        uses.member_writes.push(name.clone());
                    }
                }
                None => uses.computed_write = true,
            }
        }
        if let MemberProp::Computed(computed) = &member.prop {
            computed.visit_with(self);
        }
        true
    }

    fn binding_write(&mut self, expr: &Expr) -> bool {
        let Expr::Ident(ident) = strip_parens(expr) else {
            return false;
        };
        let Some(key) = self.bindings.get(&binding_id(ident)).copied() else {
            return false;
        };
        self.uses(key).binding_write = true;
        true
    }

    /// `Object.assign` and the `Object` methods that write their first
    /// argument.
    fn object_api(&self, callee: &Expr) -> Option<&'static str> {
        let Expr::Member(member) = strip_parens(callee) else {
            return None;
        };
        if !matches!(strip_parens(&member.obj), Expr::Ident(object)
            if is_unresolved_ident(object, "Object", self.unresolved_mark))
        {
            return None;
        }
        match member.prop.as_ident()?.sym.as_ref() {
            "assign" => Some("assign"),
            "defineProperty" => Some("defineProperty"),
            "defineProperties" => Some("defineProperties"),
            "setPrototypeOf" => Some("setPrototypeOf"),
            _ => None,
        }
    }
}

impl UseCollector<'_> {
    /// What `callee` does with the argument at `index`.
    fn argument_callee(
        &self,
        callee: &Expr,
        index: usize,
        args: &[swc_core::ecma::ast::ExprOrSpread],
    ) -> ArgumentCallee {
        let callee = match strip_parens(callee) {
            // `(0, f)(x)`
            Expr::Seq(seq) => seq.exprs.last().map_or(callee, |last| strip_parens(last)),
            other => other,
        };
        match callee {
            Expr::Fn(function) => {
                return local_function_writes(FunctionNode::Function(&function.function), index)
                    .map_or(ArgumentCallee::Unknown, ArgumentCallee::from_writes);
            }
            Expr::Arrow(arrow) => {
                return local_function_writes(FunctionNode::Arrow(arrow), index)
                    .map_or(ArgumentCallee::Unknown, ArgumentCallee::from_writes);
            }
            // TypeScript's helper inlined at the call site:
            // `(this && this.__importDefault || function (mod) { … })(x)`.
            Expr::Bin(_) if self.this_depth == 0 => {
                if let Some(functions) = this_guard_fallbacks(callee) {
                    return fallbacks_write(&functions, index)
                        .map_or(ArgumentCallee::Unknown, ArgumentCallee::from_writes);
                }
            }
            _ => {}
        }
        if is_read_only_object_prototype_call(callee, self.unresolved_mark) {
            return ArgumentCallee::Reads;
        }
        if let Some(name) = callee_name(callee) {
            if is_interop_helper_name(&name) {
                return ArgumentCallee::Reads;
            }
        }
        if let Expr::Member(member) = callee {
            if let Expr::Ident(object) = strip_parens(&member.obj) {
                if object.ctxt.outer() == self.unresolved_mark {
                    let method = static_member_name(&member.prop);
                    return builtin_argument(&object.sym, method.as_deref(), index, args);
                }
            }
        }
        // A callee reached through a value required from another module.
        let mut base = callee;
        let mut props = Vec::new();
        while let Expr::Member(member) = base {
            props.push(&member.prop);
            base = strip_parens(&member.obj);
        }
        let export = match props.as_slice() {
            [] => CalleeExport::Value,
            [prop] => static_member_name(prop).map_or(CalleeExport::Other, CalleeExport::Named),
            _ => CalleeExport::Other,
        };
        let other_module =
            |source: Atom| ArgumentCallee::OtherModule(ModuleCallee { source, export });
        match base {
            Expr::Call(call) => {
                if let Some(source) = require_call_source(call, self.unresolved_mark) {
                    return other_module(source);
                }
                ArgumentCallee::Unknown
            }
            Expr::Ident(ident) => {
                if let Some(key) = self.bindings.get(&binding_id(ident)) {
                    return self
                        .sources
                        .get(key)
                        .cloned()
                        .map_or(ArgumentCallee::Unknown, other_module);
                }
                // `h(x)` with `var h = require("src").name` calls that
                // export; a member of it, `h.x(x)`, is some other function.
                if let Some(held) = self.member_callees.get(&binding_id(ident)) {
                    return ArgumentCallee::OtherModule(ModuleCallee {
                        source: held.source.clone(),
                        export: if std::ptr::eq(base, callee) {
                            held.export.clone()
                        } else {
                            CalleeExport::Other
                        },
                    });
                }
                if std::ptr::eq(base, callee) && ident.ctxt.outer() != self.unresolved_mark {
                    return ArgumentCallee::Local(binding_id(ident));
                }
                ArgumentCallee::Unknown
            }
            _ => ArgumentCallee::Unknown,
        }
    }

    /// Records the arguments of a call or `new` that are a required value,
    /// and visits the others.
    fn arguments(&mut self, callee: &Expr, args: &[swc_core::ecma::ast::ExprOrSpread]) {
        for (index, arg) in args.iter().enumerate() {
            let key = arg
                .spread
                .is_none()
                .then(|| self.value_of(&arg.expr))
                .flatten();
            let Some(key) = key else {
                arg.visit_with(self);
                continue;
            };
            let callee = self.argument_callee(callee, index, args);
            let uses = self.uses(key);
            uses.escapes = true;
            match callee {
                ArgumentCallee::Reads => {}
                ArgumentCallee::Writes { interop_key } => {
                    uses.callee_write = true;
                    uses.interop_key_write |= interop_key;
                }
                ArgumentCallee::OtherModule(callee) => {
                    if !uses.passed_to_other_module.contains(&callee) {
                        uses.passed_to_other_module.push(callee);
                    }
                }
                ArgumentCallee::Unknown => uses.passed_to_unknown = true,
                ArgumentCallee::Local(binding) => self.local_callees.push((key, binding, index)),
            }
            if let Expr::Call(call) = strip_parens(&arg.expr) {
                call.args.visit_with(self);
            }
        }
    }
}

impl ArgumentCallee {
    fn from_writes(writes: ParamWrite) -> Self {
        if writes.writes {
            Self::Writes {
                interop_key: writes.interop_key,
            }
        } else {
            Self::Reads
        }
    }
}

/// Whether a member name is one an interop reads to tell an ES module from
/// a CommonJS value.
fn is_interop_key(name: &str) -> bool {
    name == "default" || name == "__esModule"
}

/// Whether the call of `Object.<method>` or `Reflect.<method>` whose first
/// argument is the value writes `default` or `__esModule` by a literal key:
/// the key argument of `defineProperty`, `set`, and `deleteProperty`, or a
/// key of an object literal passed to `defineProperties` or `assign`.
fn writes_interop_key(method: &str, args: &[swc_core::ecma::ast::ExprOrSpread]) -> bool {
    match method {
        "defineProperty" | "set" | "deleteProperty" => args.get(1).is_some_and(|arg| {
            arg.spread.is_none()
                && matches!(strip_parens(&arg.expr), Expr::Lit(Lit::Str(key))
                    if key.value.as_str().is_some_and(is_interop_key))
        }),
        "defineProperties" | "assign" => args.iter().skip(1).any(|arg| {
            let Expr::Object(object) = strip_parens(&arg.expr) else {
                return false;
            };
            arg.spread.is_none()
                && object.props.iter().any(|prop| {
                    let swc_core::ecma::ast::PropOrSpread::Prop(prop) = prop else {
                        return false;
                    };
                    let name = match prop.as_ref() {
                        Prop::Shorthand(ident) => Some(ident.sym.clone()),
                        Prop::KeyValue(kv) => prop_name_atom(&kv.key),
                        Prop::Getter(getter) => prop_name_atom(&getter.key),
                        Prop::Setter(setter) => prop_name_atom(&setter.key),
                        Prop::Method(method) => prop_name_atom(&method.key),
                        Prop::Assign(_) => None,
                    };
                    name.is_some_and(|name| is_interop_key(&name))
                })
        }),
        _ => false,
    }
}

fn prop_name_atom(key: &PropName) -> Option<Atom> {
    match key {
        PropName::Ident(ident) => Some(ident.sym.clone()),
        PropName::Str(key) => key.value.as_str().map(Atom::from),
        _ => None,
    }
}

/// The name a callee is written with: an identifier, or a static member.
fn callee_name(callee: &Expr) -> Option<Atom> {
    match callee {
        Expr::Ident(ident) => Some(ident.sym.clone()),
        Expr::Member(member) => static_member_name(&member.prop),
        _ => None,
    }
}

/// Interop and re-export helpers by the names transpilers give them. A
/// name is enough here: the helper is only kept from counting as an
/// unknown callee.
fn is_interop_helper_name(name: &str) -> bool {
    matches!(
        name,
        "__importDefault"
            | "__importStar"
            | "__exportStar"
            | "__createBinding"
            | "__setModuleDefault"
            | "__toESM"
            | "__toCommonJS"
            | "__reExport"
            | "_interopRequireDefault"
            | "_interopRequireWildcard"
            | "_interop_require_default"
            | "_interop_require_wildcard"
            | "interopRequireDefault"
            | "interopRequireWildcard"
            | "_interopNamespace"
            | "_interopNamespaceDefault"
            | "_interopDefault"
            | "_interopDefaultLegacy"
            | "_export_star"
            | "_exportStar"
    )
}

/// What a method of a free global does with the argument at `index`.
/// `Object`'s writing methods take their target in the branch for them in
/// `visit_call_expr`; `require`'s members are the bundler's interop.
fn builtin_argument(
    global: &str,
    method: Option<&str>,
    index: usize,
    args: &[swc_core::ecma::ast::ExprOrSpread],
) -> ArgumentCallee {
    match global {
        "require" | "JSON" | "console" | "Array" | "Promise" | "Math" | "String" | "Number"
        | "Symbol" | "Boolean" => ArgumentCallee::Reads,
        "Object" => match method {
            Some("freeze" | "seal" | "preventExtensions") if index == 0 => ArgumentCallee::Unknown,
            _ => ArgumentCallee::Reads,
        },
        "Reflect" => match method {
            Some(method @ ("set" | "defineProperty" | "deleteProperty" | "setPrototypeOf"))
                if index == 0 =>
            {
                ArgumentCallee::Writes {
                    interop_key: writes_interop_key(method, args),
                }
            }
            _ => ArgumentCallee::Reads,
        },
        _ => ArgumentCallee::Unknown,
    }
}

/// A function a required value is passed to.
#[derive(Clone, Copy)]
enum FunctionNode<'a> {
    Function(&'a Function),
    Arrow(&'a ArrowExpr),
}

/// How a function writes one of its parameters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ParamWrite {
    writes: bool,
    /// A write names `default` or `__esModule`.
    interop_key: bool,
}

/// How the function writes the parameter at `index`; `None` when a
/// parameter is not the only way its body reaches the argument (it reads
/// `arguments`, or the parameter is a pattern).
fn local_function_writes(function: FunctionNode<'_>, index: usize) -> Option<ParamWrite> {
    let params: Vec<&Pat> = match function {
        FunctionNode::Function(function) => {
            if reads_arguments(function) {
                return None;
            }
            function.params.iter().map(|param| &param.pat).collect()
        }
        FunctionNode::Arrow(arrow) => arrow.params.iter().collect(),
    };
    let Some(param) = params.get(index) else {
        // An argument past the parameters is unused.
        return Some(ParamWrite::default());
    };
    let Pat::Ident(param) = param else {
        return None;
    };
    let mut finder = ParamWrites {
        param: binding_id(&param.id),
        found: ParamWrite::default(),
    };
    match function {
        FunctionNode::Function(function) => function.visit_with(&mut finder),
        FunctionNode::Arrow(arrow) => arrow.visit_with(&mut finder),
    }
    Some(finder.found)
}

/// Finds the writes to a parameter's value anywhere in its function, nested
/// functions included.
struct ParamWrites {
    param: BindingId,
    found: ParamWrite,
}

impl ParamWrites {
    fn is_param(&self, expr: &Expr) -> bool {
        matches!(strip_parens(expr), Expr::Ident(ident) if binding_id(ident) == self.param)
    }

    /// Records a write to the member `prop` of the parameter.
    fn member_written(&mut self, prop: &MemberProp) {
        self.found.writes = true;
        self.found.interop_key |=
            static_member_name(prop).is_some_and(|name| is_interop_key(&name));
    }

    /// Records a write to a member of the parameter when `expr` is one.
    fn written_member(&mut self, expr: &Expr) {
        if let Expr::Member(member) = strip_parens(expr) {
            if self.is_param(&member.obj) {
                self.member_written(&member.prop);
            }
        }
    }
}

impl Visit for ParamWrites {
    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left {
            if self.is_param(&member.obj) {
                self.member_written(&member.prop);
            }
        }
        assign.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        self.written_member(&update.arg);
        update.visit_children_with(self);
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        if unary.op == UnaryOp::Delete {
            self.written_member(&unary.arg);
        }
        unary.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let (Callee::Expr(callee), Some(first)) = (&call.callee, call.args.first()) {
            if first.spread.is_none() && self.is_param(&first.expr) {
                if let Expr::Member(member) = strip_parens(callee) {
                    let method = member.prop.as_ident().map(|prop| prop.sym.as_ref());
                    let object_api = matches!(strip_parens(&member.obj), Expr::Ident(object)
                        if object.sym == "Object")
                        && matches!(
                            method,
                            Some(
                                "assign" | "defineProperty" | "defineProperties" | "setPrototypeOf"
                            )
                        );
                    if let (true, Some(method)) = (object_api, method) {
                        self.found.writes = true;
                        self.found.interop_key |= writes_interop_key(method, &call.args);
                    }
                }
            }
        }
        call.visit_children_with(self);
    }
}

/// The same-module functions required values are passed to, read once the
/// whole module is known.
struct LocalWriters {
    /// `with` or direct `eval` can reach any binding by name.
    dynamic_scope: bool,
    uses: crate::analysis::binding_uses::BindingUseIndex,
    /// For each callee binding and argument position, how its function
    /// writes that parameter (`None`: not only through the parameter).
    writes: HashMap<(BindingId, usize), Option<ParamWrite>>,
}

impl LocalWriters {
    fn collect(module: &Module, callees: &[(EdgeKey, BindingId, usize)]) -> Self {
        let mut wanted: HashMap<BindingId, Vec<usize>> = HashMap::default();
        for (_, binding, index) in callees {
            let indices = wanted.entry(binding.clone()).or_default();
            if !indices.contains(index) {
                indices.push(*index);
            }
        }
        let mut finder = FunctionFinder {
            wanted: &wanted,
            writes: HashMap::default(),
            this_depth: 0,
        };
        module.visit_with(&mut finder);
        Self {
            dynamic_scope: crate::rules::eval_utils::has_dynamic_scope_construct(module),
            uses: crate::analysis::binding_uses::BindingUseIndex::collect(module),
            writes: finder.writes,
        }
    }

    /// How the function bound to `callee` writes its parameter at `index`;
    /// `None` when the call does not reach that function directly or the
    /// body reaches the argument another way.
    fn writes(&self, callee: &BindingId, index: usize) -> Option<ParamWrite> {
        if self.dynamic_scope || !self.uses.has_single_declaration(callee) {
            return None;
        }
        let written = self.uses.use_sites(callee).iter().any(|site| {
            matches!(
                site.kind,
                crate::analysis::binding_uses::UseKind::Write
                    | crate::analysis::binding_uses::UseKind::ReadWrite
            )
        });
        if written {
            return None;
        }
        *self.writes.get(&(callee.clone(), index))?
    }
}

/// Reads the declarations of the wanted function bindings, in any scope: a
/// function declaration, or a declarator initialized with a function or an
/// arrow. Where `this` is the module's own, a declarator may also hold
/// TypeScript's guarded helper ([`this_guard_fallbacks`]).
struct FunctionFinder<'a> {
    wanted: &'a HashMap<BindingId, Vec<usize>>,
    writes: HashMap<(BindingId, usize), Option<ParamWrite>>,
    /// The non-arrow functions and classes around the node being visited;
    /// at zero, `this` is the module's own.
    this_depth: usize,
}

impl FunctionFinder<'_> {
    fn record(&mut self, name: &swc_core::ecma::ast::Ident, functions: &[FunctionNode<'_>]) {
        let binding = binding_id(name);
        let Some(indices) = self.wanted.get(&binding) else {
            return;
        };
        for index in indices {
            self.writes.insert(
                (binding.clone(), *index),
                fallbacks_write(functions, *index),
            );
        }
    }
}

impl Visit for FunctionFinder<'_> {
    fn visit_fn_decl(&mut self, decl: &swc_core::ecma::ast::FnDecl) {
        self.record(&decl.ident, &[FunctionNode::Function(&decl.function)]);
        decl.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Pat::Ident(name), Some(init)) = (&declarator.name, &declarator.init) {
            match strip_parens(init) {
                Expr::Fn(function) => {
                    self.record(&name.id, &[FunctionNode::Function(&function.function)])
                }
                Expr::Arrow(arrow) => self.record(&name.id, &[FunctionNode::Arrow(arrow)]),
                init @ Expr::Bin(_) if self.this_depth == 0 => {
                    if let Some(functions) = this_guard_fallbacks(init) {
                        self.record(&name.id, &functions);
                    }
                }
                _ => {}
            }
        }
        declarator.visit_children_with(self);
    }

    fn visit_function(&mut self, function: &Function) {
        self.this_depth += 1;
        function.visit_children_with(self);
        self.this_depth -= 1;
    }

    fn visit_class(&mut self, class: &Class) {
        self.this_depth += 1;
        class.visit_children_with(self);
        self.this_depth -= 1;
    }
}

/// How a callee that runs one of `functions` writes its parameter at
/// `index`: `None` when one of them reaches the argument another way, and
/// a write when one of them writes it.
fn fallbacks_write(functions: &[FunctionNode<'_>], index: usize) -> Option<ParamWrite> {
    let mut writes = ParamWrite::default();
    for function in functions {
        let write = local_function_writes(*function, index)?;
        writes.writes |= write.writes;
        writes.interop_key |= write.interop_key;
    }
    Some(writes)
}

/// The functions TypeScript's guarded helper can run, where `this` is the
/// module's own: `this && this.__name || F`, with `F` a function, an arrow,
/// or `c ? F1 : F2` of them (`__createBinding`). The guard is falsy unless
/// the module wrote that property itself, or a global copy of the same
/// tslib helper (`typescript_helper_guard` in rewrite-assumptions.md), so
/// the fallback is what runs. The name proves nothing: the fallbacks'
/// bodies decide, as for any local function.
fn this_guard_fallbacks(expr: &Expr) -> Option<Vec<FunctionNode<'_>>> {
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalOr,
        left,
        right,
        ..
    }) = strip_parens(expr)
    else {
        return None;
    };
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalAnd,
        left: guard,
        right: member,
        ..
    }) = strip_parens(left)
    else {
        return None;
    };
    let Expr::Member(member) = strip_parens(member) else {
        return None;
    };
    if !matches!(strip_parens(guard), Expr::This(_))
        || !matches!(strip_parens(&member.obj), Expr::This(_))
        || static_member_name(&member.prop).is_none()
    {
        return None;
    }
    let mut functions = Vec::new();
    collect_fallbacks(right, &mut functions)?;
    Some(functions)
}

fn collect_fallbacks<'a>(expr: &'a Expr, functions: &mut Vec<FunctionNode<'a>>) -> Option<()> {
    match strip_parens(expr) {
        Expr::Fn(function) => functions.push(FunctionNode::Function(&function.function)),
        Expr::Arrow(arrow) => functions.push(FunctionNode::Arrow(arrow)),
        Expr::Cond(cond) => {
            collect_fallbacks(&cond.cons, functions)?;
            collect_fallbacks(&cond.alt, functions)?;
        }
        _ => return None,
    }
    Some(())
}

/// `Object.prototype.m.call(x, …)` for a method `m` that only reads its
/// receiver: what `UnWebpackInterop` makes of webpack's `require.o`.
fn is_read_only_object_prototype_call(callee: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Member(call) = strip_parens(callee) else {
        return false;
    };
    if static_member_name(&call.prop).as_deref() != Some("call") {
        return false;
    }
    let Expr::Member(method) = strip_parens(&call.obj) else {
        return false;
    };
    if !matches!(
        static_member_name(&method.prop).as_deref(),
        Some("hasOwnProperty" | "propertyIsEnumerable" | "toString" | "isPrototypeOf")
    ) {
        return false;
    }
    let Expr::Member(prototype) = strip_parens(&method.obj) else {
        return false;
    };
    static_member_name(&prototype.prop).as_deref() == Some("prototype")
        && matches!(strip_parens(&prototype.obj), Expr::Ident(object)
            if is_unresolved_ident(object, "Object", unresolved_mark))
}

/// The top-level `var h = require("src").name` bindings no statement
/// writes, read as `UnEsm` reads a bundled provider's helper bindings
/// ([`top_level_required_bindings`]).
fn member_callees(module: &Module, unresolved_mark: Mark) -> HashMap<BindingId, ModuleCallee> {
    top_level_required_bindings(module, Some(unresolved_mark))
        .into_iter()
        .filter_map(|binding| {
            let name = binding.member?;
            Some((
                binding.key,
                ModuleCallee {
                    source: binding.source.into(),
                    export: CalleeExport::Named(name.into()),
                },
            ))
        })
        .collect()
}

/// Whether `function` reads its own `arguments` in its parameters or body
/// (an arrow inside it reads the same `arguments`).
fn reads_arguments(function: &Function) -> bool {
    struct Finder(bool);
    impl Visit for Finder {
        fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
            if ident.sym == "arguments" {
                self.0 = true;
            }
        }

        fn visit_function(&mut self, _: &Function) {}
    }
    let mut finder = Finder(false);
    function.params.visit_with(&mut finder);
    function.body.visit_with(&mut finder);
    finder.0
}

impl Visit for UseCollector<'_> {
    fn visit_function(&mut self, function: &Function) {
        self.this_depth += 1;
        function.visit_children_with(self);
        self.this_depth -= 1;
    }

    fn visit_class(&mut self, class: &Class) {
        self.this_depth += 1;
        class.visit_children_with(self);
        self.this_depth -= 1;
    }

    fn visit_expr(&mut self, expr: &Expr) {
        if let Some(key) = self.value_of(expr) {
            // The keyed call itself, unless a parent below classified it.
            self.uses(key).escapes = true;
            if let Expr::Call(call) = strip_parens(expr) {
                call.args.visit_with(self);
            }
            return;
        }
        expr.visit_children_with(self);
    }

    fn visit_expr_stmt(&mut self, stmt: &ExprStmt) {
        // `require("src");` evaluates the provider and drops its value.
        if let Some(call) = as_require_call(&stmt.expr, self.unresolved_mark) {
            if self.keys.contains(&key_of(call.span)) {
                return;
            }
        }
        stmt.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        let Some(init) = &declarator.init else {
            declarator.visit_children_with(self);
            return;
        };
        let defining = defining_key(init, self.keys, self.local_helpers, self.unresolved_mark);
        match &declarator.name {
            Pat::Ident(binding) => {
                if let Some(key) = self.bindings.get(&binding_id(&binding.id)).copied() {
                    if defining == Some(key) {
                        // The declarator that gives the binding is not a
                        // use; an interop helper around the call is.
                        if as_require_call(init, self.unresolved_mark).is_none() {
                            self.uses(key).interop_arg = true;
                        }
                        return;
                    }
                    // A second declaration assigns the binding again.
                    self.uses(key).binding_write = true;
                }
            }
            Pat::Object(object) => {
                if let Some(call) = as_require_call(init, self.unresolved_mark) {
                    let key = key_of(call.span);
                    if self.keys.contains(&key) {
                        // `var { a, b: c } = require("src")` reads `a` and `b`.
                        for prop in &object.props {
                            let name = match prop {
                                ObjectPatProp::KeyValue(kv) => match &kv.key {
                                    PropName::Ident(name) => Some(name.sym.clone()),
                                    PropName::Str(name) => name.value.as_str().map(Atom::from),
                                    _ => None,
                                },
                                ObjectPatProp::Assign(assign) => Some(assign.key.sym.clone()),
                                ObjectPatProp::Rest(_) => None,
                            };
                            let uses = self.uses(key);
                            match name {
                                Some(name) if !uses.member_reads.contains(&name) => {
                                    uses.member_reads.push(name)
                                }
                                Some(_) => {}
                                None => uses.computed_read = true,
                            }
                        }
                        declarator.name.visit_with(self);
                        return;
                    }
                }
            }
            _ => {}
        }
        declarator.visit_children_with(self);
    }

    fn visit_prop(&mut self, prop: &Prop) {
        // `{ p }` stores the value.
        if let Prop::Shorthand(ident) = prop {
            if let Some(key) = self.bindings.get(&binding_id(ident)).copied() {
                self.uses(key).escapes = true;
                return;
            }
        }
        prop.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if !self.member_use(member, false) {
            member.visit_children_with(self);
        }
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        let Callee::Expr(callee) = &call.callee else {
            call.visit_children_with(self);
            return;
        };
        if let Expr::Member(member) = strip_parens(callee) {
            if self.member_use(member, true) {
                self.arguments(callee, &call.args);
                return;
            }
        }
        if let Some(key) = self.value_of(callee) {
            self.uses(key).called = true;
            self.arguments(callee, &call.args);
            return;
        }
        if is_interop_callee(callee, self.local_helpers, self.unresolved_mark) {
            let mut rest = Vec::new();
            for arg in &call.args {
                match self.value_of(&arg.expr) {
                    Some(key) if arg.spread.is_none() => self.uses(key).interop_arg = true,
                    _ => rest.push(arg),
                }
            }
            callee.visit_with(self);
            for arg in rest {
                arg.visit_with(self);
            }
            return;
        }
        if let Some(method) = self.object_api(callee) {
            callee.visit_with(self);
            for (index, arg) in call.args.iter().enumerate() {
                if index == 0 && arg.spread.is_none() && self.value_of(&arg.expr).is_none() {
                    // `Object.assign(module.exports, …)` after
                    // `module.exports = require("src")`.
                    let keys = self.written_values(&arg.expr);
                    if !keys.is_empty() {
                        let interop_key = writes_interop_key(method, &call.args);
                        for key in keys {
                            let uses = self.uses(key);
                            uses.object_api_write = true;
                            uses.interop_key_write |= interop_key;
                        }
                        continue;
                    }
                }
                let value = (arg.spread.is_none())
                    .then(|| self.value_of(&arg.expr))
                    .flatten();
                match (index, value) {
                    (0, Some(key)) => {
                        let interop_key = writes_interop_key(method, &call.args);
                        let uses = self.uses(key);
                        uses.object_api_write = true;
                        uses.interop_key_write |= interop_key;
                    }
                    (_, Some(key)) if method == "assign" => self.uses(key).assign_source = true,
                    _ => arg.visit_with(self),
                }
            }
            return;
        }
        callee.visit_with(self);
        self.arguments(callee, &call.args);
    }

    fn visit_new_expr(&mut self, new: &NewExpr) {
        if let Some(key) = self.value_of(&new.callee) {
            self.uses(key).called = true;
            if let Some(args) = &new.args {
                self.arguments(&new.callee, args);
            }
            return;
        }
        new.callee.visit_with(self);
        if let Some(args) = &new.args {
            self.arguments(&new.callee, args);
        }
    }

    fn visit_opt_chain_expr(&mut self, chain: &OptChainExpr) {
        match chain.base.as_ref() {
            OptChainBase::Member(member) if self.member_use(member, false) => {}
            OptChainBase::Call(call) => self.visit_opt_call(call),
            _ => chain.visit_children_with(self),
        }
    }

    fn visit_opt_call(&mut self, call: &OptCall) {
        let member = match strip_parens(&call.callee) {
            Expr::Member(member) => Some(member),
            Expr::OptChain(chain) => match chain.base.as_ref() {
                OptChainBase::Member(member) => Some(member),
                _ => None,
            },
            _ => None,
        };
        if member.is_some_and(|member| self.member_use(member, true)) {
            call.args.visit_with(self);
            return;
        }
        if let Some(key) = self.value_of(&call.callee) {
            self.uses(key).called = true;
            call.args.visit_with(self);
            return;
        }
        call.visit_children_with(self);
    }

    fn visit_tagged_tpl(&mut self, tagged: &TaggedTpl) {
        if let Expr::Member(member) = strip_parens(&tagged.tag) {
            if self.member_use(member, true) {
                tagged.tpl.visit_with(self);
                return;
            }
        }
        tagged.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        let handled = match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) => self.member_write(member),
            AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) => {
                self.binding_write(&Expr::Ident(ident.id.clone()))
            }
            _ => false,
        };
        if !handled {
            assign.left.visit_with(self);
        }
        assign.right.visit_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        let handled = match strip_parens(&update.arg) {
            Expr::Member(member) => self.member_write(member),
            other => self.binding_write(other),
        };
        if !handled {
            update.visit_children_with(self);
        }
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        match (unary.op, strip_parens(&unary.arg)) {
            (UnaryOp::Delete, Expr::Member(member)) if self.member_write(member) => {}
            // `typeof x` neither reads a member nor lets the value escape.
            (UnaryOp::TypeOf, arg) if self.value_of(arg).is_some() => {}
            _ => unary.visit_children_with(self),
        }
    }
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
    use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::VisitMutWith;

    use swc_core::common::DUMMY_SP;
    use swc_core::ecma::ast::Ident;

    use super::*;

    fn with_module<T>(source: &str, f: impl FnOnce(&Module, Mark) -> T) -> T {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let fm = cm.new_source_file(
                FileName::Custom("consumer.js".to_string()).into(),
                source.to_string(),
            );
            let lexer = Lexer::new(
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                StringInput::from(&*fm),
                None,
            );
            let mut module = Parser::new_from(lexer)
                .parse_module()
                .expect("parse failed");
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
            f(&module, unresolved_mark)
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

    fn only(edges: &RequireEdges, source: &str) -> RequireEdge {
        let found: Vec<_> = edges
            .iter()
            .filter(|edge| edge.source.as_ref() == source)
            .collect();
        assert_eq!(found.len(), 1, "one edge of {source}: {found:?}");
        found[0].clone()
    }

    fn names(names: &[&str]) -> Vec<Atom> {
        names.iter().map(|name| Atom::from(*name)).collect()
    }

    #[test]
    fn forms_and_positions() {
        let edges = edges(
            r#"var alpha = require("./alpha.js");
var { beta, gamma: delta } = require("./beta.js");
var epsilon = require("./epsilon.js").zeta;
require("./bare.js");
use(require("./inline.js"));
function later() { var local = require("./nested.js"); }
if (flag) { require("./conditional.js"); }"#,
        );
        let alpha = only(&edges, "./alpha.js");
        assert_eq!(
            alpha.form,
            RequireForm::Declarator {
                binding: "alpha".into()
            }
        );
        assert!(alpha.top_level);
        assert_eq!(
            only(&edges, "./beta.js").form,
            RequireForm::Destructure {
                names: names(&["beta", "gamma"])
            }
        );
        assert_eq!(
            only(&edges, "./epsilon.js").form,
            RequireForm::Member {
                name: Some("zeta".into())
            }
        );
        assert_eq!(only(&edges, "./bare.js").form, RequireForm::Bare);
        assert_eq!(only(&edges, "./inline.js").form, RequireForm::Inline);
        let nested = only(&edges, "./nested.js");
        assert!(matches!(nested.form, RequireForm::Declarator { .. }));
        assert!(!nested.top_level);
        assert!(!only(&edges, "./conditional.js").top_level);
    }

    #[test]
    fn interop_reaches_a_call_directly_or_through_its_binding() {
        let edges = edges(
            r#"function i(e) { return e && e.__esModule ? e : { default: e }; }
var a = i(require("./a")), b = require("./b"), c = require.n(b), d = require("./d");
exports.x = d.__esModule;
exports.y = require("./e").value;"#,
        );
        let a = only(&edges, "./a");
        assert_eq!(a.interop, Interop::Default);
        assert_eq!(
            a.form,
            RequireForm::HelperArg {
                helper: Interop::Default,
                inline: false,
                binding: "a".into()
            }
        );
        assert_eq!(only(&edges, "./b").interop, Interop::Default);
        let d = only(&edges, "./d");
        assert_eq!(d.interop, Interop::None);
        assert!(d.esmodule_tested);
        assert_eq!(only(&edges, "./e").interop, Interop::None);
        assert!(edges.reads_through_interop("./a"));
        assert!(edges.reads_through_interop("./b"));
        assert!(edges.reads_through_interop("./d"));
        assert!(!edges.reads_through_interop("./e"));
        // A `.default` read alone is no interop.
        let plain = super::tests::edges(r#"var a = require("./a"); exports.x = a.default;"#);
        assert!(!plain.reads_through_interop("./a"));
    }

    #[test]
    fn whole_value_needs_every_call_to_be_a_plain_top_level_declarator() {
        let edges = edges(
            r#"var a = require("./a");
var b = require("./b"); var b2 = require("./b").x;
var c = require("./c"); c.__esModule;
var d = require("./d"); function f() { return require("./d"); }
var e = require.n(require("./e"));"#,
        );
        assert!(edges.uses_whole_value("./a"));
        assert!(!edges.uses_whole_value("./b"));
        assert!(!edges.uses_whole_value("./c"));
        assert!(!edges.uses_whole_value("./d"));
        assert!(!edges.uses_whole_value("./e"));
        assert!(!edges.uses_whole_value("./missing"));
    }

    #[test]
    fn a_write_to_module_exports_writes_each_required_value_it_may_hold() {
        // A module that passes a required value on and then adds to it.
        let found = edges(
            r#"module.exports = require("./a");
module.exports.extra = 3;"#,
        );
        assert_eq!(only(&found, "./a").uses.unwrap().member_writes, ["extra"]);
        let found = edges(
            r#"var t = module.exports = require("./a");
t.more = 4;
use(t.read);"#,
        );
        let uses = only(&found, "./a").uses.unwrap();
        assert_eq!(uses.member_writes, ["more"]);
        assert_eq!(uses.member_reads, ["read"]);
        let found = edges(
            r#"if (c) module.exports = require("./a"); else module.exports = require("./b");
Object.assign(module.exports, { extra: 1 });"#,
        );
        for source in ["./a", "./b"] {
            assert!(
                only(&found, source).uses.unwrap().object_api_write,
                "{source}"
            );
        }
        // Without a required value in `module.exports`, the write is the
        // module's own.
        let found = edges(
            r#"var a = require("./a");
module.exports = {};
module.exports.extra = a;"#,
        );
        assert!(!only(&found, "./a").uses.unwrap().writes());
    }

    #[test]
    fn reading_only_default_needs_a_whole_value_read_only_as_default() {
        let edges = edges(
            r#"var a = require("./a"); function fa() { return a.default; } use(a.default);
var b = require("./b"); b.default; b.other;
var c = require("./c"); c.default();
var d = require("./d"); d.default = 1;
var e = require("./e"); use(e);
var f = require("./f").default;
var g = require("./g"); g.__esModule ? g.default : g;"#,
        );
        assert!(edges.reads_only_default("./a"));
        for source in ["./b", "./c", "./d", "./e", "./f", "./g", "./missing"] {
            assert!(!edges.reads_only_default(source), "{source}");
        }
    }

    #[test]
    fn wildcard_interop_counts_a_declared_or_inlined_helper() {
        let edges = edges(
            r#"function w(e) {
    if (e && e.__esModule) return e;
    var n = {};
    if (e != null) for (var k in e) if (Object.prototype.hasOwnProperty.call(e, k)) n[k] = e[k];
    n.default = e;
    return n;
}
var a = w(require("./a"));
var b = w(require("./b")); var b2 = require("./b");"#,
        );
        assert!(edges.uses_only_wildcard_interop("./a"));
        assert!(!edges.uses_only_wildcard_interop("./b"));
        assert_eq!(only(&edges, "./a").interop, Interop::Wildcard);
    }

    #[test]
    fn uses_and_writes_are_found_anywhere_in_the_tree() {
        let edges = edges(
            r#"var p = require("./p.js");
p.read;
p.method();
a.f = p.f = fn;
x || (p.g = 1);
p.count++;
delete p.gone;
function later() { if (y) { p.h = 2; } }
Object.assign(p, extra);
Object.defineProperty(require("./q.js"), "k", { value: 1 });
require("./r.js").s = 3;
use(p);"#,
        );
        let p = only(&edges, "./p.js").uses.expect("uses of ./p.js");
        assert_eq!(p.member_reads, names(&["read"]));
        assert_eq!(p.member_calls, names(&["method"]));
        assert_eq!(p.member_writes, names(&["f", "g", "count", "gone", "h"]));
        assert!(p.object_api_write);
        assert!(p.escapes);
        assert!(p.writes());
        let q = only(&edges, "./q.js").uses.expect("uses of ./q.js");
        assert!(q.object_api_write && !q.escapes);
        let r = only(&edges, "./r.js").uses.expect("uses of ./r.js");
        assert_eq!(r.member_writes, names(&["s"]));
    }

    #[test]
    fn a_same_module_function_that_writes_its_parameter_writes_the_value() {
        let edges = edges(
            r#"function install(m) { m.f = function () {}; }
install(require("./declared.js"));
var add = (o) => { Object.assign(o, extra); };
add(require("./arrow.js"));
(function (m) { delete m.gone; })(require("./literal.js"));
function later(m, name) { list.forEach(function (k) { m[name + k] = 1; }); }
var p = require("./nested.js");
later(p, "x");
(0, install)(require("./sequence.js"));"#,
        );
        for source in [
            "./declared.js",
            "./arrow.js",
            "./literal.js",
            "./nested.js",
            "./sequence.js",
        ] {
            let uses = only(&edges, source).uses.expect("uses");
            assert!(uses.callee_write && uses.writes(), "{source}: {uses}");
            assert!(!uses.passed_to_unseen_callee(), "{source}: {uses}");
        }
    }

    #[test]
    fn a_write_that_names_an_interop_key_is_marked() {
        let edges = edges(
            r#"require("./member.js").__esModule = true;
var d = require("./deleted.js"); delete d.default;
Object.defineProperty(require("./defined.js"), "__esModule", { value: true });
Object.assign(require("./assigned.js"), { "default": x });
Object.defineProperties(require("./properties.js"), { __esModule: { value: true } });
Reflect.set(require("./reflected.js"), "default", x);
function mark(m) { if (y) { m.__esModule = true; } m.f = 1; }
mark(require("./local.js"));
var p = require("./plain.js"); p.flag = 1; p[key] = 2; p.count++;
Object.defineProperty(require("./other-key.js"), "k", { value: 1 });
Object.assign(require("./spread.js"), source, { f: 1 });
function patch(m) { m.f = 1; }
patch(require("./local-plain.js"));"#,
        );
        for source in [
            "./member.js",
            "./deleted.js",
            "./defined.js",
            "./assigned.js",
            "./properties.js",
            "./reflected.js",
            "./local.js",
        ] {
            let uses = only(&edges, source).uses.expect("uses");
            assert!(uses.writes() && uses.interop_key_write, "{source}: {uses}");
        }
        for source in [
            "./plain.js",
            "./other-key.js",
            "./spread.js",
            "./local-plain.js",
        ] {
            let uses = only(&edges, source).uses.expect("uses");
            assert!(uses.writes() && !uses.interop_key_write, "{source}: {uses}");
        }
        let local = only(&edges, "./local.js").uses.expect("uses");
        assert_eq!(
            local.to_string(),
            "escapes, written by a callee, writes default or __esModule"
        );
    }

    #[test]
    fn readers_helpers_and_unproven_callees_do_not_write_the_value() {
        let edges = edges(
            r#"function describe(m) { return Object.keys(m).join(); }
describe(require("./read.js"));
Object.keys(require("./keys.js"));
__exportStar(require("./star.js"), exports);
function spread(m) { arguments[0].x = 1; m.y = 2; }
spread(require("./arguments.js"));
var swapped = function (m) { m.f = 1; };
swapped = other;
swapped(require("./reassigned.js"));
function twice(m) { m.f = 1; }
function twice(m) {}
twice(require("./twice.js"));
var installer = require("./installer.js");
installer.install(require("./other.js"));
installer.nested.install(require("./other3.js"));
require("./direct.js")(require("./other2.js"));
register(require("./global.js"));"#,
        );
        for source in ["./read.js", "./keys.js", "./star.js"] {
            let uses = only(&edges, source).uses.expect("uses");
            assert!(
                !uses.writes() && !uses.passed_to_unseen_callee(),
                "{source}: {uses}"
            );
        }
        // A callee the call may not reach, or whose body reaches the
        // argument another way, is unknown.
        for source in [
            "./arguments.js",
            "./reassigned.js",
            "./twice.js",
            "./global.js",
        ] {
            let uses = only(&edges, source).uses.expect("uses");
            assert!(!uses.writes() && uses.passed_to_unknown, "{source}: {uses}");
        }
        let other = only(&edges, "./other.js").uses.expect("uses");
        assert_eq!(
            other.passed_to_other_module,
            [ModuleCallee {
                source: "./installer.js".into(),
                export: CalleeExport::Named("install".into()),
            }]
        );
        assert!(!other.writes());
        let other3 = only(&edges, "./other3.js").uses.expect("uses");
        assert_eq!(
            other3.passed_to_other_module,
            [ModuleCallee {
                source: "./installer.js".into(),
                export: CalleeExport::Other,
            }]
        );
        let other2 = only(&edges, "./other2.js").uses.expect("uses");
        assert_eq!(
            other2.passed_to_other_module,
            [ModuleCallee {
                source: "./direct.js".into(),
                export: CalleeExport::Value,
            }]
        );
    }

    #[test]
    fn a_binding_of_a_required_member_is_that_export() {
        let edges = edges(
            r#"var h = require("./h.js").default;
h(require("./default.js"));
h.x(require("./member-of.js"));
var r = require("./r.js").default;
r = other;
r(require("./reassigned.js"));
function later() {
    var g = require("./g.js").default;
    g(require("./nested.js"));
}"#,
        );
        let default = only(&edges, "./default.js").uses.expect("uses");
        assert_eq!(
            default.passed_to_other_module,
            [ModuleCallee {
                source: "./h.js".into(),
                export: CalleeExport::Named("default".into()),
            }]
        );
        assert!(!default.passed_to_unknown && !default.writes(), "{default}");
        let member_of = only(&edges, "./member-of.js").uses.expect("uses");
        assert_eq!(
            member_of.passed_to_other_module,
            [ModuleCallee {
                source: "./h.js".into(),
                export: CalleeExport::Other,
            }]
        );
        // A reassigned binding, or one declared inside a function, may not
        // hold the export when the call runs.
        for source in ["./reassigned.js", "./nested.js"] {
            let uses = only(&edges, source).uses.expect("uses");
            assert!(
                uses.passed_to_unknown && uses.passed_to_other_module.is_empty(),
                "{source}: {uses}"
            );
        }
    }

    #[test]
    fn a_typescript_guarded_helper_is_read_through_its_fallbacks() {
        // producer typescript@6.0.3 --module commonjs --esModuleInterop,
        // then terser@5.36.0 -c -m --toplevel: the helpers `export *` and
        // `export { x } from` need, under minified names.
        let edges = edges(
            r#"var t = this && this.__createBinding || (Object.create ? function (e, t, r, n) {
    void 0 === n && (n = r);
    var o = Object.getOwnPropertyDescriptor(t, r);
    o && !("get" in o ? !t.__esModule : o.writable || o.configurable) || (o = { enumerable: !0, get: function () { return t[r]; } }),
    Object.defineProperty(e, n, o);
} : function (e, t, r, n) { void 0 === n && (n = r), e[n] = t[r]; }),
    n = this && this.__exportStar || function (e, r) {
    for (var n in e) "default" === n || Object.prototype.hasOwnProperty.call(r, n) || t(r, e, n);
};
n(require("./a.js"), exports);
t(exports, require("./b.js"), "x");
(this && this.__read || function (m) { return Object.keys(m); })(require("./inline.js"));"#,
        );
        for source in ["./a.js", "./b.js", "./inline.js"] {
            let uses = only(&edges, source).uses.expect("uses");
            assert!(
                !uses.writes() && !uses.passed_to_unseen_callee(),
                "{source}: {uses}"
            );
        }
    }

    #[test]
    fn a_guarded_helper_counts_only_where_this_is_the_modules() {
        let edges = edges(
            r#"var w = this && this.__exportStar || function (m, o) { m.extra = 1; };
w(require("./written.js"), exports);
var c = this && this.__createBinding || (Object.create ? function (o, m, k) { o[k] = m[k]; } : function (o, m, k) { m[k] = o[k]; });
c(exports, require("./cond.js"), "x");
function later() {
    var s = this && this.__exportStar || function (m, o) { for (var k in m) o[k] = m[k]; };
    s(require("./nested.js"), exports);
    (this && this.__read || function (m) { return Object.keys(m); })(require("./inline-nested.js"));
}
class Holder {
    field = (this && this.__read || function (m) { return Object.keys(m); })(require("./class.js"));
}"#,
        );
        // hypothetical: a fallback that writes the value is a write, as
        // any local function would be, and so is either branch of `?:`.
        for source in ["./written.js", "./cond.js"] {
            let uses = only(&edges, source).uses.expect("uses");
            assert!(uses.callee_write && uses.writes(), "{source}: {uses}");
        }
        // In a function or a class body `this` is the receiver of the call,
        // whose `__name` property may be any function.
        for source in ["./nested.js", "./inline-nested.js", "./class.js"] {
            let uses = only(&edges, source).uses.expect("uses");
            assert!(!uses.writes() && uses.passed_to_unknown, "{source}: {uses}");
        }
    }

    #[test]
    fn only_a_read_only_object_prototype_method_reads_through_call() {
        let edges = edges(
            r#"Object.prototype.hasOwnProperty.call(require("./own.js"), "k");
Object.prototype.propertyIsEnumerable.call(require("./enumerable.js"), "k");
Array.prototype.push.call(require("./push.js"), 1);
function shadowed(Object) {
    Object.prototype.hasOwnProperty.call(require("./shadowed.js"), "k");
}"#,
        );
        for source in ["./own.js", "./enumerable.js"] {
            let uses = only(&edges, source).uses.expect("uses");
            assert!(
                !uses.writes() && !uses.passed_to_unseen_callee(),
                "{source}: {uses}"
            );
        }
        for source in ["./push.js", "./shadowed.js"] {
            let uses = only(&edges, source).uses.expect("uses");
            assert!(uses.passed_to_unknown, "{source}: {uses}");
        }
    }

    #[test]
    fn a_plain_read_only_consumer_neither_writes_nor_escapes() {
        let edges = edges(
            r#"var p = require("./p.js");
console.log(p.alpha, typeof p);
new p.Beta();
p?.gamma;"#,
        );
        let uses = only(&edges, "./p.js").uses.expect("uses");
        assert_eq!(uses.member_reads, names(&["alpha", "Beta", "gamma"]));
        assert!(!uses.writes() && !uses.escapes && !uses.binding_write);
    }

    #[test]
    fn a_call_missing_from_the_ast_unesm_sees_has_no_uses() {
        with_module(r#"var p = require("./p.js"); p.alpha;"#, |module, mark| {
            let mut edges = RequireEdges::collect(module, mark);
            let mut copy = module.clone();
            copy.body.clear();
            edges.add_uses(
                &copy,
                mark,
                &LocalHelperContext::collect_with_mark(&copy, mark),
            );
            assert_eq!(only(&edges, "./p.js").uses, None);
        });
    }

    #[test]
    fn a_rebuilt_import_local_carries_the_tag_of_the_ones_it_stands_for() {
        let mut tags = ImportTags::default();
        let named: BindingId = ("alpha".into(), Default::default());
        let default: BindingId = ("_alpha".into(), Default::default());
        let call = Span::new(BytePos(1), BytePos(9));
        tags.insert(named.clone(), call);
        tags.carry(&named, default.clone());
        assert_eq!(tags.edges_of(&default), &[key_of(call)]);
        // A dummy call still tags its local, with no key.
        let dummy: BindingId = ("beta".into(), Default::default());
        tags.insert(dummy.clone(), DUMMY_SP);
        assert!(tags.is_tagged(&dummy) && tags.edges_of(&dummy).is_empty());
    }

    #[test]
    fn imports_a_pass_made_from_declarators_are_tagged_with_their_require() {
        with_module(
            r#"var alpha = helper(require("./alpha.js")); var beta = other();"#,
            |module, mark| {
                let calls = declarator_require_calls(module, mark);
                assert_eq!(calls.len(), 1);
                // As a later pass would: the declarator becomes an import.
                let mut converted = crate::unpacker::parse_es_module(
                    r#"import * as alpha from "./alpha.js"; var beta = other();"#,
                    "consumer.js",
                    Default::default(),
                )
                .unwrap();
                let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = &mut converted.body[0]
                else {
                    unreachable!()
                };
                import.span = DUMMY_SP;
                let ImportSpecifier::Namespace(namespace) = &mut import.specifiers[0] else {
                    unreachable!()
                };
                let (local, call) = calls.iter().next().unwrap();
                namespace.local.ctxt = local.1;
                let mut tags = ImportTags::default();
                tag_declarator_imports(&converted, &calls, &mut tags);
                assert_eq!(tags.edges_of(local), &[key_of(*call)]);
            },
        );
    }

    #[test]
    fn a_dummy_or_shared_span_gives_no_key_but_still_counts_for_its_source() {
        with_module(
            r#"var a = require("./a"); var b = require("./b"), c = require.n(b);"#,
            |module, mark| {
                let mut module = module.clone();
                let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = &mut module.body[0] else {
                    unreachable!()
                };
                let Some(Expr::Call(call)) = var.decls[0].init.as_deref_mut() else {
                    unreachable!()
                };
                call.span = swc_core::common::DUMMY_SP;
                let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = &module.body[1] else {
                    unreachable!()
                };
                let mut shared = var.clone();
                shared.decls.truncate(1);
                shared.decls[0].name = Pat::Ident(Ident::new_no_ctxt("d".into(), DUMMY_SP).into());
                module
                    .body
                    .push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(shared))));
                let edges = RequireEdges::collect(&module, mark);
                assert!(edges.iter().next().is_none());
                assert_eq!(edges.unkeyed().len(), 3);
                assert!(edges.uses_whole_value("./a"));
                assert!(!edges.uses_whole_value("./b"));
                assert!(edges.reads_through_interop("./b"));
            },
        );
    }
}
