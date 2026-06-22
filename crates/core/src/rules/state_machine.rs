use std::collections::HashSet;

use swc_core::common::util::take::Take;
use swc_core::common::{Spanned, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, AssignTarget, BlockStmt, BreakStmt, CatchClause, CondExpr,
    ContinueStmt, Expr, ExprOrSpread, ExprStmt, ForStmt, Function, Ident, IfStmt, Lit, Pat,
    SimpleAssignTarget, Stmt, TryStmt, UnaryExpr, UnaryOp,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::helper_matcher::{binding_key, BindingKey};

#[derive(Clone, Copy)]
pub(crate) enum OpcodeReturnScan {
    SkipNestedFunctions,
    IncludeNestedFunctions,
}

impl OpcodeReturnScan {
    fn skip_nested_functions(self) -> bool {
        matches!(self, Self::SkipNestedFunctions)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum IndexLoopContinueMode {
    /// TypeScript-style state machines use the label before the break target as
    /// the continue target when loop-body jump returns are present.
    AdjacentBackEdge,
    /// Babel/regenerator recovery can infer continue from the single non-break
    /// jump target inside the loop body.
    SingleBodyJumpTarget,
}

/// Label-indexed state-machine output after opcode decoding, before structured
/// control-flow recovery finishes.
#[derive(Clone)]
pub(crate) struct StateMachineProgram {
    blocks: Vec<StateBlock>,
    try_regions: Vec<[Option<usize>; 4]>,
}

impl StateMachineProgram {
    pub(crate) fn from_labeled_stmts(
        stmts: Vec<(usize, Stmt)>,
        try_regions: Vec<[Option<usize>; 4]>,
    ) -> Self {
        Self {
            blocks: stmts
                .into_iter()
                .map(|(label, stmt)| StateBlock::new(label, vec![stmt]))
                .collect(),
            try_regions,
        }
    }

    pub(crate) fn resolve_labeled_forward_jumps(mut self, opcode_scan: OpcodeReturnScan) -> Self {
        self.blocks = resolve_labeled_forward_jump_blocks(self.blocks, opcode_scan);
        self
    }

    pub(crate) fn recover_conditional_assignments(mut self) -> Self {
        self.blocks = recover_conditional_assignment_blocks(self.blocks);
        self
    }

    pub(crate) fn recover_conditional_branches(mut self, opcode_scan: OpcodeReturnScan) -> Self {
        self.blocks = recover_conditional_branch_blocks(self.blocks, opcode_scan);
        self
    }

    pub(crate) fn into_reconstructed_stmts(self) -> Vec<Stmt> {
        let Self {
            blocks,
            try_regions,
        } = self;
        reconstruct_with_regions(label_stmts_from_blocks(blocks), &try_regions)
    }

    pub(crate) fn into_reconstructed_stmts_with_index_loops(
        self,
        continue_mode: IndexLoopContinueMode,
    ) -> Vec<Stmt> {
        recover_index_loops(self.into_reconstructed_stmts(), continue_mode)
    }
}

#[derive(Clone)]
struct StateBlock {
    label: usize,
    stmts: Vec<Stmt>,
}

impl StateBlock {
    fn new(label: usize, stmts: Vec<Stmt>) -> Self {
        Self { label, stmts }
    }

    fn terminator(&self) -> StateTerminator {
        self.stmts
            .last()
            .map(StateTerminator::from_stmt)
            .unwrap_or(StateTerminator::Fallthrough)
    }
}

enum StateTerminator {
    ConditionalJump { test: Box<Expr>, target: usize },
    Jump { target: usize },
    Return,
    Fallthrough,
}

impl StateTerminator {
    fn from_stmt(stmt: &Stmt) -> Self {
        if let Stmt::If(if_stmt) = stmt {
            if if_stmt.alt.is_none() {
                if let Some(target) = jump_target_stmt(&if_stmt.cons) {
                    return Self::ConditionalJump {
                        test: if_stmt.test.clone(),
                        target,
                    };
                }
            }
        }

        if let Some(target) = jump_target_stmt(stmt) {
            return Self::Jump { target };
        }

        if matches!(stmt, Stmt::Return(_)) {
            return Self::Return;
        }

        Self::Fallthrough
    }

    fn jump_target(&self) -> Option<usize> {
        match self {
            StateTerminator::ConditionalJump { target, .. } | StateTerminator::Jump { target } => {
                Some(*target)
            }
            StateTerminator::Return | StateTerminator::Fallthrough => None,
        }
    }
}

fn label_stmts_from_blocks(blocks: Vec<StateBlock>) -> Vec<Vec<Stmt>> {
    let max_label = blocks.iter().map(|block| block.label).max().unwrap_or(0);
    let mut label_stmts: Vec<Vec<Stmt>> = vec![vec![]; max_label + 1];
    for block in blocks {
        label_stmts[block.label].extend(block.stmts);
    }
    label_stmts
}

fn recover_conditional_assignment_blocks(blocks: Vec<StateBlock>) -> Vec<StateBlock> {
    let mut result = Vec::new();
    let mut index = 0usize;

    while index < blocks.len() {
        if let Some((block, consumed)) = try_recover_conditional_assignment(&blocks[index..]) {
            result.push(block);
            index += consumed;
        } else {
            result.push(blocks[index].clone());
            index += 1;
        }
    }

    result
}

fn try_recover_conditional_assignment(blocks: &[StateBlock]) -> Option<(StateBlock, usize)> {
    let first_block = blocks.first()?;
    let start_label = first_block.label;
    let StateTerminator::ConditionalJump { test, target } = first_block.terminator() else {
        return None;
    };
    if target <= start_label + 1 {
        return None;
    }

    let mut cursor = 1usize;
    let mut fallthrough_stmts = Vec::new();
    while let Some(block) = blocks.get(cursor) {
        if block.label >= target {
            break;
        }
        fallthrough_stmts.extend(block.stmts.iter().cloned());
        cursor += 1;
    }

    let mut target_stmts = Vec::new();
    while let Some(block) = blocks.get(cursor) {
        if block.label != target {
            break;
        }
        target_stmts.extend(block.stmts.iter().cloned());
        cursor += 1;
    }
    strip_final_jump_after(&mut fallthrough_stmts, target);
    strip_final_jump_after(&mut target_stmts, target);

    if fallthrough_stmts.len() != 1 || target_stmts.len() != 1 {
        return None;
    }

    let (fallthrough_key, left, fallthrough_value) = conditional_assignment(&fallthrough_stmts[0])?;
    let (target_key, _, target_value) = conditional_assignment(&target_stmts[0])?;
    if fallthrough_key != target_key {
        return None;
    }

    Some((
        StateBlock::new(
            start_label,
            vec![assign_stmt(
                left,
                Box::new(Expr::Cond(CondExpr {
                    span: DUMMY_SP,
                    test,
                    cons: target_value,
                    alt: fallthrough_value,
                })),
            )],
        ),
        cursor,
    ))
}

fn conditional_assignment(stmt: &Stmt) -> Option<(BindingKey, AssignTarget, Box<Expr>)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(left)) = &assign.left else {
        return None;
    };
    Some((
        binding_key(&left.id),
        assign.left.clone(),
        assign.right.clone(),
    ))
}

fn assign_stmt(left: AssignTarget, right: Box<Expr>) -> Stmt {
    Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left,
            right,
        })),
    })
}

fn recover_conditional_branch_blocks(
    mut blocks: Vec<StateBlock>,
    opcode_scan: OpcodeReturnScan,
) -> Vec<StateBlock> {
    let mut result = Vec::new();
    let mut index = 0usize;

    while index < blocks.len() {
        if let Some((block, consumed)) =
            try_recover_conditional_branch(&blocks[index..], opcode_scan)
        {
            result.push(block);
            index += consumed;
        } else {
            let label = blocks[index].label;
            let stmts = std::mem::take(&mut blocks[index].stmts);
            result.push(StateBlock::new(label, stmts));
            index += 1;
        }
    }

    result
}

fn try_recover_conditional_branch(
    blocks: &[StateBlock],
    opcode_scan: OpcodeReturnScan,
) -> Option<(StateBlock, usize)> {
    let first_block = blocks.first()?;
    let start_label = first_block.label;
    let StateTerminator::ConditionalJump { test, target } = first_block.terminator() else {
        return None;
    };
    if target <= start_label {
        return None;
    }

    let mut cursor = 1usize;
    let mut fallthrough_stmts = Vec::new();
    while let Some(block) = blocks.get(cursor) {
        if block.label >= target {
            break;
        }
        fallthrough_stmts.extend(block.stmts.iter().cloned());
        cursor += 1;
    }
    if fallthrough_stmts.is_empty() {
        return None;
    }

    let join_target = pop_final_jump(&mut fallthrough_stmts)?;
    if join_target <= target {
        return None;
    }

    let target_start = cursor;
    let mut target_stmts = Vec::new();
    while let Some(block) = blocks.get(cursor) {
        if block.label >= join_target {
            break;
        }
        target_stmts.extend(block.stmts.iter().cloned());
        cursor += 1;
    }
    if cursor == target_start {
        return None;
    }
    strip_final_jump_to(&mut target_stmts, join_target);

    if fallthrough_stmts.is_empty() && target_stmts.is_empty() {
        return None;
    }
    if stmts_contain_state_opcode_return(&fallthrough_stmts, opcode_scan)
        || stmts_contain_state_opcode_return(&target_stmts, opcode_scan)
    {
        return None;
    }

    Some((
        StateBlock::new(
            start_label,
            vec![Stmt::If(IfStmt {
                span: DUMMY_SP,
                test: invert_condition(&test),
                cons: Box::new(block_stmt(fallthrough_stmts)),
                alt: Some(Box::new(block_stmt(target_stmts))),
            })],
        ),
        cursor,
    ))
}

fn pop_final_jump(stmts: &mut Vec<Stmt>) -> Option<usize> {
    let target = jump_target_stmt(stmts.last()?)?;
    stmts.pop();
    Some(target)
}

fn strip_final_jump_after(stmts: &mut Vec<Stmt>, target: usize) {
    if stmts
        .last()
        .and_then(jump_target_stmt)
        .is_some_and(|jump_target| jump_target > target)
    {
        stmts.pop();
    }
}

fn strip_final_jump_to(stmts: &mut Vec<Stmt>, target: usize) {
    if stmts
        .last()
        .and_then(jump_target_stmt)
        .is_some_and(|jump_target| jump_target == target)
    {
        stmts.pop();
    }
}

fn block_stmt(stmts: Vec<Stmt>) -> Stmt {
    Stmt::Block(BlockStmt {
        span: DUMMY_SP,
        ctxt: Default::default(),
        stmts,
    })
}

fn recover_index_loops(mut stmts: Vec<Stmt>, continue_mode: IndexLoopContinueMode) -> Vec<Stmt> {
    let mut result = Vec::new();
    let mut index = 0usize;

    while index < stmts.len() {
        if let Some((loop_stmt, consumed)) = try_recover_index_loop(&stmts[index..], continue_mode)
        {
            result.push(loop_stmt);
            index += consumed;
        } else {
            result.push(stmts[index].take());
            index += 1;
        }
    }

    result
}

fn try_recover_index_loop(
    stmts: &[Stmt],
    continue_mode: IndexLoopContinueMode,
) -> Option<(Stmt, usize)> {
    let (test, break_target) = loop_break_test(stmts.first()?)?;
    let final_return_idx = find_loop_boundary(stmts)?;
    if final_return_idx < 3 {
        return None;
    }

    let update_idx = final_return_idx.checked_sub(1)?;
    let update = expr_stmt_expr(&stmts[update_idx])?;
    let mut body_stmts = stmts[1..update_idx].to_vec();
    let continue_target = continue_target_for_loop(
        &body_stmts,
        &stmts[final_return_idx],
        break_target,
        continue_mode,
    )?;
    convert_jump_returns(&mut body_stmts, break_target, continue_target)?;

    let consumed = if return_jump_target(&stmts[final_return_idx]).is_some() {
        final_return_idx + 1
    } else {
        update_idx + 1
    };
    Some((
        Stmt::For(ForStmt {
            span: DUMMY_SP,
            init: None,
            test: Some(test),
            update: Some(update),
            body: Box::new(Stmt::Block(BlockStmt {
                span: DUMMY_SP,
                ctxt: Default::default(),
                stmts: body_stmts,
            })),
        }),
        consumed,
    ))
}

fn continue_target_for_loop(
    body_stmts: &[Stmt],
    final_return: &Stmt,
    break_target: usize,
    continue_mode: IndexLoopContinueMode,
) -> Option<usize> {
    match continue_mode {
        IndexLoopContinueMode::AdjacentBackEdge => {
            let body_has_jump_returns = body_stmts.iter().any(|s| {
                convert_jump_return(&mut s.clone(), break_target, break_target.saturating_sub(1))
                    .is_some_and(|changed| changed)
            });
            if body_has_jump_returns {
                break_target.checked_sub(1).filter(|ct| *ct > 0)
            } else {
                return_jump_target(final_return).filter(|target| *target < break_target)
            }
        }
        IndexLoopContinueMode::SingleBodyJumpTarget => {
            single_continue_target(body_stmts, break_target).or_else(|| {
                return_jump_target(final_return).filter(|target| *target < break_target)
            })
        }
    }
}

fn single_continue_target(stmts: &[Stmt], break_target: usize) -> Option<usize> {
    let mut targets = HashSet::new();
    collect_jump_targets(stmts, &mut targets);
    targets.remove(&break_target);
    if targets.len() == 1 {
        targets.into_iter().next()
    } else {
        None
    }
}

fn collect_jump_targets(stmts: &[Stmt], targets: &mut HashSet<usize>) {
    for stmt in stmts {
        match stmt {
            Stmt::Return(_) => {
                if let Some(target) = return_jump_target(stmt) {
                    targets.insert(target);
                }
            }
            Stmt::If(if_stmt) => {
                collect_jump_target(&if_stmt.cons, targets);
                if let Some(alt) = &if_stmt.alt {
                    collect_jump_target(alt, targets);
                }
            }
            Stmt::Block(block) => collect_jump_targets(&block.stmts, targets),
            Stmt::Try(try_stmt) => {
                collect_jump_targets(&try_stmt.block.stmts, targets);
                if let Some(handler) = &try_stmt.handler {
                    collect_jump_targets(&handler.body.stmts, targets);
                }
                if let Some(finalizer) = &try_stmt.finalizer {
                    collect_jump_targets(&finalizer.stmts, targets);
                }
            }
            _ => {}
        }
    }
}

fn collect_jump_target(stmt: &Stmt, targets: &mut HashSet<usize>) {
    collect_jump_targets(std::slice::from_ref(stmt), targets);
}

fn loop_break_test(stmt: &Stmt) -> Option<(Box<Expr>, usize)> {
    let Stmt::If(if_stmt) = stmt else {
        return None;
    };
    if if_stmt.alt.is_some() {
        return None;
    }
    let target = jump_target_stmt(&if_stmt.cons)?;
    Some((invert_condition(&if_stmt.test), target))
}

fn find_loop_boundary(stmts: &[Stmt]) -> Option<usize> {
    for (i, stmt) in stmts.iter().enumerate() {
        if let Stmt::Return(_) = stmt {
            if return_jump_target(stmt).is_some() {
                return Some(i);
            }
        }
    }
    stmts
        .iter()
        .position(|stmt| return_value_stmt(stmt).is_some())
}

fn expr_stmt_expr(stmt: &Stmt) -> Option<Box<Expr>> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    Some(expr_stmt.expr.clone())
}

fn return_value_stmt(stmt: &Stmt) -> Option<&Stmt> {
    let Stmt::Return(ret) = stmt else {
        return None;
    };
    ret.arg.as_ref()?;
    Some(stmt)
}

fn convert_jump_returns(
    stmts: &mut [Stmt],
    break_target: usize,
    continue_target: usize,
) -> Option<bool> {
    let mut changed = false;
    for stmt in stmts {
        changed |= convert_jump_return(stmt, break_target, continue_target)?;
    }
    Some(changed)
}

fn convert_jump_return(
    stmt: &mut Stmt,
    break_target: usize,
    continue_target: usize,
) -> Option<bool> {
    match stmt {
        Stmt::Return(_) => {
            if let Some(target) = return_jump_target(stmt) {
                if target == break_target {
                    *stmt = Stmt::Break(BreakStmt {
                        span: DUMMY_SP,
                        label: None,
                    });
                } else if target == continue_target {
                    *stmt = Stmt::Continue(ContinueStmt {
                        span: DUMMY_SP,
                        label: None,
                    });
                } else {
                    return None;
                }
                return Some(true);
            }
            Some(false)
        }
        Stmt::If(if_stmt) => {
            let mut changed =
                convert_jump_return(&mut if_stmt.cons, break_target, continue_target)?;
            if let Some(alt) = &mut if_stmt.alt {
                changed |= convert_jump_return(alt, break_target, continue_target)?;
            }
            Some(changed)
        }
        Stmt::Block(block) => convert_jump_returns(&mut block.stmts, break_target, continue_target),
        Stmt::Try(try_stmt) => {
            let mut changed =
                convert_jump_returns(&mut try_stmt.block.stmts, break_target, continue_target)?;
            if let Some(handler) = &mut try_stmt.handler {
                changed |=
                    convert_jump_returns(&mut handler.body.stmts, break_target, continue_target)?;
            }
            if let Some(finalizer) = &mut try_stmt.finalizer {
                changed |= convert_jump_returns(
                    finalizer.stmts.as_mut_slice(),
                    break_target,
                    continue_target,
                )?;
            }
            Some(changed)
        }
        _ => Some(false),
    }
}

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
                let catch_span = catch_stmts.first().map_or(DUMMY_SP, |s| {
                    let sp = s.span();
                    if sp.lo.0 != 0 {
                        sp
                    } else {
                        DUMMY_SP
                    }
                });
                Some(CatchClause {
                    span: catch_span,
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

            let try_span = try_stmts.first().map_or(DUMMY_SP, |s| {
                let sp = s.span();
                if sp.lo.0 != 0 {
                    sp
                } else {
                    DUMMY_SP
                }
            });
            result.push(Stmt::Try(Box::new(TryStmt {
                span: try_span,
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
fn resolve_labeled_forward_jump_blocks(
    mut blocks: Vec<StateBlock>,
    opcode_scan: OpcodeReturnScan,
) -> Vec<StateBlock> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < blocks.len() {
        if let Some((recovered, consumed)) =
            try_resolve_labeled_forward_jump(&blocks[index..], opcode_scan)
        {
            result.push(recovered);
            index += consumed;
        } else {
            let label = blocks[index].label;
            let stmts = std::mem::take(&mut blocks[index].stmts);
            result.push(StateBlock::new(label, stmts));
            index += 1;
        }
    }
    result
}

fn try_resolve_labeled_forward_jump(
    blocks: &[StateBlock],
    opcode_scan: OpcodeReturnScan,
) -> Option<(StateBlock, usize)> {
    let first_block = blocks.first()?;
    let start_label = first_block.label;
    let terminator = first_block.terminator();
    let target = terminator.jump_target()?;
    let StateTerminator::ConditionalJump { test, .. } = terminator else {
        return None;
    };
    if target <= start_label {
        return None;
    }

    let max_remaining_label = blocks[1..]
        .iter()
        .map(|block| block.label)
        .max()
        .unwrap_or(0);
    if target <= max_remaining_label {
        return None;
    }

    let body_stmts: Vec<Stmt> = blocks[1..]
        .iter()
        .flat_map(|block| block.stmts.iter().cloned())
        .collect();
    if body_stmts.is_empty() || stmts_contain_state_opcode_return(&body_stmts, opcode_scan) {
        return None;
    }

    Some((
        StateBlock::new(
            start_label,
            vec![Stmt::If(IfStmt {
                span: DUMMY_SP,
                test: invert_condition(&test),
                cons: Box::new(Stmt::Block(BlockStmt {
                    span: DUMMY_SP,
                    ctxt: Default::default(),
                    stmts: body_stmts,
                })),
                alt: None,
            })],
        ),
        blocks.len(),
    ))
}

pub(crate) fn stmts_contain_state_opcode_return(
    stmts: &[Stmt],
    opcode_scan: OpcodeReturnScan,
) -> bool {
    struct Finder {
        found: bool,
        opcode_scan: OpcodeReturnScan,
    }
    impl Visit for Finder {
        fn visit_function(&mut self, func: &Function) {
            if !self.opcode_scan.skip_nested_functions() {
                func.visit_children_with(self);
            }
        }

        fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
            if !self.opcode_scan.skip_nested_functions() {
                arrow.visit_children_with(self);
            }
        }

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
    let mut finder = Finder {
        found: false,
        opcode_scan,
    };
    for stmt in stmts {
        stmt.visit_with(&mut finder);
        if finder.found {
            return true;
        }
    }
    false
}

pub(crate) fn jump_target_stmt(stmt: &Stmt) -> Option<usize> {
    match stmt {
        Stmt::Return(_) => return_jump_target(stmt),
        Stmt::Block(block) if block.stmts.len() == 1 => return_jump_target(&block.stmts[0]),
        _ => None,
    }
}

pub(crate) fn return_jump_target(stmt: &Stmt) -> Option<usize> {
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

pub(crate) fn invert_condition(test: &Expr) -> Box<Expr> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::atoms::Atom;
    use swc_core::ecma::ast::{ArrayLit, ExprStmt, Number, ReturnStmt};

    #[test]
    fn program_resolves_forward_jump_blocks() {
        let recovered = StateMachineProgram::from_labeled_stmts(
            vec![(0, if_jump("done", 2)), (1, expr_ident_stmt("work"))],
            vec![],
        )
        .resolve_labeled_forward_jumps(OpcodeReturnScan::SkipNestedFunctions)
        .into_reconstructed_stmts();

        assert_eq!(recovered.len(), 1);
        let Stmt::If(if_stmt) = &recovered[0] else {
            panic!("expected recovered if statement");
        };
        assert!(if_stmt.alt.is_none());
        assert!(matches!(if_stmt.test.as_ref(), Expr::Unary(_)));

        let Stmt::Block(block) = if_stmt.cons.as_ref() else {
            panic!("expected recovered if body block");
        };
        assert_eq!(block.stmts.len(), 1);
    }

    #[test]
    fn program_recovers_conditional_assignments() {
        let recovered = StateMachineProgram::from_labeled_stmts(
            vec![
                (0, if_jump("done", 2)),
                (1, ident_assign_stmt("value", "fallback")),
                (2, ident_assign_stmt("value", "target")),
            ],
            vec![],
        )
        .recover_conditional_assignments()
        .into_reconstructed_stmts();

        assert_eq!(recovered.len(), 1);
        let Stmt::Expr(ExprStmt { expr, .. }) = &recovered[0] else {
            panic!("expected assignment statement");
        };
        let Expr::Assign(assign) = expr.as_ref() else {
            panic!("expected assignment expression");
        };
        assert!(matches!(assign.right.as_ref(), Expr::Cond(_)));
    }

    #[test]
    fn program_recovers_conditional_if_else_branches() {
        let recovered = StateMachineProgram::from_labeled_stmts(
            vec![
                (0, if_jump("skip_then", 2)),
                (0, expr_ident_stmt("then_work")),
                (0, jump_return(3)),
                (2, expr_ident_stmt("else_work")),
            ],
            vec![],
        )
        .recover_conditional_branches(OpcodeReturnScan::SkipNestedFunctions)
        .into_reconstructed_stmts();

        assert_eq!(recovered.len(), 1);
        let Stmt::If(if_stmt) = &recovered[0] else {
            panic!("expected recovered if statement");
        };
        assert!(matches!(if_stmt.test.as_ref(), Expr::Unary(_)));
        assert!(if_stmt.alt.is_some());

        let Stmt::Block(cons) = if_stmt.cons.as_ref() else {
            panic!("expected then block");
        };
        assert_eq!(cons.stmts.len(), 1);
        let Some(alt) = &if_stmt.alt else {
            panic!("expected else block");
        };
        let Stmt::Block(alt) = alt.as_ref() else {
            panic!("expected else block");
        };
        assert_eq!(alt.stmts.len(), 1);
    }

    #[test]
    fn program_recovers_adjacent_back_edge_index_loop() {
        let recovered = StateMachineProgram::from_labeled_stmts(
            vec![
                (0, if_jump("done", 4)),
                (1, if_jump("skip", 3)),
                (2, expr_ident_stmt("update")),
                (3, jump_return(0)),
            ],
            vec![],
        )
        .into_reconstructed_stmts_with_index_loops(IndexLoopContinueMode::AdjacentBackEdge);

        assert_loop_body_continue(&recovered);
    }

    #[test]
    fn program_recovers_single_body_jump_target_index_loop() {
        let recovered = StateMachineProgram::from_labeled_stmts(
            vec![
                (0, if_jump("done", 7)),
                (1, if_jump("skip", 3)),
                (2, expr_ident_stmt("update")),
                (3, jump_return(0)),
            ],
            vec![],
        )
        .into_reconstructed_stmts_with_index_loops(IndexLoopContinueMode::SingleBodyJumpTarget);

        assert_loop_body_continue(&recovered);
    }

    fn assert_loop_body_continue(recovered: &[Stmt]) {
        assert_eq!(recovered.len(), 1);
        let Stmt::For(for_stmt) = &recovered[0] else {
            panic!("expected recovered for statement");
        };
        let Stmt::Block(block) = for_stmt.body.as_ref() else {
            panic!("expected recovered for body block");
        };
        let Stmt::If(if_stmt) = &block.stmts[0] else {
            panic!("expected conditional continue guard");
        };
        assert!(matches!(if_stmt.cons.as_ref(), Stmt::Continue(_)));
    }

    fn if_jump(test: &str, target: usize) -> Stmt {
        Stmt::If(IfStmt {
            span: DUMMY_SP,
            test: Box::new(Expr::Ident(Ident::new_no_ctxt(Atom::from(test), DUMMY_SP))),
            cons: Box::new(jump_return(target)),
            alt: None,
        })
    }

    fn ident_assign_stmt(left: &str, right: &str) -> Stmt {
        assign_stmt(ident_target(left), ident_expr(right))
    }

    fn ident_target(name: &str) -> AssignTarget {
        AssignTarget::Simple(SimpleAssignTarget::Ident(
            swc_core::ecma::ast::BindingIdent {
                id: Ident::new_no_ctxt(Atom::from(name), DUMMY_SP),
                type_ann: None,
            },
        ))
    }

    fn ident_expr(name: &str) -> Box<Expr> {
        Box::new(Expr::Ident(Ident::new_no_ctxt(Atom::from(name), DUMMY_SP)))
    }

    fn jump_return(target: usize) -> Stmt {
        Stmt::Return(ReturnStmt {
            span: DUMMY_SP,
            arg: Some(Box::new(Expr::Array(ArrayLit {
                span: DUMMY_SP,
                elems: vec![Some(number_elem(3.0)), Some(number_elem(target as f64))],
            }))),
        })
    }

    fn number_elem(value: f64) -> ExprOrSpread {
        ExprOrSpread {
            spread: None,
            expr: Box::new(Expr::Lit(Lit::Num(Number {
                span: DUMMY_SP,
                value,
                raw: None,
            }))),
        }
    }

    fn expr_ident_stmt(name: &str) -> Stmt {
        Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(Expr::Ident(Ident::new_no_ctxt(Atom::from(name), DUMMY_SP))),
        })
    }
}
