//! CommonJS runtime facts that exist only inside a structurally extracted
//! webpack factory.
//!
//! Webpack initializes each factory's `module.exports` to an object and passes
//! that same object as `exports`. Normal single-file code does not carry this
//! proof, so these rewrites are enabled only by detector-owned module metadata
//! and never run for raw output.

use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{Mark, Span, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayLit, AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, BindingIdent, Callee, Decl,
    ExportDefaultExpr, Expr, ExprStmt, Id, Ident, IfStmt, Lit, MemberExpr, MemberProp, Module,
    ModuleDecl, ModuleItem, Number, ObjectLit, Pat, SimpleAssignTarget, Stmt, UnaryOp, VarDecl,
    VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_uses::{BindingUseIndex, UseKind};
use crate::rules::eval_utils::DirectEvalAnalyzer;
use crate::rules::rename_utils::collect_module_names;
use crate::utils::paren::{strip_parens, strip_parens_mut};

pub(super) fn normalize_webpack_commonjs_runtime(
    module: &mut Module,
    unresolved_mark: Mark,
    enabled: bool,
    numeric_module_id: Option<f64>,
    legacy_module_i: bool,
) {
    if !enabled {
        return;
    }
    if let Some(module_id) = numeric_module_id {
        normalize_webpack_css_runtime(module, unresolved_mark, module_id, legacy_module_i);
    }
    if !has_supported_module_shell(module) {
        return;
    }

    let mut functions = FunctionBindingCollector::default();
    module.visit_with(&mut functions);
    let uses = BindingUseIndex::collect(module);
    functions
        .bindings
        .retain(|binding| !uses.has_direct_write(binding));
    let capture = fresh_capture_ident(module);
    let mut candidate = module.clone();
    let mut normalizer = WebpackCommonJsRuntimeNormalizer {
        unresolved_mark,
        function_bindings: functions.bindings,
        capture: capture.clone(),
        matches: 0,
    };
    candidate.visit_mut_with(&mut normalizer);
    if normalizer.matches != 1 {
        return;
    }

    let mut runtime_references = RuntimeCommonJsReferenceFinder {
        unresolved_mark,
        found: false,
    };
    candidate.visit_with(&mut runtime_references);
    if runtime_references.found {
        return;
    }

    candidate
        .body
        .insert(0, capture_declaration(capture.clone()));
    candidate
        .body
        .push(ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(
            ExportDefaultExpr {
                span: DUMMY_SP,
                expr: Box::new(Expr::Ident(capture)),
            },
        )));
    *module = candidate;
}

/// Restore the narrow webpack runtime contract emitted by CSS loader chains.
///
/// Old and current producers represent one CSS list item as
/// `[module.i/id, css, media, ...]`. Some also conditionally switch the
/// CommonJS value to `value.locals`. Once the factory wrapper is removed those
/// reads become free `module` references, even though the numeric container
/// key and webpack's initial `module.exports = {}` are detector-proven facts.
///
/// This recovery requires exactly one three/four-field CSS tuple. Replacing
/// its identity read is independent of ordinary `module.exports` handling:
/// the numeric container key proves the value even when `UnEsm` must recover
/// a separate default assignment later. The optional locals switch is held to
/// a stricter proof: it must be the only remaining CommonJS runtime use and
/// occur in an exact top-level `if` or generated `test && assignment` shape.
/// Direct eval fails closed for both facts. Named/string ids are deliberately
/// excluded because the public detector id cannot preserve their runtime type.
fn normalize_webpack_css_runtime(
    module: &mut Module,
    unresolved_mark: Mark,
    module_id: f64,
    legacy_module_i: bool,
) {
    let Some(plan) = plan_webpack_css_runtime(module, unresolved_mark, legacy_module_i) else {
        return;
    };

    let mut candidate = module.clone();
    let mut id_rewriter = CssModuleIdRewriter {
        target: plan.module_id_member_span,
        module_id,
        rewrites: 0,
    };
    candidate.visit_mut_with(&mut id_rewriter);
    if id_rewriter.rewrites != 1 {
        return;
    }

    if let Some(locals) = plan.locals {
        let capture = fresh_named_capture_ident(&candidate, "_webpackCssDefault");
        let mut locals_candidate = candidate.clone();
        if rewrite_css_locals_assignment(&mut locals_candidate, locals.assignment_span, &capture) {
            locals_candidate
                .body
                .insert(0, css_default_declaration(capture.clone()));
            locals_candidate
                .body
                .push(module_exports_assignment(capture, unresolved_mark));
            candidate = locals_candidate;
        }
    }

    *module = candidate;
}

struct WebpackCssRuntimePlan {
    module_id_member_span: Span,
    locals: Option<CssLocalsPlan>,
}

#[derive(Clone, Copy)]
struct CssLocalsPlan {
    assignment_span: Span,
    module_ident_span: Span,
}

fn plan_webpack_css_runtime(
    module: &Module,
    unresolved_mark: Mark,
    legacy_module_i: bool,
) -> Option<WebpackCssRuntimePlan> {
    let mut eval = DirectEvalAnalyzer::default();
    module.visit_with(&mut eval);
    if eval.unknown_direct_eval || !eval.known_direct_eval_sources.is_empty() {
        return None;
    }

    let mut tuples = CssTupleCollector {
        unresolved_mark,
        legacy_module_i,
        matches: Vec::new(),
    };
    module.visit_with(&mut tuples);
    let [(module_id_member_span, module_ident_span)] = tuples.matches.as_slice() else {
        return None;
    };

    if !module_identity_surface_is_stable(module, unresolved_mark) {
        return None;
    }

    let mut locals_candidates = Vec::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::If(statement)) => {
                if let Some(plan) = css_locals_if_plan(statement, unresolved_mark) {
                    locals_candidates.push(plan);
                }
            }
            ModuleItem::Stmt(Stmt::Expr(statement)) => {
                collect_immediate_css_locals_plans(
                    &statement.expr,
                    unresolved_mark,
                    &mut locals_candidates,
                );
            }
            _ => {}
        }
    }
    let locals = match locals_candidates.as_slice() {
        [plan]
            if css_locals_references_are_complete(
                module,
                unresolved_mark,
                *module_ident_span,
                *plan,
            ) =>
        {
            Some(*plan)
        }
        _ => None,
    };

    Some(WebpackCssRuntimePlan {
        module_id_member_span: *module_id_member_span,
        locals,
    })
}

struct CssTupleCollector {
    unresolved_mark: Mark,
    legacy_module_i: bool,
    /// `(whole member span, module identifier span)`.
    matches: Vec<(Span, Span)>,
}

impl Visit for CssTupleCollector {
    fn visit_array_lit(&mut self, array: &ArrayLit) {
        if matches!(array.elems.len(), 3 | 4)
            && array.elems.iter().all(|element| {
                element
                    .as_ref()
                    .is_some_and(|element| element.spread.is_none())
            })
            && matches!(
                array.elems.get(2).and_then(Option::as_ref),
                Some(element) if matches!(strip_parens(&element.expr), Expr::Lit(Lit::Str(_)))
            )
        {
            if let Some(first) = array.elems.first().and_then(Option::as_ref) {
                if let Expr::Member(member) = strip_parens(&first.expr) {
                    let properties = if self.legacy_module_i {
                        &["id", "i"][..]
                    } else {
                        &["id"][..]
                    };
                    if let Some(module) =
                        runtime_module_member(member, self.unresolved_mark, properties)
                    {
                        self.matches.push((member.span, module.span));
                    }
                }
            }
        }
        array.visit_children_with(self);
    }
}

#[derive(Default)]
struct CommonJsRuntimeReferenceCollector {
    unresolved_mark: Mark,
    modules: Vec<Span>,
    exports: Vec<Span>,
}

impl Visit for CommonJsRuntimeReferenceCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.ctxt.outer() != self.unresolved_mark {
            return;
        }
        match ident.sym.as_ref() {
            "module" => self.modules.push(ident.span),
            "exports" => self.exports.push(ident.span),
            _ => {}
        }
    }
}

fn css_locals_if_plan(statement: &IfStmt, unresolved_mark: Mark) -> Option<CssLocalsPlan> {
    if statement.alt.is_some() {
        return None;
    }
    let value = locals_test_binding(&statement.test)?;
    let assignment = single_expression_statement(&statement.cons)?;
    css_locals_assignment_plan(value, assignment, unresolved_mark)
}

/// Collect only immediate top-level sequence elements. In particular, do not
/// descend into call arguments, branches, assignment RHS values, or deferred
/// bodies: those positions do not prove the generated locals switch executes
/// in the factory's current invocation.
fn collect_immediate_css_locals_plans(
    expression: &Expr,
    unresolved_mark: Mark,
    plans: &mut Vec<CssLocalsPlan>,
) {
    match strip_parens(expression) {
        Expr::Seq(sequence) => {
            for expression in &sequence.exprs {
                collect_immediate_css_locals_plans(expression, unresolved_mark, plans);
            }
        }
        Expr::Bin(BinExpr {
            op: BinaryOp::LogicalAnd,
            left,
            right,
            ..
        }) => {
            let Some(value) = locals_test_binding(left) else {
                return;
            };
            if let Some(plan) = css_locals_assignment_plan(value, right, unresolved_mark) {
                plans.push(plan);
            }
        }
        _ => {}
    }
}

fn css_locals_assignment_plan(
    value: Id,
    assignment: &Expr,
    unresolved_mark: Mark,
) -> Option<CssLocalsPlan> {
    let Expr::Assign(assignment) = strip_parens(assignment) else {
        return None;
    };
    if assignment.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assignment.left else {
        return None;
    };
    let module = runtime_module_member(target, unresolved_mark, &["exports"])?;
    let Expr::Member(source) = strip_parens(&assignment.right) else {
        return None;
    };
    let Expr::Ident(source_value) = strip_parens(&source.obj) else {
        return None;
    };
    if !static_member_name_is(&source.prop, "locals") || source_value.to_id() != value {
        return None;
    }
    Some(CssLocalsPlan {
        assignment_span: assignment.span,
        module_ident_span: module.span,
    })
}

fn css_locals_references_are_complete(
    module: &Module,
    unresolved_mark: Mark,
    tuple_module_span: Span,
    locals: CssLocalsPlan,
) -> bool {
    let mut references = CommonJsRuntimeReferenceCollector {
        unresolved_mark,
        ..Default::default()
    };
    module.visit_with(&mut references);
    let allowed_module_spans = [tuple_module_span, locals.module_ident_span];
    references.exports.is_empty()
        && references.modules.len() == allowed_module_spans.len()
        && references.modules.iter().all(|reference| {
            allowed_module_spans
                .iter()
                .any(|allowed| allowed == reference)
        })
}

/// The identity substitution replaces one `module.id` / `module.i` read with
/// the detector-proven container key. That is sound only while nothing in the
/// factory can change the module object's identity surface: a rebinding, a
/// bare escape, a computed write, or a direct `id`/`i` write invalidates the
/// proven value at the read. Static member reads, `typeof`, computed reads,
/// and static writes to other properties (webpack's own
/// `module.exports = ...`) cannot, so they stay allowed.
fn module_identity_surface_is_stable(module: &Module, unresolved_mark: Mark) -> bool {
    let uses = BindingUseIndex::collect(module);
    let mut bindings = UnresolvedModuleBindingCollector {
        unresolved_mark,
        ids: HashSet::new(),
    };
    module.visit_with(&mut bindings);
    bindings.ids.iter().all(|binding| {
        uses.use_sites(binding).iter().all(|site| match &site.kind {
            UseKind::StaticMemberRead(_) | UseKind::ComputedMemberRead | UseKind::TypeofOperand => {
                true
            }
            UseKind::StaticMemberWrite(property) => !matches!(property.as_ref(), "id" | "i"),
            _ => false,
        })
    })
}

struct UnresolvedModuleBindingCollector {
    unresolved_mark: Mark,
    ids: HashSet<Id>,
}

impl Visit for UnresolvedModuleBindingCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym.as_ref() == "module" && ident.ctxt.outer() == self.unresolved_mark {
            self.ids.insert(ident.to_id());
        }
    }
}

fn locals_test_binding(test: &Expr) -> Option<Id> {
    let Expr::Member(member) = strip_parens(test) else {
        return None;
    };
    if !static_member_name_is(&member.prop, "locals") {
        return None;
    }
    match strip_parens(&member.obj) {
        Expr::Ident(value) => Some(value.to_id()),
        Expr::Assign(assignment) if assignment.op == AssignOp::Assign => {
            let AssignTarget::Simple(SimpleAssignTarget::Ident(value)) = &assignment.left else {
                return None;
            };
            Some(value.id.to_id())
        }
        _ => None,
    }
}

fn single_expression_statement(statement: &Stmt) -> Option<&Expr> {
    match statement {
        Stmt::Expr(statement) => Some(&statement.expr),
        Stmt::Block(block) => match block.stmts.as_slice() {
            [Stmt::Expr(statement)] => Some(&statement.expr),
            _ => None,
        },
        _ => None,
    }
}

fn runtime_module_member<'a>(
    member: &'a MemberExpr,
    unresolved_mark: Mark,
    properties: &[&str],
) -> Option<&'a Ident> {
    let Expr::Ident(module) = member.obj.as_ref() else {
        return None;
    };
    (module.sym.as_ref() == "module"
        && module.ctxt.outer() == unresolved_mark
        && properties
            .iter()
            .any(|property| static_member_name_is(&member.prop, property)))
    .then_some(module)
}

fn static_member_name_is(property: &MemberProp, expected: &str) -> bool {
    matches!(property, MemberProp::Ident(name) if name.sym.as_ref() == expected)
}

struct CssModuleIdRewriter {
    target: Span,
    module_id: f64,
    rewrites: usize,
}

impl VisitMut for CssModuleIdRewriter {
    fn visit_mut_expr(&mut self, expression: &mut Expr) {
        if matches!(expression, Expr::Member(member) if member.span == self.target) {
            *expression = Expr::Lit(Lit::Num(Number {
                span: DUMMY_SP,
                value: self.module_id,
                raw: None,
            }));
            self.rewrites += 1;
            return;
        }
        expression.visit_mut_children_with(self);
    }
}

fn rewrite_css_locals_assignment(module: &mut Module, target: Span, capture: &Ident) -> bool {
    let mut rewriter = CssLocalsAssignmentRewriter {
        target,
        capture,
        rewrites: 0,
    };
    module.visit_mut_with(&mut rewriter);
    rewriter.rewrites == 1
}

struct CssLocalsAssignmentRewriter<'a> {
    target: Span,
    capture: &'a Ident,
    rewrites: usize,
}

impl VisitMut for CssLocalsAssignmentRewriter<'_> {
    fn visit_mut_assign_expr(&mut self, assignment: &mut AssignExpr) {
        if assignment.span == self.target {
            assignment.left = AssignTarget::Simple(SimpleAssignTarget::Ident(BindingIdent::from(
                self.capture.clone(),
            )));
            self.rewrites += 1;
            return;
        }
        assignment.visit_mut_children_with(self);
    }
}

fn fresh_named_capture_ident(module: &Module, base: &str) -> Ident {
    let mut used_names = collect_module_names(module);
    let base: Atom = base.into();
    let name = if used_names.insert(base.clone()) {
        base
    } else {
        (1usize..)
            .map(|suffix| Atom::from(format!("{base}_{suffix}")))
            .find(|candidate| used_names.insert(candidate.clone()))
            .expect("the generated name space is unbounded")
    };
    Ident::new(name, DUMMY_SP, SyntaxContext::empty())
}

fn css_default_declaration(capture: Ident) -> ModuleItem {
    ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent::from(capture)),
            init: Some(Box::new(Expr::Object(ObjectLit {
                span: DUMMY_SP,
                props: Vec::new(),
            }))),
            definite: false,
        }],
    }))))
}

fn module_exports_assignment(capture: Ident, unresolved_mark: Mark) -> ModuleItem {
    let module = Ident::new(
        "module".into(),
        DUMMY_SP,
        SyntaxContext::empty().apply_mark(unresolved_mark),
    );
    ModuleItem::Stmt(Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Member(MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(Expr::Ident(module)),
                prop: MemberProp::Ident(swc_core::ecma::ast::IdentName::new(
                    "exports".into(),
                    DUMMY_SP,
                )),
            })),
            right: Box::new(Expr::Ident(capture)),
        })),
    }))
}

fn has_supported_module_shell(module: &Module) -> bool {
    match module.body.as_slice() {
        [ModuleItem::Stmt(Stmt::Expr(_))] => true,
        [ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))), ModuleItem::Stmt(Stmt::Expr(_))] => {
            matches!(var.kind, VarDeclKind::Var | VarDeclKind::Let)
                && matches!(
                    var.decls.as_slice(),
                    [VarDeclarator {
                        name: Pat::Ident(_),
                        init: None,
                        ..
                    }]
                )
        }
        _ => false,
    }
}

fn fresh_capture_ident(module: &Module) -> Ident {
    fresh_named_capture_ident(module, "_webpackDefault")
}

fn capture_declaration(capture: Ident) -> ModuleItem {
    ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent::from(capture)),
            init: None,
            definite: false,
        }],
    }))))
}

struct RuntimeCommonJsReferenceFinder {
    unresolved_mark: Mark,
    found: bool,
}

impl Visit for RuntimeCommonJsReferenceFinder {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.ctxt.outer() == self.unresolved_mark
            && matches!(ident.sym.as_ref(), "module" | "exports" | "require")
        {
            self.found = true;
        }
    }
}

#[derive(Default)]
struct FunctionBindingCollector {
    bindings: HashSet<Id>,
}

impl Visit for FunctionBindingCollector {
    fn visit_fn_decl(&mut self, function: &swc_core::ecma::ast::FnDecl) {
        self.bindings
            .insert((function.ident.sym.clone(), function.ident.ctxt));
        function.function.visit_children_with(self);
    }
}

struct WebpackCommonJsRuntimeNormalizer {
    unresolved_mark: Mark,
    function_bindings: HashSet<Id>,
    capture: Ident,
    matches: usize,
}

impl VisitMut for WebpackCommonJsRuntimeNormalizer {
    fn visit_mut_module(&mut self, module: &mut Module) {
        for item in &mut module.body {
            if let ModuleItem::Stmt(Stmt::Expr(statement)) = item {
                self.rewrite_immediate_expression(&mut statement.expr);
            }
        }
    }
}

impl WebpackCommonJsRuntimeNormalizer {
    /// Rewrite only an expression that is guaranteed to run on the module's
    /// straight-line execution path. Parentheses and unary wrappers preserve
    /// unconditional evaluation; a direct synchronous IIFE grants the same
    /// proof to its leading declaration/expression statement list.
    fn rewrite_immediate_expression(&mut self, expression: &mut Box<Expr>) {
        let expression = strip_parens_mut(expression);
        if let Some(replacement) = self.truthy_module_exports_branch(expression) {
            *expression = replacement;
            self.matches += 1;
            return;
        }
        if let Some(replacement) = self.non_undefined_factory_export(expression) {
            *expression = replacement;
            self.matches += 1;
            return;
        }

        match expression {
            Expr::Unary(unary) => self.rewrite_immediate_expression(&mut unary.arg),
            Expr::Seq(sequence) => {
                for expression in &mut sequence.exprs {
                    self.rewrite_immediate_expression(expression);
                }
            }
            Expr::Call(call) => self.rewrite_direct_iife(call),
            _ => {}
        }
    }

    fn rewrite_direct_iife(&mut self, call: &mut swc_core::ecma::ast::CallExpr) {
        let Callee::Expr(callee) = &mut call.callee else {
            return;
        };
        match strip_parens_mut(callee) {
            Expr::Fn(function)
                if !function.function.is_async && !function.function.is_generator =>
            {
                if let Some(body) = &mut function.function.body {
                    self.rewrite_immediate_statements(&mut body.stmts);
                }
            }
            Expr::Arrow(arrow) if !arrow.is_async => {
                if let swc_core::ecma::ast::ArrowFunctionBody::FunctionBody(body) = &mut *arrow.body
                {
                    self.rewrite_immediate_statements(&mut body.stmts);
                }
            }
            _ => {}
        }
    }

    fn rewrite_immediate_statements(&mut self, statements: &mut [Stmt]) {
        for statement in statements {
            match statement {
                Stmt::Expr(statement) => {
                    self.rewrite_immediate_expression(&mut statement.expr);
                }
                Stmt::Decl(_) | Stmt::Empty(_) | Stmt::Debugger(_) => {}
                // A branch, loop, switch, try, jump, or nested statement shell
                // may skip both its own body and every following statement.
                // Stop instead of claiming dominance that has not been proven.
                _ => break,
            }
        }
    }

    /// Webpack has already initialized `module.exports`, so the CommonJS arm
    /// of `module.exports ? module.exports = value : browserFallback` is the
    /// only reachable arm inside an extracted factory.
    fn truthy_module_exports_branch(&self, expr: &Expr) -> Option<Expr> {
        let Expr::Cond(conditional) = strip_parens(expr) else {
            return None;
        };
        if !self.is_module_exports_expr(&conditional.test) {
            return None;
        }
        let assignment = self.module_exports_assignment(&conditional.cons)?;
        Some(self.capture_assignment(assignment.span, assignment.right.clone()))
    }

    /// Recover the minified UMD form
    ///
    /// ```text
    /// undefined === (result = factory.apply(exports, []))
    ///     || (module.exports = result)
    /// ```
    ///
    /// only when the immediately-created factory returns a stable function
    /// binding. That both proves the result is non-undefined and makes lifting
    /// the binding read out of the factory scope safe. Broader non-undefined
    /// expressions (especially arrows that capture the factory's `this`) fail
    /// closed. Replacing the native `apply` call relies on the project's
    /// `stable_builtins` assumption; the exact generated webpack/package shape
    /// is covered by a pinned producer fixture.
    fn non_undefined_factory_export(&self, expr: &Expr) -> Option<Expr> {
        let Expr::Bin(BinExpr {
            op: BinaryOp::LogicalOr,
            left,
            right,
            ..
        }) = strip_parens(expr)
        else {
            return None;
        };
        let export_assignment = self.module_exports_assignment(right)?;
        let Expr::Ident(exported_local) = strip_parens(&export_assignment.right) else {
            return None;
        };
        let value_assignment = self.undefined_checked_assignment(left)?;
        let AssignTarget::Simple(SimpleAssignTarget::Ident(value_target)) = &value_assignment.left
        else {
            return None;
        };
        if (value_target.id.sym.clone(), value_target.id.ctxt)
            != (exported_local.sym.clone(), exported_local.ctxt)
        {
            return None;
        }

        let returned = self.immediate_apply_return(&value_assignment.right)?;
        let mut value_assignment = value_assignment.clone();
        value_assignment.right = returned;
        Some(self.capture_assignment(
            export_assignment.span,
            Box::new(Expr::Assign(value_assignment)),
        ))
    }

    fn undefined_checked_assignment<'a>(&self, expr: &'a Expr) -> Option<&'a AssignExpr> {
        let Expr::Bin(BinExpr {
            op: BinaryOp::EqEq | BinaryOp::EqEqEq,
            left,
            right,
            ..
        }) = strip_parens(expr)
        else {
            return None;
        };
        match (self.is_undefined_expr(left), self.is_undefined_expr(right)) {
            (true, false) => self.simple_assignment(right),
            (false, true) => self.simple_assignment(left),
            _ => None,
        }
    }

    fn immediate_apply_return(&self, expr: &Expr) -> Option<Box<Expr>> {
        let Expr::Call(call) = strip_parens(expr) else {
            return None;
        };
        if call.args.len() != 2 || call.args.iter().any(|argument| argument.spread.is_some()) {
            return None;
        }
        let Expr::Ident(exports) = strip_parens(&call.args[0].expr) else {
            return None;
        };
        if exports.sym.as_ref() != "exports" || exports.ctxt.outer() != self.unresolved_mark {
            return None;
        }
        let Expr::Array(arguments) = strip_parens(&call.args[1].expr) else {
            return None;
        };
        if !arguments.elems.is_empty() {
            return None;
        }

        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(callee) else {
            return None;
        };
        if !matches!(prop, MemberProp::Ident(property) if property.sym.as_ref() == "apply") {
            return None;
        }
        let Expr::Fn(factory) = strip_parens(obj) else {
            return None;
        };
        if factory.function.is_async
            || factory.function.is_generator
            || !factory.function.params.is_empty()
        {
            return None;
        }
        let body = factory.function.body.as_ref()?;
        let [Stmt::Return(return_statement)] = body.stmts.as_slice() else {
            return None;
        };
        let returned = return_statement.arg.as_ref()?;
        let Expr::Ident(returned) = strip_parens(returned) else {
            return None;
        };
        self.function_bindings
            .contains(&(returned.sym.clone(), returned.ctxt))
            .then(|| Box::new(Expr::Ident(returned.clone())))
    }

    fn simple_assignment<'a>(&self, expr: &'a Expr) -> Option<&'a AssignExpr> {
        let Expr::Assign(assignment) = strip_parens(expr) else {
            return None;
        };
        (assignment.op == AssignOp::Assign).then_some(assignment)
    }

    fn module_exports_assignment<'a>(&self, expr: &'a Expr) -> Option<&'a AssignExpr> {
        let assignment = self.simple_assignment(expr)?;
        self.is_module_exports_target(&assignment.left)
            .then_some(assignment)
    }

    fn is_module_exports_expr(&self, expr: &Expr) -> bool {
        let Expr::Member(member) = strip_parens(expr) else {
            return false;
        };
        self.is_module_exports_member(member)
    }

    fn is_module_exports_target(&self, target: &AssignTarget) -> bool {
        matches!(target, AssignTarget::Simple(SimpleAssignTarget::Member(member)) if self.is_module_exports_member(member))
    }

    fn is_module_exports_member(&self, member: &MemberExpr) -> bool {
        let Expr::Ident(module) = member.obj.as_ref() else {
            return false;
        };
        module.sym.as_ref() == "module"
            && module.ctxt.outer() == self.unresolved_mark
            && matches!(&member.prop, MemberProp::Ident(property) if property.sym.as_ref() == "exports")
    }

    fn is_undefined_expr(&self, expr: &Expr) -> bool {
        match strip_parens(expr) {
            Expr::Unary(unary) if unary.op == UnaryOp::Void => {
                matches!(strip_parens(&unary.arg), Expr::Lit(Lit::Num(number)) if number.value == 0.0)
            }
            Expr::Ident(ident) => {
                ident.sym.as_ref() == "undefined" && ident.ctxt.outer() == self.unresolved_mark
            }
            _ => false,
        }
    }

    fn capture_assignment(&self, span: swc_core::common::Span, value: Box<Expr>) -> Expr {
        Expr::Assign(AssignExpr {
            span,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Ident(BindingIdent::from(
                self.capture.clone(),
            ))),
            right: value,
        })
    }
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, SourceMap, GLOBALS};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::VisitMutWith;

    use super::*;
    use crate::driver::io::{apply_fixer, parse_js, print_js};

    fn normalize_with_module_id(
        source: &str,
        enabled: bool,
        numeric_module_id: Option<f64>,
        legacy_module_i: bool,
    ) -> String {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module =
                parse_js(source, "fixture.js", cm.clone()).expect("fixture should parse");
            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
            normalize_webpack_commonjs_runtime(
                &mut module,
                unresolved_mark,
                enabled,
                numeric_module_id,
                legacy_module_i,
            );
            apply_fixer(&mut module).expect("fixture should fix");
            print_js(&module, cm).expect("fixture should print")
        })
    }

    fn normalize(source: &str, enabled: bool) -> String {
        normalize_with_module_id(source, enabled, None, false)
    }

    #[test]
    fn css_tuple_uses_the_proven_numeric_module_identity() {
        let output = normalize_with_module_id(
            r#"
const content = loadContent();
content.push([module.id, "body {}", "", { version: 3 }]);
module.exports = content;
inject(content);
"#,
            true,
            Some(37.0),
            false,
        );

        assert!(output.contains("37,"), "{output}");
        assert!(!output.contains("module.id"), "{output}");
        assert!(output.contains("module.exports = content"), "{output}");
        assert!(!output.contains("_webpackCssDefault"), "{output}");
    }

    #[test]
    fn module_i_requires_a_legacy_container_fact() {
        let output = normalize_with_module_id(
            r#"const content = [[module.i, "body {}", ""]];"#,
            true,
            Some(37.0),
            false,
        );

        assert!(output.contains("module.i"), "{output}");
        assert!(!output.contains("37,"), "{output}");
    }

    #[test]
    fn css_locals_switch_restores_the_initial_commonjs_default() {
        let output = normalize_with_module_id(
            r#"
let content = loadContent();
if ((content = typeof content === "string"
    ? [[module.i, content, ""]]
    : content).locals) {
    module.exports = content.locals;
}
inject(content);
"#,
            true,
            Some(12.0),
            true,
        );

        assert!(output.contains("var _webpackCssDefault = {}"), "{output}");
        assert!(output.contains("12,"), "{output}");
        assert!(
            output.contains("_webpackCssDefault = content.locals"),
            "{output}"
        );
        assert!(
            output.contains("module.exports = _webpackCssDefault"),
            "{output}"
        );
        assert!(!output.contains("module.i"), "{output}");
    }

    #[test]
    fn generated_logical_and_locals_switch_restores_the_default() {
        let output = normalize_with_module_id(
            r#"
let content = loadContent();
(content = typeof content === "string"
    ? [[module.i, content, ""]]
    : content).locals && (module.exports = content.locals), inject(content);
"#,
            true,
            Some(12.0),
            true,
        );

        assert!(output.contains("var _webpackCssDefault = {}"), "{output}");
        assert!(output.contains("12,"), "{output}");
        assert!(
            output.contains("_webpackCssDefault = content.locals"),
            "{output}"
        );
        assert!(
            output.contains("module.exports = _webpackCssDefault"),
            "{output}"
        );
        assert!(!output.contains("module.i"), "{output}");
    }

    #[test]
    fn css_runtime_recovery_fails_closed_on_unproven_surfaces() {
        for input in [
            // A mismatched locals source is not the tested value.
            r#"
let content = loadContent();
const other = loadOther();
if ((content = [[module.id, "body {}", ""]]).locals) {
    module.exports = other.locals;
}
"#,
            // Nested control flow does not establish the top-level switch.
            r#"
const content = [[module.id, "body {}", ""]];
if (ready) {
    if (content.locals) module.exports = content.locals;
}
"#,
            // The generated `&&` form is accepted only as an immediate
            // top-level sequence element, never from another guarded arm.
            r#"
const content = [[module.id, "body {}", ""]];
ready && (content.locals && (module.exports = content.locals));
"#,
        ] {
            let output = normalize_with_module_id(input, true, Some(9.0), false);
            assert!(!output.contains("module.id"), "{output}");
            assert!(output.contains("9,"), "{output}");
            assert!(!output.contains("_webpackCssDefault"), "{output}");
        }

        // Direct eval can observe the removed factory binding, so even the
        // otherwise-proven identity substitution stays disabled.
        let direct_eval = normalize_with_module_id(
            r#"
const content = [[module.id, "body {}", ""]];
eval("module.exports");
"#,
            true,
            Some(9.0),
            false,
        );
        assert!(direct_eval.contains("module.id"), "{direct_eval}");

        // Computed metadata is not the generated CSS tuple contract.
        let computed = normalize_with_module_id(
            r#"const content = [[module["id"], "body {}", ""]];"#,
            true,
            Some(9.0),
            false,
        );
        assert!(computed.contains("module[\"id\"]"), "{computed}");

        let without_numeric_identity = normalize_with_module_id(
            r#"const content = [[module.id, "body {}", ""]];"#,
            true,
            None,
            false,
        );
        assert!(
            without_numeric_identity.contains("module.id"),
            "{without_numeric_identity}"
        );
    }

    #[test]
    fn css_identity_requires_a_stable_module_surface() {
        for input in [
            // A prior identity write invalidates the container key at the read.
            r#"
module.id = transform(module.id);
const content = [[module.id, "body {}", ""]];
"#,
            // A bare escape lets the callee mutate the module object.
            r#"
mutate(module);
const content = [[module.id, "body {}", ""]];
"#,
            // A computed write can hit the identity property.
            r#"
module[key] = value;
const content = [[module.id, "body {}", ""]];
"#,
        ] {
            let output = normalize_with_module_id(input, true, Some(9.0), false);
            assert!(!output.contains("9,"), "{output}");
        }
    }

    #[test]
    fn hot_module_replacement_reads_do_not_block_the_identity() {
        let output = normalize_with_module_id(
            r#"
const content = loadContent();
content.push([module.id, "body {}", ""]);
module.hot && module.hot.accept();
module.exports = content;
"#,
            true,
            Some(41.0),
            false,
        );
        assert!(output.contains("41,"), "{output}");
        assert!(!output.contains("module.id"), "{output}");
    }

    #[test]
    fn initialized_runtime_selects_the_commonjs_umd_arm() {
        let input = r#"
!function(root) {
    function choose(value) { return value; }
    module.exports ? module.exports = choose : root.syntheticChoose = choose;
}(this);
"#;
        let output = normalize(input, true);
        assert!(output.contains("_webpackDefault = choose"), "{output}");
        assert!(
            output.contains("export default _webpackDefault"),
            "{output}"
        );
        assert!(!output.contains("module.exports"), "{output}");
        assert!(!output.contains("syntheticChoose"), "{output}");

        let ordinary = normalize(input, false);
        assert!(ordinary.contains("syntheticChoose"), "{ordinary}");
    }

    #[test]
    fn local_module_binding_does_not_use_the_webpack_runtime_fact() {
        let output = normalize(
            r#"
!function(module, root) {
    function choose(value) { return value; }
    module.exports ? module.exports = choose : root.syntheticChoose = choose;
}(localModule, this);
"#,
            true,
        );
        assert!(output.contains("syntheticChoose"), "{output}");
    }

    #[test]
    fn deferred_runtime_shapes_fail_closed() {
        for input in [
            r#"
setTimeout(function() {
    function choose(value) { return value; }
    module.exports ? module.exports = choose : window.syntheticChoose = choose;
}, 0);
"#,
            r#"
setTimeout(() => {
    function choose(value) { return value; }
    module.exports ? module.exports = choose : window.syntheticChoose = choose;
}, 0);
"#,
            r#"
({
    get value() {
        function choose(value) { return value; }
        module.exports ? module.exports = choose : window.syntheticChoose = choose;
        return choose;
    }
});
"#,
            r#"
(class {
    constructor() {
        function choose(value) { return value; }
        module.exports ? module.exports = choose : window.syntheticChoose = choose;
    }
});
"#,
            r#"
(async function() {
    function choose(value) { return value; }
    await ready;
    module.exports ? module.exports = choose : window.syntheticChoose = choose;
})();
"#,
        ] {
            let output = normalize(input, true);

            assert!(output.contains("module.exports"), "{output}");
            assert!(output.contains("syntheticChoose"), "{output}");
            assert!(!output.contains("_webpackDefault"), "{output}");
            assert!(!output.contains("export default"), "{output}");
        }
    }

    #[test]
    fn control_dependent_runtime_shapes_fail_closed() {
        for guarded_body in [
            r#"if (window.flag) {
    module.exports ? module.exports = choose : window.syntheticChoose = choose;
}"#,
            r#"if (window.flag) return;
module.exports ? module.exports = choose : window.syntheticChoose = choose;"#,
            r#"window.flag && (module.exports ? module.exports = choose : window.syntheticChoose = choose);"#,
            r#"window.ready || (module.exports ? module.exports = choose : window.syntheticChoose = choose);"#,
            r#"window.flag
    ? (module.exports ? module.exports = choose : window.syntheticChoose = choose)
    : observe();"#,
            r#"try {
    observe();
    module.exports ? module.exports = choose : window.syntheticChoose = choose;
} catch (error) {}"#,
        ] {
            let input = format!(
                r#"
!function() {{
    function choose(value) {{ return value; }}
    {guarded_body}
}}();
"#
            );
            let output = normalize(&input, true);

            assert!(output.contains("module.exports"), "{output}");
            assert!(output.contains("syntheticChoose"), "{output}");
            assert!(!output.contains("_webpackDefault"), "{output}");
            assert!(!output.contains("export default"), "{output}");
        }
    }

    #[test]
    fn synchronous_arrow_iife_remains_supported() {
        let output = normalize(
            r#"
(() => {
    function choose(value) { return value; }
    module.exports ? module.exports = choose : window.syntheticChoose = choose;
})();
"#,
            true,
        );

        assert!(output.contains("_webpackDefault = choose"), "{output}");
        assert!(
            output.contains("export default _webpackDefault"),
            "{output}"
        );
        assert!(!output.contains("module.exports"), "{output}");
        assert!(!output.contains("syntheticChoose"), "{output}");
    }

    #[test]
    fn unconditional_sequence_elements_remain_supported() {
        let output = normalize(
            r#"
!function() {
    function choose(value) { return value; }
    choose.enabled = true,
        module.exports ? module.exports = choose : window.syntheticChoose = choose;
}();
"#,
            true,
        );

        assert!(output.contains("choose.enabled = true"), "{output}");
        assert!(output.contains("_webpackDefault = choose"), "{output}");
        assert!(
            output.contains("export default _webpackDefault"),
            "{output}"
        );
        assert!(!output.contains("module.exports"), "{output}");
        assert!(!output.contains("syntheticChoose"), "{output}");
    }

    #[test]
    fn other_runtime_uses_or_multiple_candidates_fail_closed() {
        for input in [
            r#"
!function(root) {
    module.exports = null;
    function choose(value) { return value; }
    module.exports ? module.exports = choose : root.syntheticChoose = choose;
}(this);
"#,
            r#"
!function(root) {
    function choose(value) { return value; }
    module.exports ? module.exports = choose : root.syntheticChoose = choose;
    module.exports ? module.exports = choose : root.syntheticChooseAgain = choose;
}(this);
"#,
        ] {
            let output = normalize(input, true);
            assert!(output.contains("module.exports"), "{output}");
            assert!(output.contains("syntheticChoose"), "{output}");
            assert!(!output.contains("_webpackDefault"), "{output}");
        }
    }

    #[test]
    fn non_undefined_immediate_factory_recovers_the_commonjs_assignment() {
        let output = normalize(
            r#"
let result;
!function() {
    function createValue(value) { return { value: value }; }
    void 0 === (result = function() { return createValue; }.apply(exports, []))
        || (module.exports = result);
}();
"#,
            true,
        );
        assert!(
            output.contains("_webpackDefault = result = createValue"),
            "{output}"
        );
        assert!(
            output.contains("export default _webpackDefault"),
            "{output}"
        );
        assert!(!output.contains(".apply(exports"), "{output}");
    }

    #[test]
    fn effectful_or_unknown_factory_returns_fail_closed() {
        for factory in [
            "function() { observe(); return createValue; }",
            "function() { return maybeValue; }",
            "function() { return () => this; }",
        ] {
            let input = format!(
                r#"
!function() {{
    function createValue(value) {{ return {{ value: value }}; }}
    let result;
    void 0 === (result = ({factory}).apply(exports, []))
        || (module.exports = result);
}}();
"#
            );
            let output = normalize(&input, true);
            assert!(output.contains(".apply(exports"), "{output}");
        }
    }

    #[test]
    fn reassigned_function_return_fails_closed() {
        let output = normalize(
            r#"
!function() {
    function createValue(value) { return { value: value }; }
    createValue = maybeValue;
    let result;
    void 0 === (result = function() { return createValue; }.apply(exports, []))
        || (module.exports = result);
}();
"#,
            true,
        );
        assert!(output.contains(".apply(exports"), "{output}");
        assert!(output.contains("module.exports"), "{output}");
    }
}
