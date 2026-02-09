use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    BlockStmt, Expr, ExprStmt, IfStmt, ModuleItem, ReturnStmt, SeqExpr, Stmt, SwitchStmt,
    ThrowStmt,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct SimplifySequence;

impl VisitMut for SimplifySequence {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        let old_items = std::mem::take(items);
        let mut new_items = Vec::with_capacity(old_items.len());

        for item in old_items {
            match item {
                ModuleItem::Stmt(stmt) => {
                    for stmt in split_stmt(stmt) {
                        new_items.push(ModuleItem::Stmt(stmt));
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
            new_stmts.extend(split_stmt(stmt));
        }

        *stmts = new_stmts;
    }
}

fn split_stmt(stmt: Stmt) -> Vec<Stmt> {
    match stmt {
        Stmt::Expr(ExprStmt { span, expr }) => match *expr {
            Expr::Seq(SeqExpr { exprs, .. }) => exprs
                .into_iter()
                .map(|expr| Stmt::Expr(ExprStmt { span, expr }))
                .collect(),
            other => vec![Stmt::Expr(ExprStmt {
                span,
                expr: Box::new(other),
            })],
        },
        Stmt::Return(ReturnStmt { span, arg: Some(arg) }) => split_return(span, arg),
        Stmt::Throw(ThrowStmt { span, arg }) => split_throw(span, arg),
        Stmt::If(if_stmt) => split_if(if_stmt),
        Stmt::Switch(switch_stmt) => split_switch(switch_stmt),
        _ => vec![stmt],
    }
}

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
