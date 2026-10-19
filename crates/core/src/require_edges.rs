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
//! A call whose span is dummy, or shared with another call, cannot be found
//! again by span: it keeps what the first pass read, which the per-source
//! queries count, but has no key, so it gets no uses and no import tag. A
//! detector that builds its module AST itself (a prepared AST) can write
//! such calls.
//!

use crate::collections::{HashMap, HashSet};
use std::fmt;

use swc_core::atoms::Atom;
use swc_core::common::{BytePos, Mark, Span};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignTarget, CallExpr, Callee, Class, Decl, Expr, ExprStmt, Function,
    Lit, MemberExpr, MemberProp, Module, ModuleDecl, ModuleItem, NewExpr, ObjectPatProp, OptCall,
    OptChainBase, OptChainExpr, Pat, Prop, PropName, SimpleAssignTarget, Stmt, TaggedTpl,
    UnaryExpr, UnaryOp, UpdateExpr, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::analysis::{binding_id, BindingId};
use crate::rules::expr_utils::is_unresolved_ident;
use crate::rules::transpiler_helper_utils::{
    classify_inline_callable, LocalHelperContext, TranspilerHelperKind,
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
}

impl RequireUses {
    /// Whether the consumer writes to the required value.
    pub fn writes(&self) -> bool {
        !self.member_writes.is_empty() || self.computed_write || self.object_api_write
    }
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
        let mut uses = UseCollector {
            unresolved_mark,
            keys: &keys,
            local_helpers,
            bindings: &sites.bindings,
            uses: HashMap::default(),
        };
        module.visit_with(&mut uses);
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

/// The edges collected for one module before `UnEsm`. The pipeline adds
/// their uses in place when the rule range reaches `UnEsm`
/// ([`crate::rules::RulePipelineOptions::with_require_record`]).
#[derive(Debug, Clone, Default)]
pub(crate) struct RequireRecord {
    pub(crate) edges: RequireEdges,
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
            if let Some(key) = self.defining_key(init) {
                self.bindings.insert(binding_id(&binding.id), key);
            }
        }
        declarator.visit_children_with(self);
    }
}

/// Records how each keyed call's value is used, at the call and through
/// the binding a declarator gives it.
struct UseCollector<'a> {
    unresolved_mark: Mark,
    keys: &'a HashSet<EdgeKey>,
    local_helpers: &'a LocalHelperContext,
    bindings: &'a HashMap<BindingId, EdgeKey>,
    uses: HashMap<EdgeKey, RequireUses>,
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

    /// A write to a member of an edge's value; `false` when `member` is on
    /// something else.
    fn member_write(&mut self, member: &MemberExpr) -> bool {
        let Some(key) = self.value_of(&member.obj) else {
            return false;
        };
        match static_member_name(&member.prop) {
            Some(name) => {
                let writes = &mut self.uses(key).member_writes;
                if !writes.contains(&name) {
                    writes.push(name);
                }
            }
            None => self.uses(key).computed_write = true,
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

impl Visit for UseCollector<'_> {
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
                call.args.visit_with(self);
                return;
            }
        }
        if let Some(key) = self.value_of(callee) {
            self.uses(key).called = true;
            call.args.visit_with(self);
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
                let value = (arg.spread.is_none())
                    .then(|| self.value_of(&arg.expr))
                    .flatten();
                match (index, value) {
                    (0, Some(key)) => self.uses(key).object_api_write = true,
                    (_, Some(key)) if method == "assign" => self.uses(key).assign_source = true,
                    _ => arg.visit_with(self),
                }
            }
            return;
        }
        call.visit_children_with(self);
    }

    fn visit_new_expr(&mut self, new: &NewExpr) {
        if let Some(key) = self.value_of(&new.callee) {
            self.uses(key).called = true;
            new.args.visit_with(self);
            return;
        }
        new.visit_children_with(self);
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
