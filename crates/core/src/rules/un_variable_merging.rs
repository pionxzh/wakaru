use crate::collections::{HashMap, HashSet};

use swc_core::common::Span;
use swc_core::ecma::ast::{
    ArrowExpr, Class, Constructor, Decl, DoWhileStmt, ExportDecl, Expr, FnDecl, ForInStmt,
    ForOfStmt, ForStmt, Function, Id, Ident, Lit, MemberExpr, Module, ModuleDecl, ModuleItem, Pat,
    Prop, PropName, PropOrSpread, StaticBlock, Stmt, TryStmt, UnaryOp, VarDecl, VarDeclKind,
    VarDeclOrExpr, VarDeclarator, WhileStmt,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::eval_utils::has_dynamic_scope_construct;
use crate::utils::paren::strip_parens;

pub struct UnVariableMerging;
pub struct UnVariableMergingDeclsOnly;

impl VisitMut for UnVariableMerging {
    fn visit_mut_module(&mut self, module: &mut Module) {
        // Moving an initializer earlier changes when its binding is written.
        // A `with` object or a direct `eval` can observe that write through a
        // path no syntactic check sees, so such a module keeps the plain split.
        let mut splitter = ForInitSplitter {
            reorder: !has_dynamic_scope_construct(module),
            frame: None,
        };
        module.visit_mut_children_with(&mut splitter);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_with(&mut ForInitSplitter::default());
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_with(&mut ForInitSplitter::default());
    }
}

impl VisitMut for UnVariableMergingDeclsOnly {
    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        split_stmts(stmts, None);
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        split_module_items(items, None);
    }
}

/// The full rule: splits declarations and hoists for-init declarators.
///
/// `reorder` allows moving an inert initializer (`is_inert_init`) ahead of a
/// kept header initializer; it is off when the module has a `with` statement
/// or a direct `eval`, or when the rule runs on a bare statement list.
/// `frame` holds the facts about the innermost enclosing function-like scope.
/// It is `None` at the module top level, where an inert initializer may move
/// only past other inert initializers.
#[derive(Default)]
struct ForInitSplitter {
    reorder: bool,
    frame: Option<FrameFacts>,
}

impl ForInitSplitter {
    fn with_frame(&mut self, facts: FrameFacts, visit: impl FnOnce(&mut Self)) {
        let saved = self.frame.replace(facts);
        visit(self);
        self.frame = saved;
    }

    fn context(&self) -> ForInitContext<'_> {
        ForInitContext {
            reorder: self.reorder,
            frame: self.frame.as_ref(),
        }
    }
}

impl VisitMut for ForInitSplitter {
    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        split_stmts(stmts, Some(&self.context()));
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        split_module_items(items, Some(&self.context()));
    }

    fn visit_mut_function(&mut self, function: &mut Function) {
        let facts = FrameFacts::of_function(function);
        self.with_frame(facts, |this| function.visit_mut_children_with(this));
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        let facts = FrameFacts::of_arrow(arrow);
        self.with_frame(facts, |this| arrow.visit_mut_children_with(this));
    }

    fn visit_mut_constructor(&mut self, ctor: &mut Constructor) {
        let facts = FrameFacts::of_constructor(ctor);
        self.with_frame(facts, |this| ctor.visit_mut_children_with(this));
    }

    fn visit_mut_static_block(&mut self, block: &mut StaticBlock) {
        let facts = FrameFacts::of_static_block(block);
        self.with_frame(facts, |this| block.visit_mut_children_with(this));
    }
}

/// What extraction may assume about the for statement being split.
struct ForInitContext<'a> {
    reorder: bool,
    frame: Option<&'a FrameFacts>,
}

/// `for_inits` is `None` for the decls-only pass, which leaves for headers
/// alone.
fn split_stmts(stmts: &mut Vec<Stmt>, for_inits: Option<&ForInitContext>) {
    let old = std::mem::take(stmts);
    let mut new_stmts: Vec<Stmt> = Vec::with_capacity(old.len());

    for stmt in old {
        match stmt {
            // For-loop with a var init: maybe extract some declarators before the loop
            Stmt::For(for_stmt) => match for_inits {
                Some(ctx) => new_stmts.extend(extract_for_init_stmts(for_stmt, ctx)),
                None => new_stmts.push(Stmt::For(for_stmt)),
            },
            // Plain var/let/const declaration with multiple declarators
            Stmt::Decl(Decl::Var(ref var_decl)) if var_decl.decls.len() > 1 => {
                let split = split_var_decl(var_decl);
                new_stmts.extend(
                    split
                        .into_iter()
                        .map(|v| Stmt::Decl(Decl::Var(Box::new(v)))),
                );
            }
            other => new_stmts.push(other),
        }
    }

    *stmts = new_stmts;
}

fn split_module_items(items: &mut Vec<ModuleItem>, for_inits: Option<&ForInitContext>) {
    let old = std::mem::take(items);
    let mut new_items: Vec<ModuleItem> = Vec::with_capacity(old.len());

    for item in old {
        match item {
            // export var a = 1, b = 2 → export var a = 1; export var b = 2;
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                span,
                decl: Decl::Var(ref var_decl),
            })) if var_decl.decls.len() > 1 => {
                let split = split_var_decl(var_decl);
                for v in split {
                    new_items.push(ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                        span,
                        decl: Decl::Var(Box::new(v)),
                    })));
                }
            }
            // Plain statement with multiple var declarators
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(ref var_decl))) if var_decl.decls.len() > 1 => {
                let split = split_var_decl(var_decl);
                for v in split {
                    new_items.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(v)))));
                }
            }
            // For-loop inside module items
            ModuleItem::Stmt(Stmt::For(for_stmt)) => match for_inits {
                Some(ctx) => {
                    for s in extract_for_init_stmts(for_stmt, ctx) {
                        new_items.push(ModuleItem::Stmt(s));
                    }
                }
                None => new_items.push(ModuleItem::Stmt(Stmt::For(for_stmt))),
            },
            other => new_items.push(other),
        }
    }

    *items = new_items;
}

/// Split a `VarDecl` with multiple declarators into multiple single-declarator `VarDecl`s.
fn split_var_decl(var_decl: &VarDecl) -> Vec<VarDecl> {
    var_decl
        .decls
        .iter()
        .map(|d| VarDecl {
            span: var_decl.span,
            ctxt: var_decl.ctxt,
            kind: var_decl.kind,
            declare: var_decl.declare,
            decls: vec![d.clone()],
        })
        .collect()
}

/// For `for (var a = 1, b = 2, c = 3; ...)`, extract declarators whose names do NOT appear
/// in the test or update expressions, and place them as separate `var` statements before
/// the for loop. Only applies to `var` kind (not `let` / `const`).
///
/// Returns a list of statements: [extracted vars..., modified for stmt].
fn extract_for_init_stmts(mut for_stmt: ForStmt, ctx: &ForInitContext) -> Vec<Stmt> {
    // Only act on `var` inits (any number of declarators)
    let should_process = matches!(
        &for_stmt.init,
        Some(VarDeclOrExpr::VarDecl(v))
            if v.kind == VarDeclKind::Var && !v.decls.is_empty()
    );

    if !should_process {
        return vec![Stmt::For(for_stmt)];
    }

    // Collect identifiers referenced in test and update (not body)
    let mut used_names: HashSet<String> = HashSet::default();
    if let Some(test) = &for_stmt.test {
        collect_ident_names_expr(test, &mut used_names);
    }
    if let Some(update) = &for_stmt.update {
        collect_ident_names_expr(update, &mut used_names);
    }

    // Take the VarDecl out of the for init
    let init_var = match std::mem::take(&mut for_stmt.init) {
        Some(VarDeclOrExpr::VarDecl(v)) => v,
        other => {
            for_stmt.init = other;
            return vec![Stmt::For(for_stmt)];
        }
    };

    let VarDecl {
        span,
        ctxt,
        kind,
        declare,
        decls,
    } = *init_var;

    // Phase 1: determine which declarator indices must stay in the for init.
    // Start with those whose bound names appear in test/update.
    let mut must_keep: HashSet<usize> = HashSet::default();
    for (i, decl) in decls.iter().enumerate() {
        let names = bound_names_pat(&decl.name);
        if names.iter().any(|n| used_names.contains(n)) {
            must_keep.insert(i);
        }
    }

    // Expand: any declarator whose init references a name bound by a must-keep declarator
    // must also stay. These cannot be safely extracted because var→let/const conversion
    // removes hoisting, and the referenced variable is only initialized at loop start.
    // Repeat until stable (handles transitive dependencies).
    loop {
        let keep_names: HashSet<String> = must_keep
            .iter()
            .flat_map(|&i| bound_names_pat(&decls[i].name))
            .collect();

        let mut changed = false;
        for (i, decl) in decls.iter().enumerate() {
            if must_keep.contains(&i) {
                continue;
            }
            let mut init_refs = HashSet::default();
            if let Some(init) = &decl.init {
                collect_ident_names_expr(init, &mut init_refs);
            }
            if init_refs.iter().any(|n| keep_names.contains(n)) {
                must_keep.insert(i);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Phase 2: partition declarators into extract/keep, preserving source order
    // on both sides. Extraction moves a declarator's initializer before every
    // kept initializer, so a declarator WITH an initializer is only extracted
    // while no kept declarator with an initializer precedes it. A declarator
    // WITHOUT an initializer evaluates nothing, so extracting it from any
    // position never reorders effects (and matchers like UnForOf rely on the
    // bare `var step;` being pulled out of the header). An inert initializer
    // may also move past kept initializers when `can_move_past_kept` proves
    // the earlier write unobservable.
    let for_facts = ctx
        .frame
        .and_then(|frame| frame.fors.get(&for_stmt.span))
        .and_then(Option::as_ref);
    let mut extract_before: Vec<VarDeclarator> = Vec::new();
    let mut keep_in_for: Vec<VarDeclarator> = Vec::new();
    let mut kept_initializer = false;
    for (i, decl) in decls.into_iter().enumerate() {
        if !must_keep.contains(&i)
            && (decl.init.is_none()
                || !kept_initializer
                || can_move_past_kept(&decl, &keep_in_for, ctx, for_facts))
        {
            extract_before.push(decl);
        } else {
            kept_initializer |= decl.init.is_some();
            keep_in_for.push(decl);
        }
    }

    let mut result: Vec<Stmt> = Vec::new();

    // Emit extracted declarators as individual var statements before the loop
    for d in extract_before {
        result.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
            span,
            ctxt,
            kind,
            declare,
            decls: vec![d],
        }))));
    }

    // Rebuild the for statement's init
    if keep_in_for.is_empty() {
        for_stmt.init = None;
    } else {
        for_stmt.init = Some(VarDeclOrExpr::VarDecl(Box::new(VarDecl {
            span,
            ctxt,
            kind,
            declare,
            decls: keep_in_for,
        })));
    }

    result.push(Stmt::For(for_stmt));
    result
}

/// Whether the declarator `decl`, which no test or update reads, can be
/// extracted although `kept_before` (the declarators kept in the header
/// before it) holds an initializer.
///
/// Extraction writes the binding before those initializers run instead of
/// after them, so the move is allowed only when nothing evaluated in between
/// can read or write the binding:
///
/// - The binding is a plain identifier and its initializer is inert
///   (`is_inert_init`): evaluating it runs no code and reads no binding, so
///   only the time of the write changes. This also keeps it from reading any
///   binding declared earlier in the header.
/// - No kept declarator before it mentions the name, `arguments`, or `eval`.
///   That rules out a direct read or write and a closure created there.
/// - If those kept declarators are all inert too, no code runs between the
///   two positions. Otherwise code runs there, and a call can reach any
///   closure that exists by then, so the binding must be out of its reach:
///   - the loop sits in a function-like scope, outside every `try` of that
///     scope, so a throw from the kept initializers leaves the scope and
///     abandons the binding instead of reaching a `catch` or `finally` that
///     reads it;
///   - the binding is not a parameter, which a mapped `arguments` object
///     aliases;
///   - every closure that references the binding is a non-hoisted one
///     created after the header's init, and the loop sits outside every
///     loop of that scope, so no such closure exists from an earlier
///     iteration.
///
/// A module with a `with` statement or a direct `eval` turns this off
/// (`ForInitContext::reorder`).
fn can_move_past_kept(
    decl: &VarDeclarator,
    kept_before: &[VarDeclarator],
    ctx: &ForInitContext,
    for_facts: Option<&ForFacts>,
) -> bool {
    if !ctx.reorder {
        return false;
    }
    let Pat::Ident(binding) = &decl.name else {
        return false;
    };
    if !decl.init.as_deref().is_some_and(is_inert_init) {
        return false;
    }
    let names = [binding.id.sym.as_str(), "arguments", "eval"];
    if kept_before.iter().any(|kept| mentions_any(kept, &names)) {
        return false;
    }
    if kept_before.iter().all(is_inert_declarator) {
        return true;
    }
    let (Some(frame), Some(for_facts)) = (ctx.frame, for_facts) else {
        return false;
    };
    if for_facts.in_try {
        return false;
    }
    let id = binding.id.to_id();
    if frame.params.contains(&id) {
        return false;
    }
    frame.captures.get(&id).is_none_or(|capture| {
        !for_facts.in_loop && !capture.hoisted && capture.first > for_facts.init_end
    })
}

fn is_inert_declarator(decl: &VarDeclarator) -> bool {
    matches!(decl.name, Pat::Ident(_)) && decl.init.as_deref().is_none_or(is_inert_init)
}

/// An initializer whose evaluation runs no code and reads no binding: an
/// inert primitive (`is_inert_primitive`), or an array or object literal
/// whose entries are all inert primitives under non-computed keys. An entry
/// that is anything else (a call, a read, a spread, a nested literal) makes
/// the whole literal non-inert.
fn is_inert_init(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Array(array) => array.elems.iter().all(|elem| {
            elem.as_ref()
                .is_none_or(|elem| elem.spread.is_none() && is_inert_primitive(&elem.expr))
        }),
        Expr::Object(object) => object.props.iter().all(|prop| {
            matches!(prop, PropOrSpread::Prop(prop) if matches!(
                &**prop,
                Prop::KeyValue(kv)
                    if !matches!(kv.key, PropName::Computed(_)) && is_inert_primitive(&kv.value)
            ))
        }),
        expr => is_inert_primitive(expr),
    }
}

/// A primitive literal other than a regular expression, the same under
/// unary `-`, `~`, `!`, or `void` (none of which can call user code on a
/// primitive; unary `+` is left out because it throws on a BigInt), or a
/// template without substitutions.
fn is_inert_primitive(expr: &Expr) -> bool {
    fn is_primitive_lit(expr: &Expr) -> bool {
        matches!(expr, Expr::Lit(lit) if !matches!(lit, Lit::Regex(_) | Lit::JSXText(_)))
    }
    match strip_parens(expr) {
        Expr::Unary(unary) => {
            matches!(
                unary.op,
                UnaryOp::Minus | UnaryOp::Tilde | UnaryOp::Bang | UnaryOp::Void
            ) && is_primitive_lit(strip_parens(&unary.arg))
        }
        Expr::Tpl(tpl) => tpl.exprs.is_empty(),
        expr => is_primitive_lit(expr),
    }
}

/// Whether any identifier in `decl` (pattern or initializer, nested
/// functions included) is spelled like one of `names`.
fn mentions_any(decl: &VarDeclarator, names: &[&str]) -> bool {
    struct Finder<'a> {
        names: &'a [&'a str],
        found: bool,
    }
    impl Visit for Finder<'_> {
        fn visit_ident(&mut self, ident: &Ident) {
            self.found |= self.names.contains(&ident.sym.as_str());
        }
    }
    let mut finder = Finder {
        names,
        found: false,
    };
    decl.visit_with(&mut finder);
    finder.found
}

/// Facts about one function-like scope (a function, arrow, constructor, or
/// class static block) that `can_move_past_kept` needs, computed from the
/// scope's syntax before the rule changes it.
#[derive(Default)]
struct FrameFacts {
    /// The for statements this scope owns (not those in nested closures),
    /// keyed by span. `None` marks a span several statements share, which
    /// therefore identifies none of them.
    fors: HashMap<Span, Option<ForFacts>>,
    /// Every binding referenced inside a closure nested in this scope.
    captures: HashMap<Id, Capture>,
    /// Bindings declared in the scope's parameter list.
    params: HashSet<Id>,
}

struct ForFacts {
    /// Inside a loop statement of this scope (its own body excluded).
    in_loop: bool,
    /// Inside a `try` statement of this scope.
    in_try: bool,
    /// The creation order of the last closure in the for header's init; a
    /// closure numbered higher is created after the init runs.
    init_end: usize,
}

struct Capture {
    /// Creation order of the first closure that references the binding.
    first: usize,
    /// Some closure that references it is a hoisted function declaration,
    /// which exists before any statement of the scope runs.
    hoisted: bool,
}

impl FrameFacts {
    fn of_function(function: &Function) -> Self {
        let mut analyzer = FrameAnalyzer::default();
        analyzer.collect_params(&function.params);
        function.params.visit_with(&mut analyzer);
        function.body.visit_with(&mut analyzer);
        analyzer.facts
    }

    fn of_arrow(arrow: &ArrowExpr) -> Self {
        let mut analyzer = FrameAnalyzer::default();
        analyzer.collect_params(&arrow.params);
        arrow.params.visit_with(&mut analyzer);
        arrow.body.visit_with(&mut analyzer);
        analyzer.facts
    }

    fn of_constructor(ctor: &Constructor) -> Self {
        let mut analyzer = FrameAnalyzer::default();
        analyzer.collect_params(&ctor.params);
        ctor.params.visit_with(&mut analyzer);
        ctor.body.visit_with(&mut analyzer);
        analyzer.facts
    }

    fn of_static_block(block: &StaticBlock) -> Self {
        let mut analyzer = FrameAnalyzer::default();
        block.body.visit_with(&mut analyzer);
        analyzer.facts
    }
}

/// Walks one scope in source order, numbering the closures it creates and
/// the for statements it owns from one counter, so that `a < b` means `a`
/// comes first when the scope runs straight through. Loops are the one way
/// back, which is why `ForFacts::in_loop` is recorded.
#[derive(Default)]
struct FrameAnalyzer {
    facts: FrameFacts,
    ordinal: usize,
    loop_depth: usize,
    try_depth: usize,
    /// Number and hoisting of the outermost nested closure being walked.
    /// A closure nested deeper only exists once that one has run.
    closure: Option<(usize, bool)>,
}

impl FrameAnalyzer {
    fn collect_params<N: for<'a> VisitWith<ParamBindings<'a>>>(&mut self, params: &N) {
        params.visit_with(&mut ParamBindings(&mut self.facts.params));
    }

    fn enter_closure(&mut self, hoisted: bool, visit: impl FnOnce(&mut Self)) {
        if self.closure.is_some() {
            visit(self);
            return;
        }
        self.ordinal += 1;
        self.closure = Some((self.ordinal, hoisted));
        visit(self);
        self.closure = None;
    }

    fn in_loop(&mut self, visit: impl FnOnce(&mut Self)) {
        self.loop_depth += 1;
        visit(self);
        self.loop_depth -= 1;
    }
}

impl Visit for FrameAnalyzer {
    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.enter_closure(true, |this| decl.function.visit_children_with(this));
    }

    fn visit_function(&mut self, function: &Function) {
        self.enter_closure(false, |this| function.visit_children_with(this));
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.enter_closure(false, |this| arrow.visit_children_with(this));
    }

    fn visit_class(&mut self, class: &Class) {
        self.enter_closure(false, |this| class.visit_children_with(this));
    }

    fn visit_ident(&mut self, ident: &Ident) {
        let Some((ordinal, hoisted)) = self.closure else {
            return;
        };
        let capture = self.facts.captures.entry(ident.to_id()).or_insert(Capture {
            first: ordinal,
            hoisted,
        });
        capture.first = capture.first.min(ordinal);
        capture.hoisted |= hoisted;
    }

    fn visit_for_stmt(&mut self, stmt: &ForStmt) {
        if self.closure.is_some() {
            stmt.visit_children_with(self);
            return;
        }
        self.ordinal += 1;
        let in_loop = self.loop_depth > 0;
        let in_try = self.try_depth > 0;
        stmt.init.visit_with(self);
        let facts = ForFacts {
            in_loop,
            in_try,
            init_end: self.ordinal,
        };
        if !stmt.span.is_dummy() {
            self.facts
                .fors
                .entry(stmt.span)
                .and_modify(|slot| *slot = None)
                .or_insert(Some(facts));
        }
        self.in_loop(|this| {
            stmt.test.visit_with(this);
            stmt.update.visit_with(this);
            stmt.body.visit_with(this);
        });
    }

    fn visit_while_stmt(&mut self, stmt: &WhileStmt) {
        self.in_loop(|this| stmt.visit_children_with(this));
    }

    fn visit_do_while_stmt(&mut self, stmt: &DoWhileStmt) {
        self.in_loop(|this| stmt.visit_children_with(this));
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        self.in_loop(|this| stmt.visit_children_with(this));
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        self.in_loop(|this| stmt.visit_children_with(this));
    }

    fn visit_try_stmt(&mut self, stmt: &TryStmt) {
        self.try_depth += 1;
        stmt.visit_children_with(self);
        self.try_depth -= 1;
    }
}

/// Collects every identifier a parameter list binds (and, conservatively,
/// any binding inside a default value).
struct ParamBindings<'a>(&'a mut HashSet<Id>);

impl Visit for ParamBindings<'_> {
    fn visit_binding_ident(&mut self, ident: &swc_core::ecma::ast::BindingIdent) {
        self.0.insert(ident.id.to_id());
    }
}

/// Collect all identifier names referenced (not bound) in an expression.
fn collect_ident_names_expr(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Ident(i) => {
            out.insert(i.sym.to_string());
        }
        Expr::Member(MemberExpr { obj, prop, .. }) => {
            collect_ident_names_expr(obj, out);
            // Only computed props introduce new ident refs; static props are property names
            if let swc_core::ecma::ast::MemberProp::Computed(c) = prop {
                collect_ident_names_expr(&c.expr, out);
            }
        }
        Expr::Call(c) => {
            if let swc_core::ecma::ast::Callee::Expr(e) = &c.callee {
                collect_ident_names_expr(e, out);
            }
            for arg in &c.args {
                collect_ident_names_expr(&arg.expr, out);
            }
        }
        Expr::Bin(b) => {
            collect_ident_names_expr(&b.left, out);
            collect_ident_names_expr(&b.right, out);
        }
        Expr::Unary(u) => collect_ident_names_expr(&u.arg, out),
        Expr::Update(u) => collect_ident_names_expr(&u.arg, out),
        Expr::Assign(a) => {
            collect_ident_names_expr(&a.right, out);
        }
        Expr::Seq(s) => {
            for e in &s.exprs {
                collect_ident_names_expr(e, out);
            }
        }
        Expr::Cond(c) => {
            collect_ident_names_expr(&c.test, out);
            collect_ident_names_expr(&c.cons, out);
            collect_ident_names_expr(&c.alt, out);
        }
        Expr::New(n) => {
            collect_ident_names_expr(&n.callee, out);
            if let Some(args) = &n.args {
                for arg in args {
                    collect_ident_names_expr(&arg.expr, out);
                }
            }
        }
        Expr::Array(a) => {
            for elem in a.elems.iter().flatten() {
                collect_ident_names_expr(&elem.expr, out);
            }
        }
        Expr::Object(o) => {
            for prop in &o.props {
                if let swc_core::ecma::ast::PropOrSpread::Prop(p) = prop {
                    match p.as_ref() {
                        swc_core::ecma::ast::Prop::KeyValue(kv) => {
                            collect_ident_names_expr(&kv.value, out);
                        }
                        swc_core::ecma::ast::Prop::Shorthand(i) => {
                            out.insert(i.sym.to_string());
                        }
                        _ => {}
                    }
                }
            }
        }
        Expr::Paren(p) => collect_ident_names_expr(&p.expr, out),
        _ => {}
    }
}

/// Collect all identifier names bound in a pattern (the LHS of a declarator).
fn bound_names_pat(pat: &Pat) -> Vec<String> {
    let mut names = Vec::new();
    collect_bound_names(pat, &mut names);
    names
}

fn collect_bound_names(pat: &Pat, out: &mut Vec<String>) {
    match pat {
        Pat::Ident(i) => out.push(i.sym.to_string()),
        Pat::Array(a) => {
            for elem in a.elems.iter().flatten() {
                collect_bound_names(elem, out);
            }
        }
        Pat::Object(o) => {
            for prop in &o.props {
                match prop {
                    swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                        collect_bound_names(&kv.value, out);
                    }
                    swc_core::ecma::ast::ObjectPatProp::Assign(a) => {
                        out.push(a.key.sym.to_string());
                    }
                    swc_core::ecma::ast::ObjectPatProp::Rest(r) => {
                        collect_bound_names(&r.arg, out);
                    }
                }
            }
        }
        Pat::Rest(r) => collect_bound_names(&r.arg, out),
        Pat::Assign(a) => collect_bound_names(&a.left, out),
        _ => {}
    }
}
