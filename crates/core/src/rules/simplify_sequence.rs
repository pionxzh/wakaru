use crate::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignTarget, BinaryOp, BlockStmt, Callee, ClassExpr, Constructor, Decl,
    Expr, ExprOrSpread, ExprStmt, FnExpr, ForHead, ForInStmt, ForOfStmt, ForStmt, Function,
    GetterProp, Ident, IfStmt, ImportSpecifier, Invalid, Lit, MemberExpr, MemberProp, ModuleDecl,
    ModuleItem, OptChainBase, ParenExpr, Pat, Prop, PropName, PropOrSpread, ReturnStmt, SeqExpr,
    SetterProp, SimpleAssignTarget, Stmt, SwitchStmt, ThrowStmt, UnaryExpr, UnaryOp, VarDecl,
    VarDeclKind, VarDeclOrExpr, VarDeclarator, YieldExpr,
};
use swc_core::ecma::utils::{ExprCtx, ExprExt};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::decl_utils::{binding_id, BindingId};
use super::RewriteLevel;

use crate::js_names::is_stable_builtin_alias_root;
use crate::utils::paren::strip_parens;

pub struct SimplifySequence {
    unresolved_mark: Mark,
    level: RewriteLevel,
    source_import_reads_are_observable: bool,
    observable_ident_reads: HashSet<BindingId>,
    nested_observable_ident_reads: HashSet<BindingId>,
    lexical_scopes: Vec<LexicalScope>,
    function_lexical_scope_depths: Vec<usize>,
}

struct LexicalScope {
    // Retained for deferred function execution: a body may run before an
    // enclosing lexical binding reaches its initializer.
    all: HashSet<BindingId>,
    // Drained in source order for immediately evaluated statements.
    future: HashSet<BindingId>,
}

impl LexicalScope {
    fn new(bindings: HashSet<BindingId>) -> Self {
        Self {
            all: bindings.clone(),
            future: bindings,
        }
    }
}

impl SimplifySequence {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self::new_with_level(unresolved_mark, RewriteLevel::Standard)
    }

    pub fn new_with_level(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self::new_with_import_semantics(unresolved_mark, level, true)
    }

    pub(crate) fn new_with_import_semantics(
        unresolved_mark: Mark,
        level: RewriteLevel,
        source_import_reads_are_observable: bool,
    ) -> Self {
        Self {
            unresolved_mark,
            level,
            source_import_reads_are_observable,
            observable_ident_reads: HashSet::default(),
            nested_observable_ident_reads: HashSet::default(),
            lexical_scopes: Vec::new(),
            function_lexical_scope_depths: Vec::new(),
        }
    }

    fn visit_function_like_children<T>(&mut self, node: &mut T)
    where
        T: VisitMutWith<Self>,
    {
        self.function_lexical_scope_depths
            .push(self.lexical_scopes.len());
        node.visit_mut_children_with(self);
        self.function_lexical_scope_depths.pop();
    }
}

impl VisitMut for SimplifySequence {
    fn visit_mut_module(&mut self, module: &mut swc_core::ecma::ast::Module) {
        let outer_observable_ident_reads = self.observable_ident_reads.clone();
        let outer_nested_observable_ident_reads = self.nested_observable_ident_reads.clone();
        let import_bindings = collect_import_binding_ids_from_module_items(&module.body);
        self.observable_ident_reads
            .extend(import_bindings.iter().cloned());
        if self.source_import_reads_are_observable {
            self.nested_observable_ident_reads.extend(import_bindings);
        }
        self.observable_ident_reads
            .extend(collect_cjs_require_binding_ids_from_module_items(
                &module.body,
                self.unresolved_mark,
            ));

        module.visit_mut_children_with(self);

        self.observable_ident_reads = outer_observable_ident_reads;
        self.nested_observable_ident_reads = outer_nested_observable_ident_reads;
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        let old_items = std::mem::take(items);
        let mut new_items = Vec::with_capacity(old_items.len());
        self.lexical_scopes.push(LexicalScope::new(
            collect_lexical_decl_ids_from_module_items(&old_items),
        ));

        for mut item in old_items {
            item.visit_mut_children_with(self);

            match item {
                ModuleItem::Stmt(stmt) => {
                    let declared = collect_lexical_decl_ids_from_stmt(&stmt);
                    for stmt in split_stmt(stmt, self.level) {
                        if !is_pure_no_op_stmt(
                            &stmt,
                            self.unresolved_mark,
                            &self.lexical_scopes,
                            self.function_lexical_scope_depths.last().copied(),
                            &self.observable_ident_reads,
                            &self.nested_observable_ident_reads,
                        ) {
                            new_items.push(ModuleItem::Stmt(stmt));
                        }
                    }
                    remove_ids_from_current_scope(&mut self.lexical_scopes, &declared);
                }
                ModuleItem::ModuleDecl(decl) => {
                    let declared = collect_lexical_decl_ids_from_module_decl(&decl);
                    new_items.push(ModuleItem::ModuleDecl(decl));
                    remove_ids_from_current_scope(&mut self.lexical_scopes, &declared);
                }
            }
        }

        self.lexical_scopes.pop();
        *items = new_items;
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        let old_stmts = std::mem::take(stmts);
        let mut new_stmts = Vec::with_capacity(old_stmts.len());
        self.lexical_scopes
            .push(LexicalScope::new(collect_lexical_decl_ids_from_stmts(
                &old_stmts,
            )));

        for mut stmt in old_stmts {
            stmt.visit_mut_children_with(self);

            let declared = collect_lexical_decl_ids_from_stmt(&stmt);
            for s in split_stmt(stmt, self.level) {
                if !is_pure_no_op_stmt(
                    &s,
                    self.unresolved_mark,
                    &self.lexical_scopes,
                    self.function_lexical_scope_depths.last().copied(),
                    &self.observable_ident_reads,
                    &self.nested_observable_ident_reads,
                ) {
                    new_stmts.push(s);
                }
            }
            remove_ids_from_current_scope(&mut self.lexical_scopes, &declared);
        }

        self.lexical_scopes.pop();
        *stmts = new_stmts;
    }

    fn visit_mut_function(&mut self, function: &mut Function) {
        self.visit_function_like_children(function);
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        self.visit_function_like_children(arrow);
    }

    fn visit_mut_constructor(&mut self, constructor: &mut Constructor) {
        self.visit_function_like_children(constructor);
    }

    fn visit_mut_getter_prop(&mut self, getter: &mut GetterProp) {
        self.visit_function_like_children(getter);
    }

    fn visit_mut_setter_prop(&mut self, setter: &mut SetterProp) {
        self.visit_function_like_children(setter);
    }
}

/// Returns true for expression statements that are provably side-effect-free.
/// String literals are intentionally excluded because they may be directive prologues
/// (e.g., "use strict") handled by a later pass.
fn is_pure_no_op_stmt(
    stmt: &Stmt,
    unresolved_mark: Mark,
    lexical_scopes: &[LexicalScope],
    function_lexical_scope_depth: Option<usize>,
    observable_ident_reads: &HashSet<BindingId>,
    nested_observable_ident_reads: &HashSet<BindingId>,
) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    // Never drop string literals — may be "use strict" directives
    if matches!(expr.as_ref(), Expr::Lit(Lit::Str(_))) {
        return false;
    }
    // Never drop function/arrow/class expressions — they can represent
    // intentional wrapper patterns, and class evaluation can throw.
    if is_fn_arrow_or_class(expr) {
        return false;
    }
    // Identifier reads are observable: unresolved identifiers throw ReferenceError,
    // and lexical bindings can throw before initialization (TDZ).
    if is_observable_ident_read(
        expr,
        unresolved_mark,
        lexical_scopes,
        function_lexical_scope_depth,
        observable_ident_reads,
        nested_observable_ident_reads,
    ) {
        return false;
    }
    if is_observable_typeof(expr, unresolved_mark) {
        return false;
    }
    if is_this_read(expr) {
        return false;
    }
    // Object literal evaluation can perform observable identifier lookups,
    // spreads, value evaluation, and computed-key coercion even when its result
    // is unused.
    if has_object_literal(expr) {
        return false;
    }
    if has_observable_binary_coercion(expr) {
        return false;
    }
    if has_observable_unary_throw(expr) {
        return false;
    }
    if has_new_expr(expr) {
        return false;
    }
    let unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
    // `Math.random()` advances the global PRNG. SWC treats every unresolved
    // `Math.*` method as a pure callee, so a discarded call would be deleted
    // and later samples would shift. A reference that is not called stays removable.
    if expr_calls_global_math_random(expr, unresolved_ctxt) {
        return false;
    }
    let ctx = ExprCtx {
        unresolved_ctxt,
        is_unresolved_ref_safe: false,
        in_strict: false,
        remaining_depth: 4,
    };
    !expr.may_have_side_effects(ctx)
}

/// True when evaluating `expr` invokes unresolved `Math.random`.
/// Function, arrow, and class bodies are not evaluated by the surrounding expression.
fn expr_calls_global_math_random(expr: &Expr, unresolved_ctxt: SyntaxContext) -> bool {
    match expr {
        Expr::Fn(_) | Expr::Arrow(_) | Expr::Class(_) => false,
        Expr::Paren(paren) => expr_calls_global_math_random(&paren.expr, unresolved_ctxt),
        Expr::Unary(unary) => expr_calls_global_math_random(&unary.arg, unresolved_ctxt),
        Expr::Bin(bin) => {
            expr_calls_global_math_random(&bin.left, unresolved_ctxt)
                || expr_calls_global_math_random(&bin.right, unresolved_ctxt)
        }
        Expr::Seq(seq) => seq
            .exprs
            .iter()
            .any(|item| expr_calls_global_math_random(item, unresolved_ctxt)),
        Expr::Cond(cond) => {
            expr_calls_global_math_random(&cond.test, unresolved_ctxt)
                || expr_calls_global_math_random(&cond.cons, unresolved_ctxt)
                || expr_calls_global_math_random(&cond.alt, unresolved_ctxt)
        }
        Expr::Call(call) => call_invokes_global_math_random(
            call.callee.as_expr().map(|callee| callee.as_ref()),
            &call.args,
            unresolved_ctxt,
        ),
        Expr::OptChain(opt) => match opt.base.as_ref() {
            OptChainBase::Call(call) => call_invokes_global_math_random(
                Some(call.callee.as_ref()),
                &call.args,
                unresolved_ctxt,
            ),
            OptChainBase::Member(member) => {
                expr_calls_global_math_random(&member.obj, unresolved_ctxt)
                    || computed_prop_calls_global_math_random(&member.prop, unresolved_ctxt)
            }
        },
        Expr::Member(member) => {
            expr_calls_global_math_random(&member.obj, unresolved_ctxt)
                || computed_prop_calls_global_math_random(&member.prop, unresolved_ctxt)
        }
        // SWC treats these as pure when every nested value is a pure callee,
        // including Math.random(). Evaluating them still samples.
        Expr::Array(array) => array
            .elems
            .iter()
            .flatten()
            .any(|elem| expr_calls_global_math_random(&elem.expr, unresolved_ctxt)),
        Expr::Object(object) => object.props.iter().any(|prop| match prop {
            PropOrSpread::Spread(spread) => {
                expr_calls_global_math_random(&spread.expr, unresolved_ctxt)
            }
            PropOrSpread::Prop(prop) => match prop.as_ref() {
                Prop::KeyValue(kv) => {
                    prop_name_calls_global_math_random(&kv.key, unresolved_ctxt)
                        || expr_calls_global_math_random(&kv.value, unresolved_ctxt)
                }
                Prop::Getter(getter) => {
                    prop_name_calls_global_math_random(&getter.key, unresolved_ctxt)
                }
                Prop::Setter(setter) => {
                    prop_name_calls_global_math_random(&setter.key, unresolved_ctxt)
                }
                Prop::Method(method) => {
                    prop_name_calls_global_math_random(&method.key, unresolved_ctxt)
                }
                Prop::Shorthand(_) | Prop::Assign(_) => false,
            },
        }),
        Expr::New(new_expr) => {
            expr_calls_global_math_random(&new_expr.callee, unresolved_ctxt)
                || new_expr
                    .args
                    .iter()
                    .flatten()
                    .any(|arg| expr_calls_global_math_random(&arg.expr, unresolved_ctxt))
        }
        _ => false,
    }
}

fn call_invokes_global_math_random(
    callee: Option<&Expr>,
    args: &[ExprOrSpread],
    unresolved_ctxt: SyntaxContext,
) -> bool {
    if callee
        .is_some_and(|callee| is_global_math_random_callee(strip_parens(callee), unresolved_ctxt))
    {
        return true;
    }
    callee.is_some_and(|callee| expr_calls_global_math_random(callee, unresolved_ctxt))
        || args
            .iter()
            .any(|arg| expr_calls_global_math_random(&arg.expr, unresolved_ctxt))
}

fn prop_name_calls_global_math_random(name: &PropName, unresolved_ctxt: SyntaxContext) -> bool {
    match name {
        PropName::Computed(computed) => {
            expr_calls_global_math_random(&computed.expr, unresolved_ctxt)
        }
        _ => false,
    }
}

fn computed_prop_calls_global_math_random(
    prop: &MemberProp,
    unresolved_ctxt: SyntaxContext,
) -> bool {
    match prop {
        MemberProp::Computed(computed) => {
            expr_calls_global_math_random(&computed.expr, unresolved_ctxt)
        }
        _ => false,
    }
}

fn is_global_math_random_callee(expr: &Expr, unresolved_ctxt: SyntaxContext) -> bool {
    let Expr::Member(member) = expr else {
        return false;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };
    let MemberProp::Ident(prop) = &member.prop else {
        return false;
    };
    obj.sym.as_ref() == "Math"
        && prop.sym.as_ref() == "random"
        && (obj.ctxt == unresolved_ctxt || obj.ctxt == SyntaxContext::empty())
}

fn is_observable_typeof(expr: &Expr, unresolved_mark: Mark) -> bool {
    match expr {
        Expr::Unary(unary) if unary.op == UnaryOp::TypeOf => {
            let Expr::Ident(ident) = unary.arg.as_ref() else {
                return false;
            };
            let unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
            ident.ctxt != unresolved_ctxt
        }
        Expr::Paren(paren) => is_observable_typeof(&paren.expr, unresolved_mark),
        _ => false,
    }
}

fn is_this_read(expr: &Expr) -> bool {
    match expr {
        Expr::This(_) => true,
        Expr::Paren(paren) => is_this_read(&paren.expr),
        _ => false,
    }
}

fn is_fn_arrow_or_class(expr: &Expr) -> bool {
    match expr {
        Expr::Fn(_) | Expr::Arrow(_) | Expr::Class(_) => true,
        Expr::Paren(paren) => is_fn_arrow_or_class(&paren.expr),
        _ => false,
    }
}

fn is_observable_ident_read(
    expr: &Expr,
    unresolved_mark: Mark,
    lexical_scopes: &[LexicalScope],
    function_lexical_scope_depth: Option<usize>,
    observable_ident_reads: &HashSet<BindingId>,
    nested_observable_ident_reads: &HashSet<BindingId>,
) -> bool {
    if let Expr::Ident(ident) = strip_parens(expr) {
        let binding = (ident.sym.clone(), ident.ctxt);
        return (ident.ctxt == SyntaxContext::empty().apply_mark(unresolved_mark)
            && ident.sym.as_ref() != "undefined")
            || lexical_scopes
                .iter()
                .any(|scope| scope.future.contains(&binding))
            || function_lexical_scope_depth.is_some_and(|depth| {
                lexical_scopes[..depth]
                    .iter()
                    .any(|scope| scope.all.contains(&binding))
            })
            || observable_ident_reads.contains(&binding);
    }

    let mut detector = ObservableIdentReadDetector {
        unresolved_ctxt: SyntaxContext::empty().apply_mark(unresolved_mark),
        lexical_scopes,
        function_lexical_scope_depth,
        observable_ident_reads: nested_observable_ident_reads,
        found: false,
    };
    expr.visit_with(&mut detector);
    detector.found
}

struct ObservableIdentReadDetector<'a> {
    unresolved_ctxt: SyntaxContext,
    lexical_scopes: &'a [LexicalScope],
    function_lexical_scope_depth: Option<usize>,
    observable_ident_reads: &'a HashSet<BindingId>,
    found: bool,
}

impl Visit for ObservableIdentReadDetector<'_> {
    fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
        if self.found {
            return;
        }
        let binding = (ident.sym.clone(), ident.ctxt);
        self.found = (ident.ctxt == self.unresolved_ctxt && ident.sym.as_ref() != "undefined")
            || self
                .lexical_scopes
                .iter()
                .any(|scope| scope.future.contains(&binding))
            || self.function_lexical_scope_depth.is_some_and(|depth| {
                self.lexical_scopes[..depth]
                    .iter()
                    .any(|scope| scope.all.contains(&binding))
            })
            || self.observable_ident_reads.contains(&binding);
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        if self.found {
            return;
        }
        if unary.op == UnaryOp::TypeOf {
            if let Expr::Ident(ident) = strip_parens(&unary.arg) {
                if ident.ctxt == self.unresolved_ctxt {
                    return;
                }
            }
        }
        unary.arg.visit_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if let Expr::Ident(root) = strip_parens(&member.obj) {
            if root.ctxt == self.unresolved_ctxt && is_stable_builtin_alias_root(root.sym.as_ref())
            {
                if let swc_core::ecma::ast::MemberProp::Computed(computed) = &member.prop {
                    computed.expr.visit_with(self);
                }
                return;
            }
        }
        member.visit_children_with(self);
    }

    fn visit_fn_expr(&mut self, _: &FnExpr) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

    fn visit_class_expr(&mut self, _: &ClassExpr) {
        // Class evaluation can run computed keys and static initializers.
        self.found = true;
    }
}

fn collect_import_binding_ids_from_module_items(items: &[ModuleItem]) -> HashSet<BindingId> {
    let mut ids = HashSet::default();
    for item in items {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Named(named) => {
                    ids.insert((named.local.sym.clone(), named.local.ctxt));
                }
                ImportSpecifier::Default(default) => {
                    ids.insert((default.local.sym.clone(), default.local.ctxt));
                }
                ImportSpecifier::Namespace(namespace) => {
                    ids.insert((namespace.local.sym.clone(), namespace.local.ctxt));
                }
            }
        }
    }
    ids
}

fn collect_cjs_require_binding_ids_from_module_items(
    items: &[ModuleItem],
    unresolved_mark: Mark,
) -> HashSet<BindingId> {
    let mut ids = HashSet::default();
    for item in items {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            let Some(init) = &decl.init else {
                continue;
            };
            if is_cjs_require_init(init, unresolved_mark) {
                ids.insert((binding.id.sym.clone(), binding.id.ctxt));
            }
        }
    }
    ids
}

fn is_cjs_require_init(expr: &Expr, unresolved_mark: Mark) -> bool {
    match expr {
        Expr::Call(call) => {
            let Callee::Expr(callee) = &call.callee else {
                return false;
            };
            let Expr::Ident(ident) = strip_parens(callee) else {
                return false;
            };
            ident.sym.as_ref() == "require" && ident.ctxt.outer() == unresolved_mark
        }
        Expr::Member(member) => is_cjs_require_init(&member.obj, unresolved_mark),
        Expr::Paren(paren) => is_cjs_require_init(&paren.expr, unresolved_mark),
        _ => false,
    }
}

fn collect_lexical_decl_ids_from_module_items(items: &[ModuleItem]) -> HashSet<BindingId> {
    let mut ids = HashSet::default();
    for item in items {
        match item {
            ModuleItem::Stmt(stmt) => collect_lexical_decl_ids_from_stmt_into(stmt, &mut ids),
            ModuleItem::ModuleDecl(decl) => {
                collect_lexical_decl_ids_from_module_decl_into(decl, &mut ids)
            }
        }
    }
    ids
}

fn collect_lexical_decl_ids_from_stmts(stmts: &[Stmt]) -> HashSet<BindingId> {
    let mut ids = HashSet::default();
    for stmt in stmts {
        collect_lexical_decl_ids_from_stmt_into(stmt, &mut ids);
    }
    ids
}

fn collect_lexical_decl_ids_from_module_decl(decl: &ModuleDecl) -> HashSet<BindingId> {
    let mut ids = HashSet::default();
    collect_lexical_decl_ids_from_module_decl_into(decl, &mut ids);
    ids
}

fn collect_lexical_decl_ids_from_module_decl_into(decl: &ModuleDecl, ids: &mut HashSet<BindingId>) {
    if let ModuleDecl::ExportDecl(export) = decl {
        collect_lexical_decl_ids_from_decl(&export.decl, ids);
    }
}

fn collect_lexical_decl_ids_from_stmt(stmt: &Stmt) -> HashSet<BindingId> {
    let mut ids = HashSet::default();
    collect_lexical_decl_ids_from_stmt_into(stmt, &mut ids);
    ids
}

fn collect_lexical_decl_ids_from_stmt_into(stmt: &Stmt, ids: &mut HashSet<BindingId>) {
    if let Stmt::Decl(decl) = stmt {
        collect_lexical_decl_ids_from_decl(decl, ids);
    }
}

fn collect_lexical_decl_ids_from_decl(decl: &Decl, ids: &mut HashSet<BindingId>) {
    match decl {
        Decl::Var(var) if matches!(var.kind, VarDeclKind::Let | VarDeclKind::Const) => {
            for decl in &var.decls {
                collect_binding_ids_from_pat(&decl.name, ids);
            }
        }
        Decl::Class(class) => {
            ids.insert((class.ident.sym.clone(), class.ident.ctxt));
        }
        _ => {}
    }
}

fn collect_binding_ids_from_pat(pat: &Pat, ids: &mut HashSet<BindingId>) {
    match pat {
        Pat::Ident(ident) => {
            ids.insert((ident.id.sym.clone(), ident.id.ctxt));
        }
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_binding_ids_from_pat(elem, ids);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                        collect_binding_ids_from_pat(&kv.value, ids);
                    }
                    swc_core::ecma::ast::ObjectPatProp::Assign(assign) => {
                        ids.insert((assign.key.sym.clone(), assign.key.ctxt));
                    }
                    swc_core::ecma::ast::ObjectPatProp::Rest(rest) => {
                        collect_binding_ids_from_pat(&rest.arg, ids);
                    }
                }
            }
        }
        Pat::Rest(rest) => collect_binding_ids_from_pat(&rest.arg, ids),
        Pat::Assign(assign) => collect_binding_ids_from_pat(&assign.left, ids),
        _ => {}
    }
}

fn remove_ids_from_current_scope(scopes: &mut [LexicalScope], remove: &HashSet<BindingId>) {
    if let Some(current) = scopes.last_mut() {
        for id in remove {
            current.future.remove(id);
        }
    }
}

fn has_object_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Object(_) => true,
        Expr::Paren(paren) => has_object_literal(&paren.expr),
        _ => false,
    }
}

fn has_observable_binary_coercion(expr: &Expr) -> bool {
    match expr {
        Expr::Bin(bin) if binary_op_can_throw_on_literals(bin.op, &bin.left, &bin.right) => true,
        Expr::Bin(bin)
            if binary_op_can_coerce_or_throw(bin.op)
                && (!is_known_primitive_literal(&bin.left)
                    || !is_known_primitive_literal(&bin.right)) =>
        {
            true
        }
        Expr::Bin(bin) => {
            has_observable_binary_coercion(&bin.left) || has_observable_binary_coercion(&bin.right)
        }
        Expr::Paren(paren) => has_observable_binary_coercion(&paren.expr),
        _ => false,
    }
}

fn has_observable_unary_throw(expr: &Expr) -> bool {
    match expr {
        Expr::Unary(unary) if unary_can_throw_on_literal(unary) => true,
        Expr::Unary(unary) => has_observable_unary_throw(&unary.arg),
        Expr::Paren(paren) => has_observable_unary_throw(&paren.expr),
        _ => false,
    }
}

fn has_new_expr(expr: &Expr) -> bool {
    match expr {
        Expr::New(_) => true,
        Expr::Paren(paren) => has_new_expr(&paren.expr),
        _ => false,
    }
}

fn binary_op_can_throw_on_literals(op: BinaryOp, left: &Expr, right: &Expr) -> bool {
    match op {
        BinaryOp::In | BinaryOp::InstanceOf => is_known_primitive_literal(right),
        BinaryOp::ZeroFillRShift => has_bigint_literal(left) || has_bigint_literal(right),
        BinaryOp::Add
        | BinaryOp::Sub
        | BinaryOp::Mul
        | BinaryOp::Div
        | BinaryOp::Mod
        | BinaryOp::Exp
        | BinaryOp::BitOr
        | BinaryOp::BitXor
        | BinaryOp::BitAnd
        | BinaryOp::LShift
        | BinaryOp::RShift => has_bigint_literal(left) || has_bigint_literal(right),
        _ => false,
    }
}

fn unary_can_throw_on_literal(unary: &UnaryExpr) -> bool {
    unary.op == UnaryOp::Plus && has_bigint_literal(&unary.arg)
}

fn has_bigint_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(Lit::BigInt(_)) => true,
        Expr::Paren(paren) => has_bigint_literal(&paren.expr),
        Expr::Unary(unary) if matches!(unary.op, UnaryOp::Minus | UnaryOp::Plus) => {
            has_bigint_literal(&unary.arg)
        }
        _ => false,
    }
}

fn binary_op_can_coerce_or_throw(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::EqEq
            | BinaryOp::NotEq
            | BinaryOp::Lt
            | BinaryOp::LtEq
            | BinaryOp::Gt
            | BinaryOp::GtEq
            | BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::Mod
            | BinaryOp::BitOr
            | BinaryOp::BitXor
            | BinaryOp::BitAnd
            | BinaryOp::LShift
            | BinaryOp::RShift
            | BinaryOp::ZeroFillRShift
            | BinaryOp::Exp
            | BinaryOp::In
            | BinaryOp::InstanceOf
    )
}

fn is_known_primitive_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(Lit::Str(_) | Lit::Bool(_) | Lit::Null(_) | Lit::Num(_) | Lit::BigInt(_)) => true,
        Expr::Paren(paren) => is_known_primitive_literal(&paren.expr),
        _ => false,
    }
}

fn split_stmt(stmt: Stmt, level: RewriteLevel) -> Vec<Stmt> {
    match stmt {
        Stmt::Expr(ExprStmt { span, expr }) => {
            // Check assignment-member pattern: (a = expr)[prop] = val
            if let Some(stmts) = try_split_assign_member(&expr, span) {
                return stmts;
            }
            match *expr {
                Expr::Seq(SeqExpr { exprs, .. }) => exprs
                    .into_iter()
                    .map(|expr| Stmt::Expr(ExprStmt { span, expr }))
                    .collect(),
                Expr::Yield(yield_expr) => split_yield_arg_sequence(yield_expr.clone(), span)
                    .unwrap_or_else(|| {
                        vec![Stmt::Expr(ExprStmt {
                            span,
                            expr: Box::new(Expr::Yield(yield_expr)),
                        })]
                    }),
                Expr::Paren(paren) => split_expr_stmt_paren(paren, span),
                other => vec![Stmt::Expr(ExprStmt {
                    span,
                    expr: Box::new(other),
                })],
            }
        }
        Stmt::Return(ReturnStmt {
            span,
            arg: Some(arg),
        }) => split_return(span, arg),
        Stmt::Throw(ThrowStmt { span, arg }) => split_throw(span, arg),
        Stmt::If(if_stmt) => split_if(if_stmt, level),
        Stmt::Switch(switch_stmt) => split_switch(switch_stmt),
        Stmt::Decl(Decl::Var(var)) => split_var_decl(var, level),
        Stmt::For(for_stmt) => split_for_stmt(for_stmt, level),
        Stmt::ForIn(for_in_stmt) => split_for_in_stmt(for_in_stmt),
        Stmt::ForOf(for_of_stmt) => split_for_of_stmt(for_of_stmt),
        _ => vec![stmt],
    }
}

fn split_expr_stmt_paren(paren: ParenExpr, span: swc_core::common::Span) -> Vec<Stmt> {
    match *paren.expr {
        Expr::Seq(SeqExpr { exprs, .. }) => exprs
            .into_iter()
            .map(|expr| Stmt::Expr(ExprStmt { span, expr }))
            .collect(),
        inner => vec![Stmt::Expr(ExprStmt {
            span,
            expr: Box::new(Expr::Paren(ParenExpr {
                expr: Box::new(inner),
                ..paren
            })),
        })],
    }
}

fn split_yield_arg_sequence(
    mut yield_expr: YieldExpr,
    span: swc_core::common::Span,
) -> Option<Vec<Stmt>> {
    let Expr::Seq(SeqExpr { mut exprs, .. }) = *yield_expr.arg.take()? else {
        return None;
    };
    if exprs.len() <= 1 {
        return None;
    }

    yield_expr.arg = Some(exprs.remove(0));
    let mut stmts = Vec::with_capacity(exprs.len() + 1);
    stmts.push(Stmt::Expr(ExprStmt {
        span,
        expr: Box::new(Expr::Yield(yield_expr)),
    }));
    stmts.extend(
        exprs
            .into_iter()
            .map(|expr| Stmt::Expr(ExprStmt { span, expr })),
    );
    Some(stmts)
}

// ---------------------------------------------------------------------------
// Assignment-member pattern: (a = expr)[prop] = val  →  a = expr; a[prop] = val
// ---------------------------------------------------------------------------

fn try_split_assign_member(expr: &Expr, span: swc_core::common::Span) -> Option<Vec<Stmt>> {
    let Expr::Assign(outer) = expr else {
        return None;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &outer.left else {
        return None;
    };
    // member.obj should be a (possibly paren-wrapped) assignment expr
    let obj = strip_parens(&member.obj);
    let Expr::Assign(inner) = obj else {
        return None;
    };
    // inner assign must assign to a simple ident
    let AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) = &inner.left else {
        return None;
    };

    let inner_stmt = Stmt::Expr(ExprStmt {
        span,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: inner.span,
            op: inner.op,
            left: inner.left.clone(),
            right: inner.right.clone(),
        })),
    });

    let new_member = MemberExpr {
        span: member.span,
        obj: Box::new(Expr::Ident(ident.id.clone())),
        prop: member.prop.clone(),
    };
    let outer_stmt = Stmt::Expr(ExprStmt {
        span,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: outer.span,
            op: outer.op,
            left: AssignTarget::Simple(SimpleAssignTarget::Member(new_member)),
            right: outer.right.clone(),
        })),
    });

    Some(vec![inner_stmt, outer_stmt])
}

// ---------------------------------------------------------------------------
// Variable declaration: split by declarator, extract sequence inits
// ---------------------------------------------------------------------------

fn split_var_decl(var: Box<VarDecl>, level: RewriteLevel) -> Vec<Stmt> {
    let span = var.span;
    let kind = var.kind;
    let ctxt = var.ctxt;
    let mut result = Vec::new();

    for decl in var.decls {
        if let Some(init) = decl.init {
            if level == RewriteLevel::Minimal && sequence_blocks_decl_name_inference(&init) {
                result.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
                    span,
                    ctxt,
                    kind,
                    declare: false,
                    decls: vec![VarDeclarator {
                        span: decl.span,
                        name: decl.name,
                        init: Some(init),
                        definite: decl.definite,
                    }],
                }))));
                continue;
            }
            let (prefix, last) = split_expr_seq(init);
            for expr in prefix {
                result.push(Stmt::Expr(ExprStmt { span, expr }));
            }
            result.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
                span,
                ctxt,
                kind,
                declare: false,
                decls: vec![VarDeclarator {
                    span: decl.span,
                    name: decl.name,
                    init: Some(last),
                    definite: decl.definite,
                }],
            }))));
        } else {
            result.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
                span,
                ctxt,
                kind,
                declare: false,
                decls: vec![decl],
            }))));
        }
    }

    result
}

// ---------------------------------------------------------------------------
// For loop: extract sequence from init expression
// ---------------------------------------------------------------------------

fn split_for_stmt(mut for_stmt: ForStmt, level: RewriteLevel) -> Vec<Stmt> {
    let mut prefix = Vec::new();

    if let Some(init) = for_stmt.init.take() {
        match init {
            VarDeclOrExpr::Expr(expr) => {
                let (pre, last) = split_expr_seq(expr);
                if pre.is_empty() {
                    if is_assign_expr(&last) {
                        // Keep assignment initializers in the loop header.
                        for_stmt.init = Some(VarDeclOrExpr::Expr(last));
                    } else if can_split_standalone_for_init_expr(&last) {
                        prefix.push(Stmt::Expr(ExprStmt {
                            span: for_stmt.span,
                            expr: last,
                        }));
                    } else {
                        for_stmt.init = Some(VarDeclOrExpr::Expr(last));
                    }
                } else {
                    for p in pre {
                        prefix.push(Stmt::Expr(ExprStmt {
                            span: for_stmt.span,
                            expr: p,
                        }));
                    }
                    // Keep last as init only if it's an assignment expression
                    if is_assign_expr(&last) {
                        for_stmt.init = Some(VarDeclOrExpr::Expr(last));
                    } else {
                        prefix.push(Stmt::Expr(ExprStmt {
                            span: for_stmt.span,
                            expr: last,
                        }));
                        // for_stmt.init stays None
                    }
                }
            }
            VarDeclOrExpr::VarDecl(var) => {
                let (extracted, new_var) = extract_var_decl_prefix(var, for_stmt.span, level);
                prefix.extend(extracted);
                for_stmt.init = Some(VarDeclOrExpr::VarDecl(new_var));
            }
        }
    }

    if prefix.is_empty() {
        return vec![Stmt::For(for_stmt)];
    }

    prefix.push(Stmt::For(for_stmt));
    prefix
}

fn can_split_standalone_for_init_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Call(call) => matches!(
            &call.callee,
            swc_core::ecma::ast::Callee::Expr(callee)
                if matches!(strip_parens(callee), Expr::Ident(_))
        ),
        Expr::Paren(paren) => can_split_standalone_for_init_expr(&paren.expr),
        _ => false,
    }
}

/// Extract sequence prefixes from a for-loop declaration while preserving
/// declarator evaluation order and lexical scope.
///
/// Walk left to right without moving a comma prefix ahead of earlier
/// initializer effects:
/// - `var`: flush already-collected declarators as statements, then the prefix.
/// - `let` / `const`: cannot hoist those declarators out of the `for`, so a
///   later declarator's prefix stays unsplit. A first prefix may lift only when
///   it does not reference any lexical binding in the whole header (TDZ).
fn extract_var_decl_prefix(
    var: Box<VarDecl>,
    span: swc_core::common::Span,
    level: RewriteLevel,
) -> (Vec<Stmt>, Box<VarDecl>) {
    let kind = var.kind;
    let ctxt = var.ctxt;
    let var_span = var.span;
    let is_var = kind == VarDeclKind::Var;
    let mut prefix = Vec::new();
    let mut new_decls = Vec::new();
    let mut header_bindings = HashSet::default();
    if !is_var {
        for decl in &var.decls {
            collect_binding_ids_from_pat(&decl.name, &mut header_bindings);
        }
    }
    let mut future_header_names: HashSet<Atom> =
        header_bindings.iter().map(|(sym, _)| sym.clone()).collect();

    for decl in var.decls {
        if !is_var {
            let mut current_bindings = HashSet::default();
            collect_binding_ids_from_pat(&decl.name, &mut current_bindings);
            for (sym, _) in current_bindings {
                future_header_names.remove(&sym);
            }
        }
        if let Some(init) = decl.init {
            if level == RewriteLevel::Minimal && sequence_blocks_decl_name_inference(&init) {
                new_decls.push(VarDeclarator {
                    span: decl.span,
                    name: decl.name,
                    init: Some(init),
                    definite: decl.definite,
                });
                continue;
            }
            let (pre, last) = split_expr_seq(init);
            let keep_unsplit = !pre.is_empty()
                && (seq_prefix_has_string_lit(&pre)
                    || (!is_var
                        && (!new_decls.is_empty()
                            || seq_prefix_refs_lexical_header(
                                &pre,
                                &header_bindings,
                                &future_header_names,
                            ))));
            if keep_unsplit {
                new_decls.push(VarDeclarator {
                    span: decl.span,
                    name: decl.name,
                    init: Some(rejoin_seq(pre, last)),
                    definite: decl.definite,
                });
                continue;
            }
            if !pre.is_empty() && is_var {
                flush_var_declarators(&mut prefix, &mut new_decls, var_span, ctxt, kind);
            }
            for p in pre {
                prefix.push(Stmt::Expr(ExprStmt { span, expr: p }));
            }
            new_decls.push(VarDeclarator {
                span: decl.span,
                name: decl.name,
                init: Some(last),
                definite: decl.definite,
            });
        } else {
            new_decls.push(decl);
        }
    }

    let new_var = Box::new(VarDecl {
        span: var_span,
        ctxt,
        kind,
        declare: false,
        decls: new_decls,
    });

    (prefix, new_var)
}

fn flush_var_declarators(
    prefix: &mut Vec<Stmt>,
    new_decls: &mut Vec<VarDeclarator>,
    span: swc_core::common::Span,
    ctxt: SyntaxContext,
    kind: VarDeclKind,
) {
    if new_decls.is_empty() {
        return;
    }
    prefix.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span,
        ctxt,
        kind,
        declare: false,
        decls: std::mem::take(new_decls),
    }))));
}

fn rejoin_seq(mut prefix: Vec<Box<Expr>>, last: Box<Expr>) -> Box<Expr> {
    if prefix.is_empty() {
        return last;
    }
    prefix.push(last);
    Box::new(Expr::Seq(SeqExpr {
        span: DUMMY_SP,
        exprs: prefix,
    }))
}

fn seq_prefix_has_string_lit(prefix: &[Box<Expr>]) -> bool {
    prefix
        .iter()
        .any(|expr| matches!(strip_parens(expr), Expr::Lit(Lit::Str(_))))
}

fn seq_prefix_refs_lexical_header(
    prefix: &[Box<Expr>],
    bindings: &HashSet<BindingId>,
    future_names: &HashSet<Atom>,
) -> bool {
    prefix
        .iter()
        .any(|expr| expr_refs_lexical_header(expr, bindings, future_names))
}

fn expr_refs_lexical_header(
    expr: &Expr,
    bindings: &HashSet<BindingId>,
    future_names: &HashSet<Atom>,
) -> bool {
    if bindings.is_empty() && future_names.is_empty() {
        return false;
    }
    struct Finder<'a> {
        bindings: &'a HashSet<BindingId>,
        future_names: &'a HashSet<Atom>,
        hit: bool,
    }
    impl Visit for Finder<'_> {
        fn visit_ident(&mut self, ident: &Ident) {
            // SWC resolves a read before a later same-list lexical declarator
            // to an outer binding context, even though JavaScript evaluates it
            // against that declarator's TDZ. Use names only for those future
            // bindings; current and earlier bindings still require exact IDs.
            if self.bindings.contains(&binding_id(ident)) || self.future_names.contains(&ident.sym)
            {
                self.hit = true;
            }
        }
    }
    let mut finder = Finder {
        bindings,
        future_names,
        hit: false,
    };
    expr.visit_with(&mut finder);
    finder.hit
}

fn sequence_blocks_decl_name_inference(expr: &Expr) -> bool {
    let expr = match expr {
        Expr::Paren(paren) => paren.expr.as_ref(),
        other => other,
    };
    let Expr::Seq(seq) = expr else {
        return false;
    };
    let Some(last) = seq.exprs.last() else {
        return false;
    };
    is_anonymous_function_or_class(last)
}

fn is_anonymous_function_or_class(expr: &Expr) -> bool {
    match expr {
        Expr::Fn(fn_expr) => fn_expr.ident.is_none(),
        Expr::Class(class_expr) => class_expr.ident.is_none(),
        Expr::Paren(paren) => is_anonymous_function_or_class(&paren.expr),
        _ => false,
    }
}

fn is_assign_expr(expr: &Box<Expr>) -> bool {
    matches!(**expr, Expr::Assign(_))
}

// ---------------------------------------------------------------------------
// For-in / For-of: extract sequence from the iterable expression
// ---------------------------------------------------------------------------

fn split_for_in_stmt(mut stmt: ForInStmt) -> Vec<Stmt> {
    if for_head_has_lexical_decl(&stmt.left) {
        return vec![Stmt::ForIn(stmt)];
    }
    let dummy = Box::new(Expr::Invalid(Invalid { span: DUMMY_SP }));
    let right = std::mem::replace(&mut stmt.right, dummy);
    let (pre, last) = split_expr_seq(right);
    stmt.right = last;
    if pre.is_empty() {
        return vec![Stmt::ForIn(stmt)];
    }
    let mut result: Vec<Stmt> = pre
        .into_iter()
        .map(|e| {
            Stmt::Expr(ExprStmt {
                span: stmt.span,
                expr: e,
            })
        })
        .collect();
    result.push(Stmt::ForIn(stmt));
    result
}

fn split_for_of_stmt(mut stmt: ForOfStmt) -> Vec<Stmt> {
    if for_head_has_lexical_decl(&stmt.left) {
        return vec![Stmt::ForOf(stmt)];
    }
    let dummy = Box::new(Expr::Invalid(Invalid { span: DUMMY_SP }));
    let right = std::mem::replace(&mut stmt.right, dummy);
    let (pre, last) = split_expr_seq(right);
    stmt.right = last;
    if pre.is_empty() {
        return vec![Stmt::ForOf(stmt)];
    }
    let mut result: Vec<Stmt> = pre
        .into_iter()
        .map(|e| {
            Stmt::Expr(ExprStmt {
                span: stmt.span,
                expr: e,
            })
        })
        .collect();
    result.push(Stmt::ForOf(stmt));
    result
}

fn for_head_has_lexical_decl(head: &ForHead) -> bool {
    matches!(
        head,
        ForHead::VarDecl(var) if matches!(var.kind, VarDeclKind::Let | VarDeclKind::Const)
    )
}

// ---------------------------------------------------------------------------
// Existing helpers
// ---------------------------------------------------------------------------

fn split_return(span: swc_core::common::Span, arg: Box<Expr>) -> Vec<Stmt> {
    let (prefix, last) = split_expr_seq(arg);
    if prefix.is_empty() {
        return vec![Stmt::Return(ReturnStmt {
            span,
            arg: Some(last),
        })];
    }
    let mut stmts = expr_stmts(span, prefix);
    stmts.push(Stmt::Return(ReturnStmt {
        span,
        arg: Some(last),
    }));
    stmts
}

fn split_throw(span: swc_core::common::Span, arg: Box<Expr>) -> Vec<Stmt> {
    let (prefix, last) = split_expr_seq(arg);
    if prefix.is_empty() {
        return vec![Stmt::Throw(ThrowStmt { span, arg: last })];
    }
    let mut stmts = expr_stmts(span, prefix);
    stmts.push(Stmt::Throw(ThrowStmt { span, arg: last }));
    stmts
}

fn split_if(mut if_stmt: IfStmt, level: RewriteLevel) -> Vec<Stmt> {
    if_stmt.cons = normalize_branch_stmt(*if_stmt.cons, level);
    if let Some(alt) = if_stmt.alt.take() {
        if_stmt.alt = Some(normalize_branch_stmt(*alt, level));
    }

    let (prefix, last_test) = split_expr_seq(if_stmt.test.clone());
    if prefix.is_empty() {
        return vec![Stmt::If(if_stmt)];
    }

    if_stmt.test = last_test;

    let mut stmts = expr_stmts(if_stmt.span, prefix);
    stmts.push(Stmt::If(if_stmt));
    stmts
}

fn split_switch(mut switch_stmt: SwitchStmt) -> Vec<Stmt> {
    let (prefix, last_discriminant) = split_expr_seq(switch_stmt.discriminant.clone());
    if prefix.is_empty() {
        return vec![Stmt::Switch(switch_stmt)];
    }

    switch_stmt.discriminant = last_discriminant;

    let mut stmts = expr_stmts(switch_stmt.span, prefix);
    stmts.push(Stmt::Switch(switch_stmt));
    stmts
}

fn normalize_branch_stmt(stmt: Stmt, level: RewriteLevel) -> Box<Stmt> {
    let mut split = split_stmt(stmt, level);
    if split.len() == 1 {
        Box::new(split.pop().expect("length checked"))
    } else {
        Box::new(Stmt::Block(BlockStmt {
            span: DUMMY_SP,
            ctxt: Default::default(),
            stmts: split,
        }))
    }
}

fn split_expr_seq(expr: Box<Expr>) -> (Vec<Box<Expr>>, Box<Expr>) {
    match *expr {
        Expr::Paren(paren) => split_expr_seq(paren.expr),
        Expr::Seq(SeqExpr { mut exprs, .. }) => {
            if exprs.len() <= 1 {
                let only = exprs
                    .pop()
                    .expect("sequence expressions should be non-empty");
                (Vec::new(), only)
            } else {
                let last = exprs.pop().expect("sequence length checked");
                (exprs, last)
            }
        }
        other => (Vec::new(), Box::new(other)),
    }
}

fn expr_stmts(span: swc_core::common::Span, exprs: Vec<Box<Expr>>) -> Vec<Stmt> {
    exprs
        .into_iter()
        .map(|expr| Stmt::Expr(ExprStmt { span, expr }))
        .collect()
}
