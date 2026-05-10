use swc_core::common::{Span, DUMMY_SP};
use swc_core::ecma::ast::{
    BinExpr, BinaryOp, BlockStmt, Expr, ExprStmt, IfStmt, ModuleItem, ReturnStmt, Stmt, UnaryExpr,
    UnaryOp,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnConditionals;

impl VisitMut for UnConditionals {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        let old = std::mem::take(items);
        for item in old {
            match item {
                ModuleItem::Stmt(stmt) => {
                    let converted = convert_stmt(stmt);
                    items.extend(converted.into_iter().map(ModuleItem::Stmt));
                }
                other => items.push(other),
            }
        }
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);

        let old = std::mem::take(stmts);
        for stmt in old {
            stmts.extend(convert_stmt(stmt));
        }
    }
}

/// Convert a single statement, returning one or more statements.
fn convert_stmt(stmt: Stmt) -> Vec<Stmt> {
    match stmt {
        Stmt::Expr(ExprStmt { expr, span }) => try_convert_expr_stmt_to_if(span, *expr),
        Stmt::Return(ReturnStmt {
            span,
            arg: Some(arg),
        }) => {
            let is_cond = matches!(*arg, Expr::Cond(_));
            if is_cond {
                try_split_return_ternary(*arg, span).expect("checked it is Cond above")
            } else {
                vec![Stmt::Return(ReturnStmt {
                    span,
                    arg: Some(arg),
                })]
            }
        }
        other => vec![other],
    }
}

/// Try to convert an ExprStmt-level expression to an if statement.
/// Returns a Vec<Stmt> which is either the converted if statement(s) or
/// the original ExprStmt wrapped in a Vec.
fn try_convert_expr_stmt_to_if(span: Span, expr: Expr) -> Vec<Stmt> {
    match expr {
        Expr::Cond(cond_expr) => {
            // Only convert if at least one branch is action-like (has side effects)
            if !is_action_expr(&cond_expr.cons) && !is_action_expr(&cond_expr.alt) {
                return vec![Stmt::Expr(ExprStmt {
                    span,
                    expr: Box::new(Expr::Cond(cond_expr)),
                })];
            }
            vec![convert_cond_to_if(
                *cond_expr.test,
                cond_expr.cons,
                cond_expr.alt,
            )]
        }
        Expr::Bin(BinExpr {
            op: BinaryOp::LogicalAnd,
            left,
            right,
            ..
        }) => {
            // x && action() → if (x) { action(); }
            // But only if right-hand side is "action-like" (not a simple value)
            if !is_action_expr(&right) {
                return vec![Stmt::Expr(ExprStmt {
                    span,
                    expr: Box::new(Expr::Bin(BinExpr {
                        span: DUMMY_SP,
                        op: BinaryOp::LogicalAnd,
                        left,
                        right,
                    })),
                })];
            }
            vec![Stmt::If(IfStmt {
                span: DUMMY_SP,
                test: left,
                cons: Box::new(expr_to_block_stmt(*right)),
                alt: None,
            })]
        }
        Expr::Bin(BinExpr {
            op: BinaryOp::LogicalOr,
            left,
            right,
            ..
        }) => {
            // x || action() → if (!x) { action(); }
            if !is_action_expr(&right) {
                return vec![Stmt::Expr(ExprStmt {
                    span,
                    expr: Box::new(Expr::Bin(BinExpr {
                        span: DUMMY_SP,
                        op: BinaryOp::LogicalOr,
                        left,
                        right,
                    })),
                })];
            }
            vec![Stmt::If(IfStmt {
                span: DUMMY_SP,
                test: negate_expr(*left),
                cons: Box::new(expr_to_block_stmt(*right)),
                alt: None,
            })]
        }
        // LogicalNullish (??) - do NOT convert
        other => vec![Stmt::Expr(ExprStmt {
            span,
            expr: Box::new(other),
        })],
    }
}

/// Check if an expression is "action-like" - has clear side effects worth converting to if/else.
/// Only consider: call expressions, new expressions, assignments, yield, await.
/// Pure reads (identifiers, literals, member access, etc.) are NOT action-like.
fn is_action_expr(expr: &Box<Expr>) -> bool {
    match expr.as_ref() {
        Expr::Call(_) | Expr::New(_) | Expr::Assign(_) | Expr::Yield(_) | Expr::Await(_) => true,
        Expr::Seq(seq) => seq.exprs.iter().any(is_action_expr),
        Expr::Paren(paren) => is_action_expr(&paren.expr),
        _ => false,
    }
}

/// Convert a ternary expression to an if statement.
fn convert_cond_to_if(test: Expr, cons: Box<Expr>, alt: Box<Expr>) -> Stmt {
    let cons_stmt = convert_cons_branch_to_stmt(*cons);
    let alt_stmt = convert_alt_branch_to_stmt(*alt);

    Stmt::If(IfStmt {
        span: DUMMY_SP,
        test: Box::new(test),
        cons: Box::new(cons_stmt),
        alt: Some(Box::new(alt_stmt)),
    })
}

/// Convert the consequent branch of a ternary to a statement.
/// If the cons is itself a ternary, wrap it in a block (not an else-if).
fn convert_cons_branch_to_stmt(expr: Expr) -> Stmt {
    match expr {
        Expr::Cond(inner) => {
            // Nested ternary in cons position → convert to if, wrapped in a block
            let inner_if = convert_cond_to_if(*inner.test, inner.cons, inner.alt);
            Stmt::Block(BlockStmt {
                span: DUMMY_SP,
                ctxt: Default::default(),
                stmts: vec![inner_if],
            })
        }
        other => expr_to_block_stmt(other),
    }
}

/// Convert the alternate branch of a ternary to a statement.
/// If the alt is another ternary/convertible-logical, make it an else-if (not wrapped in block).
fn convert_alt_branch_to_stmt(expr: Expr) -> Stmt {
    match expr {
        // Another ternary → becomes else-if chain
        Expr::Cond(inner) => convert_cond_to_if(*inner.test, inner.cons, inner.alt),
        // Logical AND in alt → convert to if statement (not wrapped in block)
        Expr::Bin(BinExpr {
            op: BinaryOp::LogicalAnd,
            left,
            right,
            ..
        }) if is_action_expr(&right) => Stmt::If(IfStmt {
            span: DUMMY_SP,
            test: left,
            cons: Box::new(expr_to_block_stmt(*right)),
            alt: None,
        }),
        // Logical OR in alt → convert to if statement
        Expr::Bin(BinExpr {
            op: BinaryOp::LogicalOr,
            left,
            right,
            ..
        }) if is_action_expr(&right) => Stmt::If(IfStmt {
            span: DUMMY_SP,
            test: negate_expr(*left),
            cons: Box::new(expr_to_block_stmt(*right)),
            alt: None,
        }),
        // Wrap in block
        other => expr_to_block_stmt(other),
    }
}

/// Wrap an expression in a block statement.
/// Sequence expressions (including paren-wrapped) are expanded into multiple ExprStmts.
fn expr_to_block_stmt(expr: Expr) -> Stmt {
    let inner = match expr {
        Expr::Paren(paren) => *paren.expr,
        other => other,
    };
    let stmts = match inner {
        Expr::Seq(seq) => seq
            .exprs
            .into_iter()
            .map(|e| {
                Stmt::Expr(ExprStmt {
                    span: DUMMY_SP,
                    expr: e,
                })
            })
            .collect(),
        other => vec![Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(other),
        })],
    };
    Stmt::Block(BlockStmt {
        span: DUMMY_SP,
        ctxt: Default::default(),
        stmts,
    })
}

/// Negate an expression, removing double negation.
fn negate_expr(expr: Expr) -> Box<Expr> {
    if let Expr::Unary(UnaryExpr {
        op: UnaryOp::Bang,
        arg,
        ..
    }) = expr
    {
        return arg;
    }
    Box::new(Expr::Unary(UnaryExpr {
        span: DUMMY_SP,
        op: UnaryOp::Bang,
        arg: Box::new(expr),
    }))
}

/// Try to split a `return cond ? a : b ? c : d` into
/// `if (cond) { return a; } if (b) { return c; } return d;`
/// Only converts if the top-level expression is a ternary.
fn try_split_return_ternary(expr: Expr, return_span: Span) -> Option<Vec<Stmt>> {
    let Expr::Cond(cond) = expr else {
        return None;
    };

    let mut stmts = Vec::new();
    build_return_chain(*cond.test, cond.cons, cond.alt, &mut stmts, return_span);
    Some(stmts)
}

fn build_return_chain(
    test: Expr,
    cons: Box<Expr>,
    alt: Box<Expr>,
    stmts: &mut Vec<Stmt>,
    span: Span,
) {
    // if (test) { return cons; }
    stmts.push(Stmt::If(IfStmt {
        span: DUMMY_SP,
        test: Box::new(test),
        cons: Box::new(Stmt::Block(BlockStmt {
            span: DUMMY_SP,
            ctxt: Default::default(),
            stmts: vec![Stmt::Return(ReturnStmt {
                span,
                arg: Some(cons),
            })],
        })),
        alt: None,
    }));

    // Recurse or emit final return
    match *alt {
        Expr::Cond(next_cond) => {
            build_return_chain(*next_cond.test, next_cond.cons, next_cond.alt, stmts, span);
        }
        other => {
            stmts.push(Stmt::Return(ReturnStmt {
                span,
                arg: Some(Box::new(other)),
            }));
        }
    }
}
