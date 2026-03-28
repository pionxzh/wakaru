use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, BlockStmt, Decl, Expr, ExprStmt, ForInStmt, ForOfStmt, ForStmt,
    IfStmt, Invalid, Lit, MemberExpr, ModuleItem, ReturnStmt, SeqExpr, SimpleAssignTarget, Stmt,
    SwitchStmt, ThrowStmt, VarDecl, VarDeclOrExpr, VarDeclarator,
};
use swc_core::ecma::utils::{ExprCtx, ExprExt};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct SimplifySequence {
    unresolved_mark: Mark,
}

impl SimplifySequence {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }
}

impl VisitMut for SimplifySequence {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        let old_items = std::mem::take(items);
        let mut new_items = Vec::with_capacity(old_items.len());

        for item in old_items {
            match item {
                ModuleItem::Stmt(stmt) => {
                    for stmt in split_stmt(stmt) {
                        if !is_pure_no_op_stmt(&stmt, self.unresolved_mark) {
                            new_items.push(ModuleItem::Stmt(stmt));
                        }
                    }
                }
                ModuleItem::ModuleDecl(decl) => new_items.push(ModuleItem::ModuleDecl(decl)),
            }
        }

        *items = new_items;
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);

        let old_stmts = std::mem::take(stmts);
        let mut new_stmts = Vec::with_capacity(old_stmts.len());

        for stmt in old_stmts {
            for s in split_stmt(stmt) {
                if !is_pure_no_op_stmt(&s, self.unresolved_mark) {
                    new_stmts.push(s);
                }
            }
        }

        *stmts = new_stmts;
    }
}

/// Returns true for expression statements that are provably side-effect-free.
/// String literals are intentionally excluded because they may be directive prologues
/// (e.g., "use strict") handled by a later pass.
fn is_pure_no_op_stmt(stmt: &Stmt, unresolved_mark: Mark) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    // Never drop string literals — may be "use strict" directives
    if matches!(expr.as_ref(), Expr::Lit(Lit::Str(_))) {
        return false;
    }
    let unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
    let ctx = ExprCtx {
        unresolved_ctxt,
        is_unresolved_ref_safe: false,
        in_strict: false,
        remaining_depth: 4,
    };
    !expr.may_have_side_effects(ctx)
}

fn split_stmt(stmt: Stmt) -> Vec<Stmt> {
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
                other => vec![Stmt::Expr(ExprStmt {
                    span,
                    expr: Box::new(other),
                })],
            }
        }
        Stmt::Return(ReturnStmt { span, arg: Some(arg) }) => split_return(span, arg),
        Stmt::Throw(ThrowStmt { span, arg }) => split_throw(span, arg),
        Stmt::If(if_stmt) => split_if(if_stmt),
        Stmt::Switch(switch_stmt) => split_switch(switch_stmt),
        Stmt::Decl(Decl::Var(var)) => split_var_decl(var),
        Stmt::For(for_stmt) => split_for_stmt(for_stmt),
        Stmt::ForIn(for_in_stmt) => split_for_in_stmt(for_in_stmt),
        Stmt::ForOf(for_of_stmt) => split_for_of_stmt(for_of_stmt),
        _ => vec![stmt],
    }
}

// ---------------------------------------------------------------------------
// Assignment-member pattern: (a = expr)[prop] = val  →  a = expr; a[prop] = val
// ---------------------------------------------------------------------------

fn try_split_assign_member(
    expr: &Expr,
    span: swc_core::common::Span,
) -> Option<Vec<Stmt>> {
    let Expr::Assign(outer) = expr else {
        return None;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &outer.left else {
        return None;
    };
    // member.obj should be a (possibly paren-wrapped) assignment expr
    let obj = strip_paren(&member.obj);
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

fn strip_paren(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => strip_paren(&paren.expr),
        _ => expr,
    }
}

// ---------------------------------------------------------------------------
// Variable declaration: split by declarator, extract sequence inits
// ---------------------------------------------------------------------------

fn split_var_decl(var: Box<VarDecl>) -> Vec<Stmt> {
    let span = var.span;
    let kind = var.kind;
    let ctxt = var.ctxt;
    let mut result = Vec::new();

    for decl in var.decls {
        if let Some(init) = decl.init {
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

fn split_for_stmt(mut for_stmt: ForStmt) -> Vec<Stmt> {
    let mut prefix = Vec::new();

    if let Some(init) = for_stmt.init.take() {
        match init {
            VarDeclOrExpr::Expr(expr) => {
                let (pre, last) = split_expr_seq(expr);
                if pre.is_empty() {
                    // Not a sequence — restore unchanged
                    for_stmt.init = Some(VarDeclOrExpr::Expr(last));
                } else {
                    for p in pre {
                        prefix.push(Stmt::Expr(ExprStmt { span: for_stmt.span, expr: p }));
                    }
                    // Keep last as init only if it's an assignment expression
                    if is_assign_expr(&last) {
                        for_stmt.init = Some(VarDeclOrExpr::Expr(last));
                    } else {
                        prefix.push(Stmt::Expr(ExprStmt { span: for_stmt.span, expr: last }));
                        // for_stmt.init stays None
                    }
                }
            }
            VarDeclOrExpr::VarDecl(var) => {
                let (extracted, new_var) = extract_var_decl_prefix(var, for_stmt.span);
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

/// Extract sequence prefixes from each declarator's init, without splitting
/// the var decl into individual declarations (needed for for-loop scope).
fn extract_var_decl_prefix(
    var: Box<VarDecl>,
    span: swc_core::common::Span,
) -> (Vec<Stmt>, Box<VarDecl>) {
    let kind = var.kind;
    let ctxt = var.ctxt;
    let var_span = var.span;
    let mut prefix = Vec::new();
    let mut new_decls = Vec::new();

    for decl in var.decls {
        if let Some(init) = decl.init {
            let (pre, last) = split_expr_seq(init);
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

fn is_assign_expr(expr: &Box<Expr>) -> bool {
    matches!(**expr, Expr::Assign(_))
}

// ---------------------------------------------------------------------------
// For-in / For-of: extract sequence from the iterable expression
// ---------------------------------------------------------------------------

fn split_for_in_stmt(mut stmt: ForInStmt) -> Vec<Stmt> {
    let dummy = Box::new(Expr::Invalid(Invalid { span: DUMMY_SP }));
    let right = std::mem::replace(&mut stmt.right, dummy);
    let (pre, last) = split_expr_seq(right);
    stmt.right = last;
    if pre.is_empty() {
        return vec![Stmt::ForIn(stmt)];
    }
    let mut result: Vec<Stmt> = pre
        .into_iter()
        .map(|e| Stmt::Expr(ExprStmt { span: stmt.span, expr: e }))
        .collect();
    result.push(Stmt::ForIn(stmt));
    result
}

fn split_for_of_stmt(mut stmt: ForOfStmt) -> Vec<Stmt> {
    let dummy = Box::new(Expr::Invalid(Invalid { span: DUMMY_SP }));
    let right = std::mem::replace(&mut stmt.right, dummy);
    let (pre, last) = split_expr_seq(right);
    stmt.right = last;
    if pre.is_empty() {
        return vec![Stmt::ForOf(stmt)];
    }
    let mut result: Vec<Stmt> = pre
        .into_iter()
        .map(|e| Stmt::Expr(ExprStmt { span: stmt.span, expr: e }))
        .collect();
    result.push(Stmt::ForOf(stmt));
    result
}

// ---------------------------------------------------------------------------
// Existing helpers
// ---------------------------------------------------------------------------

fn split_return(span: swc_core::common::Span, arg: Box<Expr>) -> Vec<Stmt> {
    let (prefix, last) = split_expr_seq(arg);
    if prefix.is_empty() {
        return vec![Stmt::Return(ReturnStmt { span, arg: Some(last) })];
    }
    let mut stmts = expr_stmts(span, prefix);
    stmts.push(Stmt::Return(ReturnStmt { span, arg: Some(last) }));
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

fn split_if(mut if_stmt: IfStmt) -> Vec<Stmt> {
    if_stmt.cons = normalize_branch_stmt(*if_stmt.cons);
    if let Some(alt) = if_stmt.alt.take() {
        if_stmt.alt = Some(normalize_branch_stmt(*alt));
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

fn normalize_branch_stmt(stmt: Stmt) -> Box<Stmt> {
    let mut split = split_stmt(stmt);
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
                let only = exprs.pop().expect("sequence expressions should be non-empty");
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
