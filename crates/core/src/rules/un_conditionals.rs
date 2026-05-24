use std::collections::HashSet;

use swc_core::common::{Span, DUMMY_SP};
use swc_core::ecma::ast::{
    BinExpr, BinaryOp, BlockStmt, BreakStmt, CondExpr, Expr, ExprStmt, Ident, IfStmt, Lit,
    ModuleItem, ReturnStmt, Stmt, SwitchCase, SwitchStmt, UnaryExpr, UnaryOp,
};
use swc_core::ecma::utils::ExprFactory;
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::decl_utils::same_ident;

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
            if let Some(switch_stmt) = try_cond_to_switch_expr_stmt(&cond_expr) {
                return vec![switch_stmt];
            }

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
                    expr: Box::new((*left).make_bin(BinaryOp::LogicalAnd, *right)),
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
                    expr: Box::new((*left).make_bin(BinaryOp::LogicalOr, *right)),
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

#[derive(Clone)]
struct SwitchChain {
    discriminant: Ident,
    cases: Vec<(Box<Expr>, Box<Expr>)>,
    default: Box<Expr>,
}

fn try_cond_to_switch_expr_stmt(cond: &CondExpr) -> Option<Stmt> {
    let chain = collect_switch_chain(cond)?;
    if !chain_has_action(&chain) {
        return None;
    }

    Some(Stmt::Switch(SwitchStmt {
        span: DUMMY_SP,
        discriminant: Box::new(Expr::Ident(chain.discriminant.clone())),
        cases: switch_cases_from_expr_chain(chain),
    }))
}

fn try_cond_to_switch_return(cond: &CondExpr, return_span: Span) -> Option<Stmt> {
    let chain = collect_switch_chain(cond)?;
    let mut cases = Vec::with_capacity(chain.cases.len() + 1);

    for (test, body) in chain.cases {
        cases.push(SwitchCase {
            span: DUMMY_SP,
            test: Some(test),
            cons: vec![Stmt::Return(ReturnStmt {
                span: return_span,
                arg: Some(body),
            })],
        });
    }

    cases.push(SwitchCase {
        span: DUMMY_SP,
        test: None,
        cons: vec![Stmt::Return(ReturnStmt {
            span: return_span,
            arg: Some(chain.default),
        })],
    });

    Some(Stmt::Switch(SwitchStmt {
        span: DUMMY_SP,
        discriminant: Box::new(Expr::Ident(chain.discriminant)),
        cases,
    }))
}

fn collect_switch_chain(cond: &CondExpr) -> Option<SwitchChain> {
    let mut discriminant = None;
    let mut cases = Vec::new();
    let mut seen_cases = HashSet::new();
    let mut current = cond;

    loop {
        let (case_discriminant, case_test) = extract_strict_case_test(&current.test)?;
        if let Some(existing) = &discriminant {
            if !same_ident(existing, &case_discriminant) {
                return None;
            }
        } else {
            discriminant = Some(case_discriminant);
        }

        let key = literal_case_key(&case_test)?;
        if !seen_cases.insert(key) {
            return None;
        }
        cases.push((case_test, current.cons.clone()));

        match current.alt.as_ref() {
            Expr::Cond(next) => current = next,
            _ => {
                if cases.len() < 2 {
                    return None;
                }
                return Some(SwitchChain {
                    discriminant: discriminant.expect("set by first case"),
                    cases,
                    default: current.alt.clone(),
                });
            }
        }
    }
}

fn extract_strict_case_test(test: &Expr) -> Option<(Ident, Box<Expr>)> {
    let Expr::Bin(BinExpr {
        op: BinaryOp::EqEqEq,
        left,
        right,
        ..
    }) = unparen_expr(test)
    else {
        return None;
    };

    match (unparen_expr(left), unparen_expr(right)) {
        (Expr::Ident(discriminant), case) if literal_case_key(case).is_some() => {
            Some((discriminant.clone(), Box::new(case.clone())))
        }
        (case, Expr::Ident(discriminant)) if literal_case_key(case).is_some() => {
            Some((discriminant.clone(), Box::new(case.clone())))
        }
        _ => None,
    }
}

fn switch_cases_from_expr_chain(chain: SwitchChain) -> Vec<SwitchCase> {
    let mut cases = Vec::with_capacity(chain.cases.len() + 1);
    for (test, body) in chain.cases {
        cases.push(SwitchCase {
            span: DUMMY_SP,
            test: Some(test),
            cons: expr_to_case_stmts(*body, true),
        });
    }

    cases.push(SwitchCase {
        span: DUMMY_SP,
        test: None,
        cons: expr_to_case_stmts(*chain.default, false),
    });

    cases
}

fn expr_to_case_stmts(expr: Expr, append_break: bool) -> Vec<Stmt> {
    let inner = strip_paren_expr(expr);
    let mut stmts = match inner {
        Expr::Seq(seq) => seq
            .exprs
            .into_iter()
            .flat_map(|expr| expr_to_case_stmts(*expr, false))
            .collect(),
        Expr::Cond(cond) => vec![convert_cond_to_if(*cond.test, cond.cons, cond.alt)],
        other => convert_stmt(Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(other),
        })),
    };

    if append_break {
        stmts.push(Stmt::Break(BreakStmt {
            span: DUMMY_SP,
            label: None,
        }));
    }

    stmts
}

fn chain_has_action(chain: &SwitchChain) -> bool {
    chain.cases.iter().any(|(_, body)| is_action_expr(body)) || is_action_expr(&chain.default)
}

fn literal_case_key(expr: &Expr) -> Option<String> {
    match unparen_expr(expr) {
        Expr::Lit(Lit::Str(value)) => Some(format!("str:{}", value.value.to_string_lossy())),
        Expr::Lit(Lit::Bool(value)) => Some(format!("bool:{}", value.value)),
        Expr::Lit(Lit::Null(_)) => Some("null".to_string()),
        Expr::Lit(Lit::Num(value)) => Some(format!(
            "num:{}:{}",
            value.value,
            value.value.is_sign_positive()
        )),
        Expr::Lit(Lit::BigInt(value)) => Some(format!("bigint:{}", value.value)),
        _ => None,
    }
}

fn unparen_expr(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => unparen_expr(&paren.expr),
        other => other,
    }
}

fn strip_paren_expr(expr: Expr) -> Expr {
    match expr {
        Expr::Paren(paren) => strip_paren_expr(*paren.expr),
        other => other,
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
/// Sequence expressions (including paren-wrapped) are expanded into converted statements.
fn expr_to_block_stmt(expr: Expr) -> Stmt {
    let inner = match expr {
        Expr::Paren(paren) => *paren.expr,
        other => other,
    };
    let stmts = match inner {
        Expr::Seq(seq) => seq
            .exprs
            .into_iter()
            .flat_map(|expr| {
                convert_stmt(Stmt::Expr(ExprStmt {
                    span: DUMMY_SP,
                    expr,
                }))
            })
            .collect(),
        other => convert_stmt(Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(other),
        })),
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

    if let Some(switch_stmt) = try_cond_to_switch_return(&cond, return_span) {
        return Some(vec![switch_stmt]);
    }

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
