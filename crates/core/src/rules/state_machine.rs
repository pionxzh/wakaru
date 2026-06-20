use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrowExpr, BlockStmt, CatchClause, Expr, ExprOrSpread, Function, Ident, IfStmt, Lit, Pat, Stmt,
    TryStmt, UnaryExpr, UnaryOp,
};
use swc_core::ecma::visit::{Visit, VisitWith};

pub(crate) fn reconstruct_with_regions(
    label_stmts: Vec<Vec<Stmt>>,
    trys: &[[Option<usize>; 4]],
) -> Vec<Stmt> {
    if trys.is_empty() {
        return label_stmts.into_iter().flatten().collect();
    }

    let mut result: Vec<Stmt> = Vec::new();
    let n = label_stmts.len();
    let mut i = 0usize;

    while i < n {
        let region = trys.iter().find(|r| r[0] == Some(i));
        if let Some(region) = region {
            let [_try_start, catch_start, finally_start, next] = *region;

            let try_end = catch_start.or(finally_start).unwrap_or(n);
            let try_stmts: Vec<Stmt> = label_stmts[i..try_end.min(n)]
                .iter()
                .flatten()
                .cloned()
                .collect();

            let catch_clause = if let Some(cs) = catch_start {
                let catch_end = finally_start.or(next).unwrap_or(n);
                let cs = cs.min(n);
                let catch_stmts: Vec<Stmt> = label_stmts[cs..catch_end.min(n)]
                    .iter()
                    .flatten()
                    .cloned()
                    .collect();
                Some(CatchClause {
                    span: DUMMY_SP,
                    param: Some(Pat::Ident(swc_core::ecma::ast::BindingIdent {
                        id: Ident::new_no_ctxt("error".into(), DUMMY_SP),
                        type_ann: None,
                    })),
                    body: BlockStmt {
                        span: DUMMY_SP,
                        ctxt: Default::default(),
                        stmts: catch_stmts,
                    },
                })
            } else {
                None
            };

            let finally_block = if let Some(fs) = finally_start {
                let finally_end = next.unwrap_or(n);
                let fs = fs.min(n);
                let finally_stmts: Vec<Stmt> = label_stmts[fs..finally_end.min(n)]
                    .iter()
                    .flatten()
                    .cloned()
                    .collect();
                Some(BlockStmt {
                    span: DUMMY_SP,
                    ctxt: Default::default(),
                    stmts: finally_stmts,
                })
            } else {
                None
            };

            result.push(Stmt::Try(Box::new(TryStmt {
                span: DUMMY_SP,
                block: BlockStmt {
                    span: DUMMY_SP,
                    ctxt: Default::default(),
                    stmts: try_stmts,
                },
                handler: catch_clause,
                finalizer: finally_block,
            })));

            i = next.unwrap_or(n);
        } else {
            let in_region = trys.iter().any(|r| {
                let start = r[0].unwrap_or(usize::MAX);
                let end = r[3].or(r[2]).or(r[1]).unwrap_or(0);
                i > start && i < end
            });
            if !in_region {
                result.extend(label_stmts[i].iter().cloned());
            }
            i += 1;
        }
    }

    result
}

/// Resolve forward jumps of the form `if (test) { return [3, N]; }` using
/// label-index pairs. Stmts between the jump and label N become the "then"
/// body; stmts at label N+ continue after the if-block. Only resolves jumps
/// where the body between the jump and target is opcode-free.
pub(crate) fn resolve_labeled_forward_jumps(stmts: Vec<(usize, Stmt)>) -> Vec<(usize, Stmt)> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < stmts.len() {
        if let Some((recovered_label, recovered, consumed)) =
            try_resolve_labeled_forward_jump(&stmts[index..])
        {
            result.push((recovered_label, recovered));
            index += consumed;
        } else {
            result.push(stmts[index].clone());
            index += 1;
        }
    }
    result
}

fn try_resolve_labeled_forward_jump(stmts: &[(usize, Stmt)]) -> Option<(usize, Stmt, usize)> {
    let (start_label, first_stmt) = stmts.first()?;
    let Stmt::If(if_stmt) = first_stmt else {
        return None;
    };
    if if_stmt.alt.is_some() {
        return None;
    }
    let target = jump_target_stmt(&if_stmt.cons)?;
    if target <= *start_label {
        return None;
    }

    let max_remaining_label = stmts[1..].iter().map(|(l, _)| *l).max().unwrap_or(0);
    if target <= max_remaining_label {
        return None;
    }

    let body_stmts: Vec<Stmt> = stmts[1..].iter().map(|(_, s)| s.clone()).collect();
    if body_stmts.is_empty() || stmts_contain_state_opcode_return(&body_stmts) {
        return None;
    }

    Some((
        *start_label,
        Stmt::If(IfStmt {
            span: DUMMY_SP,
            test: invert_condition(&if_stmt.test),
            cons: Box::new(Stmt::Block(BlockStmt {
                span: DUMMY_SP,
                ctxt: Default::default(),
                stmts: body_stmts,
            })),
            alt: None,
        }),
        stmts.len(),
    ))
}

fn stmts_contain_state_opcode_return(stmts: &[Stmt]) -> bool {
    struct Finder {
        found: bool,
    }
    impl Visit for Finder {
        fn visit_function(&mut self, _func: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}

        fn visit_return_stmt(&mut self, ret: &swc_core::ecma::ast::ReturnStmt) {
            if let Some(Expr::Array(arr)) = ret.arg.as_deref() {
                if arr
                    .elems
                    .first()
                    .and_then(|e| e.as_ref())
                    .is_some_and(|e| matches!(e.expr.as_ref(), Expr::Lit(Lit::Num(_))))
                {
                    self.found = true;
                    return;
                }
            }
            ret.visit_children_with(self);
        }
    }
    let mut finder = Finder { found: false };
    for stmt in stmts {
        stmt.visit_with(&mut finder);
        if finder.found {
            return true;
        }
    }
    false
}

fn jump_target_stmt(stmt: &Stmt) -> Option<usize> {
    match stmt {
        Stmt::Return(_) => return_jump_target(stmt),
        Stmt::Block(block) if block.stmts.len() == 1 => return_jump_target(&block.stmts[0]),
        _ => None,
    }
}

fn return_jump_target(stmt: &Stmt) -> Option<usize> {
    let Stmt::Return(ret) = stmt else {
        return None;
    };
    let Expr::Array(arr) = ret.arg.as_deref()? else {
        return None;
    };
    if arr.elems.len() < 2 {
        return None;
    }
    let opcode = jump_array_elem_number(arr.elems.first()?)?;
    if opcode != 3 {
        return None;
    }
    Some(jump_array_elem_number(arr.elems.get(1)?)? as usize)
}

fn jump_array_elem_number(elem: &Option<ExprOrSpread>) -> Option<u32> {
    let Expr::Lit(Lit::Num(num)) = elem.as_ref()?.expr.as_ref() else {
        return None;
    };
    Some(num.value as u32)
}

fn invert_condition(test: &Expr) -> Box<Expr> {
    if let Expr::Unary(unary) = test {
        if unary.op == UnaryOp::Bang {
            return unary.arg.clone();
        }
    }

    Box::new(Expr::Unary(UnaryExpr {
        span: DUMMY_SP,
        op: UnaryOp::Bang,
        arg: Box::new(test.clone()),
    }))
}
