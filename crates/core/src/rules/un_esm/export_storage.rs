//! Per-name storage model of CommonJS exports.
//!
//! A producer that compiles ESM to CommonJS decides where each export's value
//! lives, and its `exports.X = ...` statements follow from that decision. This
//! module inventories every access to each `exports` property in a resolved
//! module and decides, per export name, which storage model the accesses are
//! consistent with:
//!
//! - [`ExportStorage::Getter`]: a getter definition exposes a binding, and
//!   nothing else writes the property.
//! - [`ExportStorage::Mirror`]: a module-level binding is the storage, and
//!   every write of the property copies it. Every write of the binding is
//!   mirrored in the same statement or the next one, where only other mirror
//!   statements may come in between. A property that the module never reads
//!   may instead be one final copy after every write of the binding.
//! - [`ExportStorage::Property`]: the property itself is the storage.
//! - [`ExportStorage::Unrecovered`]: no model fits; the name keeps its
//!   CommonJS accesses.
//!
//! See `docs/proposals/cjs-export-storage.md` for the producer shapes and the
//! reasoning behind each condition. [`analyze_export_storage`] only reports
//! decisions (it also backs `wakaru debug cjs-exports`);
//! [`recover_export_storage`] rewrites the property, mirror, and getter names
//! that [`storage_candidates`] selects. A getter of an imported member or a
//! `require` result, and the names left to the statement path, are rewritten
//! by `UnEsm`'s statement classification.

use crate::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::util::take::Take;
use swc_core::common::DUMMY_SP;
use swc_core::common::{Mark, Span, Spanned};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, AssignTarget, AutoAccessor, BinExpr, BinaryOp, BindingIdent,
    CallExpr, Callee, Class, ClassDecl, ClassProp, Constructor, Decl, ExportDecl,
    ExportNamedSpecifier, ExportSpecifier, Expr, ExprOrSpread, FnDecl, ForHead, ForInStmt,
    ForOfStmt, Function, Ident, MemberExpr, MemberProp, Module, ModuleDecl, ModuleExportName,
    ModuleItem, NamedExport, OptCall, OptChainBase, Pat, PrivateProp, SimpleAssignTarget,
    StaticBlock, Stmt, TaggedTpl, ThisExpr, UnaryExpr, UnaryOp, UpdateExpr, VarDecl, VarDeclKind,
    VarDeclarator,
};
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::js_names::{is_reserved_binding_name, is_valid_identifier_name};
use crate::rules::decl_utils::{collect_decl_names, fresh_binding_ident};

use crate::analysis::BindingId;
use crate::rules::cocos_rf::framed_cc_rf_push_calls;
use crate::rules::constructor_sensitivity::static_member_name;
use crate::rules::eval_utils::{module_has_with_stmt, DirectEvalPresence};
use crate::rules::un_enum::is_enum_iife_callee;
use crate::utils::paren::strip_parens;
use crate::utils::prototype_members::is_prototype_mutating_member_name;

use super::export_star::export_star_statement_indices;
use super::{
    extract_define_property_getter_expr, extract_export_getter_map,
    extract_getter_expr_return_expr, fresh_prefixed_name, function_observes_receiver,
    is_cjs_export_object_expr, is_esmodule_descriptor, is_esmodule_name_arg,
    is_object_define_property_global_call, is_unresolved_ident, is_unresolved_member_expr,
    is_void_or_undefined, leading_export_sentinels, make_name_ident, make_str,
};

/// The storage model chosen for one export name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportStorage {
    Getter,
    Mirror,
    Property,
    Unrecovered,
}

impl ExportStorage {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Getter => "getter",
            Self::Mirror => "mirror",
            Self::Property => "property",
            Self::Unrecovered => "unrecovered",
        }
    }
}

/// Why a storage model does not fit a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rejection {
    pub(crate) storage: ExportStorage,
    pub(crate) message: String,
    pub(crate) span: Option<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExportStorageDecision {
    pub(crate) name: Atom,
    pub(crate) storage: ExportStorage,
    /// The binding a getter returns or a mirror copies, as written.
    pub(crate) binding: Option<String>,
    /// Earlier models that were tried and rejected, in decision order. For an
    /// unrecovered name, the last entry is the reason no model fits.
    pub(crate) rejected: Vec<Rejection>,
    pub(crate) accesses: AccessCounts,
    /// What the mirror rewrite needs beyond the report.
    pub(crate) mirror: Option<MirrorFacts>,
    /// For a getter that returns a module-level binding holding a value the
    /// module computes itself (not a `require` result or an import), that
    /// binding. The storage rewrite exports it live.
    pub(crate) getter: Option<BindingId>,
    /// The name is initialized as the argument of an enum IIFE that `UnEnum`
    /// can fold into the export.
    pub(crate) enum_initializer: bool,
}

/// Facts about a mirror name that decide whether its local can replace the
/// property everywhere (see [`recover_export_storage`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MirrorFacts {
    pub(crate) binding: BindingId,
    /// The module-body item that declares the binding.
    pub(crate) declaration_index: usize,
    /// `let`, `const`, or `class`: reading it before its declaration throws.
    pub(crate) lexical: bool,
    /// The earliest module-body item with a read or call of the property
    /// outside a function.
    pub(crate) first_eager_access: Option<usize>,
    /// TypeScript's exported enum or namespace initializer,
    /// `L = exports.x || (exports.x = {})` or `L || (exports.x = L = {})`,
    /// which `UnEnum` folds later.
    pub(crate) enum_initializer: bool,
    /// Reads that are the `exports.x` operand of `L = exports.x || (…)`.
    /// They run in the scope of that write of `L`, so no other binding named
    /// `L` can shadow it there.
    pub(crate) initializer_reads: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AccessCounts {
    /// Plain `=` writes, excluding leading `void 0` sentinels.
    pub(crate) writes: usize,
    pub(crate) sentinels: usize,
    /// Compound, logical, update, and pattern-target writes.
    pub(crate) other_writes: usize,
    pub(crate) reads: usize,
    pub(crate) calls: usize,
    pub(crate) getters: usize,
    /// Accesses inside a function, arrow, or class body.
    pub(crate) deferred: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExportStorageReport {
    /// The module never refers to the CommonJS `exports` object.
    NoCommonJsExports,
    /// A module-level condition failed, so no name can be classified.
    ModuleGate {
        message: String,
        span: Option<Span>,
        /// Statement-by-statement recovery cannot convert part of this
        /// module safely, so the whole module stays CommonJS. Either an
        /// `exports` property is written inside top-level control flow,
        /// where that recovery cannot reach it, or the `exports` binding is
        /// reassigned or aliased, so a static `exports.X` access no longer
        /// proves which object it touches.
        keep_commonjs: bool,
    },
    Names(Vec<ExportStorageDecision>),
}

/// Classify every static export name of a resolved module.
///
/// Property-storage recovery may mix static `exports.name` and
/// `module.exports.name` roots only after the module gate proves that neither
/// receiver can be rebound, replaced, aliased, or observed dynamically
/// (`commonjs_exports_data_properties` in docs/rewrite-assumptions.md).
pub(crate) fn analyze_export_storage(
    module: &Module,
    unresolved_mark: Mark,
) -> ExportStorageReport {
    if let Some(span) = top_level_this(module, unresolved_mark) {
        return ExportStorageReport::ModuleGate {
            message: "top-level `this` is `module.exports`".to_string(),
            span: Some(span),
            keep_commonjs: true,
        };
    }
    let mut inventory = Inventory::new(unresolved_mark);
    inventory.export_star_statements = export_star_statement_indices(module, unresolved_mark);
    inventory.cc_rf_push_calls = framed_cc_rf_push_calls(&module.body, unresolved_mark);
    module.visit_with(&mut inventory);
    if !inventory.saw_exports {
        return ExportStorageReport::NoCommonJsExports;
    }
    let keep_commonjs = inventory.nested_write || inventory.binding_escape;
    if let Some((message, span)) = inventory.gate {
        return ExportStorageReport::ModuleGate {
            message,
            span: Some(span),
            keep_commonjs,
        };
    }
    if module_has_with_stmt(module) {
        return ExportStorageReport::ModuleGate {
            message: "the module contains a `with` statement".to_string(),
            span: None,
            keep_commonjs,
        };
    }
    let mut direct_eval = DirectEvalPresence::default();
    module.visit_with(&mut direct_eval);
    if direct_eval.found {
        return ExportStorageReport::ModuleGate {
            message: "the module contains a direct `eval` call".to_string(),
            span: None,
            keep_commonjs,
        };
    }

    let declarations = collect_module_declarations(module, unresolved_mark);
    let decisions = inventory
        .order
        .iter()
        // The interop marker is not an export.
        .filter(|name| name.as_ref() != "__esModule")
        .map(|name| classify(name, &inventory.names[name], &inventory, &declarations))
        .collect();
    ExportStorageReport::Names(decisions)
}

// ============================================================
// Inventory
// ============================================================

/// A statement position: the statement list (module body, block, function
/// body, switch case) and the index in it. Function and class bodies start a
/// fresh list, so an expression-bodied arrow never shares a position with the
/// statement that creates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct StmtPos {
    list: usize,
    index: usize,
}

const MODULE_LIST: usize = 0;

#[derive(Debug, Clone, Copy)]
struct Site {
    span: Span,
    stmt: StmtPos,
    /// Index of the enclosing module-body item.
    module_index: usize,
    /// Visit order, which follows evaluation order within a statement: an
    /// assignment is recorded after its right-hand side.
    seq: usize,
    deferred: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MirrorEvidence {
    /// The written value reads the binding after any write to it in the same
    /// expression: `exports.x = x`, `exports.x = x = v`, `exports.x = ++x`.
    Value,
    /// The write is part of a chain that also initializes or assigns the
    /// binding with the same value: `var x = exports.x = v`,
    /// `x = exports.x = v`.
    Chain,
}

#[derive(Debug, Clone)]
struct PropertyWrite {
    site: Site,
    /// The write runs exactly once when the module body runs: it is the
    /// expression of a top-level statement or the initializer of a top-level
    /// declarator, an element of a top-level sequence, or a link of such an
    /// assignment chain.
    unconditional: bool,
    mirror: Option<(BindingId, MirrorEvidence)>,
    is_void: bool,
    /// The value is a function literal that reads `this`.
    observes_receiver: bool,
}

#[derive(Debug, Clone)]
enum GetterTarget {
    Binding(Ident),
    Member(Ident, Atom),
    Other,
}

#[derive(Debug, Default)]
struct NameFacts {
    plain_writes: Vec<PropertyWrite>,
    other_writes: Vec<(&'static str, Site)>,
    reads: Vec<Site>,
    calls: Vec<Site>,
    getters: Vec<(GetterTarget, Site)>,
    enum_initializer: bool,
    initializer_reads: usize,
}

impl NameFacts {
    fn all_sites(&self) -> impl Iterator<Item = &Site> {
        self.plain_writes
            .iter()
            .map(|write| &write.site)
            .chain(self.other_writes.iter().map(|(_, site)| site))
            .chain(self.reads.iter())
            .chain(self.calls.iter())
            .chain(self.getters.iter().map(|(_, site)| site))
    }

    /// Leading `exports.X = void 0` statements before any other access of the
    /// name. TypeScript emits them ahead of the real write.
    fn is_leading_sentinel(&self, write: &PropertyWrite) -> bool {
        if !write.is_void || !write.unconditional {
            return false;
        }
        !self
            .all_sites()
            .any(|site| site.seq < write.site.seq && !self.is_unconditional_void_write(site))
    }

    fn is_unconditional_void_write(&self, site: &Site) -> bool {
        self.plain_writes
            .iter()
            .any(|write| write.site.seq == site.seq && write.is_void && write.unconditional)
    }
}

/// The first `this` of a CommonJS module body that is not bound by a function
/// or class body, unless it is a TypeScript helper guard.
///
/// CommonJS runs the module body with `this` set to `module.exports`; an ES
/// module body has `this` undefined. Such a `this` is another name for the
/// exports object, so converting the module would make its writes throw and
/// its reads change value, like an aliased `exports` binding. Arrow functions,
/// a class heritage, computed class keys, and decorators see the outer `this`.
///
/// TypeScript declares each helper as `(this && this.__name) || impl`. The
/// guard picks `impl` in both module systems unless the module writes that
/// property itself, so it does not count when the name starts with `__` and
/// the module never accesses it through `exports` or `module.exports`.
///
/// A module that already has import or export declarations is ESM, and one
/// that never refers to `require`, `exports`, or `module` has nothing to
/// convert; both report nothing.
fn top_level_this(module: &Module, unresolved_mark: Mark) -> Option<Span> {
    if module
        .body
        .iter()
        .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
    {
        return None;
    }
    let mut finder = TopLevelThis {
        unresolved_mark,
        first: None,
        commonjs: false,
        guards: Vec::new(),
        exported_names: HashSet::default(),
    };
    module.visit_with(&mut finder);
    if !finder.commonjs {
        return None;
    }
    let guard = finder
        .guards
        .iter()
        .find(|(name, _)| finder.exported_names.contains(name))
        .map(|(_, span)| *span);
    match (finder.first, guard) {
        (Some(this), Some(guard)) => Some(if guard.lo < this.lo { guard } else { this }),
        (this, guard) => this.or(guard),
    }
}

struct TopLevelThis {
    unresolved_mark: Mark,
    first: Option<Span>,
    commonjs: bool,
    /// Helper guards `this && this.__name`, by property name.
    guards: Vec<(Atom, Span)>,
    /// Static property names accessed on `exports` or `module.exports`.
    exported_names: HashSet<Atom>,
}

impl TopLevelThis {
    fn helper_guard(&self, bin: &BinExpr) -> Option<Atom> {
        if bin.op != BinaryOp::LogicalAnd || !matches!(strip_parens(&bin.left), Expr::This(_)) {
            return None;
        }
        let Expr::Member(member) = strip_parens(&bin.right) else {
            return None;
        };
        if !matches!(strip_parens(&member.obj), Expr::This(_)) {
            return None;
        }
        static_member_name(&member.prop).filter(|name| name.starts_with("__"))
    }

    fn is_exports_object(&self, expr: &Expr) -> bool {
        match strip_parens(expr) {
            Expr::Ident(ident) => is_unresolved_ident(ident, "exports", self.unresolved_mark),
            Expr::Member(member) => {
                matches!(strip_parens(&member.obj), Expr::Ident(object)
                    if is_unresolved_ident(object, "module", self.unresolved_mark))
                    && static_member_name(&member.prop).as_deref() == Some("exports")
            }
            _ => false,
        }
    }
}

impl Visit for TopLevelThis {
    fn visit_this_expr(&mut self, this: &ThisExpr) {
        if self.first.is_none() {
            self.first = Some(this.span);
        }
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if ["require", "exports", "module"]
            .iter()
            .any(|name| is_unresolved_ident(ident, name, self.unresolved_mark))
        {
            self.commonjs = true;
        }
    }

    fn visit_bin_expr(&mut self, bin: &BinExpr) {
        if let Some(name) = self.helper_guard(bin) {
            self.guards.push((name, bin.span));
            return;
        }
        bin.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if self.is_exports_object(&member.obj) {
            if let Some(name) = static_member_name(&member.prop) {
                self.exported_names.insert(name);
            }
        }
        member.visit_children_with(self);
    }

    // Bodies with their own `this`. Function also covers methods, accessors,
    // and class methods, whose keys are visited separately.
    fn visit_function(&mut self, _: &Function) {}
    fn visit_constructor(&mut self, _: &Constructor) {}
    fn visit_class_prop(&mut self, prop: &ClassProp) {
        prop.key.visit_with(self);
        prop.decorators.visit_with(self);
    }
    fn visit_private_prop(&mut self, prop: &PrivateProp) {
        prop.decorators.visit_with(self);
    }
    fn visit_auto_accessor(&mut self, accessor: &AutoAccessor) {
        accessor.key.visit_with(self);
        accessor.decorators.visit_with(self);
    }
    fn visit_static_block(&mut self, _: &StaticBlock) {}
}

struct Inventory {
    unresolved_mark: Mark,
    saw_exports: bool,
    gate: Option<(String, Span)>,
    order: Vec<Atom>,
    names: HashMap<Atom, NameFacts>,
    /// Every write of a local binding, including initializing declarators.
    local_writes: HashMap<BindingId, Vec<Site>>,
    /// Statements that only copy local bindings into `exports` properties,
    /// such as `exports.a = a, exports.b = b;`.
    mirror_statements: HashSet<StmtPos>,
    /// Assignments that run exactly once with the module body, keyed by
    /// address (see [`PropertyWrite::unconditional`]).
    unconditional_assigns: HashSet<*const AssignExpr>,
    /// Whether the current module-body item is an expression statement.
    in_module_expr_stmt: bool,
    nested_write: bool,
    /// Module-body statements that become `export * from`; see
    /// [`export_star_statement_indices`].
    export_star_statements: HashSet<usize>,
    /// The `exports` binding is reassigned, or used as a value other than a
    /// static member object, a `typeof` operand, or a call argument.
    binding_escape: bool,
    stmt: StmtPos,
    module_index: usize,
    next_list: usize,
    seq: usize,
    function_depth: usize,
    /// The binding a property write chain initializes or assigns, keyed by
    /// the address of the chain's next property assignment.
    chain_binding: Option<(*const AssignExpr, BindingId)>,
    /// Sole arguments of an enum IIFE, the position TypeScript gives an enum
    /// initializer and the only one `UnEnum` folds. A namespace IIFE takes
    /// the same argument, but `UnEnum` does not fold it, so its initializer
    /// is an ordinary mirror chain here.
    iife_argument_assigns: HashSet<*const AssignExpr>,
    iife_argument_bins: HashSet<*const BinExpr>,
    /// Cocos registration calls whose `module` argument does not escape the
    /// export surface (`cocos_registration_frame`).
    cc_rf_push_calls: HashSet<*const CallExpr>,
    /// The `exports.x` operands of `L = exports.x || (exports.x = {})`; see
    /// [`MirrorFacts::initializer_reads`].
    initializer_reads: HashSet<*const MemberExpr>,
}

impl Inventory {
    fn new(unresolved_mark: Mark) -> Self {
        Self {
            unresolved_mark,
            saw_exports: false,
            gate: None,
            order: Vec::new(),
            names: HashMap::default(),
            local_writes: HashMap::default(),
            mirror_statements: HashSet::default(),
            unconditional_assigns: HashSet::default(),
            in_module_expr_stmt: false,
            nested_write: false,
            export_star_statements: HashSet::default(),
            binding_escape: false,
            stmt: StmtPos {
                list: MODULE_LIST,
                index: 0,
            },
            module_index: 0,
            next_list: MODULE_LIST + 1,
            seq: 0,
            function_depth: 0,
            chain_binding: None,
            iife_argument_assigns: HashSet::default(),
            iife_argument_bins: HashSet::default(),
            cc_rf_push_calls: HashSet::default(),
            initializer_reads: HashSet::default(),
        }
    }

    fn site(&mut self, span: Span) -> Site {
        self.seq += 1;
        Site {
            span,
            stmt: self.stmt,
            module_index: self.module_index,
            seq: self.seq,
            deferred: self.function_depth > 0,
        }
    }

    fn fail(&mut self, message: impl Into<String>, span: Span) {
        if self.gate.is_none() {
            self.gate = Some((message.into(), span));
        }
    }

    fn facts(&mut self, name: &Atom) -> &mut NameFacts {
        if !self.names.contains_key(name) {
            self.order.push(name.clone());
        }
        self.names.entry(name.clone()).or_default()
    }

    /// `exports` or `module.exports`. The module gate rejects any other use
    /// of `module.exports`, so both name the same object.
    fn is_exports(&self, expr: &Expr) -> bool {
        match strip_parens(expr) {
            Expr::Ident(ident) => is_unresolved_ident(ident, "exports", self.unresolved_mark),
            Expr::Member(member) => {
                matches!(strip_parens(&member.obj), Expr::Ident(object)
                    if is_unresolved_ident(object, "module", self.unresolved_mark))
                    && static_member_name(&member.prop).as_deref() == Some("exports")
            }
            _ => false,
        }
    }

    fn collect_unconditional_assigns(&mut self, expr: &Expr) {
        match strip_parens(expr) {
            Expr::Assign(assign) => {
                self.unconditional_assigns
                    .insert(assign as *const AssignExpr);
                if assign.op == AssignOp::Assign {
                    self.collect_unconditional_assigns(&assign.right);
                }
            }
            Expr::Seq(sequence) => {
                for expr in &sequence.exprs {
                    self.collect_unconditional_assigns(expr);
                }
            }
            _ => {}
        }
    }

    /// Reassigning the CommonJS wrapper bindings replaces the object every
    /// recovered name refers to.
    fn check_wrapper_write(
        &mut self,
        sym: &Atom,
        ctxt: swc_core::common::SyntaxContext,
        span: Span,
    ) {
        if ctxt.outer() == self.unresolved_mark && matches!(sym.as_ref(), "exports" | "module") {
            if sym.as_ref() == "exports" {
                self.saw_exports = true;
                self.binding_escape = true;
            }
            self.fail(format!("`{sym}` is reassigned"), span);
        }
    }

    /// A write of an `exports` property inside top-level control flow.
    fn note_exports_write_position(&mut self) {
        if self.function_depth == 0 && (self.stmt.list != MODULE_LIST || !self.in_module_expr_stmt)
        {
            self.nested_write = true;
        }
    }

    fn is_local(&self, ident: &Ident) -> bool {
        ident.ctxt.outer() != self.unresolved_mark
    }

    /// The static name of an `exports.X` member. A computed or prototype key
    /// fails the module gate and returns `None`.
    fn exports_member_name(&mut self, member: &MemberExpr) -> Option<Atom> {
        if !self.is_exports(&member.obj) {
            return None;
        }
        self.saw_exports = true;
        let Some(name) = static_member_name(&member.prop) else {
            self.fail("computed `exports` key", member.span);
            return None;
        };
        if is_prototype_mutating_member_name(name.as_ref()) {
            self.fail(format!("prototype member `exports.{name}`"), member.span);
            return None;
        }
        Some(name)
    }

    fn exports_target_name(&mut self, expr: &Expr) -> Option<Atom> {
        match strip_parens(expr) {
            Expr::Member(member) => self.exports_member_name(member),
            _ => None,
        }
    }

    fn record_local_write(&mut self, ident: &Ident) {
        self.check_wrapper_write(&ident.sym, ident.ctxt, ident.span);
        if self.is_local(ident) {
            let site = self.site(ident.span);
            self.local_writes
                .entry((ident.sym.clone(), ident.ctxt))
                .or_default()
                .push(site);
        }
    }

    fn record_pattern_writes<T>(&mut self, pattern: &T)
    where
        T: Spanned + VisitWith<swc_core::ecma::utils::DestructuringFinder<swc_core::ecma::ast::Id>>,
    {
        let ids: Vec<swc_core::ecma::ast::Id> = find_pat_ids(pattern);
        for (sym, ctxt) in ids {
            self.check_wrapper_write(&sym, ctxt, pattern.span());
            if ctxt.outer() != self.unresolved_mark {
                let site = self.site(pattern.span());
                self.local_writes.entry((sym, ctxt)).or_default().push(site);
            }
        }
    }

    /// The binding a written value reads, for a value that is the binding's
    /// current value when the property is written.
    fn value_mirror(&self, value: &Expr) -> Option<BindingId> {
        match strip_parens(value) {
            Expr::Ident(ident) if self.is_local(ident) => Some((ident.sym.clone(), ident.ctxt)),
            Expr::Assign(assign) => match &assign.left {
                AssignTarget::Simple(SimpleAssignTarget::Ident(binding))
                    if self.is_local(&binding.id) =>
                {
                    Some((binding.id.sym.clone(), binding.id.ctxt))
                }
                AssignTarget::Simple(SimpleAssignTarget::Member(member))
                    if assign.op == AssignOp::Assign && self.is_exports(&member.obj) =>
                {
                    self.value_mirror(&assign.right)
                }
                _ => None,
            },
            Expr::Update(update) if update.prefix => match strip_parens(&update.arg) {
                Expr::Ident(ident) if self.is_local(ident) => Some((ident.sym.clone(), ident.ctxt)),
                _ => None,
            },
            Expr::Seq(sequence) => sequence
                .exprs
                .last()
                .and_then(|last| self.value_mirror(last)),
            _ => None,
        }
    }

    fn is_void_value(&self, value: &Expr) -> bool {
        match strip_parens(value) {
            Expr::Assign(assign) if assign.op == AssignOp::Assign => match &assign.left {
                AssignTarget::Simple(SimpleAssignTarget::Member(member))
                    if self.is_exports(&member.obj) =>
                {
                    self.is_void_value(&assign.right)
                }
                _ => false,
            },
            value => is_void_or_undefined(value, self.unresolved_mark),
        }
    }

    /// Mark the property assignment that `value` starts, if any, as part of a
    /// chain that also writes `binding`.
    ///
    /// TypeScript's exported enum and namespace initializer,
    /// `L = exports.x || (exports.x = {})`, counts too: afterwards `L` and the
    /// property hold the same object either way. It is left to `UnEnum` only
    /// when `iife_argument` says it is still the argument of the enum IIFE;
    /// Terser can inline the IIFE, and `UnEnum` does not fold what remains.
    fn note_chain(&mut self, value: &Expr, binding: BindingId, iife_argument: bool) {
        let value = match strip_parens(value) {
            Expr::Bin(bin) if bin.op == BinaryOp::LogicalOr => {
                let Some(name) = self.static_exports_name(&bin.left) else {
                    return;
                };
                match strip_parens(&bin.right) {
                    Expr::Assign(assign)
                        if matches!(&assign.left,
                            AssignTarget::Simple(SimpleAssignTarget::Member(member))
                                if self.static_exports_name_of(member).as_ref() == Some(&name)) =>
                    {
                        if iife_argument {
                            self.facts(&name).enum_initializer = true;
                        }
                        if let Expr::Member(read) = strip_parens(&bin.left) {
                            self.initializer_reads.insert(read as *const MemberExpr);
                        }
                        &bin.right
                    }
                    _ => return,
                }
            }
            _ => value,
        };
        if let Expr::Assign(assign) = strip_parens(value) {
            if assign.op == AssignOp::Assign {
                if let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left {
                    if self.is_exports(&member.obj) {
                        self.chain_binding = Some((assign as *const AssignExpr, binding));
                    }
                }
            }
        }
    }

    /// TypeScript's exported enum and namespace argument,
    /// `L || (exports.x = L = {})`, which `UnEnum` folds into the export while
    /// it is still the argument of the enum IIFE.
    fn note_enum_initializer(&mut self, bin: &BinExpr) {
        if !self.iife_argument_bins.contains(&(bin as *const BinExpr)) {
            return;
        }
        // Collapsed `exports.x || (exports.x = {})`, after a minifier drops
        // the unused local.
        if let Some(name) = self.static_exports_name(&bin.left) {
            if matches!(strip_parens(&bin.right), Expr::Assign(assign)
                if assign.op == AssignOp::Assign
                    && matches!(&assign.left,
                        AssignTarget::Simple(SimpleAssignTarget::Member(member))
                            if self.static_exports_name_of(member).as_ref() == Some(&name))
                    && matches!(strip_parens(&assign.right), Expr::Object(object)
                        if object.props.is_empty()))
            {
                self.facts(&name).enum_initializer = true;
            }
            return;
        }
        let Expr::Ident(local) = strip_parens(&bin.left) else {
            return;
        };
        let Expr::Assign(assign) = strip_parens(&bin.right) else {
            return;
        };
        let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
            return;
        };
        let Some(name) = self.static_exports_name_of(member) else {
            return;
        };
        if matches!(strip_parens(&assign.right), Expr::Assign(inner)
            if matches!(&inner.left, AssignTarget::Simple(SimpleAssignTarget::Ident(binding))
                if binding.id.sym == local.sym && binding.id.ctxt == local.ctxt))
        {
            self.facts(&name).enum_initializer = true;
        }
    }

    /// The static name of `exports.x`, without recording or gating anything.
    fn static_exports_name(&self, expr: &Expr) -> Option<Atom> {
        match strip_parens(expr) {
            Expr::Member(member) => self.static_exports_name_of(member),
            _ => None,
        }
    }

    fn static_exports_name_of(&self, member: &MemberExpr) -> Option<Atom> {
        if self.is_exports(&member.obj) {
            static_member_name(&member.prop)
        } else {
            None
        }
    }

    /// `exports.a = a;` or `exports.a = a, exports.b = b;` with local
    /// identifier values: nothing in it reads a property or runs code.
    fn is_mirror_statement(&self, stmt: &Stmt) -> bool {
        let Stmt::Expr(statement) = stmt else {
            return false;
        };
        let is_copy = |expr: &Expr| {
            let Expr::Assign(assign) = strip_parens(expr) else {
                return false;
            };
            assign.op == AssignOp::Assign
                && matches!(&assign.left, AssignTarget::Simple(SimpleAssignTarget::Member(member))
                    if self.is_exports(&member.obj) && static_member_name(&member.prop).is_some())
                && matches!(strip_parens(&assign.right), Expr::Ident(ident) if self.is_local(ident))
        };
        match strip_parens(&statement.expr) {
            Expr::Seq(sequence) => sequence.exprs.iter().all(|expr| is_copy(expr)),
            expr => is_copy(expr),
        }
    }

    fn enter_body<F: FnOnce(&mut Self)>(&mut self, visit: F) {
        let saved = self.stmt;
        self.stmt = StmtPos {
            list: self.next_list,
            index: 0,
        };
        self.next_list += 1;
        self.function_depth += 1;
        visit(self);
        self.function_depth -= 1;
        self.stmt = saved;
    }

    /// `exports` passed directly to a call fails the module gate without
    /// [`Self::binding_escape`]: helper calls such as
    /// `__exportStar(require("./dep"), exports)` keep their own recognizers
    /// on the statement path.
    fn visit_call_args(&mut self, args: &[ExprOrSpread]) {
        for arg in args {
            match strip_parens(&arg.expr) {
                Expr::Ident(ident)
                    if arg.spread.is_none()
                        && is_unresolved_ident(ident, "exports", self.unresolved_mark) =>
                {
                    self.saw_exports = true;
                    self.fail("`exports` is passed to a call", ident.span);
                }
                _ => arg.visit_with(self),
            }
        }
    }

    /// `key in exports` only tests the object and cannot alias it; the
    /// CommonJS export-star loop checks it before each copy. A bare
    /// `exports` there still fails the module gate, without
    /// [`Self::binding_escape`].
    fn visit_in_operand(&mut self, operand: &Expr) {
        match strip_parens(operand) {
            Expr::Ident(ident) if is_unresolved_ident(ident, "exports", self.unresolved_mark) => {
                self.saw_exports = true;
                self.fail("`exports` is used as a value", ident.span);
            }
            _ => operand.visit_with(self),
        }
    }

    fn visit_call_target(&mut self, callee: &Expr) {
        if let Expr::Member(member) = strip_parens(callee) {
            if let Some(name) = self.exports_member_name(member) {
                let site = self.site(member.span);
                self.facts(&name).calls.push(site);
                return;
            }
        }
        callee.visit_with(self);
    }

    /// `Object.defineProperty(exports, ...)` and `require.d(exports, ...)`
    /// pass `exports` itself as an argument. Return true when the call is a
    /// recognized export definition and was recorded.
    fn record_export_definition(&mut self, call: &CallExpr) -> bool {
        if call
            .args
            .first()
            .is_none_or(|arg| !self.is_exports(&arg.expr))
        {
            return false;
        }
        if is_object_define_property_global_call(call, self.unresolved_mark) && call.args.len() == 3
        {
            if is_esmodule_name_arg(&call.args[1].expr)
                && is_esmodule_descriptor(&call.args[2].expr)
            {
                self.saw_exports = true;
                return true;
            }
            let Expr::Lit(swc_core::ecma::ast::Lit::Str(name)) = strip_parens(&call.args[1].expr)
            else {
                return false;
            };
            let Some(name) = name.value.as_str().map(Atom::from) else {
                return false;
            };
            let Some(getter) = extract_define_property_getter_expr(&call.args[2].expr) else {
                return false;
            };
            self.record_getter(name, &getter, call.span);
            return true;
        }
        if let Some(getters) = webpack_getters(call, self.unresolved_mark) {
            for (name, getter) in getters {
                self.record_getter(name, &getter, call.span);
            }
            return true;
        }
        false
    }

    fn record_getter(&mut self, name: Atom, getter: &Expr, span: Span) {
        self.saw_exports = true;
        // A getter defined inside a function may be installed any number of
        // times, or only once the function runs; webpack's factory IIFEs are
        // unwrapped by a later recovery that needs these accesses intact.
        if self.function_depth > 0 {
            self.fail("an export getter is defined inside a function", span);
            return;
        }
        if is_prototype_mutating_member_name(name.as_ref()) {
            self.fail(format!("prototype member `exports.{name}`"), span);
            return;
        }
        let target = match strip_parens(getter) {
            Expr::Ident(ident) if self.is_local(ident) => GetterTarget::Binding(ident.clone()),
            Expr::Member(member) => match (strip_parens(&member.obj), &member.prop) {
                (Expr::Ident(base), MemberProp::Ident(prop)) if self.is_local(base) => {
                    GetterTarget::Member(base.clone(), prop.sym.clone())
                }
                _ => GetterTarget::Other,
            },
            _ => GetterTarget::Other,
        };
        let site = self.site(span);
        self.facts(&name).getters.push((target, site));
    }
}

fn webpack_getters(call: &CallExpr, unresolved_mark: Mark) -> Option<Vec<(Atom, Box<Expr>)>> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !is_unresolved_member_expr(callee, "require", "d", unresolved_mark) {
        return None;
    }
    match call.args.len() {
        2 => {
            let Expr::Object(map) = strip_parens(&call.args[1].expr) else {
                return None;
            };
            extract_export_getter_map(map)
        }
        3 => {
            let Expr::Lit(swc_core::ecma::ast::Lit::Str(name)) = strip_parens(&call.args[1].expr)
            else {
                return None;
            };
            let name = Atom::from(name.value.as_str()?);
            Some(vec![(
                name,
                extract_getter_expr_return_expr(&call.args[2].expr)?,
            )])
        }
        _ => None,
    }
}

impl Visit for Inventory {
    fn visit_module_items(&mut self, items: &[ModuleItem]) {
        let saved = self.stmt;
        for (index, item) in items.iter().enumerate() {
            if self.export_star_statements.contains(&index) {
                self.saw_exports = true;
                continue;
            }
            self.stmt = StmtPos {
                list: MODULE_LIST,
                index,
            };
            self.module_index = index;
            self.in_module_expr_stmt = matches!(item, ModuleItem::Stmt(Stmt::Expr(_)));
            match item {
                ModuleItem::Stmt(Stmt::Expr(statement)) => {
                    self.collect_unconditional_assigns(&statement.expr);
                }
                // `var local = exports.x = value;`, which the statement path
                // splits into a declaration and an export.
                ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                    for init in var.decls.iter().filter_map(|decl| decl.init.as_deref()) {
                        if matches!(strip_parens(init), Expr::Assign(_)) {
                            self.collect_unconditional_assigns(init);
                        }
                    }
                }
                _ => {}
            }
            if matches!(item, ModuleItem::Stmt(stmt) if self.is_mirror_statement(stmt)) {
                self.mirror_statements.insert(self.stmt);
            }
            item.visit_with(self);
        }
        self.stmt = saved;
    }

    fn visit_stmts(&mut self, stmts: &[Stmt]) {
        let saved = self.stmt;
        let list = self.next_list;
        self.next_list += 1;
        for (index, stmt) in stmts.iter().enumerate() {
            self.stmt = StmtPos { list, index };
            if self.is_mirror_statement(stmt) {
                self.mirror_statements.insert(self.stmt);
            }
            stmt.visit_with(self);
        }
        self.stmt = saved;
    }

    fn visit_function(&mut self, function: &Function) {
        self.enter_body(|this| function.visit_children_with(this));
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.enter_body(|this| arrow.visit_children_with(this));
    }

    fn visit_class(&mut self, class: &Class) {
        class.super_class.visit_with(self);
        self.enter_body(|this| class.body.visit_with(this));
    }

    fn visit_class_decl(&mut self, class: &ClassDecl) {
        self.record_local_write(&class.ident);
        class.class.visit_with(self);
    }

    fn visit_fn_decl(&mut self, function: &FnDecl) {
        function.function.visit_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let Some(init) = &declarator.init {
            init_writes(self, &declarator.name, init);
            init.visit_with(self);
        }
        // Default values inside a declaration pattern can still access
        // `exports`; the binding identifiers themselves are not accesses.
        if !matches!(declarator.name, Pat::Ident(_)) {
            declarator.name.visit_with(self);
        }
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        let chain = self
            .chain_binding
            .take()
            .filter(|(address, _)| std::ptr::eq(*address, assign))
            .map(|(_, binding)| binding);
        match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Member(member))
                if self.is_exports(&member.obj) =>
            {
                let unconditional = self.function_depth == 0
                    && self
                        .unconditional_assigns
                        .contains(&(assign as *const AssignExpr));
                if !unconditional {
                    self.note_exports_write_position();
                }
                let Some(name) = self.exports_member_name(member) else {
                    assign.right.visit_with(self);
                    return;
                };
                // Register the name in source order before the right-hand
                // side, which may write other names first.
                self.facts(&name);
                if assign.op != AssignOp::Assign {
                    assign.right.visit_with(self);
                    let site = self.site(member.span);
                    self.facts(&name).other_writes.push(("compound", site));
                    return;
                }
                if let Some(binding) = &chain {
                    self.note_chain(&assign.right, binding.clone(), false);
                }
                let mirror = self
                    .value_mirror(&assign.right)
                    .map(|binding| (binding, MirrorEvidence::Value))
                    .or_else(|| chain.map(|binding| (binding, MirrorEvidence::Chain)));
                let is_void = self.is_void_value(&assign.right);
                let observes_receiver = value_observes_receiver(&assign.right);
                assign.right.visit_with(self);
                let site = self.site(member.span);
                self.facts(&name).plain_writes.push(PropertyWrite {
                    site,
                    unconditional,
                    mirror,
                    is_void,
                    observes_receiver,
                });
            }
            AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
                if self.is_local(&binding.id) {
                    let iife_argument = self
                        .iife_argument_assigns
                        .contains(&(assign as *const AssignExpr));
                    self.note_chain(
                        &assign.right,
                        (binding.id.sym.clone(), binding.id.ctxt),
                        iife_argument,
                    );
                }
                assign.right.visit_with(self);
                self.record_local_write(&binding.id);
            }
            AssignTarget::Pat(pattern) => {
                pattern.visit_with(self);
                assign.right.visit_with(self);
                self.record_pattern_writes(pattern);
            }
            AssignTarget::Simple(_) => assign.visit_children_with(self),
        }
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        match strip_parens(&update.arg) {
            Expr::Member(member) if self.is_exports(&member.obj) => {
                self.note_exports_write_position();
                if let Some(name) = self.exports_member_name(member) {
                    let site = self.site(member.span);
                    self.facts(&name).other_writes.push(("update", site));
                }
            }
            Expr::Ident(ident) => self.record_local_write(ident),
            _ => update.visit_children_with(self),
        }
    }

    fn visit_pat(&mut self, pattern: &Pat) {
        if let Pat::Expr(target) = pattern {
            if matches!(strip_parens(target), Expr::Member(member) if self.is_exports(&member.obj))
            {
                self.note_exports_write_position();
            }
            if let Some(name) = self.exports_target_name(target) {
                let site = self.site(target.span());
                self.facts(&name).other_writes.push(("pattern", site));
                return;
            }
        }
        pattern.visit_children_with(self);
    }

    fn visit_for_in_stmt(&mut self, statement: &ForInStmt) {
        visit_for_head(self, &statement.left);
        statement.right.visit_with(self);
        statement.body.visit_with(self);
    }

    fn visit_for_of_stmt(&mut self, statement: &ForOfStmt) {
        visit_for_head(self, &statement.left);
        statement.right.visit_with(self);
        statement.body.visit_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let (Callee::Expr(callee), [argument]) = (&call.callee, call.args.as_slice()) {
            if argument.spread.is_none()
                && matches!(strip_parens(callee), Expr::Fn(_) | Expr::Arrow(_))
                && is_enum_iife_callee(callee)
            {
                match strip_parens(&argument.expr) {
                    Expr::Assign(assign) => {
                        self.iife_argument_assigns
                            .insert(assign as *const AssignExpr);
                    }
                    Expr::Bin(bin) => {
                        self.iife_argument_bins.insert(bin as *const BinExpr);
                    }
                    _ => {}
                }
            }
        }
        if self.record_export_definition(call) {
            for arg in call.args.iter().skip(1) {
                arg.visit_with(self);
            }
            return;
        }
        if self.cc_rf_push_calls.contains(&(call as *const CallExpr)) {
            // `cocos_registration_frame`: the frame keeps the `module`
            // handle, and nothing it does reads or writes an export property.
            for arg in call.args.iter().skip(1) {
                arg.visit_with(self);
            }
            return;
        }
        match &call.callee {
            Callee::Expr(callee) => self.visit_call_target(callee),
            callee => callee.visit_with(self),
        }
        self.visit_call_args(&call.args);
    }

    fn visit_opt_call(&mut self, call: &OptCall) {
        self.visit_call_target(&call.callee);
        self.visit_call_args(&call.args);
    }

    fn visit_tagged_tpl(&mut self, tagged: &TaggedTpl) {
        self.visit_call_target(&tagged.tag);
        tagged.tpl.visit_with(self);
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        match unary.op {
            UnaryOp::TypeOf if self.is_exports(&unary.arg) => {
                self.saw_exports = true;
            }
            UnaryOp::TypeOf
                if matches!(strip_parens(&unary.arg), Expr::Ident(ident)
                    if is_unresolved_ident(ident, "module", self.unresolved_mark)) => {}
            UnaryOp::Delete if matches!(strip_parens(&unary.arg), Expr::Member(member) if self.is_exports(&member.obj)) =>
            {
                self.saw_exports = true;
                self.note_exports_write_position();
                self.fail("`delete` of an `exports` property", unary.span);
            }
            _ => unary.visit_children_with(self),
        }
    }

    fn visit_bin_expr(&mut self, bin: &BinExpr) {
        if bin.op == BinaryOp::LogicalOr {
            self.note_enum_initializer(bin);
        }
        if bin.op == BinaryOp::In {
            bin.left.visit_with(self);
            self.visit_in_operand(&bin.right);
        } else {
            bin.visit_children_with(self);
        }
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if self.is_exports(&member.obj) {
            if let Some(name) = self.exports_member_name(member) {
                let site = self.site(member.span);
                let initializer = self
                    .initializer_reads
                    .contains(&(member as *const MemberExpr));
                let facts = self.facts(&name);
                facts.reads.push(site);
                if initializer {
                    facts.initializer_reads += 1;
                }
            }
            return;
        }
        if matches!(strip_parens(&member.obj), Expr::Ident(object)
            if is_unresolved_ident(object, "module", self.unresolved_mark))
        {
            // `module.exports.name` was handled above; any other use of
            // `module.exports` can replace or leak the object. Other static
            // members (`module.hot`, `module.id`) do not touch exports.
            match static_member_name(&member.prop).as_deref() {
                Some("exports") => self.fail("`module.exports` is used as a value", member.span),
                Some(_) => {}
                None => self.fail("computed `module` key", member.span),
            }
            return;
        }
        member.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if is_unresolved_ident(ident, "exports", self.unresolved_mark) {
            self.saw_exports = true;
            self.binding_escape = true;
            self.fail("`exports` is used as a value", ident.span);
        } else if is_unresolved_ident(ident, "module", self.unresolved_mark) {
            self.fail("`module` is referenced", ident.span);
        }
    }
}

fn init_writes(inventory: &mut Inventory, name: &Pat, init: &Expr) {
    match name {
        Pat::Ident(binding) => {
            if inventory.is_local(&binding.id) {
                inventory.note_chain(init, (binding.id.sym.clone(), binding.id.ctxt), false);
            }
            inventory.record_local_write(&binding.id);
        }
        pattern => inventory.record_pattern_writes(pattern),
    }
}

fn visit_for_head(inventory: &mut Inventory, head: &ForHead) {
    match head {
        ForHead::VarDecl(declaration) => {
            for declarator in &declaration.decls {
                inventory.record_pattern_writes(&declarator.name);
                if !matches!(declarator.name, Pat::Ident(_)) {
                    declarator.name.visit_with(inventory);
                }
            }
        }
        ForHead::Pat(pattern) => {
            pattern.visit_with(inventory);
            inventory.record_pattern_writes(pattern.as_ref());
        }
        ForHead::UsingDecl(declaration) => declaration.visit_with(inventory),
    }
}

// ============================================================
// Module-level declarations
// ============================================================

#[derive(Debug, Clone, Copy)]
struct ModuleDeclaration {
    /// The declared value is a function that reads `this`, so a direct call
    /// through a copy of it would see a different receiver.
    observes_receiver: bool,
    index: usize,
    lexical: bool,
    /// A function, a class, or a variable whose initializer has no `require`
    /// call: a value the module computes itself. An import or a `require`
    /// result is left to the statement path, which re-exports it.
    own_value: bool,
}

/// Whether `expr` calls the CommonJS `require` anywhere.
fn contains_require_call(expr: &Expr, unresolved_mark: Mark) -> bool {
    struct Finder {
        unresolved_mark: Mark,
        found: bool,
    }
    impl Visit for Finder {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if matches!(&call.callee, Callee::Expr(callee)
                if matches!(strip_parens(callee), Expr::Ident(id)
                    if is_unresolved_ident(id, "require", self.unresolved_mark)))
            {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }
    let mut finder = Finder {
        unresolved_mark,
        found: false,
    };
    expr.visit_with(&mut finder);
    finder.found
}

fn value_observes_receiver(value: &Expr) -> bool {
    matches!(strip_parens(value), Expr::Fn(function) if function_observes_receiver(&function.function))
}

/// Bindings declared directly in the module body.
fn collect_module_declarations(
    module: &Module,
    unresolved_mark: Mark,
) -> HashMap<BindingId, ModuleDeclaration> {
    let mut declarations = HashMap::default();
    for (index, item) in module.body.iter().enumerate() {
        let plain = ModuleDeclaration {
            observes_receiver: false,
            index,
            lexical: false,
            own_value: false,
        };
        let decl = match item {
            ModuleItem::Stmt(Stmt::Decl(decl)) => decl,
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => &export.decl,
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                for specifier in &import.specifiers {
                    let local = match specifier {
                        swc_core::ecma::ast::ImportSpecifier::Named(named) => &named.local,
                        swc_core::ecma::ast::ImportSpecifier::Default(default) => &default.local,
                        swc_core::ecma::ast::ImportSpecifier::Namespace(namespace) => {
                            &namespace.local
                        }
                    };
                    declarations.insert((local.sym.clone(), local.ctxt), plain);
                }
                continue;
            }
            _ => continue,
        };
        match decl {
            Decl::Fn(function) => {
                declarations.insert(
                    (function.ident.sym.clone(), function.ident.ctxt),
                    ModuleDeclaration {
                        observes_receiver: function_observes_receiver(&function.function),
                        own_value: true,
                        ..plain
                    },
                );
            }
            Decl::Class(class) => {
                declarations.insert(
                    (class.ident.sym.clone(), class.ident.ctxt),
                    ModuleDeclaration {
                        lexical: true,
                        own_value: true,
                        ..plain
                    },
                );
            }
            Decl::Var(var) => {
                for declarator in &var.decls {
                    let observes_receiver = matches!(&declarator.name, Pat::Ident(_))
                        && declarator
                            .init
                            .as_deref()
                            .is_some_and(value_observes_receiver);
                    let own_value = declarator
                        .init
                        .as_deref()
                        .is_none_or(|init| !contains_require_call(init, unresolved_mark));
                    for id in find_pat_ids::<_, swc_core::ecma::ast::Id>(&declarator.name) {
                        declarations.insert(
                            id,
                            ModuleDeclaration {
                                observes_receiver,
                                lexical: var.kind != VarDeclKind::Var,
                                own_value,
                                ..plain
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }
    declarations
}

// ============================================================
// Classification
// ============================================================

fn classify(
    name: &Atom,
    facts: &NameFacts,
    inventory: &Inventory,
    declarations: &HashMap<BindingId, ModuleDeclaration>,
) -> ExportStorageDecision {
    let (sentinels, writes): (Vec<&PropertyWrite>, Vec<&PropertyWrite>) = facts
        .plain_writes
        .iter()
        .partition(|write| facts.is_leading_sentinel(write));
    let accesses = AccessCounts {
        writes: writes.len(),
        sentinels: sentinels.len(),
        other_writes: facts.other_writes.len(),
        reads: facts.reads.len(),
        calls: facts.calls.len(),
        getters: facts.getters.len(),
        deferred: facts.all_sites().filter(|site| site.deferred).count(),
    };
    let mut decision = ExportStorageDecision {
        name: name.clone(),
        storage: ExportStorage::Unrecovered,
        binding: None,
        rejected: Vec::new(),
        accesses,
        mirror: None,
        getter: None,
        enum_initializer: facts.enum_initializer,
    };

    if let Some((target, site)) = facts.getters.first() {
        match classify_getter(facts, &writes, target, site, declarations) {
            Ok(binding) => {
                decision.storage = ExportStorage::Getter;
                decision.binding = Some(binding);
                if let GetterTarget::Binding(ident) = target {
                    let id = (ident.sym.clone(), ident.ctxt);
                    if declarations
                        .get(&id)
                        .is_some_and(|declaration| declaration.own_value)
                    {
                        decision.getter = Some(id);
                    }
                }
            }
            Err(rejection) => decision.rejected.push(rejection),
        }
        return decision;
    }

    match classify_mirror(facts, &writes, inventory, declarations) {
        Ok(binding) => {
            let declaration = declarations[&binding];
            decision.storage = ExportStorage::Mirror;
            decision.binding = Some(binding.0.to_string());
            decision.mirror = Some(MirrorFacts {
                binding,
                declaration_index: declaration.index,
                lexical: declaration.lexical,
                first_eager_access: facts
                    .reads
                    .iter()
                    .chain(&facts.calls)
                    .filter(|site| !site.deferred)
                    .map(|site| site.module_index)
                    .min(),
                enum_initializer: facts.enum_initializer,
                initializer_reads: facts.initializer_reads,
            });
            return decision;
        }
        Err(Some(rejection)) => {
            let receiver = rejection.storage == ExportStorage::Unrecovered;
            decision.rejected.push(rejection);
            if receiver {
                return decision;
            }
        }
        Err(None) => {}
    }

    match classify_property(facts, &writes) {
        Ok(()) => decision.storage = ExportStorage::Property,
        Err(rejection) => decision.rejected.push(rejection),
    }
    decision
}

fn classify_getter(
    facts: &NameFacts,
    writes: &[&PropertyWrite],
    target: &GetterTarget,
    site: &Site,
    declarations: &HashMap<BindingId, ModuleDeclaration>,
) -> Result<String, Rejection> {
    let reject = |message: String, span: Option<Span>| Rejection {
        storage: ExportStorage::Getter,
        message,
        span,
    };
    if facts.getters.len() > 1 {
        return Err(reject(
            "the property has more than one getter definition".to_string(),
            Some(facts.getters[1].1.span),
        ));
    }
    if let Some(write) = writes.first() {
        return Err(reject(
            "the property also has a direct write".to_string(),
            Some(write.site.span),
        ));
    }
    if let Some((_, site)) = facts.other_writes.first() {
        return Err(reject(
            "the property also has a direct write".to_string(),
            Some(site.span),
        ));
    }
    let binding = match target {
        GetterTarget::Binding(ident) => {
            if let Some(call) = facts.calls.first() {
                if binding_observes_receiver(&(ident.sym.clone(), ident.ctxt), declarations) {
                    return Err(receiver_rejection(call));
                }
            }
            ident.sym.to_string()
        }
        GetterTarget::Member(base, prop) => format!("{}.{prop}", base.sym),
        GetterTarget::Other => {
            return Err(reject(
                "the getter returns neither a binding nor a member of one".to_string(),
                Some(site.span),
            ))
        }
    };
    Ok(binding)
}

/// `Err(None)` means the mirror model does not apply at all (no writes copy a
/// binding), which is not worth reporting as a rejection.
fn classify_mirror(
    facts: &NameFacts,
    writes: &[&PropertyWrite],
    inventory: &Inventory,
    declarations: &HashMap<BindingId, ModuleDeclaration>,
) -> Result<BindingId, Option<Rejection>> {
    let reject = |message: String, span: Option<Span>| {
        Some(Rejection {
            storage: ExportStorage::Mirror,
            message,
            span,
        })
    };
    // Only a module-level binding can be the storage of an export. A copied
    // parameter or function local is just a value.
    let Some(binding) = writes.iter().find_map(|write| {
        write
            .mirror
            .as_ref()
            .map(|(binding, _)| binding)
            .filter(|binding| declarations.contains_key(*binding))
            .cloned()
    }) else {
        return Err(None);
    };
    let shown = binding.0.to_string();
    if let Some((kind, site)) = facts.other_writes.first() {
        return Err(reject(
            format!("the property has a {kind} write, which does not copy `{shown}`"),
            Some(site.span),
        ));
    }
    for write in writes {
        match &write.mirror {
            Some((other, _)) if other == &binding => {}
            Some((other, _)) if declarations.contains_key(other) => {
                return Err(reject(
                    format!(
                        "writes copy different bindings, `{shown}` and `{}`",
                        other.0
                    ),
                    Some(write.site.span),
                ))
            }
            _ => {
                return Err(reject(
                    format!("a write does not copy `{shown}`"),
                    Some(write.site.span),
                ))
            }
        }
    }
    let local_writes = inventory
        .local_writes
        .get(&binding)
        .map_or(&[][..], Vec::as_slice);
    if is_final_copy(facts, writes, local_writes) {
        return Ok(binding);
    }
    for local_write in local_writes {
        if !is_mirrored(local_write, writes, &inventory.mirror_statements) {
            return Err(reject(
                format!("a write of `{shown}` is not mirrored in the same or the next statement"),
                Some(local_write.span),
            ));
        }
    }
    if let Some(call) = facts.calls.first() {
        if binding_observes_receiver(&binding, declarations) {
            return Err(Some(receiver_rejection(call)));
        }
    }
    Ok(binding)
}

/// A property that the module never reads, written once by a module
/// statement after every write of the binding (rollup places all
/// `exports.x = x` copies at the end). Only an importer can observe the
/// property, and it sees the final value either way.
fn is_final_copy(facts: &NameFacts, writes: &[&PropertyWrite], local_writes: &[Site]) -> bool {
    let [write] = writes else {
        return false;
    };
    facts.reads.is_empty()
        && facts.calls.is_empty()
        && write.unconditional
        && local_writes
            .iter()
            .all(|local| !local.deferred && local.module_index < write.site.module_index)
}

/// Whether a write of the binding is copied into the property before anything
/// can observe the property: later in the same statement, or in a following
/// statement with only other mirror statements in between.
fn is_mirrored(
    local_write: &Site,
    writes: &[&PropertyWrite],
    mirror_statements: &HashSet<StmtPos>,
) -> bool {
    writes.iter().any(|write| {
        let Some((_, evidence)) = &write.mirror else {
            return false;
        };
        let mirror = &write.site;
        if mirror.stmt == local_write.stmt {
            return *evidence == MirrorEvidence::Chain || mirror.seq > local_write.seq;
        }
        mirror.stmt.list == local_write.stmt.list
            && mirror.stmt.index > local_write.stmt.index
            && (local_write.stmt.index + 1..mirror.stmt.index).all(|index| {
                mirror_statements.contains(&StmtPos {
                    list: mirror.stmt.list,
                    index,
                })
            })
    })
}

fn classify_property(facts: &NameFacts, writes: &[&PropertyWrite]) -> Result<(), Rejection> {
    let Some(call) = facts.calls.first() else {
        return Ok(());
    };
    if writes.iter().any(|write| write.observes_receiver) {
        Err(receiver_rejection(call))
    } else {
        Ok(())
    }
}

/// Direct calls rely on `call_receiver_independence`: the `exports` receiver
/// of `exports.f()` is an artifact of lowering `f()`. Only a function that
/// visibly reads `this` keeps its CommonJS call.
fn binding_observes_receiver(
    binding: &BindingId,
    declarations: &HashMap<BindingId, ModuleDeclaration>,
) -> bool {
    declarations
        .get(binding)
        .is_some_and(|declaration| declaration.observes_receiver)
}

fn receiver_rejection(call: &Site) -> Rejection {
    Rejection {
        storage: ExportStorage::Unrecovered,
        message: "a direct call passes `exports` as the receiver to a function that reads `this`"
            .to_string(),
        span: Some(call.span),
    }
}

// ============================================================
// Property storage recovery (A)
// ============================================================

#[derive(Default)]
pub(super) struct PropertyStoragePlan {
    /// The module gate failed in a way statement-by-statement recovery
    /// cannot convert safely (see [`ExportStorageReport::ModuleGate`]), so
    /// the module stays CommonJS.
    pub(super) keep_commonjs: bool,
    /// Names [`recover_export_storage`] rewrites.
    pub(super) names: HashSet<Atom>,
}

pub(super) fn property_storage_plan(module: &Module, unresolved_mark: Mark) -> PropertyStoragePlan {
    match analyze_export_storage(module, unresolved_mark) {
        ExportStorageReport::Names(decisions) => {
            let identifier_counts = count_identifiers(module);
            let candidates =
                storage_candidates(module, unresolved_mark, &decisions, &identifier_counts);
            PropertyStoragePlan {
                keep_commonjs: false,
                names: candidates
                    .property
                    .iter()
                    .copied()
                    .chain(candidates.mirror.iter().map(|(decision, _)| *decision))
                    .chain(candidates.getter.iter().map(|(decision, _)| *decision))
                    .map(|decision| decision.name.clone())
                    .collect(),
            }
        }
        ExportStorageReport::ModuleGate { keep_commonjs, .. } => PropertyStoragePlan {
            keep_commonjs,
            names: HashSet::default(),
        },
        ExportStorageReport::NoCommonJsExports => PropertyStoragePlan::default(),
    }
}

/// The names the storage rewrite owns, by the model it applies.
struct StorageCandidates<'a> {
    property: Vec<&'a ExportStorageDecision>,
    mirror: Vec<(&'a ExportStorageDecision, &'a MirrorFacts)>,
    getter: Vec<(&'a ExportStorageDecision, &'a BindingId)>,
}

/// Select the names to rewrite.
///
/// A property-storage name whose only access is one whole top-level write
/// (after leading sentinels) is left to the statement classification. That
/// path exports the value directly (`export default value`,
/// `export const f = function f() {}`), where property storage would need a
/// fresh local whenever the name is taken, as it is by the function's own
/// name or by `default`. A name written more than once goes through property
/// storage, which keeps every write instead of exporting only the last. A
/// mirror name whose only access is one `exports.x = local;` statement is left
/// to the statement path too; that path already exports the local.
///
/// A mirror name whose local cannot stand in for the property at every read
/// falls back to property storage, which is valid for any name that passes
/// the module gate: the local is shadowed somewhere (a read rewritten to it
/// could resolve to the inner binding), or a read outside functions runs
/// before a lexical local is initialized (the property is `undefined` there;
/// the local would throw). TypeScript enum initializers stay on the
/// statement path for `UnEnum`.
///
/// A getter name whose getter returns a binding the module computes itself
/// (see [`ExportStorageDecision::getter`]) is rewritten like a mirror name:
/// the definition goes, reads become the binding, and `export { local as x }`
/// follows the binding's declaration. A read before a lexical binding's
/// declaration throws through the getter too, so only shadowing keeps such a
/// name on the statement path.
fn storage_candidates<'a>(
    module: &Module,
    unresolved_mark: Mark,
    decisions: &'a [ExportStorageDecision],
    identifier_counts: &HashMap<BindingId, usize>,
) -> StorageCandidates<'a> {
    let mut standalone_writes: HashMap<Atom, usize> = HashMap::default();
    let mut standalone_copies: HashMap<Atom, usize> = HashMap::default();
    for item in &module.body {
        if let Some((name, copies_ident)) = standalone_value_write(item, unresolved_mark) {
            *standalone_writes.entry(name.clone()).or_default() += 1;
            if copies_ident {
                *standalone_copies.entry(name).or_default() += 1;
            }
        }
    }
    let existing_exports = existing_export_names(module);
    let mut candidates = StorageCandidates {
        property: Vec::new(),
        mirror: Vec::new(),
        getter: Vec::new(),
    };
    for decision in decisions {
        // `exports.exports` is the slot itself once `module.exports` is set
        // to `module`; the statement path keeps its boundary for that name.
        if decision.name.as_ref() == "exports" || existing_exports.contains(&decision.name) {
            continue;
        }
        let accesses = &decision.accesses;
        let only_top_level_writes = accesses.other_writes == 0
            && accesses.reads == 0
            && accesses.calls == 0
            && accesses.deferred == 0;
        let count = |counts: &HashMap<Atom, usize>| counts.get(&decision.name).copied();
        match decision.storage {
            ExportStorage::Property => {
                if decision.enum_initializer
                    && un_enum_folds_collapsed_initializer(decision, identifier_counts)
                {
                    continue;
                }
                if !(only_top_level_writes
                    && accesses.writes == 1
                    && count(&standalone_writes) == Some(1))
                {
                    candidates.property.push(decision);
                }
            }
            ExportStorage::Mirror => {
                let Some(mirror) = &decision.mirror else {
                    continue;
                };
                if mirror.enum_initializer
                    || (only_top_level_writes
                        && accesses.writes == 1
                        && count(&standalone_copies) == Some(1))
                {
                    continue;
                }
                if mirror_local_replaces_reads(decision, mirror, identifier_counts) {
                    candidates.mirror.push((decision, mirror));
                } else {
                    candidates.property.push(decision);
                }
            }
            ExportStorage::Getter => {
                let Some(binding) = &decision.getter else {
                    continue;
                };
                if accesses.reads + accesses.calls == 0 || !is_shadowed(binding, identifier_counts)
                {
                    candidates.getter.push((decision, binding));
                }
            }
            ExportStorage::Unrecovered => {}
        }
    }
    candidates
}

/// Whether another binding in the module has the same name, so a read
/// rewritten to `binding` could resolve to that one instead.
fn is_shadowed(binding: &BindingId, identifier_counts: &HashMap<BindingId, usize>) -> bool {
    let (sym, ctxt) = binding;
    identifier_counts
        .keys()
        .any(|(other, other_ctxt)| other == sym && other_ctxt != ctxt)
}

fn mirror_local_replaces_reads(
    decision: &ExportStorageDecision,
    mirror: &MirrorFacts,
    identifier_counts: &HashMap<BindingId, usize>,
) -> bool {
    if decision.accesses.reads + decision.accesses.calls == 0 {
        return true;
    }
    let shadowed = decision.accesses.reads + decision.accesses.calls > mirror.initializer_reads
        && is_shadowed(&mirror.binding, identifier_counts);
    let early = mirror.lexical
        && mirror
            .first_eager_access
            .is_some_and(|index| index <= mirror.declaration_index);
    !shadowed && !early
}

/// Rewrite every export name whose storage `UnEsm` can identify, in every
/// position: property storage (see [`recover_property_storage`]), and mirror
/// storage and getters of a local binding (see [`recover_mirror_storage`]).
pub(super) fn recover_export_storage(module: &mut Module, unresolved_mark: Mark) {
    let ExportStorageReport::Names(decisions) = analyze_export_storage(module, unresolved_mark)
    else {
        return;
    };
    let identifier_counts = count_identifiers(module);
    let candidates = storage_candidates(module, unresolved_mark, &decisions, &identifier_counts);
    let getter_names: HashSet<Atom> = candidates
        .getter
        .iter()
        .map(|(decision, _)| decision.name.clone())
        .collect();
    let mirrors: Vec<(Atom, BindingId)> = candidates
        .mirror
        .iter()
        .map(|(decision, mirror)| (decision.name.clone(), mirror.binding.clone()))
        .chain(
            candidates
                .getter
                .iter()
                .map(|(decision, binding)| (decision.name.clone(), (*binding).clone())),
        )
        .collect();
    if !candidates.property.is_empty() {
        recover_property_storage(
            module,
            unresolved_mark,
            &decisions,
            &candidates.property,
            &identifier_counts,
        );
    }
    if !getter_names.is_empty() {
        remove_getter_definitions(module, unresolved_mark, &getter_names);
    }
    recover_mirror_storage(module, unresolved_mark, &mirrors);
}

/// Drop the top-level getter definitions of `names`. The module gate already
/// failed for a getter defined anywhere else.
fn remove_getter_definitions(module: &mut Module, unresolved_mark: Mark, names: &HashSet<Atom>) {
    module.body.retain(|item| {
        let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
            return true;
        };
        let Expr::Call(call) = strip_parens(&statement.expr) else {
            return true;
        };
        let [target, name, descriptor] = call.args.as_slice() else {
            return true;
        };
        let is_definition = is_object_define_property_global_call(call, unresolved_mark)
            && is_cjs_export_object_expr(&target.expr, unresolved_mark)
            && matches!(strip_parens(&name.expr), Expr::Lit(swc_core::ecma::ast::Lit::Str(name))
                if name.value.as_str().is_some_and(|name| names.contains(&Atom::from(name))))
            && extract_define_property_getter_expr(&descriptor.expr).is_some();
        !is_definition
    });
}

/// Rewrite every export name whose storage is the property itself to one
/// module-level `var` binding, in every position. Mirror names whose local
/// cannot replace the property also land here (see [`storage_candidates`]).
///
/// A hoisted `var` is `undefined` until a write runs, exactly like an
/// unassigned property, so reads before the first write, writes inside
/// hoisted functions, and repeated or conditional writes keep their
/// behavior. The first standalone top-level write becomes the declaration
/// (`export var x = value`); otherwise `var x;` is declared at the top.
/// Leading `exports.x = void 0` statements repeat that initial value and are
/// dropped. `VarDeclToLetConst` narrows the declaration kind later.
fn recover_property_storage(
    module: &mut Module,
    unresolved_mark: Mark,
    decisions: &[ExportStorageDecision],
    candidates: &[&ExportStorageDecision],
    identifier_counts: &HashMap<BindingId, usize>,
) {
    let mut declaration_sites: HashMap<Atom, usize> = HashMap::default();
    for (index, item) in module.body.iter().enumerate() {
        if let Some((name, _)) = standalone_value_write(item, unresolved_mark) {
            declaration_sites.entry(name).or_insert(index);
        }
    }

    let seeds = seed_aliases(module, candidates, &declaration_sites, identifier_counts);
    let mut used_names: HashSet<Atom> = identifier_counts
        .keys()
        .map(|(sym, _)| sym.clone())
        .collect();
    used_names.extend(decisions.iter().map(|decision| decision.name.clone()));
    let mut locals: HashMap<Atom, Ident> = HashMap::default();
    for decision in candidates {
        let name = &decision.name;
        // A seed alias that disappears frees its own name.
        let freed = seeds
            .get(name)
            .filter(|seed| seed.binding.0 == *name)
            .map_or(0, |_| 2);
        let occurrences: usize = identifier_counts
            .iter()
            .filter(|((sym, _), _)| sym == name)
            .map(|(_, count)| count)
            .sum();
        let local_name = if is_valid_identifier_name(name)
            && !is_reserved_binding_name(name)
            && name.as_ref() != "default"
            && occurrences == freed
        {
            name.clone()
        } else {
            fresh_prefixed_name(&identifier_base(name), &mut used_names)
        };
        locals.insert(name.clone(), fresh_binding_ident(local_name, DUMMY_SP));
    }

    let candidate_names: HashSet<Atom> = locals.keys().cloned().collect();
    let sentinels = leading_export_sentinels(module, &candidate_names, unresolved_mark);
    module.visit_mut_with(&mut PropertyStorageRewriter {
        unresolved_mark,
        locals: &locals,
    });

    let seed_declarations: HashSet<usize> =
        seeds.values().map(|seed| seed.declaration_index).collect();
    let mut seeds = seeds;
    let mut seed_inits: HashMap<Atom, Box<Expr>> = HashMap::default();
    let declared_at: HashMap<usize, &Atom> = declaration_sites
        .iter()
        .filter(|(name, _)| locals.contains_key(*name))
        .map(|(name, index)| (*index, name))
        .collect();
    let mut body = Vec::with_capacity(module.body.len() + candidates.len() * 2);
    for decision in candidates {
        if declaration_sites.contains_key(&decision.name) {
            continue;
        }
        let local = locals[&decision.name].clone();
        let exported =
            decision.accesses.writes + decision.accesses.sentinels + decision.accesses.other_writes
                > 0;
        push_declaration(&mut body, &decision.name, local, None, exported);
    }
    for (index, item) in std::mem::take(&mut module.body).into_iter().enumerate() {
        if sentinels.contains(&index) {
            continue;
        }
        if seed_declarations.contains(&index) {
            let name = seeds
                .iter()
                .find(|(_, seed)| seed.declaration_index == index)
                .map(|(name, _)| name.clone())
                .expect("a seed declaration belongs to a name");
            seeds.remove(&name);
            if let ModuleItem::Stmt(Stmt::Decl(Decl::Var(mut var))) = item {
                if let Some(init) = var.decls[0].init.take() {
                    seed_inits.insert(name, init);
                }
            }
            continue;
        }
        let Some(name) = declared_at.get(&index) else {
            body.push(item);
            continue;
        };
        let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
            unreachable!("a declaration site is an expression statement");
        };
        let Expr::Assign(assign) = *strip_parens_owned(statement.expr) else {
            unreachable!("a declaration site is an assignment");
        };
        let local = locals[*name].clone();
        let init = seed_inits.remove(*name).unwrap_or(assign.right);
        push_declaration(&mut body, name, local, Some(init), true);
    }
    module.body = body;
}

struct SeedAlias {
    binding: BindingId,
    declaration_index: usize,
}

/// sucrase seeds a property-storage export from a local it never uses again:
/// `let n = 0; exports.n = n;`. When the declaration directly precedes the
/// copy and the local has no other occurrence, the copy's declaration can
/// take the initializer and the local can go.
fn seed_aliases(
    module: &Module,
    candidates: &[&ExportStorageDecision],
    declaration_sites: &HashMap<Atom, usize>,
    identifier_counts: &HashMap<BindingId, usize>,
) -> HashMap<Atom, SeedAlias> {
    let mut seeds = HashMap::default();
    for decision in candidates {
        let Some(&index) = declaration_sites.get(&decision.name) else {
            continue;
        };
        let Some(previous) = index.checked_sub(1) else {
            continue;
        };
        let ModuleItem::Stmt(Stmt::Expr(statement)) = &module.body[index] else {
            continue;
        };
        let Expr::Assign(assign) = strip_parens(&statement.expr) else {
            continue;
        };
        let Expr::Ident(copied) = strip_parens(&assign.right) else {
            continue;
        };
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = &module.body[previous] else {
            continue;
        };
        let [declarator] = var.decls.as_slice() else {
            continue;
        };
        let Pat::Ident(binding) = &declarator.name else {
            continue;
        };
        let binding = (binding.id.sym.clone(), binding.id.ctxt);
        if declarator.init.is_none()
            || binding != (copied.sym.clone(), copied.ctxt)
            || identifier_counts.get(&binding) != Some(&2)
        {
            continue;
        }
        seeds.insert(
            decision.name.clone(),
            SeedAlias {
                binding,
                declaration_index: previous,
            },
        );
    }
    seeds
}

/// Occurrences of every identifier, binding and reference alike.
/// A collapsed `exports.x || (exports.x = {})` enum argument is left for
/// `UnEnum` only when its fold preconditions hold: the initializer is the
/// name's only read and write, and the public name is a legal binding that no
/// identifier in the module uses. Otherwise `UnEnum` declines and the
/// property storage rewrite recovers the name instead.
fn un_enum_folds_collapsed_initializer(
    decision: &ExportStorageDecision,
    identifier_counts: &HashMap<BindingId, usize>,
) -> bool {
    let accesses = &decision.accesses;
    accesses.reads == 1
        && accesses.writes == 1
        && accesses.calls == 0
        && accesses.other_writes == 0
        && accesses.getters == 0
        && is_valid_identifier_name(&decision.name)
        && !is_reserved_binding_name(&decision.name)
        && !identifier_counts
            .keys()
            .any(|(sym, _)| *sym == decision.name)
}

fn count_identifiers(module: &Module) -> HashMap<BindingId, usize> {
    struct Counter(HashMap<BindingId, usize>);
    impl Visit for Counter {
        fn visit_ident(&mut self, ident: &Ident) {
            *self.0.entry((ident.sym.clone(), ident.ctxt)).or_default() += 1;
        }
    }
    let mut counter = Counter(HashMap::default());
    module.visit_with(&mut counter);
    counter.0
}

/// `exports.x = value;` as a whole top-level statement, with a value that is
/// neither a `void 0` sentinel nor another export assignment, and whether the
/// value is an identifier.
fn standalone_value_write(item: &ModuleItem, unresolved_mark: Mark) -> Option<(Atom, bool)> {
    let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
        return None;
    };
    let Expr::Assign(assign) = strip_parens(&statement.expr) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return None;
    };
    if !is_cjs_export_object_expr(&member.obj, unresolved_mark) {
        return None;
    }
    let value = strip_parens(&assign.right);
    if is_void_or_undefined(value, unresolved_mark) {
        return None;
    }
    if let Expr::Assign(inner) = value {
        if matches!(&inner.left, AssignTarget::Simple(SimpleAssignTarget::Member(inner))
            if is_cjs_export_object_expr(&inner.obj, unresolved_mark))
        {
            return None;
        }
    }
    static_member_name(&member.prop).map(|name| (name, matches!(value, Expr::Ident(_))))
}

fn strip_parens_owned(expr: Box<Expr>) -> Box<Expr> {
    match *expr {
        Expr::Paren(paren) => strip_parens_owned(paren.expr),
        expr => Box::new(expr),
    }
}

fn existing_export_names(module: &Module) -> HashSet<Atom> {
    let mut names = HashSet::default();
    for item in &module.body {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::ExportDecl(export) => {
                let mut declared = HashSet::default();
                collect_decl_names(&export.decl, &mut declared);
                names.extend(declared);
            }
            ModuleDecl::ExportNamed(export) => {
                for specifier in &export.specifiers {
                    let exported = match specifier {
                        ExportSpecifier::Named(named) => {
                            named.exported.as_ref().unwrap_or(&named.orig)
                        }
                        ExportSpecifier::Default(_) => {
                            names.insert(Atom::from("default"));
                            continue;
                        }
                        ExportSpecifier::Namespace(namespace) => &namespace.name,
                    };
                    names.insert(module_export_name_atom(exported));
                }
            }
            ModuleDecl::ExportDefaultDecl(_) | ModuleDecl::ExportDefaultExpr(_) => {
                names.insert(Atom::from("default"));
            }
            _ => {}
        }
    }
    names
}

fn module_export_name_atom(name: &ModuleExportName) -> Atom {
    match name {
        ModuleExportName::Ident(ident) => ident.sym.clone(),
        ModuleExportName::Str(value) => Atom::from(value.value.to_string_lossy().as_ref()),
    }
}

/// A binding-name base for an export name that cannot be used as is.
fn identifier_base(name: &Atom) -> Atom {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '$' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Atom::from(sanitized)
}

fn push_declaration(
    body: &mut Vec<ModuleItem>,
    name: &Atom,
    local: Ident,
    init: Option<Box<Expr>>,
    exported: bool,
) {
    let declaration = Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent {
                id: local.clone(),
                type_ann: None,
            }),
            init,
            definite: false,
        }],
    }));
    if exported && local.sym == *name {
        body.push(ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
            span: DUMMY_SP,
            decl: declaration,
        })));
        return;
    }
    body.push(ModuleItem::Stmt(Stmt::Decl(declaration)));
    if exported {
        let exported = if is_valid_identifier_name(name) {
            ModuleExportName::Ident(make_name_ident(name.clone()))
        } else {
            ModuleExportName::Str(make_str(name))
        };
        body.push(ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(
            NamedExport {
                span: DUMMY_SP,
                specifiers: vec![ExportSpecifier::Named(ExportNamedSpecifier {
                    span: DUMMY_SP,
                    orig: ModuleExportName::Ident(local),
                    exported: Some(exported),
                    is_type_only: false,
                })],
                src: None,
                type_only: false,
                with: None,
            },
        )));
    }
}

struct PropertyStorageRewriter<'a> {
    unresolved_mark: Mark,
    locals: &'a HashMap<Atom, Ident>,
}

impl PropertyStorageRewriter<'_> {
    fn local_for(&self, member: &MemberExpr) -> Option<Ident> {
        if !is_cjs_export_object_expr(&member.obj, self.unresolved_mark) {
            return None;
        }
        let name = static_member_name(&member.prop)?;
        let mut local = self.locals.get(&name)?.clone();
        local.span = member.span;
        Some(local)
    }
}

impl VisitMut for PropertyStorageRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        let local = match expr {
            Expr::Member(member) => self.local_for(member),
            Expr::OptChain(chain) => match chain.base.as_ref() {
                OptChainBase::Member(member) => self.local_for(member),
                OptChainBase::Call(_) => None,
            },
            _ => None,
        };
        if let Some(local) = local {
            *expr = Expr::Ident(local);
            return;
        }
        expr.visit_mut_children_with(self);
    }

    fn visit_mut_simple_assign_target(&mut self, target: &mut SimpleAssignTarget) {
        if let SimpleAssignTarget::Member(member) = target {
            if let Some(local) = self.local_for(member) {
                *target = SimpleAssignTarget::Ident(BindingIdent {
                    id: local,
                    type_ann: None,
                });
                return;
            }
        }
        target.visit_mut_children_with(self);
    }

    fn visit_mut_pat(&mut self, pattern: &mut Pat) {
        if let Pat::Expr(target) = pattern {
            if let Expr::Member(member) = strip_parens(target) {
                if let Some(local) = self.local_for(member) {
                    *pattern = Pat::Ident(BindingIdent {
                        id: local,
                        type_ann: None,
                    });
                    return;
                }
            }
        }
        pattern.visit_mut_children_with(self);
    }
}

// ============================================================
// Mirror storage recovery (B)
// ============================================================

/// Export each mirror name's local live and remove the property.
///
/// Every write of the property copies the local's current value, and every
/// write of the local is copied into the property before anything can read
/// it (the classification's proof). So the property equals the local
/// wherever it is observed: a mirror write `exports.x = rhs` becomes `rhs`, a
/// read or call target `exports.x` becomes the local, and the module exports
/// the local as `x`. Statements left with no effect (`count;`, `void 0;`)
/// are removed. The export specifier follows the local's declaration.
fn recover_mirror_storage(
    module: &mut Module,
    unresolved_mark: Mark,
    mirrors: &[(Atom, BindingId)],
) {
    if mirrors.is_empty() {
        return;
    }
    let locals: HashMap<Atom, Ident> = mirrors
        .iter()
        .map(|(name, (sym, ctxt))| (name.clone(), Ident::new(sym.clone(), DUMMY_SP, *ctxt)))
        .collect();
    module.visit_mut_with(&mut MirrorStorageRewriter {
        unresolved_mark,
        locals: &locals,
        changed: false,
    });

    let mut pending: Vec<(Atom, &BindingId)> = mirrors
        .iter()
        .map(|(name, binding)| (name.clone(), binding))
        .collect();
    let mut body = Vec::with_capacity(module.body.len() + pending.len());
    for item in std::mem::take(&mut module.body) {
        let declared = module_item_binding_ids(&item);
        body.push(item);
        if declared.is_empty() {
            continue;
        }
        let (here, rest): (Vec<_>, Vec<_>) = pending
            .into_iter()
            .partition(|(_, binding)| declared.contains(*binding));
        pending = rest;
        if !here.is_empty() {
            body.push(local_export(&here));
        }
    }
    if !pending.is_empty() {
        body.push(local_export(&pending));
    }
    module.body = body;
}

/// Bindings a module-body item declares.
fn module_item_binding_ids(item: &ModuleItem) -> Vec<BindingId> {
    let decl = match item {
        ModuleItem::Stmt(Stmt::Decl(decl)) => decl,
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => &export.decl,
        ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
            return import
                .specifiers
                .iter()
                .map(|specifier| {
                    let local = match specifier {
                        swc_core::ecma::ast::ImportSpecifier::Named(named) => &named.local,
                        swc_core::ecma::ast::ImportSpecifier::Default(default) => &default.local,
                        swc_core::ecma::ast::ImportSpecifier::Namespace(namespace) => {
                            &namespace.local
                        }
                    };
                    (local.sym.clone(), local.ctxt)
                })
                .collect();
        }
        _ => return Vec::new(),
    };
    match decl {
        Decl::Fn(function) => vec![(function.ident.sym.clone(), function.ident.ctxt)],
        Decl::Class(class) => vec![(class.ident.sym.clone(), class.ident.ctxt)],
        Decl::Var(var) => var
            .decls
            .iter()
            .flat_map(|declarator| find_pat_ids::<_, swc_core::ecma::ast::Id>(&declarator.name))
            .collect(),
        _ => Vec::new(),
    }
}

/// `export { local as name, ... }`.
fn local_export(names: &[(Atom, &BindingId)]) -> ModuleItem {
    let specifiers = names
        .iter()
        .map(|(name, (sym, ctxt))| {
            let exported = if sym == name {
                None
            } else if is_valid_identifier_name(name) || is_reserved_binding_name(name) {
                // An export name is an IdentifierName: reserved words such as
                // `default` need no quotes.
                Some(ModuleExportName::Ident(make_name_ident(name.clone())))
            } else {
                Some(ModuleExportName::Str(make_str(name)))
            };
            ExportSpecifier::Named(ExportNamedSpecifier {
                span: DUMMY_SP,
                orig: ModuleExportName::Ident(Ident::new(sym.clone(), DUMMY_SP, *ctxt)),
                exported,
                is_type_only: false,
            })
        })
        .collect();
    ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
        span: DUMMY_SP,
        specifiers,
        src: None,
        type_only: false,
        with: None,
    }))
}

struct MirrorStorageRewriter<'a> {
    unresolved_mark: Mark,
    locals: &'a HashMap<Atom, Ident>,
    /// Whether the statement being visited was rewritten.
    changed: bool,
}

impl MirrorStorageRewriter<'_> {
    fn local_for(&self, member: &MemberExpr) -> Option<Ident> {
        if !is_cjs_export_object_expr(&member.obj, self.unresolved_mark) {
            return None;
        }
        let name = static_member_name(&member.prop)?;
        let mut local = self.locals.get(&name)?.clone();
        local.span = member.span;
        Some(local)
    }

    fn is_mirror_write(&self, assign: &AssignExpr) -> bool {
        assign.op == AssignOp::Assign
            && matches!(&assign.left,
                AssignTarget::Simple(SimpleAssignTarget::Member(member))
                    if self.local_for(member).is_some())
    }

    /// Visit one statement and report whether the rewrite touched it,
    /// keeping the flag of the enclosing statement.
    fn visit_statement<T: VisitMutWith<Self>>(&mut self, node: &mut T) -> bool {
        let outer = std::mem::replace(&mut self.changed, false);
        node.visit_mut_with(self);
        let changed = self.changed;
        self.changed = outer || changed;
        changed
    }

    /// A rewritten expression statement keeps only the parts with an effect.
    fn prune(&self, stmt: Stmt) -> Option<Stmt> {
        let Stmt::Expr(mut statement) = stmt else {
            return Some(stmt);
        };
        let exprs = match *strip_parens_owned(statement.expr) {
            Expr::Seq(sequence) => sequence.exprs,
            expr => vec![Box::new(expr)],
        };
        let mut kept: Vec<Box<Expr>> = exprs
            .into_iter()
            .filter(|expr| !self.has_no_effect(expr))
            .collect();
        statement.expr = match kept.len() {
            0 => return None,
            1 => kept.pop().expect("one expression"),
            _ => Box::new(Expr::Seq(swc_core::ecma::ast::SeqExpr {
                span: statement.span,
                exprs: kept,
            })),
        };
        Some(Stmt::Expr(statement))
    }

    fn has_no_effect(&self, expr: &Expr) -> bool {
        match strip_parens(expr) {
            Expr::Ident(ident) => {
                ident.ctxt.outer() != self.unresolved_mark || ident.sym.as_ref() == "undefined"
            }
            Expr::Lit(_) => true,
            Expr::Unary(unary) if unary.op == UnaryOp::Void => self.has_no_effect(&unary.arg),
            _ => false,
        }
    }
}

impl VisitMut for MirrorStorageRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Assign(assign) = expr {
            if self.is_mirror_write(assign) {
                let mut value = assign.right.take();
                value.visit_mut_with(self);
                *expr = *value;
                self.changed = true;
                return;
            }
        }
        let local = match expr {
            Expr::Member(member) => self.local_for(member),
            Expr::OptChain(chain) => match chain.base.as_ref() {
                OptChainBase::Member(member) => self.local_for(member),
                OptChainBase::Call(_) => None,
            },
            _ => None,
        };
        if let Some(local) = local {
            *expr = Expr::Ident(local);
            self.changed = true;
            return;
        }
        expr.visit_mut_children_with(self);
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        let mut out = Vec::with_capacity(items.len());
        for mut item in std::mem::take(items) {
            let changed = self.visit_statement(&mut item);
            match item {
                ModuleItem::Stmt(stmt) if changed => {
                    out.extend(self.prune(stmt).map(ModuleItem::Stmt));
                }
                item => out.push(item),
            }
        }
        *items = out;
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        let mut out = Vec::with_capacity(stmts.len());
        for mut stmt in std::mem::take(stmts) {
            if self.visit_statement(&mut stmt) {
                out.extend(self.prune(stmt));
            } else {
                out.push(stmt);
            }
        }
        *stmts = out;
    }
}
