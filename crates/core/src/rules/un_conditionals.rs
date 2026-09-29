use crate::collections::HashSet;

use swc_core::common::{Span, DUMMY_SP};
use swc_core::ecma::ast::{
    BinExpr, BinaryOp, BlockStmt, BreakStmt, CondExpr, Expr, ExprStmt, Ident, IfStmt, Lit,
    ModuleItem, ReturnStmt, Stmt, SwitchCase, SwitchStmt, UnaryExpr, UnaryOp,
};
use swc_core::ecma::utils::ExprFactory;
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::decl_utils::same_ident;
use crate::utils::paren::strip_parens_owned;

/// Rewrites short-circuit and ternary expression statements into `if`
/// statements, and splits return ternaries.
///
/// The default pass converts a statement only when a branch is itself an
/// action (call, assignment, ...). [`UnConditionals::with_nested_actions`]
/// also converts branches whose action sits under another `&&`, `||`, or
/// ternary, such as `a && (b ? f() : g())`. The pipeline enables that only in
/// the cleanup pass: class and helper recovery match the expression form of
/// inlined helpers such as `t && (Object.setPrototypeOf ? ... : ...)`.
#[derive(Default)]
pub struct UnConditionals {
    nested_actions: bool,
}

impl UnConditionals {
    pub fn with_nested_actions() -> Self {
        Self {
            nested_actions: true,
        }
    }
}

impl VisitMut for UnConditionals {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        let old = std::mem::take(items);
        for item in old {
            match item {
                ModuleItem::Stmt(stmt) => {
                    let converted = convert_stmt(stmt, self.nested_actions);
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
            stmts.extend(convert_stmt(stmt, self.nested_actions));
        }
    }
}

pub struct UnConditionalsAssignmentOnly;

impl VisitMut for UnConditionalsAssignmentOnly {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        let old = std::mem::take(items);
        for item in old {
            match item {
                ModuleItem::Stmt(stmt) => {
                    let converted = convert_assignment_only_stmt(stmt);
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
            stmts.extend(convert_assignment_only_stmt(stmt));
        }
    }
}

pub struct UnConditionalsExprStmtOnly;

impl VisitMut for UnConditionalsExprStmtOnly {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        let old = std::mem::take(items);
        for item in old {
            match item {
                ModuleItem::Stmt(stmt) => {
                    let converted = convert_cond_expr_stmt_only(stmt);
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
            stmts.extend(convert_cond_expr_stmt_only(stmt));
        }
    }
}

/// Convert a single statement, returning one or more statements.
fn convert_stmt(stmt: Stmt, nested: bool) -> Vec<Stmt> {
    match stmt {
        Stmt::Expr(ExprStmt { expr, span }) => try_convert_expr_stmt_to_if(span, *expr, nested),
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

fn convert_cond_expr_stmt_only(stmt: Stmt) -> Vec<Stmt> {
    match stmt {
        Stmt::Expr(ExprStmt { expr, span }) if matches!(*expr, Expr::Cond(_)) => {
            try_convert_expr_stmt_to_if(span, *expr, false)
        }
        other => vec![other],
    }
}

fn convert_assignment_only_stmt(stmt: Stmt) -> Vec<Stmt> {
    match stmt {
        Stmt::Expr(ExprStmt { expr, span }) => try_convert_assignment_only_expr_stmt(span, *expr),
        other => vec![other],
    }
}

fn try_convert_assignment_only_expr_stmt(span: Span, expr: Expr) -> Vec<Stmt> {
    match expr {
        Expr::Bin(BinExpr {
            op: BinaryOp::LogicalAnd,
            left,
            right,
            ..
        }) if is_assignment_only_expr(&right) => vec![Stmt::If(IfStmt {
            span,
            test: left,
            cons: Box::new(expr_to_assignment_only_block_stmt(*right)),
            alt: None,
        })],
        Expr::Bin(BinExpr {
            op: BinaryOp::LogicalOr,
            left,
            right,
            ..
        }) if is_assignment_only_expr(&right) => vec![Stmt::If(IfStmt {
            span,
            test: negate_expr(*left),
            cons: Box::new(expr_to_assignment_only_block_stmt(*right)),
            alt: None,
        })],
        other => vec![Stmt::Expr(ExprStmt {
            span,
            expr: Box::new(other),
        })],
    }
}

fn is_assignment_only_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Assign(_) => true,
        Expr::Seq(seq) => seq.exprs.iter().all(|expr| is_assignment_only_expr(expr)),
        Expr::Paren(paren) => is_assignment_only_expr(&paren.expr),
        _ => false,
    }
}

fn expr_to_assignment_only_block_stmt(expr: Expr) -> Stmt {
    let inner = match expr {
        Expr::Paren(paren) => *paren.expr,
        other => other,
    };
    let stmts = match inner {
        Expr::Seq(seq) => seq
            .exprs
            .into_iter()
            .flat_map(|expr| {
                convert_assignment_only_stmt(Stmt::Expr(ExprStmt {
                    span: DUMMY_SP,
                    expr,
                }))
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

/// Try to convert an ExprStmt-level expression to an if statement.
/// Returns a Vec<Stmt> which is either the converted if statement(s) or
/// the original ExprStmt wrapped in a Vec.
fn try_convert_expr_stmt_to_if(span: Span, expr: Expr, nested: bool) -> Vec<Stmt> {
    match expr {
        Expr::Cond(cond_expr) => {
            if let Some(switch_stmt) = try_cond_to_switch_expr_stmt(&cond_expr, nested) {
                return vec![switch_stmt];
            }

            // Only convert if at least one branch is action-like (has side effects)
            if !is_action_expr(&cond_expr.cons, nested) && !is_action_expr(&cond_expr.alt, nested) {
                return vec![Stmt::Expr(ExprStmt {
                    span,
                    expr: Box::new(Expr::Cond(cond_expr)),
                })];
            }
            vec![convert_cond_to_if(
                span,
                *cond_expr.test,
                cond_expr.cons,
                cond_expr.alt,
                nested,
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
            if !is_action_expr(&right, nested) {
                return vec![Stmt::Expr(ExprStmt {
                    span,
                    expr: Box::new((*left).make_bin(BinaryOp::LogicalAnd, *right)),
                })];
            }
            vec![Stmt::If(IfStmt {
                span,
                test: left,
                cons: Box::new(expr_to_block_stmt(*right, nested)),
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
            if !is_action_expr(&right, nested) {
                return vec![Stmt::Expr(ExprStmt {
                    span,
                    expr: Box::new((*left).make_bin(BinaryOp::LogicalOr, *right)),
                })];
            }
            vec![Stmt::If(IfStmt {
                span,
                test: negate_expr(*left),
                cons: Box::new(expr_to_block_stmt(*right, nested)),
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
/// Actions are calls (including optional calls), `new`, assignments, updates,
/// `delete`, yield, and await, and a sequence holding one. With `nested`, a
/// ternary with an action branch and `&&` / `||` with an action on the right
/// also count; the recursive statement conversion of the branch recovers them.
/// Pure reads (identifiers, literals, member access, etc.) are NOT action-like.
fn is_action_expr(expr: &Box<Expr>, nested: bool) -> bool {
    match expr.as_ref() {
        Expr::Call(_)
        | Expr::New(_)
        | Expr::Assign(_)
        | Expr::Update(_)
        | Expr::Yield(_)
        | Expr::Await(_) => true,
        Expr::Unary(UnaryExpr {
            op: UnaryOp::Delete,
            ..
        }) => true,
        Expr::OptChain(opt) => opt.base.is_call(),
        Expr::Seq(seq) => seq.exprs.iter().any(|expr| is_action_expr(expr, nested)),
        Expr::Paren(paren) => is_action_expr(&paren.expr, nested),
        Expr::Cond(cond) if nested => {
            is_action_expr(&cond.cons, nested) || is_action_expr(&cond.alt, nested)
        }
        Expr::Bin(BinExpr {
            op: BinaryOp::LogicalAnd | BinaryOp::LogicalOr,
            right,
            ..
        }) if nested => is_action_expr(right, nested),
        _ => false,
    }
}

#[derive(Clone)]
struct SwitchChain {
    discriminant: Ident,
    cases: Vec<(Box<Expr>, Box<Expr>)>,
    default: Box<Expr>,
}

fn try_cond_to_switch_expr_stmt(cond: &CondExpr, nested: bool) -> Option<Stmt> {
    let chain = collect_switch_chain(cond)?;
    if !chain_has_action(&chain, nested) {
        return None;
    }

    Some(Stmt::Switch(SwitchStmt {
        span: DUMMY_SP,
        body_ctxt: Default::default(),
        discriminant: Box::new(Expr::Ident(chain.discriminant.clone())),
        cases: switch_cases_from_expr_chain(chain, nested),
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
        body_ctxt: Default::default(),
        discriminant: Box::new(Expr::Ident(chain.discriminant)),
        cases,
    }))
}

fn collect_switch_chain(cond: &CondExpr) -> Option<SwitchChain> {
    let mut discriminant = None;
    let mut cases = Vec::new();
    let mut seen_cases = HashSet::default();
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

fn switch_cases_from_expr_chain(chain: SwitchChain, nested: bool) -> Vec<SwitchCase> {
    let mut cases = Vec::with_capacity(chain.cases.len() + 1);
    for (test, body) in chain.cases {
        cases.push(SwitchCase {
            span: DUMMY_SP,
            test: Some(test),
            cons: expr_to_case_stmts(*body, true, nested),
        });
    }

    cases.push(SwitchCase {
        span: DUMMY_SP,
        test: None,
        cons: expr_to_case_stmts(*chain.default, false, nested),
    });

    cases
}

fn expr_to_case_stmts(expr: Expr, append_break: bool, nested: bool) -> Vec<Stmt> {
    let inner = strip_parens_owned(expr);
    let mut stmts = match inner {
        Expr::Seq(seq) => seq
            .exprs
            .into_iter()
            .flat_map(|expr| expr_to_case_stmts(*expr, false, nested))
            .collect(),
        Expr::Cond(cond) => vec![convert_cond_to_if(
            cond.span, *cond.test, cond.cons, cond.alt, nested,
        )],
        other => convert_stmt(
            Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: Box::new(other),
            }),
            nested,
        ),
    };

    if append_break {
        stmts.push(Stmt::Break(BreakStmt {
            span: DUMMY_SP,
            label: None,
        }));
    }

    stmts
}

fn chain_has_action(chain: &SwitchChain, nested: bool) -> bool {
    chain
        .cases
        .iter()
        .any(|(_, body)| is_action_expr(body, nested))
        || is_action_expr(&chain.default, nested)
}

fn literal_case_key(expr: &Expr) -> Option<String> {
    match unparen_expr(expr) {
        Expr::Lit(Lit::Str(value)) => Some(format!("str:{}", value.value.to_string_lossy())),
        Expr::Lit(Lit::Bool(value)) => Some(format!("bool:{}", value.value)),
        Expr::Lit(Lit::Null(_)) => Some("null".to_string()),
        Expr::Lit(Lit::Num(value)) => Some(numeric_case_key(value.value)),
        Expr::Unary(UnaryExpr { op, arg, .. }) if matches!(op, UnaryOp::Plus | UnaryOp::Minus) => {
            let Expr::Lit(Lit::Num(value)) = unparen_expr(arg) else {
                return None;
            };
            let value = if *op == UnaryOp::Minus {
                -value.value
            } else {
                value.value
            };
            Some(numeric_case_key(value))
        }
        Expr::Lit(Lit::BigInt(value)) => Some(format!("bigint:{}", value.value)),
        _ => None,
    }
}

fn numeric_case_key(value: f64) -> String {
    format!("num:{}:{}", value, value.is_sign_positive())
}

fn unparen_expr(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => unparen_expr(&paren.expr),
        other => other,
    }
}

/// Convert a ternary expression to an if statement.
fn convert_cond_to_if(
    span: Span,
    test: Expr,
    cons: Box<Expr>,
    alt: Box<Expr>,
    nested: bool,
) -> Stmt {
    let cons_stmt = convert_cons_branch_to_stmt(*cons, nested);
    let alt_stmt = convert_alt_branch_to_stmt(*alt, nested);

    Stmt::If(IfStmt {
        span,
        test: Box::new(test),
        cons: Box::new(cons_stmt),
        alt: Some(Box::new(alt_stmt)),
    })
}

/// Convert the consequent branch of a ternary to a statement.
/// If the cons is itself a ternary, wrap it in a block (not an else-if).
fn convert_cons_branch_to_stmt(expr: Expr, nested: bool) -> Stmt {
    match expr {
        Expr::Cond(inner) => {
            // Nested ternary in cons position → convert to if, wrapped in a block
            let inner_if =
                convert_cond_to_if(inner.span, *inner.test, inner.cons, inner.alt, nested);
            Stmt::Block(BlockStmt {
                span: DUMMY_SP,
                ctxt: Default::default(),
                stmts: vec![inner_if],
            })
        }
        other => expr_to_block_stmt(other, nested),
    }
}

/// Convert the alternate branch of a ternary to a statement.
/// If the alt is another ternary/convertible-logical, make it an else-if (not wrapped in block).
fn convert_alt_branch_to_stmt(expr: Expr, nested: bool) -> Stmt {
    match expr {
        // Another ternary → becomes else-if chain
        Expr::Cond(inner) => {
            convert_cond_to_if(inner.span, *inner.test, inner.cons, inner.alt, nested)
        }
        // Logical AND in alt → convert to if statement (not wrapped in block)
        Expr::Bin(BinExpr {
            span,
            op: BinaryOp::LogicalAnd,
            left,
            right,
        }) if is_action_expr(&right, nested) => Stmt::If(IfStmt {
            span,
            test: left,
            cons: Box::new(expr_to_block_stmt(*right, nested)),
            alt: None,
        }),
        // Logical OR in alt → convert to if statement
        Expr::Bin(BinExpr {
            span,
            op: BinaryOp::LogicalOr,
            left,
            right,
        }) if is_action_expr(&right, nested) => Stmt::If(IfStmt {
            span,
            test: negate_expr(*left),
            cons: Box::new(expr_to_block_stmt(*right, nested)),
            alt: None,
        }),
        // Wrap in block
        other => expr_to_block_stmt(other, nested),
    }
}

/// Wrap an expression in a block statement.
/// Sequence expressions (including paren-wrapped) are expanded into converted statements.
fn expr_to_block_stmt(expr: Expr, nested: bool) -> Stmt {
    let inner = match expr {
        Expr::Paren(paren) => *paren.expr,
        other => other,
    };
    let stmts = match inner {
        Expr::Seq(seq) => seq
            .exprs
            .into_iter()
            .flat_map(|expr| {
                convert_stmt(
                    Stmt::Expr(ExprStmt {
                        span: DUMMY_SP,
                        expr,
                    }),
                    nested,
                )
            })
            .collect(),
        other => convert_stmt(
            Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: Box::new(other),
            }),
            nested,
        ),
    };
    Stmt::Block(BlockStmt {
        span: DUMMY_SP,
        ctxt: Default::default(),
        stmts,
    })
}

/// Negate an expression, removing double negation and flipping equality
/// operators. Relational operators keep the `!` because `!(a < b)` is not
/// `a >= b` when either side is NaN.
fn negate_expr(expr: Expr) -> Box<Expr> {
    match expr {
        Expr::Unary(UnaryExpr {
            op: UnaryOp::Bang,
            arg,
            ..
        }) => arg,
        Expr::Bin(mut bin)
            if matches!(
                bin.op,
                BinaryOp::EqEq | BinaryOp::NotEq | BinaryOp::EqEqEq | BinaryOp::NotEqEq
            ) =>
        {
            bin.op = match bin.op {
                BinaryOp::EqEq => BinaryOp::NotEq,
                BinaryOp::NotEq => BinaryOp::EqEq,
                BinaryOp::EqEqEq => BinaryOp::NotEqEq,
                _ => BinaryOp::EqEqEq,
            };
            Box::new(Expr::Bin(bin))
        }
        expr => Box::new(Expr::Unary(UnaryExpr {
            span: DUMMY_SP,
            op: UnaryOp::Bang,
            arg: Box::new(expr),
        })),
    }
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
    build_return_chain(
        cond.span,
        *cond.test,
        cond.cons,
        cond.alt,
        &mut stmts,
        return_span,
    );
    Some(stmts)
}

/// `cond_span` is the span of the ternary this `if` replaces.
fn build_return_chain(
    cond_span: Span,
    test: Expr,
    cons: Box<Expr>,
    alt: Box<Expr>,
    stmts: &mut Vec<Stmt>,
    span: Span,
) {
    // if (test) { return cons; }
    stmts.push(Stmt::If(IfStmt {
        span: cond_span,
        test: Box::new(test),
        cons: Box::new(Stmt::Block(BlockStmt {
            span: DUMMY_SP,
            ctxt: Default::default(),
            stmts: return_stmts_from_expr(*cons, span),
        })),
        alt: None,
    }));

    // Recurse or emit final return
    match *alt {
        Expr::Cond(next_cond) => {
            build_return_chain(
                next_cond.span,
                *next_cond.test,
                next_cond.cons,
                next_cond.alt,
                stmts,
                span,
            );
        }
        other => {
            stmts.push(Stmt::Return(ReturnStmt {
                span,
                arg: Some(Box::new(other)),
            }));
        }
    }
}

fn return_stmts_from_expr(expr: Expr, span: Span) -> Vec<Stmt> {
    match expr {
        Expr::Cond(_) => try_split_return_ternary(expr, span).expect("checked it is Cond above"),
        other => vec![Stmt::Return(ReturnStmt {
            span,
            arg: Some(Box::new(other)),
        })],
    }
}
