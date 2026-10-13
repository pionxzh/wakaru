use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::common::{Mark, Span};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, AssignPat, AssignPatProp, BinExpr, BinaryOp, CallExpr, Callee,
    Class, CondExpr, Decl, DoWhileStmt, Expr, ForInStmt, ForOfStmt, ForStmt, Function, IfStmt,
    ImportSpecifier, Lit, MemberProp, Module, ModuleDecl, ModuleItem, OptCall, OptChainBase,
    OptChainExpr, Stmt, SwitchStmt, TryStmt, WhileStmt,
};
use swc_core::ecma::ast::{
    BindingIdent, Ident, IdentName, Invalid, KeyValueProp, MemberExpr, NewExpr, Pat, Prop,
    PropName, PropOrSpread, SimpleAssignTarget, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::collections::HashMap;
use crate::facts::{ExportFact, ExportKind, ModuleFacts, ModuleFactsMap};
use crate::rules::lowered_dynamic_import::LoweredImportMatcher;
use crate::rules::transpiler_helper_utils::{LocalHelperContext, TranspilerHelperKind};

use crate::analysis::binding_id;
use crate::analysis::binding_uses::BindingId;
use crate::collections::HashSet;
use crate::rules::expr_utils::is_unresolved_ident;
use crate::utils::paren::strip_parens;

/// Where a `require` call that stays a call runs.
///
/// The classes follow the positions a source writes a require in: a load
/// that a bundler emitted for an `import` runs on every evaluation of the
/// module body, while a require in a function or a conditional position was
/// written by the source itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequirePosition {
    /// Runs on every evaluation of the module body.
    TopLevel,
    /// Inside a function, arrow, method, accessor, or class body (an IIFE
    /// included).
    Function,
    /// A branch of `if` or `?:`, the right side of `&&`, `||`, `??` or their
    /// assignments, a pattern default, an optional call or member, or a
    /// loop, `switch`, or `try` body.
    Conditional,
}

impl RequirePosition {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::TopLevel => "top-level",
            Self::Function => "function",
            Self::Conditional => "conditional",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeptRequire {
    pub span: Span,
    pub position: RequirePosition,
    /// The specifier of a `require("literal")` call.
    pub source: Option<Atom>,
}

/// Every call of the free `require` in `module`, in visit order, except one
/// whose only argument is a numeric literal.
///
/// A numeric id names a module outside the input (unpack rewrites the ids it
/// resolves into relative paths), so the call fails in CommonJS as well as in
/// ESM: keeping such a module CommonJS would not make it run.
///
/// `module` must be resolved with `unresolved_mark`.
pub(crate) fn kept_require_calls(module: &Module, unresolved_mark: Mark) -> Vec<KeptRequire> {
    let mut finder = KeptRequireFinder {
        unresolved_mark,
        function_depth: 0,
        conditional_depth: 0,
        found: Vec::new(),
        blocking: false,
        imported_helpers: HashSet::default(),
    };
    module.visit_with(&mut finder);
    finder.found
}

/// [`kept_require_calls`] after `UnEsm` converted the module. With
/// `later_unpack_stages`, it leaves out the requires a later stage of unpack
/// may still turn into imports from the other modules' facts, in the
/// top-level forms those stages accept:
///
/// - the require argument of an imported helper that initializes a
///   top-level `var`, `var x = helper(require("./p"))` or
///   `var x = ns.helper(require("./p"))` (`cross_module_interop_import`);
/// - a source of a top-level default-object composition statement,
///   `Object.assign(target, require("./p") || {})`
///   (`commonjs_default_object_composition`);
/// - anywhere, the require of a lowered `import()` around an imported
///   helper, `Promise.resolve().then(() => helper(require("./p")))`
///   (`run_cross_module_lowered_dynamic_imports`).
///
/// One the later stage leaves stays in ES module output, where the
/// `commonjs_require_unrecovered` warning reports it. A single file has no
/// later stage, and the stages read no other form, so every other require
/// counts.
pub(crate) fn blocking_require_calls(
    module: &Module,
    unresolved_mark: Mark,
    later_unpack_stages: bool,
) -> Vec<KeptRequire> {
    if !later_unpack_stages {
        return kept_require_calls(module, unresolved_mark);
    }
    let mut imported = HashSet::default();
    for item in &module.body {
        if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item {
            imported.extend(import.specifiers.iter().map(|specifier| match specifier {
                ImportSpecifier::Named(named) => binding_id(&named.local),
                ImportSpecifier::Default(default) => binding_id(&default.local),
                ImportSpecifier::Namespace(namespace) => binding_id(&namespace.local),
            }));
        }
    }
    let mut finder = KeptRequireFinder {
        unresolved_mark,
        function_depth: 0,
        conditional_depth: 0,
        found: Vec::new(),
        blocking: true,
        imported_helpers: imported,
    };
    module.visit_with(&mut finder);
    finder.found
}

/// [`kept_require_calls`] for one node. The calls come in the order a
/// default `Visit` or `VisitMut` traversal of `node` meets them, so a
/// rewriting pass can pair them with the calls it visits.
pub(crate) fn kept_require_calls_in<N: VisitWith<KeptRequireFinder>>(
    node: &N,
    unresolved_mark: Mark,
) -> Vec<KeptRequire> {
    let mut finder = KeptRequireFinder {
        unresolved_mark,
        function_depth: 0,
        conditional_depth: 0,
        found: Vec::new(),
        blocking: false,
        imported_helpers: HashSet::default(),
    };
    node.visit_with(&mut finder);
    finder.found
}

/// The calls of [`kept_require_calls`] when `module` has import or export
/// declarations. ESM has no `require` binding, so each one throws a
/// `ReferenceError` when it runs; CommonJS output reports nothing.
pub(crate) fn esm_output_require_calls(module: &Module, unresolved_mark: Mark) -> Vec<KeptRequire> {
    if !module
        .body
        .iter()
        .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
    {
        return Vec::new();
    }
    kept_require_calls(module, unresolved_mark)
}

fn string_require_source(call: &CallExpr) -> Option<Atom> {
    match call.args.as_slice() {
        [arg] if arg.spread.is_none() => match strip_parens(&arg.expr) {
            Expr::Lit(Lit::Str(source)) => Some(Atom::from(source.value.as_str().unwrap_or(""))),
            _ => None,
        },
        _ => None,
    }
}

/// Whether `call` calls the free `require` with anything but a numeric id.
pub(crate) fn is_kept_require_call(call: &CallExpr, unresolved_mark: Mark) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !matches!(strip_parens(callee), Expr::Ident(id)
        if is_unresolved_ident(id, "require", unresolved_mark))
    {
        return false;
    }
    !matches!(call.args.as_slice(), [arg]
        if arg.spread.is_none() && matches!(strip_parens(&arg.expr), Expr::Lit(Lit::Num(_))))
}

pub(crate) struct KeptRequireFinder {
    unresolved_mark: Mark,
    function_depth: usize,
    conditional_depth: usize,
    found: Vec<KeptRequire>,
    /// Skip the top-level requires a later stage converts
    /// ([`blocking_require_calls`]).
    blocking: bool,
    /// Import bindings whose call of a require is left for a later stage.
    imported_helpers: HashSet<BindingId>,
}

impl KeptRequireFinder {
    fn position(&self) -> RequirePosition {
        if self.function_depth > 0 {
            RequirePosition::Function
        } else if self.conditional_depth > 0 {
            RequirePosition::Conditional
        } else {
            RequirePosition::TopLevel
        }
    }

    /// `helper` or `ns.helper` for an import binding `helper` or `ns`.
    fn is_imported_helper(&self, callee: &Expr) -> bool {
        let helper = match strip_parens(callee) {
            Expr::Ident(id) => id,
            Expr::Member(member) => match strip_parens(&member.obj) {
                Expr::Ident(id) => id,
                _ => return false,
            },
            _ => return false,
        };
        self.imported_helpers.contains(&binding_id(helper))
    }

    /// The specifier of a lowered `import()` around an imported wildcard
    /// helper, `Promise.resolve().then(() => helper(require("./p")))` and
    /// the other forms `run_cross_module_lowered_dynamic_imports` restores
    /// anywhere in a module.
    fn lowered_import_specifier<'a>(&self, call: &'a CallExpr) -> Option<&'a Expr> {
        if !self.blocking || self.imported_helpers.is_empty() {
            return None;
        }
        LoweredImportMatcher {
            is_wildcard_helper: |callee: &Expr| self.is_imported_helper(callee),
            unresolved_mark: Some(self.unresolved_mark),
        }
        .specifier(call)
    }

    fn calls_imported_helper(&self, call: &CallExpr) -> bool {
        if !self.blocking || self.imported_helpers.is_empty() {
            return false;
        }
        let Callee::Expr(callee) = &call.callee else {
            return false;
        };
        self.is_imported_helper(callee)
            && call.args.first().is_some_and(|first| {
                first.spread.is_none()
                    && matches!(&*first.expr, Expr::Call(inner)
                        if is_kept_require_call(inner, self.unresolved_mark))
            })
    }

    /// Visits `helper(require("./p"), …)` without the require argument
    /// itself, but with what that require holds.
    fn visit_skipping_helper_require(&mut self, call: &CallExpr) {
        call.callee.visit_with(self);
        for (index, arg) in call.args.iter().enumerate() {
            match &*arg.expr {
                Expr::Call(inner) if index == 0 => inner.visit_children_with(self),
                _ => arg.visit_with(self),
            }
        }
    }

    /// Visits `Object.assign(target, require("./p") || {}, …)` without its
    /// source requires.
    fn visit_skipping_composition_requires(&mut self, call: &CallExpr) {
        call.callee.visit_with(self);
        for (index, arg) in call.args.iter().enumerate() {
            match &*arg.expr {
                Expr::Bin(BinExpr {
                    op: BinaryOp::LogicalOr,
                    left,
                    right,
                    ..
                }) if index > 0 => {
                    if let Expr::Call(inner) = &**left {
                        inner.visit_children_with(self);
                    }
                    right.visit_with(self);
                }
                _ => arg.visit_with(self),
            }
        }
    }

    fn in_function<N: VisitWith<Self> + ?Sized>(&mut self, node: &N) {
        self.function_depth += 1;
        node.visit_children_with(self);
        self.function_depth -= 1;
    }

    fn conditional<N: VisitWith<Self> + ?Sized>(&mut self, node: &N) {
        self.conditional_depth += 1;
        node.visit_with(self);
        self.conditional_depth -= 1;
    }
}

impl Visit for KeptRequireFinder {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some(specifier) = self.lowered_import_specifier(call) {
            // Only the specifier stays: the pass replaces the whole call.
            specifier.visit_with(self);
            return;
        }
        if is_kept_require_call(call, self.unresolved_mark) {
            self.found.push(KeptRequire {
                span: call.span,
                position: self.position(),
                source: string_require_source(call),
            });
        }
        call.visit_children_with(self);
    }

    fn visit_module_item(&mut self, item: &ModuleItem) {
        if self.blocking {
            match item {
                ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                    for declarator in &var.decls {
                        declarator.name.visit_with(self);
                        match declarator.init.as_deref().map(strip_parens) {
                            Some(Expr::Call(call)) if self.calls_imported_helper(call) => {
                                self.visit_skipping_helper_require(call);
                            }
                            _ => declarator.init.visit_with(self),
                        }
                    }
                    return;
                }
                ModuleItem::Stmt(Stmt::Expr(statement)) => {
                    if let Expr::Call(call) = strip_parens(&statement.expr) {
                        if is_composition_call(call, self.unresolved_mark) {
                            self.visit_skipping_composition_requires(call);
                            return;
                        }
                    }
                }
                _ => {}
            }
        }
        item.visit_children_with(self);
    }

    fn visit_function(&mut self, function: &Function) {
        self.in_function(function);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.in_function(arrow);
    }

    // Field initializers, static blocks, and computed keys are counted with
    // the methods: a source that requires there wrote it itself.
    fn visit_class(&mut self, class: &Class) {
        self.in_function(class);
    }

    fn visit_if_stmt(&mut self, stmt: &IfStmt) {
        stmt.test.visit_with(self);
        self.conditional(&stmt.cons);
        if let Some(alt) = &stmt.alt {
            self.conditional(alt);
        }
    }

    fn visit_cond_expr(&mut self, expr: &CondExpr) {
        expr.test.visit_with(self);
        self.conditional(&expr.cons);
        self.conditional(&expr.alt);
    }

    fn visit_bin_expr(&mut self, expr: &BinExpr) {
        if matches!(
            expr.op,
            BinaryOp::LogicalAnd | BinaryOp::LogicalOr | BinaryOp::NullishCoalescing
        ) {
            expr.left.visit_with(self);
            self.conditional(&expr.right);
        } else {
            expr.visit_children_with(self);
        }
    }

    fn visit_assign_expr(&mut self, expr: &AssignExpr) {
        if matches!(
            expr.op,
            AssignOp::AndAssign | AssignOp::OrAssign | AssignOp::NullishAssign
        ) {
            expr.left.visit_with(self);
            self.conditional(&expr.right);
        } else {
            expr.visit_children_with(self);
        }
    }

    // A default runs only when the destructured value is undefined.
    fn visit_assign_pat(&mut self, pat: &AssignPat) {
        pat.left.visit_with(self);
        self.conditional(&pat.right);
    }

    fn visit_assign_pat_prop(&mut self, prop: &AssignPatProp) {
        prop.key.visit_with(self);
        if let Some(value) = &prop.value {
            self.conditional(value);
        }
    }

    fn visit_opt_chain_expr(&mut self, expr: &OptChainExpr) {
        match &*expr.base {
            OptChainBase::Member(member) => {
                member.obj.visit_with(self);
                if let MemberProp::Computed(computed) = &member.prop {
                    self.conditional(computed);
                }
            }
            OptChainBase::Call(call) => self.visit_opt_call(call),
        }
    }

    fn visit_opt_call(&mut self, call: &OptCall) {
        call.callee.visit_with(self);
        self.conditional(&call.args);
    }

    fn visit_for_stmt(&mut self, stmt: &ForStmt) {
        stmt.init.visit_with(self);
        self.conditional(&stmt.test);
        self.conditional(&stmt.update);
        self.conditional(&stmt.body);
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        self.conditional(&stmt.left);
        stmt.right.visit_with(self);
        self.conditional(&stmt.body);
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        self.conditional(&stmt.left);
        stmt.right.visit_with(self);
        self.conditional(&stmt.body);
    }

    fn visit_while_stmt(&mut self, stmt: &WhileStmt) {
        self.conditional_depth += 1;
        stmt.visit_children_with(self);
        self.conditional_depth -= 1;
    }

    fn visit_do_while_stmt(&mut self, stmt: &DoWhileStmt) {
        self.conditional_depth += 1;
        stmt.visit_children_with(self);
        self.conditional_depth -= 1;
    }

    fn visit_switch_stmt(&mut self, stmt: &SwitchStmt) {
        stmt.discriminant.visit_with(self);
        self.conditional(&stmt.cases);
    }

    fn visit_try_stmt(&mut self, stmt: &TryStmt) {
        self.conditional_depth += 1;
        stmt.visit_children_with(self);
        self.conditional_depth -= 1;
    }
}

/// `Object.assign(target, require("./p") || {}, …)` with every source in
/// that form.
fn is_composition_call(call: &CallExpr, unresolved_mark: Mark) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let is_assign = matches!(strip_parens(callee), Expr::Member(member)
        if matches!(strip_parens(&member.obj), Expr::Ident(object)
            if is_unresolved_ident(object, "Object", unresolved_mark))
            && member.prop.as_ident().is_some_and(|prop| prop.sym == "assign"));
    is_assign
        && call.args.len() >= 2
        && call.args[1..].iter().all(|arg| {
            arg.spread.is_none()
                && matches!(&*arg.expr, Expr::Bin(BinExpr {
                    op: BinaryOp::LogicalOr,
                    left,
                    right,
                    ..
                }) if matches!(&**left, Expr::Call(inner) if is_kept_require_call(inner, unresolved_mark))
                    && matches!(&**right, Expr::Object(object) if object.props.is_empty()))
        })
}

/// What a kept `require("./p")` returns once `./p` became an ES module, and
/// how a read of it keeps the meaning it had when `./p` was CommonJS.
#[derive(Clone, Copy)]
enum ProviderShape<'a> {
    /// `module.exports = v`, now only `export default v`: the value is the
    /// namespace's `default`. `marked`: the module also marked itself
    /// `__esModule`, so a `.default` read of it already meant that default.
    Default { marked: bool },
    /// `module.exports = v` plus named exports, which are on the namespace,
    /// not on `v`: a call or `new` reads `default`, a named export stays a
    /// namespace read.
    DefaultWithNamed(&'a ModuleFacts),
    /// The module marked itself `__esModule`. Its namespace is its exports
    /// object, and Node adds the `__esModule` marker to the namespace only
    /// when the module has a default export (`require(esm)`, since v22.12
    /// and v20.19), so only then does an interop that tests it still read
    /// the default.
    Marked { has_default: bool },
    /// `module.exports` replaced another way: the value is unknown.
    ReplacedValue,
}

fn provider_shape(provider: &ModuleFacts) -> Option<ProviderShape<'_>> {
    if provider.stays_commonjs {
        return None;
    }
    let has_default = provider
        .exports
        .iter()
        .any(|export| export.kind == ExportKind::Default);
    if provider.commonjs_value_is_default
        && has_default
        && !provider.has_export_all
        && provider.reexports.is_empty()
    {
        return Some(if provider.exports.len() == 1 {
            ProviderShape::Default {
                marked: provider.marks_es_module,
            }
        } else {
            ProviderShape::DefaultWithNamed(provider)
        });
    }
    if provider.marks_es_module {
        Some(ProviderShape::Marked { has_default })
    } else if provider.replaces_module_exports {
        Some(ProviderShape::ReplacedValue)
    } else {
        None
    }
}

fn is_named_export(exports: &[ExportFact], name: &str) -> bool {
    exports
        .iter()
        .any(|export| export.kind == ExportKind::Named && export.exported == *name)
}

/// In a module that stays CommonJS, makes each kept `require("./p")` of a
/// provider recovered as ESM read what it read when the provider was
/// CommonJS, and returns the requires it cannot, with the reason, for the
/// `commonjs_require_of_esm` warning.
///
/// `require` of an ES module returns its namespace. A provider that was
/// `module.exports = v` has `v` as the namespace's `default`: the call reads
/// `.default` when it is the only export, and otherwise each call, `new`, or
/// read of a name that is not a named export goes through `.default`,
/// directly or through a binding every use of which is one of those. A
/// provider that marked itself `__esModule` has lost the marker, so a local
/// interop helper around the call, which returned it unchanged, is dropped.
pub(crate) fn run_kept_require_default_reads(
    module: &mut Module,
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
    unresolved_mark: Mark,
) -> Vec<(Atom, &'static str)> {
    if module
        .body
        .iter()
        .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
    {
        return Vec::new();
    }
    let shape_of = |source: &str| {
        module_facts
            .get_from(current_filename, source)
            .and_then(provider_shape)
    };
    let interop_sources = current_filename
        .and_then(|filename| module_facts.get(filename))
        .map_or(&[][..], |own| own.interop_require_sources.as_slice());

    // Bindings of a `DefaultWithNamed` provider, declared once, whose every
    // use is a call, a `new`, or a static member read (see
    // `BindingUseCheck`).
    let mut bindings = BindingCollector {
        unresolved_mark,
        shape_of: &shape_of,
        candidates: HashMap::default(),
        declarations: HashMap::default(),
    };
    module.visit_with(&mut bindings);
    let mut uses = BindingUseCheck {
        rejected: bindings
            .declarations
            .iter()
            .filter(|(id, count)| **count > 1 && bindings.candidates.contains_key(*id))
            .map(|(id, _)| id.clone())
            .collect(),
        candidates: bindings.candidates,
    };
    module.visit_with(&mut uses);
    let good: HashMap<BindingId, Atom> = uses
        .candidates
        .into_iter()
        .filter(|(id, _)| !uses.rejected.contains(id))
        .map(|(id, (source, _))| (id, source))
        .collect();

    let mut rewriter = DefaultReadRewriter {
        unresolved_mark,
        shape_of: &shape_of,
        good: &good,
        local_helpers: LocalHelperContext::collect_with_mark(module, unresolved_mark),
        interop_sources,
        esmodule_tested_sources: esmodule_tested_sources(module, unresolved_mark),
        unhandled: Vec::new(),
    };
    module.visit_mut_with(&mut rewriter);
    rewriter.unhandled
}

/// Sources whose required value `module` reads `__esModule` of, as
/// `require("./p").__esModule` or `x.__esModule` for a binding
/// `var x = require("./p")`.
fn esmodule_tested_sources(module: &Module, unresolved_mark: Mark) -> HashSet<Atom> {
    struct Bindings {
        unresolved_mark: Mark,
        sources: HashMap<BindingId, Atom>,
    }
    impl Visit for Bindings {
        fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
            if let (Pat::Ident(binding), Some(init)) = (&declarator.name, &declarator.init) {
                if let Some(source) = require_source(init, self.unresolved_mark) {
                    self.sources.insert(binding_id(&binding.id), source);
                }
            }
            declarator.visit_children_with(self);
        }
    }
    struct Tests<'a> {
        unresolved_mark: Mark,
        bindings: &'a HashMap<BindingId, Atom>,
        tested: HashSet<Atom>,
    }
    impl Visit for Tests<'_> {
        fn visit_member_expr(&mut self, member: &MemberExpr) {
            if member
                .prop
                .as_ident()
                .is_some_and(|prop| prop.sym == "__esModule")
            {
                let source = match strip_parens(&member.obj) {
                    Expr::Ident(id) => self.bindings.get(&binding_id(id)).cloned(),
                    object => require_source(object, self.unresolved_mark),
                };
                self.tested.extend(source);
            }
            member.visit_children_with(self);
        }
    }
    let mut bindings = Bindings {
        unresolved_mark,
        sources: HashMap::default(),
    };
    module.visit_with(&mut bindings);
    let mut tests = Tests {
        unresolved_mark,
        bindings: &bindings.sources,
        tested: HashSet::default(),
    };
    module.visit_with(&mut tests);
    tests.tested
}

fn require_source(expr: &Expr, unresolved_mark: Mark) -> Option<Atom> {
    match strip_parens(expr) {
        Expr::Call(call) if is_kept_require_call(call, unresolved_mark) => {
            string_require_source(call)
        }
        _ => None,
    }
}

fn default_member(object: Expr) -> Expr {
    Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: Box::new(object),
        prop: MemberProp::Ident(IdentName::new("default".into(), DUMMY_SP)),
    })
}

fn take_expr(expr: &mut Expr) -> Expr {
    std::mem::replace(expr, Expr::Invalid(Invalid { span: DUMMY_SP }))
}

struct BindingCollector<'a, F> {
    unresolved_mark: Mark,
    shape_of: &'a F,
    /// Each candidate's source and the provider's exports that read `this`.
    candidates: HashMap<BindingId, (Atom, &'a [Atom])>,
    /// How often each binding is declared or assigned. A second `var` of
    /// the same name writes it like an assignment does.
    declarations: HashMap<BindingId, usize>,
}

impl<'a, F: Fn(&str) -> Option<ProviderShape<'a>>> Visit for BindingCollector<'a, F> {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Pat::Ident(binding), Some(init)) = (&declarator.name, &declarator.init) {
            if let Some(source) = require_source(init, self.unresolved_mark) {
                if let Some(ProviderShape::DefaultWithNamed(provider)) = (self.shape_of)(&source) {
                    self.candidates.insert(
                        binding_id(&binding.id),
                        (source, provider.receiver_observing_exports.as_slice()),
                    );
                }
            }
        }
        declarator.visit_children_with(self);
    }

    fn visit_binding_ident(&mut self, binding: &BindingIdent) {
        *self
            .declarations
            .entry(binding_id(&binding.id))
            .or_default() += 1;
    }
}

/// Rejects a candidate binding with any use but a call, a `new`, or a
/// static member read. A member write, an optional member, and a call of a
/// named export that reads `this` are rejected too: the rewrite leaves them
/// on the namespace, which is no longer the value they wrote to or passed as
/// the receiver.
struct BindingUseCheck<'a> {
    candidates: HashMap<BindingId, (Atom, &'a [Atom])>,
    rejected: HashSet<BindingId>,
}

impl BindingUseCheck<'_> {
    fn reject_member_object(&mut self, member: &MemberExpr) -> bool {
        match self.candidate(&member.obj) {
            Some(id) => {
                self.rejected.insert(id);
                true
            }
            None => false,
        }
    }

    fn candidate(&self, expr: &Expr) -> Option<BindingId> {
        match strip_parens(expr) {
            Expr::Ident(id) => {
                let id = binding_id(id);
                self.candidates.contains_key(&id).then_some(id)
            }
            _ => None,
        }
    }
}

impl Visit for BindingUseCheck<'_> {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        // The binding itself is no use. A second declaration of it was
        // counted by `BindingCollector`.
        declarator.init.visit_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if self.candidate(callee).is_some() {
                call.args.visit_with(self);
                return;
            }
            // `x.name()` passes the namespace as the receiver of a named
            // export, where it passed the CommonJS value before.
            if let Expr::Member(member) = strip_parens(callee) {
                if let (Some(id), Some(name)) =
                    (self.candidate(&member.obj), member.prop.as_ident())
                {
                    if self.candidates[&id].1.contains(&name.sym) {
                        self.rejected.insert(id);
                    }
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_simple_assign_target(&mut self, target: &SimpleAssignTarget) {
        if let SimpleAssignTarget::Member(member) = target {
            if self.reject_member_object(member) {
                return;
            }
        }
        target.visit_children_with(self);
    }

    fn visit_opt_chain_base(&mut self, base: &OptChainBase) {
        if let OptChainBase::Member(member) = base {
            if self.reject_member_object(member) {
                return;
            }
        }
        base.visit_children_with(self);
    }

    fn visit_new_expr(&mut self, new: &NewExpr) {
        if self.candidate(&new.callee).is_some() {
            new.args.visit_with(self);
            return;
        }
        new.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if self.candidate(&member.obj).is_some() && member.prop.as_ident().is_some() {
            return;
        }
        member.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        let id = binding_id(ident);
        if self.candidates.contains_key(&id) {
            self.rejected.insert(id);
        }
    }
}

struct DefaultReadRewriter<'a, F> {
    unresolved_mark: Mark,
    shape_of: &'a F,
    good: &'a HashMap<BindingId, Atom>,
    local_helpers: LocalHelperContext,
    interop_sources: &'a [Atom],
    /// Sources whose required value the module still tests for
    /// `__esModule`, directly or through a binding.
    esmodule_tested_sources: HashSet<Atom>,
    unhandled: Vec<(Atom, &'static str)>,
}

impl<'a, F: Fn(&str) -> Option<ProviderShape<'a>>> DefaultReadRewriter<'a, F> {
    fn report(&mut self, source: Atom, reason: &'static str) {
        if !self.unhandled.iter().any(|(seen, _)| *seen == source) {
            self.unhandled.push((source, reason));
        }
    }

    fn good_binding(&self, expr: &Expr) -> Option<&'a Atom> {
        match strip_parens(expr) {
            Expr::Ident(id) => self.good.get(&binding_id(id)),
            _ => None,
        }
    }

    fn named_exports(&self, source: &str) -> Option<&'a [ExportFact]> {
        match (self.shape_of)(source) {
            Some(ProviderShape::DefaultWithNamed(provider)) => Some(&provider.exports),
            _ => None,
        }
    }

    /// A provider recovered with a default export that also marked itself
    /// `__esModule` (a Babel runtime helper's CommonJS shim sets
    /// `module.exports.default = module.exports` beside the marker).
    fn marked_default_provider(&self, source: &str) -> bool {
        match (self.shape_of)(source) {
            Some(ProviderShape::Default { marked }) => marked,
            Some(ProviderShape::DefaultWithNamed(provider)) => provider.marks_es_module,
            _ => false,
        }
    }

    /// `require("./p").name(…)` where the named export `name` reads `this`:
    /// the call passes the namespace as the receiver, where it passed the
    /// CommonJS value before.
    fn calls_receiver_observing_export(&self, callee: &Expr) -> Option<Atom> {
        let Expr::Member(member) = strip_parens(callee) else {
            return None;
        };
        let source = require_source(&member.obj, self.unresolved_mark)?;
        let name = member.prop.as_ident()?;
        match (self.shape_of)(&source) {
            Some(ProviderShape::DefaultWithNamed(provider))
                if provider.receiver_observing_exports.contains(&name.sym) =>
            {
                Some(source)
            }
            _ => None,
        }
    }

    /// A kept require in a position no rule above rewrote.
    fn other_use(&mut self, source: Atom) {
        match (self.shape_of)(&source) {
            Some(ProviderShape::DefaultWithNamed(_)) => self.report(
                source,
                "the value it assigned to `module.exports` is now the namespace's `default`, beside its named exports, so a read of that value gets the namespace",
            ),
            Some(ProviderShape::Marked { has_default: false })
                if self.interop_sources.contains(&source) =>
            {
                self.report(
                    source,
                    "it has no default export, so its namespace has no `__esModule` marker and an interop that reads its default gets the namespace",
                )
            }
            // Node marks the namespace of a module with a default export, so
            // an interop that still tests `__esModule` reads the default.
            Some(ProviderShape::Marked { has_default: true })
                if self.interop_sources.contains(&source)
                    && !self.esmodule_tested_sources.contains(&source) =>
            {
                self.report(
                    source,
                    "the interop that read its default was removed, so a read of the required value gets the namespace instead of the default",
                )
            }
            Some(ProviderShape::ReplacedValue) => self.report(
                source,
                "the value it assigned to `module.exports` may not be the namespace's `default`, so a read of that value gets the namespace",
            ),
            _ => {}
        }
    }
}

impl<'a, F: Fn(&str) -> Option<ProviderShape<'a>>> VisitMut for DefaultReadRewriter<'a, F> {
    fn visit_mut_var_declarator(&mut self, declarator: &mut VarDeclarator) {
        // A good binding keeps its init; its uses are rewritten instead.
        if let (Pat::Ident(binding), Some(_)) = (&declarator.name, &declarator.init) {
            if self.good.contains_key(&binding_id(&binding.id)) {
                return;
            }
        }
        declarator.visit_mut_children_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        // `require("./p")` alone: the default-only provider's value.
        if let Some(source) = require_source(expr, self.unresolved_mark) {
            if matches!(
                (self.shape_of)(&source),
                Some(ProviderShape::Default { .. })
            ) {
                *expr = default_member(take_expr(expr));
            } else {
                self.other_use(source);
            }
            return;
        }
        match expr {
            // `H(require("./p"))` of a marked provider: the helper returned it
            // unchanged.
            Expr::Call(call)
                if matches!(&call.callee, Callee::Expr(callee)
                    if self.local_helpers.is_helper_callee(callee, TranspilerHelperKind::InteropRequireDefault)
                        || self.local_helpers.is_helper_callee(callee, TranspilerHelperKind::InteropRequireWildcard))
                    && call.args.len() == 1
                    && call.args[0].spread.is_none() =>
            {
                if let Some(source) = require_source(&call.args[0].expr, self.unresolved_mark) {
                    if matches!((self.shape_of)(&source), Some(ProviderShape::Marked { .. })) {
                        *expr = take_expr(&mut call.args[0].expr);
                        return;
                    }
                }
                expr.visit_mut_children_with(self);
            }
            // `require("./p")(…)` or `x(…)`.
            Expr::Call(call) => {
                if let Callee::Expr(callee) = &mut call.callee {
                    if self.rewrite_callee(callee) {
                        call.args.visit_mut_with(self);
                        return;
                    }
                    if let Some(source) = self.calls_receiver_observing_export(callee) {
                        self.report(
                            source,
                            "a call of its named export passes the namespace as `this`, where it passed the value it assigned to `module.exports`",
                        );
                    }
                }
                expr.visit_mut_children_with(self);
            }
            Expr::New(new) => {
                if self.rewrite_callee(&mut new.callee) {
                    new.args.visit_mut_with(self);
                    return;
                }
                expr.visit_mut_children_with(self);
            }
            // `require("./p").name` or `x.name`.
            Expr::Member(member) => {
                let name = member.prop.as_ident().map(|prop| prop.sym.clone());
                let source = require_source(&member.obj, self.unresolved_mark)
                    .or_else(|| self.good_binding(&member.obj).cloned());
                // A marked provider's `.default` meant its ES default, which
                // is the namespace's `default` too.
                if let (Some(source), Some("default")) = (&source, name.as_deref()) {
                    if self.marked_default_provider(source) {
                        return;
                    }
                }
                if let (Some(source), Some(name)) = (source, name) {
                    if let Some(exports) = self.named_exports(&source) {
                        if !is_named_export(exports, &name) {
                            *member.obj = default_member(take_expr(&mut member.obj));
                        }
                        return;
                    }
                }
                expr.visit_mut_children_with(self);
            }
            _ => expr.visit_mut_children_with(self),
        }
    }
}

impl<'a, F: Fn(&str) -> Option<ProviderShape<'a>>> DefaultReadRewriter<'a, F> {
    /// Reads `.default` of a called or constructed `require("./p")` or good
    /// binding of a `DefaultWithNamed` provider.
    fn rewrite_callee(&mut self, callee: &mut Box<Expr>) -> bool {
        let source = require_source(callee, self.unresolved_mark)
            .or_else(|| self.good_binding(callee).cloned());
        let Some(source) = source else {
            return false;
        };
        if self.named_exports(&source).is_none() {
            return false;
        }
        **callee = default_member(take_expr(callee));
        true
    }
}

/// Evidence in a module, before any rule runs, that its source was an ES
/// module, as labels for a diagnostic.
///
/// Module level: webpack's `require.r(exports)` and `require.d(exports, …)`,
/// an `__esModule` definition, assignment, or read (not an object-literal
/// key: babel-runtime 6 shims write `module.exports = { default: require(x),
/// __esModule: true }` by hand), and a getter definition on `exports` (swc,
/// tsc). Require level: `require(x).default`, `(0, require(x).y)(…)`, and
/// `h._(require(x))` (swc's interop helpers). `(0, b.y)(…)` through a
/// variable bound to a require is not evidence: Terser writes it when it
/// inlines a member alias in CommonJS source.
pub(crate) fn es_source_evidence(module: &Module, unresolved_mark: Mark) -> Vec<&'static str> {
    struct Collector {
        unresolved_mark: Mark,
        found: Vec<&'static str>,
    }

    impl Collector {
        fn add(&mut self, label: &'static str) {
            if !self.found.contains(&label) {
                self.found.push(label);
            }
        }

        fn is_free(&self, expr: &Expr, name: &str) -> bool {
            matches!(strip_parens(expr), Expr::Ident(id)
                if is_unresolved_ident(id, name, self.unresolved_mark))
        }

        fn is_require(&self, expr: &Expr) -> bool {
            matches!(strip_parens(expr), Expr::Call(call)
                if is_kept_require_call(call, self.unresolved_mark))
        }
    }

    impl Visit for Collector {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                let first_is_exports = call
                    .args
                    .first()
                    .is_some_and(|arg| self.is_free(&arg.expr, "exports"));
                if let Expr::Member(member) = strip_parens(callee) {
                    let prop = member.prop.as_ident().map(|prop| prop.sym.as_ref());
                    if self.is_free(&member.obj, "require") && first_is_exports {
                        match prop {
                            Some("r") => self.add("require.r"),
                            Some("d") => self.add("require.d"),
                            _ => {}
                        }
                    }
                    if self.is_free(&member.obj, "Object")
                        && prop == Some("defineProperty")
                        && first_is_exports
                    {
                        match call.args.get(1).map(|arg| strip_parens(&arg.expr)) {
                            Some(Expr::Lit(Lit::Str(key))) if key.value == *"__esModule" => {
                                self.add("__esModule")
                            }
                            _ if matches!(call.args.get(2).map(|arg| strip_parens(&arg.expr)),
                            Some(Expr::Object(descriptor))
                                if descriptor.props.iter().any(|prop| match prop {
                                    PropOrSpread::Prop(prop) => matches!(&**prop,
                                        Prop::KeyValue(KeyValueProp { key, .. })
                                            if matches!(key, PropName::Ident(name) if name.sym == "get"))
                                        || matches!(&**prop, Prop::Getter(_))
                                        || matches!(&**prop, Prop::Method(method)
                                            if matches!(&method.key, PropName::Ident(name) if name.sym == "get")),
                                    PropOrSpread::Spread(_) => false,
                                })) =>
                            {
                                self.add("export getter")
                            }
                            _ => {}
                        }
                    }
                    if prop == Some("_")
                        && call
                            .args
                            .first()
                            .is_some_and(|arg| self.is_require(&arg.expr))
                    {
                        self.add("_(require())");
                    }
                }
                if let Expr::Seq(seq) = strip_parens(callee) {
                    if let [zero, Expr::Member(member)] =
                        seq.exprs.iter().map(|e| &**e).collect::<Vec<_>>()[..]
                    {
                        if matches!(zero, Expr::Lit(Lit::Num(n)) if n.value == 0.0)
                            && self.is_require(&member.obj)
                        {
                            self.add("(0, require().x)()");
                        }
                    }
                }
            }
            call.visit_children_with(self);
        }

        fn visit_member_expr(&mut self, member: &MemberExpr) {
            match member.prop.as_ident().map(|prop| prop.sym.as_ref()) {
                Some("default") if self.is_require(&member.obj) => self.add("require().default"),
                Some("__esModule") => self.add("__esModule"),
                _ => {}
            }
            member.visit_children_with(self);
        }
    }

    let mut collector = Collector {
        unresolved_mark,
        found: Vec::new(),
    };
    module.visit_with(&mut collector);
    collector.found
}

/// The `es_source_kept_commonjs` message for an output `module` that stayed
/// CommonJS while its input showed ES-source `evidence` and it keeps a
/// require that runs on every evaluation of the module body. `None` when it
/// does not apply: a module whose kept requires all sit in a function or a
/// conditional position was written that way by its source.
pub(crate) fn es_source_kept_commonjs_message(
    module: &Module,
    unresolved_mark: Mark,
    evidence: &[&str],
) -> Option<String> {
    if evidence.is_empty()
        || module
            .body
            .iter()
            .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
    {
        return None;
    }
    let calls = kept_require_calls(module, unresolved_mark);
    if !calls
        .iter()
        .any(|call| call.position == RequirePosition::TopLevel)
    {
        return None;
    }
    let requires = calls
        .iter()
        .map(|call| match &call.source {
            Some(source) => format!("\"{source}\" ({})", call.position.as_str()),
            None => format!("<dynamic> ({})", call.position.as_str()),
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "module stayed CommonJS although its source looks like an ES module ({}); kept requires: {requires}",
        evidence.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, FileName, SourceMap, GLOBALS};
    use swc_core::ecma::parser::{parse_file_as_module, EsSyntax, Syntax};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::VisitMutWith;

    use super::*;

    fn with_module<T>(source: &str, run: impl FnOnce(&Module, Mark) -> T) -> T {
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
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
            run(&module, unresolved_mark)
        })
    }

    fn positions(source: &str) -> Vec<RequirePosition> {
        with_module(source, |module, mark| {
            kept_require_calls(module, mark)
                .into_iter()
                .map(|call| call.position)
                .collect()
        })
    }

    fn blocking(source: &str) -> usize {
        with_module(source, |module, mark| {
            blocking_require_calls(module, mark, true).len()
        })
    }

    use RequirePosition::{Conditional, Function, TopLevel};

    #[test]
    fn classifies_where_a_require_runs() {
        assert_eq!(
            positions(r#"require("a").b(); f(require("c"));"#),
            [TopLevel, TopLevel]
        );
        assert_eq!(
            positions(r#"function f() { require("a"); } var g = () => require("b");"#),
            [Function, Function]
        );
        assert_eq!(
            positions(r#"(function () { require("a"); })(); class C { m() { require("b"); } }"#),
            [Function, Function]
        );
        assert_eq!(
            positions(
                r#"var o = { get x() { return require("a"); }, set x(v) { require("b"); } };"#
            ),
            [Function, Function]
        );
        assert_eq!(
            positions(
                r#"if (require("t")) require("a"); else require("b");
var p = window.P ? window.P : require("c");
var q = window.Q || require("d");
o?.f(require("e"));"#
            ),
            [
                TopLevel,
                Conditional,
                Conditional,
                Conditional,
                Conditional,
                Conditional
            ]
        );
        assert_eq!(
            positions(
                r#"for (var i = require("a"); i; i++) require("b");
for (var k of require("c")) require("d");
switch (require("e")) { case 1: require("f"); }
try { require("g"); } catch (e) {}"#
            ),
            [
                TopLevel,
                Conditional,
                TopLevel,
                Conditional,
                TopLevel,
                Conditional,
                Conditional
            ]
        );
    }

    #[test]
    fn skips_numeric_ids_and_other_requires() {
        assert_eq!(
            positions(r#"require(123); require.e(1); r.bind(r, 2);"#),
            []
        );
        assert_eq!(
            positions(r#"require(name); require(`./${x}.json`);"#),
            [TopLevel, TopLevel]
        );
        assert_eq!(positions(r#"var require = load; require("a");"#), []);
    }

    fn evidence(source: &str) -> Vec<&'static str> {
        with_module(source, es_source_evidence)
    }

    #[test]
    fn reads_es_source_evidence() {
        assert_eq!(
            evidence(r#"require.r(exports); require.d(exports, { a: () => a });"#),
            ["require.r", "require.d"]
        );
        assert_eq!(
            evidence(r#"Object.defineProperty(exports, "__esModule", { value: true });"#),
            ["__esModule"]
        );
        assert_eq!(evidence(r#"exports.__esModule = true;"#), ["__esModule"]);
        assert_eq!(
            evidence(
                r#"Object.defineProperty(exports, "a", { enumerable: !0, get: function () { return a; } });"#
            ),
            ["export getter"]
        );
        assert_eq!(
            evidence(r#"var n = require("./a").default;"#),
            ["require().default"]
        );
        assert_eq!(
            evidence(r#"(0, require("./a").Z)(1);"#),
            ["(0, require().x)()"]
        );
        assert_eq!(
            evidence(r#"var n = require("./h")._(require("./a"));"#),
            ["_(require())"]
        );
    }

    #[test]
    fn plain_commonjs_shows_no_es_source_evidence() {
        // wild-observed: Terser output of CommonJS source after it inlined a
        // member alias.
        assert!(evidence(r#"var i = require("./a"); (0, i.defineMethod)("x", f);"#).is_empty());
        // babel-runtime 6 shim, written by hand.
        assert!(
            evidence(r#"module.exports = { default: require("./a"), __esModule: true };"#)
                .is_empty()
        );
        assert!(
            evidence(r#"var t = require("./a")("toStringTag"); module.exports = t;"#).is_empty()
        );
        assert!(evidence(r#"Object.defineProperty(exports, "a", { value: 1 });"#).is_empty());
    }

    #[test]
    fn only_es_module_output_reports_a_require() {
        with_module(r#"exports.a = require("./a")();"#, |module, mark| {
            assert!(esm_output_require_calls(module, mark).is_empty());
        });
        with_module(r#"export const a = require("./a")();"#, |module, mark| {
            let calls = esm_output_require_calls(module, mark);
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].source.as_deref(), Some("./a"));
        });
    }

    #[test]
    fn imported_helper_and_composition_sources_do_not_block() {
        assert_eq!(
            blocking(
                r#"import h from "./h"; var a = h(require("./a")); var b = h.x(require("./b"));"#
            ),
            0
        );
        assert_eq!(
            blocking(
                r#"export default {}; Object.assign(o, require("./a") || {}, require("./b") || {});"#
            ),
            0
        );
        // A local helper is no import, and a composition needs every source
        // in its `|| {}` form.
        assert_eq!(
            blocking(
                r#"import h from "./h"; function g(x) { return x; } var a = g(require("./a"));"#
            ),
            1
        );
        assert_eq!(
            blocking(
                r#"export default {}; Object.assign(o, require("./a"), require("./b") || {});"#
            ),
            2
        );
    }

    #[test]
    fn helper_and_composition_requires_block_outside_the_forms_later_stages_read() {
        // The cross-module pass reads only a top-level `var` initializer, and
        // the composition pass only a top-level statement.
        assert_eq!(
            blocking(r#"import h from "./h"; export const x = h.wrap(require("./a"));"#),
            1
        );
        assert_eq!(
            blocking(r#"import h from "./h"; exports.x = h(require("./a"));"#),
            1
        );
        assert_eq!(
            blocking(r#"import h from "./h"; export function f() { return h(require("./a")); }"#),
            1
        );
        assert_eq!(
            blocking(
                r#"export default {}; function f() { Object.assign(o, require("./a") || {}); }"#
            ),
            1
        );
        // The lowered `import()` pass reads its form in any position.
        assert_eq!(
            blocking(
                r#"import * as t from "./t"; export function f() { return Promise.resolve().then(() => t.__importStar(require("./a"))); }"#
            ),
            0
        );
        // A single file has no later stage.
        with_module(
            r#"import h from "./h"; var a = h(require("./a")); export default {}; Object.assign(o, require("./b") || {});"#,
            |module, mark| assert_eq!(blocking_require_calls(module, mark, false).len(), 2),
        );
    }
}
