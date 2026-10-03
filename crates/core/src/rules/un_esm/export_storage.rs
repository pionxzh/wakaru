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
//! reasoning behind each condition. The analysis only reports decisions; it
//! does not rewrite the module.

use crate::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::common::{Mark, Span, Spanned};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, BindingIdent, CallExpr,
    Callee, Class, ClassDecl, Decl, ExportDecl, ExportNamedSpecifier, ExportSpecifier, Expr,
    ExprOrSpread, FnDecl, ForHead, ForInStmt, ForOfStmt, Function, Ident, MemberExpr, MemberProp,
    Module, ModuleDecl, ModuleExportName, ModuleItem, NamedExport, OptCall, OptChainBase, Pat,
    SimpleAssignTarget, Stmt, TaggedTpl, UnaryExpr, UnaryOp, UpdateExpr, VarDecl, VarDeclKind,
    VarDeclarator,
};
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::js_names::{is_reserved_binding_name, is_valid_identifier_name};
use crate::rules::decl_utils::{collect_decl_names, fresh_binding_ident};

use crate::analysis::BindingId;
use crate::rules::constructor_sensitivity::static_member_name;
use crate::rules::eval_utils::{module_has_with_stmt, DirectEvalPresence};
use crate::utils::paren::strip_parens;
use crate::utils::prototype_members::is_prototype_mutating_member_name;

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
pub(crate) fn analyze_export_storage(
    module: &Module,
    unresolved_mark: Mark,
) -> ExportStorageReport {
    let mut inventory = Inventory::new(unresolved_mark);
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

    let declarations = collect_module_declarations(module);
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
    /// expression of a top-level statement, an element of a top-level
    /// sequence, or a link of such an assignment chain.
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
    /// property hold the same object either way.
    fn note_chain(&mut self, value: &Expr, binding: BindingId) {
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
            self.stmt = StmtPos {
                list: MODULE_LIST,
                index,
            };
            self.module_index = index;
            self.in_module_expr_stmt = matches!(item, ModuleItem::Stmt(Stmt::Expr(_)));
            if let ModuleItem::Stmt(Stmt::Expr(statement)) = item {
                self.collect_unconditional_assigns(&statement.expr);
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
                    self.note_chain(&assign.right, binding.clone());
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
                    self.note_chain(&assign.right, (binding.id.sym.clone(), binding.id.ctxt));
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
        if self.record_export_definition(call) {
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
                self.facts(&name).reads.push(site);
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
                inventory.note_chain(init, (binding.id.sym.clone(), binding.id.ctxt));
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
}

fn value_observes_receiver(value: &Expr) -> bool {
    matches!(strip_parens(value), Expr::Fn(function) if function_observes_receiver(&function.function))
}

/// Bindings declared directly in the module body.
fn collect_module_declarations(module: &Module) -> HashMap<BindingId, ModuleDeclaration> {
    let mut declarations = HashMap::default();
    let plain = ModuleDeclaration {
        observes_receiver: false,
    };
    for item in &module.body {
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
                    },
                );
            }
            Decl::Class(class) => {
                declarations.insert((class.ident.sym.clone(), class.ident.ctxt), plain);
            }
            Decl::Var(var) => {
                for declarator in &var.decls {
                    let observes_receiver = matches!(&declarator.name, Pat::Ident(_))
                        && declarator
                            .init
                            .as_deref()
                            .is_some_and(value_observes_receiver);
                    for id in find_pat_ids::<_, swc_core::ecma::ast::Id>(&declarator.name) {
                        declarations.insert(id, ModuleDeclaration { observes_receiver });
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
    };

    if let Some((target, site)) = facts.getters.first() {
        match classify_getter(facts, &writes, target, site, declarations) {
            Ok(binding) => {
                decision.storage = ExportStorage::Getter;
                decision.binding = Some(binding);
            }
            Err(rejection) => decision.rejected.push(rejection),
        }
        return decision;
    }

    match classify_mirror(facts, &writes, inventory, declarations) {
        Ok(binding) => {
            decision.storage = ExportStorage::Mirror;
            decision.binding = Some(binding);
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
) -> Result<String, Option<Rejection>> {
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
        return Ok(shown);
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
    Ok(shown)
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
    /// Names [`recover_property_storage_exports`] rewrites.
    pub(super) names: HashSet<Atom>,
}

pub(super) fn property_storage_plan(module: &Module, unresolved_mark: Mark) -> PropertyStoragePlan {
    match analyze_export_storage(module, unresolved_mark) {
        ExportStorageReport::Names(decisions) => PropertyStoragePlan {
            keep_commonjs: false,
            names: property_storage_candidates(module, unresolved_mark, &decisions)
                .into_iter()
                .map(|decision| decision.name.clone())
                .collect(),
        },
        ExportStorageReport::ModuleGate { keep_commonjs, .. } => PropertyStoragePlan {
            keep_commonjs,
            names: HashSet::default(),
        },
        ExportStorageReport::NoCommonJsExports => PropertyStoragePlan::default(),
    }
}

/// Property-storage names that need the rewrite. A name with only whole
/// top-level writes and leading sentinels, never read in the module, is left
/// to the statement classification: an importer sees the last value, which
/// that path already exports.
fn property_storage_candidates<'a>(
    module: &Module,
    unresolved_mark: Mark,
    decisions: &'a [ExportStorageDecision],
) -> Vec<&'a ExportStorageDecision> {
    let mut standalone_writes: HashMap<Atom, usize> = HashMap::default();
    for item in &module.body {
        if let Some(name) = standalone_value_write(item, unresolved_mark) {
            *standalone_writes.entry(name).or_default() += 1;
        }
    }
    let existing_exports = existing_export_names(module);
    decisions
        .iter()
        .filter(|decision| decision.storage == ExportStorage::Property)
        // `exports.exports` is the slot itself once `module.exports` is set to
        // `module`; the statement path keeps its boundary for that name.
        .filter(|decision| decision.name.as_ref() != "exports")
        .filter(|decision| !existing_exports.contains(&decision.name))
        .filter(|decision| {
            let accesses = &decision.accesses;
            let simple = accesses.other_writes == 0
                && accesses.reads == 0
                && accesses.calls == 0
                && accesses.deferred == 0
                && standalone_writes
                    .get(&decision.name)
                    .copied()
                    .unwrap_or_default()
                    == accesses.writes;
            !simple
        })
        .collect()
}

/// Rewrite every export name whose storage is the property itself to one
/// module-level `var` binding, in every position.
///
/// A hoisted `var` is `undefined` until a write runs, exactly like an
/// unassigned property, so reads before the first write, writes inside
/// hoisted functions, and repeated or conditional writes keep their
/// behavior. The first standalone top-level write becomes the declaration
/// (`export var x = value`); otherwise `var x;` is declared at the top.
/// Leading `exports.x = void 0` statements repeat that initial value and are
/// dropped. `VarDeclToLetConst` narrows the declaration kind later.
///
/// A name with one standalone top-level write and no other access is left to
/// the statement classification, which already produces the same export.
pub(super) fn recover_property_storage_exports(module: &mut Module, unresolved_mark: Mark) {
    let ExportStorageReport::Names(decisions) = analyze_export_storage(module, unresolved_mark)
    else {
        return;
    };

    let candidates = property_storage_candidates(module, unresolved_mark, &decisions);
    if candidates.is_empty() {
        return;
    }
    let mut declaration_sites: HashMap<Atom, usize> = HashMap::default();
    for (index, item) in module.body.iter().enumerate() {
        if let Some(name) = standalone_value_write(item, unresolved_mark) {
            declaration_sites.entry(name).or_insert(index);
        }
    }

    let identifier_counts = count_identifiers(module);
    let seeds = seed_aliases(module, &candidates, &declaration_sites, &identifier_counts);
    let mut used_names: HashSet<Atom> = identifier_counts
        .keys()
        .map(|(sym, _)| sym.clone())
        .collect();
    used_names.extend(decisions.iter().map(|decision| decision.name.clone()));
    let mut locals: HashMap<Atom, Ident> = HashMap::default();
    for decision in &candidates {
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
    for decision in &candidates {
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
/// neither a `void 0` sentinel nor another export assignment.
fn standalone_value_write(item: &ModuleItem, unresolved_mark: Mark) -> Option<Atom> {
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
    static_member_name(&member.prop)
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
