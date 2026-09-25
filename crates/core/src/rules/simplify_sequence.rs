use crate::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, BlockStmt, Decl, Expr, ExprStmt, ForHead, ForInStmt, ForOfStmt,
    ForStmt, Ident, IfStmt, Invalid, Lit, MemberExpr, ModuleItem, ParenExpr, Pat, ReturnStmt,
    SeqExpr, SimpleAssignTarget, Stmt, SwitchStmt, ThrowStmt, VarDecl, VarDeclKind, VarDeclOrExpr,
    VarDeclarator, YieldExpr,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::decl_utils::{binding_id, BindingId};
use super::RewriteLevel;

use crate::utils::paren::strip_parens;

pub struct SimplifySequence {
    level: RewriteLevel,
}

impl SimplifySequence {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self::new_with_level(unresolved_mark, RewriteLevel::Standard)
    }

    pub fn new_with_level(_unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self { level }
    }
}

// Splitting keeps every statement, including ones that do nothing: an
// expression statement that was already dead in the input stays in the output.
impl VisitMut for SimplifySequence {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        let old_items = std::mem::take(items);
        let mut new_items = Vec::with_capacity(old_items.len());
        for mut item in old_items {
            item.visit_mut_children_with(self);
            match item {
                ModuleItem::Stmt(stmt) => {
                    new_items.extend(
                        split_stmt(stmt, self.level)
                            .into_iter()
                            .map(ModuleItem::Stmt),
                    );
                }
                ModuleItem::ModuleDecl(decl) => new_items.push(ModuleItem::ModuleDecl(decl)),
            }
        }
        *items = new_items;
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        let old_stmts = std::mem::take(stmts);
        let mut new_stmts = Vec::with_capacity(old_stmts.len());
        for mut stmt in old_stmts {
            stmt.visit_mut_children_with(self);
            new_stmts.extend(split_stmt(stmt, self.level));
        }
        *stmts = new_stmts;
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
