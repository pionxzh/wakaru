use crate::collections::HashSet;
use std::ops::Range;

use swc_core::atoms::Atom;
use swc_core::common::{Spanned, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, AssignTarget, BlockStmt, BreakStmt, CatchClause, CondExpr,
    ContinueStmt, Expr, ExprOrSpread, ExprStmt, ForStmt, Function, Ident, IfStmt, Lit, Pat,
    SimpleAssignTarget, Stmt, SwitchCase, TryStmt, UnaryExpr, UnaryOp,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::decl_utils::fresh_binding_ident;
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
pub(crate) enum ForwardJumpJoin {
    /// Fold only guards that jump past every remaining block. Regenerator
    /// machines encode the post-yield continuation in `_ctx.next` rather than
    /// linear fallthrough, so folding up to a mid-machine join could silently
    /// keep blocks that the original control flow skips.
    EndOfMachine,
    /// Also fold guards that jump to a mid-machine join. Sound for TypeScript
    /// `__generator` machines: yields always resume at the lexically next
    /// label, and every other transfer is an explicit `[3, N]` opcode that the
    /// body scan rejects.
    MidMachine,
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

/// One try-table entry: the `try_start`, `catch_start`, `finally_start`, and
/// `next` labels, each optional.
pub(crate) type TryRegion = [Option<usize>; 4];

/// Label-indexed state-machine output after opcode decoding, before structured
/// control-flow recovery finishes.
#[derive(Clone)]
pub(crate) struct StateMachineProgram {
    blocks: Vec<StateBlock>,
    /// Indexed like the decoder's try table, which `catch_bindings` shares. A
    /// region that branch recovery already rebuilt inside a folded branch is
    /// `None`, so the final reconstruction does not emit it a second time.
    try_regions: Vec<Option<TryRegion>>,
    catch_bindings: CatchBindings,
    /// When set, `if (!test) goto END; body; update; goto HEAD` runs are
    /// rebuilt as `for` loops wherever a label range is flattened: at the top
    /// of the machine, inside a rebuilt try/catch/finally part, and inside a
    /// folded branch. Set before the folds run so their bodies are covered.
    index_loops: Option<IndexLoopContinueMode>,
}

/// The catch-clause binding each try region declares when the machine is
/// rebuilt. Decoders replace the caught-value reads inside a catch label with
/// the same identifier, so the declaration and its references share one
/// context. Regions are indexed like the try-region table.
#[derive(Clone)]
pub(crate) struct CatchBindings {
    name: Atom,
    idents: Vec<Option<Ident>>,
}

impl Default for CatchBindings {
    fn default() -> Self {
        Self {
            name: Atom::from("error"),
            idents: Vec::new(),
        }
    }
}

impl CatchBindings {
    /// Chooses the catch parameter's spelling: `error`, or `error_1`, `error_2`,
    /// ... when the machine already spells that identifier. Printed JavaScript
    /// has no context, so a spelled name could be captured by, or capture, the
    /// binding it belongs to. `folded_aliases` are the lowered catch temps the
    /// decoder folds into the binding (`error_1 = _a.sent()`); their spelling
    /// disappears with them, so it stays available.
    pub(crate) fn for_cases(cases: &[SwitchCase], folded_aliases: &HashSet<Atom>) -> Self {
        struct Names(HashSet<Atom>);
        impl Visit for Names {
            fn visit_ident(&mut self, ident: &Ident) {
                self.0.insert(ident.sym.clone());
            }
        }
        let mut names = Names(HashSet::default());
        for case in cases {
            case.visit_with(&mut names);
        }
        let mut name = Atom::from("error");
        let mut suffix = 1usize;
        while names.0.contains(&name) && !folded_aliases.contains(&name) {
            name = Atom::from(format!("error_{suffix}"));
            suffix += 1;
        }
        Self {
            name,
            idents: Vec::new(),
        }
    }

    /// The binding for `label_idx` when that label starts a catch region.
    pub(crate) fn for_label(
        &mut self,
        label_idx: usize,
        trys: &[[Option<usize>; 4]],
    ) -> Option<Ident> {
        let region = trys
            .iter()
            .position(|region| region[1] == Some(label_idx))?;
        Some(self.for_region(region))
    }

    fn for_region(&mut self, region: usize) -> Ident {
        if self.idents.len() <= region {
            self.idents.resize(region + 1, None);
        }
        self.idents[region]
            .get_or_insert_with(|| fresh_binding_ident(self.name.clone(), DUMMY_SP))
            .clone()
    }
}

impl StateMachineProgram {
    pub(crate) fn from_labeled_stmts(
        stmts: Vec<(usize, Stmt)>,
        try_regions: Vec<TryRegion>,
    ) -> Self {
        Self {
            blocks: stmts
                .into_iter()
                .map(|(label, stmt)| StateBlock::new(label, vec![stmt]))
                .collect(),
            try_regions: try_regions.into_iter().map(Some).collect(),
            catch_bindings: CatchBindings::default(),
            index_loops: None,
        }
    }

    pub(crate) fn with_catch_bindings(mut self, catch_bindings: CatchBindings) -> Self {
        self.catch_bindings = catch_bindings;
        self
    }

    pub(crate) fn with_index_loops(mut self, continue_mode: IndexLoopContinueMode) -> Self {
        self.index_loops = Some(continue_mode);
        self
    }

    pub(crate) fn resolve_labeled_forward_jumps(
        mut self,
        opcode_scan: OpcodeReturnScan,
        join_mode: ForwardJumpJoin,
    ) -> Self {
        self.blocks = resolve_labeled_forward_jump_blocks(
            std::mem::take(&mut self.blocks),
            &mut self.try_regions,
            &mut self.catch_bindings,
            opcode_scan,
            join_mode,
            self.index_loops,
        );
        self
    }

    pub(crate) fn recover_conditional_assignments(mut self) -> Self {
        let regions = active_regions(&self.try_regions);
        self.blocks =
            recover_conditional_assignment_blocks(std::mem::take(&mut self.blocks), &regions);
        self
    }

    pub(crate) fn recover_conditional_branches(mut self, opcode_scan: OpcodeReturnScan) -> Self {
        self.blocks = recover_conditional_branch_blocks(
            std::mem::take(&mut self.blocks),
            &mut self.try_regions,
            &mut self.catch_bindings,
            opcode_scan,
            self.index_loops,
        );
        self
    }

    pub(crate) fn into_reconstructed_stmts(self) -> Vec<Stmt> {
        let regions = active_regions(&self.try_regions);
        let Self {
            blocks,
            mut catch_bindings,
            index_loops,
            ..
        } = self;
        let label_stmts = label_stmts_from_blocks(blocks);
        let end = label_stmts.len();
        reconstruct_label_range(
            &label_stmts,
            0..end,
            &regions,
            &mut catch_bindings,
            index_loops,
            0,
        )
    }

    pub(crate) fn into_reconstructed_stmts_with_index_loops(
        self,
        continue_mode: IndexLoopContinueMode,
    ) -> Vec<Stmt> {
        self.with_index_loops(continue_mode)
            .into_reconstructed_stmts()
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

/// The regions still awaiting reconstruction, with their try-table index.
fn active_regions(try_regions: &[Option<TryRegion>]) -> Vec<(usize, TryRegion)> {
    try_regions
        .iter()
        .enumerate()
        .filter_map(|(index, region)| Some((index, (*region)?)))
        .collect()
}

/// Splits the active try regions touched by a fold over labels
/// `start_label..join` into the regions to rebuild inside its first branch
/// (`start_label..split`) and inside its second branch (`split..join`). Regions
/// wholly outside the fold, or enclosing all of it, need nothing and are not
/// returned. `None` when a region straddles the guard, the split, or the join:
/// rebuilding it on either side would move statements across its
/// try/catch/finally edges.
fn place_regions_in_branches(
    try_regions: &[Option<TryRegion>],
    start_label: usize,
    split: usize,
    join: usize,
) -> Option<(Vec<(usize, TryRegion)>, Vec<(usize, TryRegion)>)> {
    let mut first = Vec::new();
    let mut second = Vec::new();
    for (index, region) in active_regions(try_regions) {
        let touches_fold = region
            .iter()
            .flatten()
            .any(|&boundary| start_label < boundary && boundary < join);
        if !touches_fold {
            continue;
        }
        let start = region[0]?;
        let end = region[3].or(region[2]).or(region[1])?;
        if start > start_label && end <= split {
            first.push((index, region));
        } else if start >= split && end <= join {
            second.push((index, region));
        } else {
            return None;
        }
    }
    Some((first, second))
}

/// Flattens the blocks of one folded branch, rebuilding the try regions placed
/// inside it. `None` when a block lies outside `range`, which the label-ordered
/// block walk never produces for well-formed machines.
fn reconstruct_branch_blocks(
    blocks: Vec<StateBlock>,
    range: Range<usize>,
    regions: &[(usize, TryRegion)],
    catch_bindings: &mut CatchBindings,
    index_loops: Option<IndexLoopContinueMode>,
) -> Option<Vec<Stmt>> {
    if blocks.iter().any(|block| !range.contains(&block.label)) {
        return None;
    }
    if regions.is_empty() && index_loops.is_none() {
        return Some(blocks.into_iter().flat_map(|block| block.stmts).collect());
    }
    let mut label_stmts: Vec<Vec<Stmt>> = vec![vec![]; range.end];
    for block in blocks {
        label_stmts[block.label].extend(block.stmts);
    }
    // The fold's guard sits at `range.start`; a loop whose back-edge returns
    // to that label is the loop this guard tests, not a loop inside the
    // branch, so only loops headed strictly inside the branch are rebuilt.
    let min_loop_head = range.start + 1;
    Some(reconstruct_label_range(
        &label_stmts,
        range,
        regions,
        catch_bindings,
        index_loops,
        min_loop_head,
    ))
}

fn mark_regions_folded(try_regions: &mut [Option<TryRegion>], folded: &[(usize, TryRegion)]) {
    for (index, _) in folded {
        try_regions[*index] = None;
    }
}

fn recover_conditional_assignment_blocks(
    blocks: Vec<StateBlock>,
    regions: &[(usize, TryRegion)],
) -> Vec<StateBlock> {
    let mut result = Vec::new();
    let mut index = 0usize;

    while index < blocks.len() {
        if let Some((block, consumed)) =
            try_recover_conditional_assignment(&blocks[index..], regions)
        {
            result.push(block);
            index += consumed;
        } else {
            result.push(blocks[index].clone());
            index += 1;
        }
    }

    result
}

fn try_recover_conditional_assignment(
    blocks: &[StateBlock],
    regions: &[(usize, TryRegion)],
) -> Option<(StateBlock, usize)> {
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
    // The fold merges every consumed block into one assignment, so a try
    // boundary at any consumed label past the guard would vanish with it.
    let last_label = blocks[cursor - 1].label;
    let crosses_try_boundary = regions.iter().any(|(_, region)| {
        region
            .iter()
            .flatten()
            .any(|&boundary| start_label < boundary && boundary <= last_label)
    });
    if crosses_try_boundary {
        return None;
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
    try_regions: &mut [Option<TryRegion>],
    catch_bindings: &mut CatchBindings,
    opcode_scan: OpcodeReturnScan,
    index_loops: Option<IndexLoopContinueMode>,
) -> Vec<StateBlock> {
    let mut result = Vec::new();
    let mut index = 0usize;

    while index < blocks.len() {
        if let Some((block, consumed)) = try_recover_conditional_branch(
            &blocks[index..],
            try_regions,
            catch_bindings,
            opcode_scan,
            index_loops,
        ) {
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

/// Recover `if (cond) goto T; <fallthrough>; goto J; T: <target>; J:` as
/// `if (!cond) { fallthrough } else { target }`. A try region that lies
/// entirely inside one branch is rebuilt inside that branch. A region that
/// straddles a branch boundary makes the fold bail out, so the guard opcode
/// survives and fails the decode closed instead of dropping the region.
fn try_recover_conditional_branch(
    blocks: &[StateBlock],
    try_regions: &mut [Option<TryRegion>],
    catch_bindings: &mut CatchBindings,
    opcode_scan: OpcodeReturnScan,
    index_loops: Option<IndexLoopContinueMode>,
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
    let mut fallthrough_blocks = Vec::new();
    while let Some(block) = blocks.get(cursor) {
        if block.label >= target {
            break;
        }
        fallthrough_blocks.push(block.clone());
        cursor += 1;
    }

    let join_target = pop_final_block_jump(&mut fallthrough_blocks)?;
    if join_target <= target {
        return None;
    }

    let target_start = cursor;
    let mut target_blocks = Vec::new();
    while let Some(block) = blocks.get(cursor) {
        if block.label >= join_target {
            break;
        }
        target_blocks.push(block.clone());
        cursor += 1;
    }
    if cursor == target_start {
        return None;
    }
    strip_final_block_jump_to(&mut target_blocks, join_target);

    let (fallthrough_regions, target_regions) =
        place_regions_in_branches(try_regions, start_label, target, join_target)?;
    let fallthrough_stmts = reconstruct_branch_blocks(
        fallthrough_blocks,
        start_label..target,
        &fallthrough_regions,
        catch_bindings,
        index_loops,
    )?;
    let target_stmts = reconstruct_branch_blocks(
        target_blocks,
        target..join_target,
        &target_regions,
        catch_bindings,
        index_loops,
    )?;

    if fallthrough_stmts.is_empty() && target_stmts.is_empty() {
        return None;
    }
    if stmts_contain_state_opcode_return(&fallthrough_stmts, opcode_scan)
        || stmts_contain_state_opcode_return(&target_stmts, opcode_scan)
    {
        return None;
    }
    mark_regions_folded(try_regions, &fallthrough_regions);
    mark_regions_folded(try_regions, &target_regions);

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

/// Pops the jump that ends the last block, returning its target. Empty
/// trailing blocks are skipped so a block whose only statement was a folded
/// alias does not hide the jump.
fn pop_final_block_jump(blocks: &mut [StateBlock]) -> Option<usize> {
    let last = blocks
        .iter_mut()
        .rev()
        .find(|block| !block.stmts.is_empty())?;
    let target = jump_target_stmt(last.stmts.last()?)?;
    last.stmts.pop();
    Some(target)
}

fn strip_final_block_jump_to(blocks: &mut [StateBlock], target: usize) {
    if let Some(last) = blocks
        .iter_mut()
        .rev()
        .find(|block| !block.stmts.is_empty())
    {
        strip_final_jump_to(&mut last.stmts, target);
    }
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

/// Rebuild `if (!test) goto END; body; update; goto HEAD` runs as `for`
/// loops over one flattened label range. Statements that precede the break
/// test but sit at or after the back-edge target belong to the loop head (a
/// `yield` merged into its `sent()` consumer lands there when the consumer is
/// a separate statement): they move into the loop body ahead of the break
/// test instead of staying outside the loop, where they would run once.
fn recover_index_loops_labeled(
    stmts: Vec<(usize, Stmt)>,
    continue_mode: IndexLoopContinueMode,
    min_loop_head: usize,
) -> Vec<(usize, Stmt)> {
    let plain: Vec<Stmt> = stmts.iter().map(|(_, stmt)| stmt.clone()).collect();
    let mut result: Vec<(usize, Stmt)> = Vec::new();
    let mut index = 0usize;

    while index < stmts.len() {
        let recovered = try_recover_index_loop(&plain[index..], continue_mode)
            .filter(|recovered| recovered.head.is_none_or(|head| head >= min_loop_head));
        let Some(recovered) = recovered else {
            if let Some(loop_stmt) =
                try_recover_unconditional_loop(&mut result, &stmts[index], min_loop_head)
            {
                result.push(loop_stmt);
            } else {
                result.push(stmts[index].clone());
            }
            index += 1;
            continue;
        };
        let label = stmts[index].0;
        let mut head_pre = Vec::new();
        if let Some(head) = recovered.head {
            while result.last().is_some_and(|(label, _)| *label >= head) {
                head_pre.push(result.pop().expect("checked by last()"));
            }
        }
        head_pre.reverse();
        let loop_label = head_pre.first().map_or(label, |(label, _)| *label);
        let consumed = recovered.consumed;
        result.push((
            loop_label,
            recovered.into_stmt(head_pre.into_iter().map(|(_, stmt)| stmt)),
        ));
        index += consumed;
    }

    result
}

/// A bare back-edge `goto HEAD` with no exit guard ahead of it is a
/// `for (;;)` loop: the statements at or after `HEAD` that were already
/// emitted are its body. A jump inside the body to the label right after the
/// back-edge is a `break`; a jump back to `HEAD` is a `continue`; any other
/// jump keeps the loop unrecovered.
fn try_recover_unconditional_loop(
    result: &mut Vec<(usize, Stmt)>,
    back_edge: &(usize, Stmt),
    min_loop_head: usize,
) -> Option<(usize, Stmt)> {
    let (label, stmt) = back_edge;
    let head = return_jump_target(stmt)?;
    if head > *label || head < min_loop_head {
        return None;
    }
    let body_start = result
        .iter()
        .rposition(|(stmt_label, _)| *stmt_label < head)
        .map_or(0, |position| position + 1);
    if body_start == result.len() {
        return None;
    }
    let mut body: Vec<Stmt> = result[body_start..]
        .iter()
        .map(|(_, stmt)| stmt.clone())
        .collect();
    convert_jump_returns(&mut body, label + 1, head)?;
    let loop_label = result[body_start].0;
    result.truncate(body_start);
    Some((
        loop_label,
        Stmt::For(ForStmt {
            span: DUMMY_SP,
            init: None,
            test: None,
            update: None,
            body: Box::new(Stmt::Block(BlockStmt {
                span: DUMMY_SP,
                ctxt: Default::default(),
                stmts: body,
            })),
        }),
    ))
}

struct RecoveredIndexLoop {
    /// The loop test, already inverted from the break guard.
    test: Box<Expr>,
    /// The break guard as it stood in the machine, converted to `if (c) break;`.
    break_guard: Stmt,
    update: Box<Expr>,
    body: Vec<Stmt>,
    consumed: usize,
    /// The back-edge target label, when the loop ends in a jump.
    head: Option<usize>,
}

impl RecoveredIndexLoop {
    fn into_stmt(self, head_pre: impl Iterator<Item = Stmt>) -> Stmt {
        let mut head_pre: Vec<Stmt> = head_pre.collect();
        let (test, body) = if head_pre.is_empty() {
            (Some(self.test), self.body)
        } else {
            head_pre.push(self.break_guard);
            head_pre.extend(self.body);
            (None, head_pre)
        };
        Stmt::For(ForStmt {
            span: DUMMY_SP,
            init: None,
            test,
            update: Some(self.update),
            body: Box::new(Stmt::Block(BlockStmt {
                span: DUMMY_SP,
                ctxt: Default::default(),
                stmts: body,
            })),
        })
    }
}

fn try_recover_index_loop(
    stmts: &[Stmt],
    continue_mode: IndexLoopContinueMode,
) -> Option<RecoveredIndexLoop> {
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
    let mut break_guard = stmts[0].clone();
    convert_jump_return(&mut break_guard, break_target, continue_target)?;

    let head = return_jump_target(&stmts[final_return_idx]);
    let consumed = if head.is_some() {
        final_return_idx + 1
    } else {
        update_idx + 1
    };
    Some(RecoveredIndexLoop {
        test,
        break_guard,
        update,
        body: body_stmts,
        consumed,
        head,
    })
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
    let mut targets = HashSet::default();
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

/// Flattens the labels in `range`, rebuilding each try region that starts
/// there as a `try` statement. `regions` carries the try-table index of every
/// region to rebuild; the index selects the catch binding the decoder already
/// substituted into the catch body. Labels covered by a region are consumed by
/// it and emitted nowhere else.
fn reconstruct_label_range(
    label_stmts: &[Vec<Stmt>],
    range: Range<usize>,
    regions: &[(usize, TryRegion)],
    catch_bindings: &mut CatchBindings,
    index_loops: Option<IndexLoopContinueMode>,
    min_loop_head: usize,
) -> Vec<Stmt> {
    let labeled =
        reconstruct_label_range_labeled(label_stmts, range, regions, catch_bindings, index_loops);
    let labeled = match index_loops {
        Some(continue_mode) => recover_index_loops_labeled(labeled, continue_mode, min_loop_head),
        None => labeled,
    };
    labeled.into_iter().map(|(_, stmt)| stmt).collect()
}

/// [`reconstruct_label_range`] before loop recovery, keeping each statement's
/// label so loop recovery can tell head statements from the code before the
/// loop. A rebuilt try statement carries its region's start label.
fn reconstruct_label_range_labeled(
    label_stmts: &[Vec<Stmt>],
    range: Range<usize>,
    regions: &[(usize, TryRegion)],
    catch_bindings: &mut CatchBindings,
    index_loops: Option<IndexLoopContinueMode>,
) -> Vec<(usize, Stmt)> {
    let n = range.end.min(label_stmts.len());
    let start = range.start.min(n);
    if regions.is_empty() {
        return label_stmts[start..n]
            .iter()
            .enumerate()
            .flat_map(|(offset, stmts)| {
                stmts
                    .iter()
                    .cloned()
                    .map(move |stmt| (start + offset, stmt))
            })
            .collect();
    }

    let mut result: Vec<(usize, Stmt)> = Vec::new();
    let mut i = range.start;

    while i < n {
        let region = regions.iter().find(|(_, region)| region[0] == Some(i));
        if let Some(&(region_index, region)) = region {
            let [_try_start, catch_start, finally_start, next] = region;

            // A region nested inside this one is rebuilt inside the part that
            // holds it; flattening it would drop its `finally` guarantee.
            let inner_regions = |part: Range<usize>| -> Vec<(usize, TryRegion)> {
                regions
                    .iter()
                    .filter(|(index, inner)| {
                        *index != region_index
                            && inner[0].is_some_and(|start| part.contains(&start))
                    })
                    .copied()
                    .collect()
            };

            let try_end = catch_start.or(finally_start).unwrap_or(n).min(n);
            let try_stmts = reconstruct_label_range(
                label_stmts,
                i..try_end,
                &inner_regions(i..try_end),
                catch_bindings,
                index_loops,
                i,
            );

            let catch_clause = if let Some(cs) = catch_start {
                let catch_end = finally_start.or(next).unwrap_or(n).min(n);
                let cs = cs.min(n);
                let catch_stmts = reconstruct_label_range(
                    label_stmts,
                    cs..catch_end,
                    &inner_regions(cs..catch_end),
                    catch_bindings,
                    index_loops,
                    cs,
                );
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
                        id: catch_bindings.for_region(region_index),
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
                let finally_end = next.unwrap_or(n).min(n);
                let fs = fs.min(n);
                let finally_stmts = reconstruct_label_range(
                    label_stmts,
                    fs..finally_end,
                    &inner_regions(fs..finally_end),
                    catch_bindings,
                    index_loops,
                    fs,
                );
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
            result.push((
                i,
                Stmt::Try(Box::new(TryStmt {
                    span: try_span,
                    block: BlockStmt {
                        span: DUMMY_SP,
                        ctxt: Default::default(),
                        stmts: try_stmts,
                    },
                    handler: catch_clause,
                    finalizer: finally_block,
                })),
            ));

            i = next.unwrap_or(n);
        } else {
            let in_region = regions.iter().any(|(_, r)| {
                let start = r[0].unwrap_or(usize::MAX);
                let end = r[3].or(r[2]).or(r[1]).unwrap_or(0);
                i > start && i < end
            });
            if !in_region {
                result.extend(label_stmts[i].iter().cloned().map(|stmt| (i, stmt)));
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
    try_regions: &mut [Option<TryRegion>],
    catch_bindings: &mut CatchBindings,
    opcode_scan: OpcodeReturnScan,
    join_mode: ForwardJumpJoin,
    index_loops: Option<IndexLoopContinueMode>,
) -> Vec<StateBlock> {
    // Folding an inner guard can make an enclosing guard's body jump-free, so
    // iterate to a fixpoint. Each fold strictly reduces the block count, which
    // bounds the number of passes.
    loop {
        let before = blocks.len();
        blocks = resolve_labeled_forward_jump_blocks_once(
            blocks,
            try_regions,
            catch_bindings,
            opcode_scan,
            join_mode,
            index_loops,
        );
        if blocks.len() == before {
            return blocks;
        }
    }
}

fn resolve_labeled_forward_jump_blocks_once(
    mut blocks: Vec<StateBlock>,
    try_regions: &mut [Option<TryRegion>],
    catch_bindings: &mut CatchBindings,
    opcode_scan: OpcodeReturnScan,
    join_mode: ForwardJumpJoin,
    index_loops: Option<IndexLoopContinueMode>,
) -> Vec<StateBlock> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < blocks.len() {
        if let Some((recovered, consumed)) = try_resolve_labeled_forward_jump(
            &blocks[index..],
            try_regions,
            catch_bindings,
            opcode_scan,
            join_mode,
            index_loops,
        ) {
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

/// Recover `if (cond) goto T; <body>` as `if (!cond) { body }` where the body
/// is every following block below label T. T may be a mid-machine join with
/// its own statements; those stay in place as the continuation. A try region
/// that lies entirely inside the body is rebuilt there; one that straddles the
/// guard or the join makes the fold bail out. Any jump left inside the body
/// (including into it from elsewhere, which leaves that jump opcode
/// unresolved) fails the decode closed via the caller's final opcode scan
/// instead of producing wrong control flow.
fn try_resolve_labeled_forward_jump(
    blocks: &[StateBlock],
    try_regions: &mut [Option<TryRegion>],
    catch_bindings: &mut CatchBindings,
    opcode_scan: OpcodeReturnScan,
    join_mode: ForwardJumpJoin,
    index_loops: Option<IndexLoopContinueMode>,
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
    if matches!(join_mode, ForwardJumpJoin::EndOfMachine) {
        let max_remaining_label = blocks[1..]
            .iter()
            .map(|block| block.label)
            .max()
            .unwrap_or(0);
        if target <= max_remaining_label {
            return None;
        }
    }

    // A guard that jumps to a block which immediately loops back skips the
    // rest of a loop body: that is a `continue`, which index-loop recovery
    // restores from the jump. Wrapping the body in an `if` instead would be
    // equivalent but hide the original control flow.
    let target_loops_back = blocks
        .iter()
        .rev()
        .find(|block| block.label == target)
        .is_some_and(|block| {
            matches!(block.terminator(), StateTerminator::Jump { target: back } if back < target)
        });
    if target_loops_back {
        return None;
    }

    let mut cursor = 1usize;
    let mut body_blocks = Vec::new();
    while let Some(block) = blocks.get(cursor) {
        if block.label >= target {
            break;
        }
        body_blocks.push(block.clone());
        cursor += 1;
    }
    // Folded body statements move to `start_label`. A try region that only
    // partly overlaps the body would have statements pulled across its
    // try/catch/finally edges; one contained in the body is rebuilt inside it.
    let (body_regions, _) = place_regions_in_branches(try_regions, start_label, target, target)?;
    let body_stmts = reconstruct_branch_blocks(
        body_blocks,
        start_label..target,
        &body_regions,
        catch_bindings,
        index_loops,
    )?;
    if body_stmts.is_empty() || stmts_contain_state_opcode_return(&body_stmts, opcode_scan) {
        return None;
    }
    mark_regions_folded(try_regions, &body_regions);

    let mut stmts = first_block.stmts.clone();
    stmts.pop();
    stmts.push(Stmt::If(IfStmt {
        span: DUMMY_SP,
        test: invert_condition(&test),
        cons: Box::new(Stmt::Block(BlockStmt {
            span: DUMMY_SP,
            ctxt: Default::default(),
            stmts: body_stmts,
        })),
        alt: None,
    }));

    Some((StateBlock::new(start_label, stmts), cursor))
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
            if ret.arg.as_deref().is_some_and(returns_opcode_array) {
                self.found = true;
                return;
            }
            ret.visit_children_with(self);
        }
    }

    /// Whether a return value is an opcode array once the wrappers a minifier
    /// leaves around it are peeled: parentheses, a trailing sequence element, or
    /// either branch of a conditional.
    fn returns_opcode_array(expr: &Expr) -> bool {
        match expr {
            Expr::Paren(paren) => returns_opcode_array(&paren.expr),
            Expr::Seq(seq) => seq
                .exprs
                .last()
                .is_some_and(|last| returns_opcode_array(last)),
            Expr::Cond(cond) => returns_opcode_array(&cond.cons) || returns_opcode_array(&cond.alt),
            Expr::Array(arr) => arr
                .elems
                .first()
                .and_then(|e| e.as_ref())
                .is_some_and(|e| matches!(e.expr.as_ref(), Expr::Lit(Lit::Num(_)))),
            _ => false,
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
    use swc_core::ecma::ast::{ArrayLit, ExprStmt, Number, ReturnStmt};

    #[test]
    fn program_resolves_forward_jump_blocks() {
        let recovered = StateMachineProgram::from_labeled_stmts(
            vec![(0, if_jump("done", 2)), (1, expr_ident_stmt("work"))],
            vec![],
        )
        .resolve_labeled_forward_jumps(
            OpcodeReturnScan::SkipNestedFunctions,
            ForwardJumpJoin::EndOfMachine,
        )
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
    fn program_rebuilds_try_region_inside_recovered_else_branch() {
        // if (take_try) goto 1; else_work; goto 4;
        // 1: try_work  3: handle  4:          with region [1, 3, , 4]
        let recovered = with_globals(|| {
            StateMachineProgram::from_labeled_stmts(
                vec![
                    (0, if_jump("take_try", 1)),
                    (0, expr_ident_stmt("else_work")),
                    (0, jump_return(4)),
                    (1, expr_ident_stmt("try_work")),
                    (3, expr_ident_stmt("handle")),
                ],
                vec![[Some(1), Some(3), None, Some(4)]],
            )
            .recover_conditional_branches(OpcodeReturnScan::SkipNestedFunctions)
            .into_reconstructed_stmts()
        });

        assert_eq!(recovered.len(), 1, "{recovered:#?}");
        let Stmt::If(if_stmt) = &recovered[0] else {
            panic!("expected recovered if statement");
        };
        let Some(alt) = &if_stmt.alt else {
            panic!("expected else block");
        };
        let Stmt::Block(alt) = alt.as_ref() else {
            panic!("expected else block");
        };
        assert_eq!(alt.stmts.len(), 1);
        let Stmt::Try(try_stmt) = &alt.stmts[0] else {
            panic!("expected try inside the else branch, got {:#?}", alt.stmts);
        };
        assert_eq!(try_stmt.block.stmts.len(), 1);
        assert!(try_stmt.handler.is_some());
        assert!(!stmts_contain_state_opcode_return(
            &recovered,
            OpcodeReturnScan::SkipNestedFunctions
        ));
    }

    #[test]
    fn program_rebuilds_try_region_inside_forward_jump_body() {
        // if (skip) goto 4;  1: try_work  3: handle  4:   with region [1, 3, , 4]
        let recovered = with_globals(|| {
            StateMachineProgram::from_labeled_stmts(
                vec![
                    (0, if_jump("skip", 4)),
                    (1, expr_ident_stmt("try_work")),
                    (3, expr_ident_stmt("handle")),
                ],
                vec![[Some(1), Some(3), None, Some(4)]],
            )
            .resolve_labeled_forward_jumps(
                OpcodeReturnScan::SkipNestedFunctions,
                ForwardJumpJoin::MidMachine,
            )
            .into_reconstructed_stmts()
        });

        assert_eq!(recovered.len(), 1, "{recovered:#?}");
        let Stmt::If(if_stmt) = &recovered[0] else {
            panic!("expected recovered if statement");
        };
        assert!(if_stmt.alt.is_none());
        let Stmt::Block(cons) = if_stmt.cons.as_ref() else {
            panic!("expected if body block");
        };
        assert_eq!(cons.stmts.len(), 1);
        assert!(
            matches!(cons.stmts[0], Stmt::Try(_)),
            "expected try inside the if body, got {:#?}",
            cons.stmts
        );
    }

    #[test]
    fn program_keeps_guard_when_try_region_straddles_branch_join() {
        // The region's `next` label (5) lies past the branch join (4), so the
        // target branch would cut the region in half. The fold must bail out
        // and leave the guard opcode for the caller's fail-closed scan.
        let recovered = with_globals(|| {
            StateMachineProgram::from_labeled_stmts(
                vec![
                    (0, if_jump("take_try", 1)),
                    (0, expr_ident_stmt("else_work")),
                    (0, jump_return(4)),
                    (1, expr_ident_stmt("try_work")),
                    (3, expr_ident_stmt("handle")),
                    (4, expr_ident_stmt("after")),
                ],
                vec![[Some(1), Some(3), None, Some(5)]],
            )
            .recover_conditional_branches(OpcodeReturnScan::SkipNestedFunctions)
            .into_reconstructed_stmts()
        });

        assert!(stmts_contain_state_opcode_return(
            &recovered,
            OpcodeReturnScan::SkipNestedFunctions
        ));
    }

    #[test]
    fn program_keeps_conditional_assignment_apart_from_try_region() {
        // The target assignment starts a try region; folding it into a
        // conditional expression would drop the region.
        let recovered = with_globals(|| {
            StateMachineProgram::from_labeled_stmts(
                vec![
                    (0, if_jump("done", 2)),
                    (1, ident_assign_stmt("value", "fallback")),
                    (2, ident_assign_stmt("value", "target")),
                ],
                vec![[Some(2), Some(3), None, Some(4)]],
            )
            .recover_conditional_assignments()
            .into_reconstructed_stmts()
        });

        assert!(stmts_contain_state_opcode_return(
            &recovered,
            OpcodeReturnScan::SkipNestedFunctions
        ));
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

    #[test]
    fn program_recovers_index_loop_inside_try_region() {
        // 0: init   1: if (done) goto 3   2: work; update; goto 1
        // 4: cleanup                         region [0, , 4, 6]
        let recovered = with_globals(|| {
            StateMachineProgram::from_labeled_stmts(
                vec![
                    (0, expr_ident_stmt("init")),
                    (1, if_jump("done", 3)),
                    (2, expr_ident_stmt("work")),
                    (2, expr_ident_stmt("update")),
                    (2, jump_return(1)),
                    (4, expr_ident_stmt("cleanup")),
                ],
                vec![[Some(0), None, Some(4), Some(6)]],
            )
            .into_reconstructed_stmts_with_index_loops(IndexLoopContinueMode::AdjacentBackEdge)
        });

        assert_eq!(recovered.len(), 1, "{recovered:#?}");
        let Stmt::Try(try_stmt) = &recovered[0] else {
            panic!("expected try statement, got {recovered:#?}");
        };
        assert_eq!(try_stmt.block.stmts.len(), 2, "{:#?}", try_stmt.block.stmts);
        let Stmt::For(for_stmt) = &try_stmt.block.stmts[1] else {
            panic!("expected loop inside try, got {:#?}", try_stmt.block.stmts);
        };
        let Stmt::Block(body) = for_stmt.body.as_ref() else {
            panic!("expected loop body block");
        };
        assert_eq!(body.stmts.len(), 1);
        assert!(for_stmt.update.is_some());
        assert!(!stmts_contain_state_opcode_return(
            &recovered,
            OpcodeReturnScan::SkipNestedFunctions
        ));
    }

    #[test]
    fn program_keeps_loop_head_statements_inside_the_loop() {
        // 0: init   3: fetch   4: if (done) goto 7; work   6: update; goto 3
        // 7: after. `fetch` sits at the back-edge target, so it is the loop
        // head and must run every iteration, ahead of the break guard.
        let recovered = StateMachineProgram::from_labeled_stmts(
            vec![
                (0, expr_ident_stmt("init")),
                (3, expr_ident_stmt("fetch")),
                (4, if_jump("done", 7)),
                (4, expr_ident_stmt("work")),
                (6, expr_ident_stmt("update")),
                (6, jump_return(3)),
                (7, expr_ident_stmt("after")),
            ],
            vec![],
        )
        .into_reconstructed_stmts_with_index_loops(IndexLoopContinueMode::AdjacentBackEdge);

        assert_eq!(recovered.len(), 3, "{recovered:#?}");
        let Stmt::For(for_stmt) = &recovered[1] else {
            panic!("expected loop, got {recovered:#?}");
        };
        assert!(
            for_stmt.test.is_none(),
            "head statements need the guard in the body"
        );
        assert!(for_stmt.update.is_some());
        let Stmt::Block(body) = for_stmt.body.as_ref() else {
            panic!("expected loop body block");
        };
        assert_eq!(body.stmts.len(), 3, "{:#?}", body.stmts);
        assert!(matches!(&body.stmts[0], Stmt::Expr(_)), "fetch first");
        let Stmt::If(guard) = &body.stmts[1] else {
            panic!("expected break guard, got {:#?}", body.stmts);
        };
        assert!(matches!(guard.cons.as_ref(), Stmt::Break(_)));
        assert!(!stmts_contain_state_opcode_return(
            &recovered,
            OpcodeReturnScan::SkipNestedFunctions
        ));
    }

    #[test]
    fn program_does_not_fold_a_loop_test_guard_into_a_branch() {
        // 0: if (done) goto 4   1: work   2: update; goto 0   4: after
        // The guard at 0 is the loop's exit test; folding it as `if (!done)
        // { for (;;) … }` would evaluate the test once.
        let recovered = with_globals(|| {
            StateMachineProgram::from_labeled_stmts(
                vec![
                    (0, if_jump("done", 4)),
                    (1, expr_ident_stmt("work")),
                    (2, expr_ident_stmt("update")),
                    (2, jump_return(0)),
                    (4, expr_ident_stmt("after")),
                ],
                vec![],
            )
            .with_index_loops(IndexLoopContinueMode::AdjacentBackEdge)
            .resolve_labeled_forward_jumps(
                OpcodeReturnScan::SkipNestedFunctions,
                ForwardJumpJoin::MidMachine,
            )
            .into_reconstructed_stmts()
        });

        assert_eq!(recovered.len(), 2, "{recovered:#?}");
        let Stmt::For(for_stmt) = &recovered[0] else {
            panic!("expected the loop at the top, got {recovered:#?}");
        };
        assert!(for_stmt.test.is_some(), "the guard is the loop test");
    }

    #[test]
    fn program_recovers_unconditional_loop_from_bare_back_edge() {
        // 0: init   1: fetch   2: if (done) return value   3: goto 1   4: after
        let recovered = with_globals(|| {
            let mut guard = if_jump("done", 9);
            if let Stmt::If(if_stmt) = &mut guard {
                *if_stmt.cons = Stmt::Return(swc_core::ecma::ast::ReturnStmt {
                    span: DUMMY_SP,
                    arg: Some(ident_expr("value")),
                });
            }
            StateMachineProgram::from_labeled_stmts(
                vec![
                    (0, expr_ident_stmt("init")),
                    (1, expr_ident_stmt("fetch")),
                    (2, guard),
                    (3, jump_return(1)),
                    (4, expr_ident_stmt("after")),
                ],
                vec![],
            )
            .into_reconstructed_stmts_with_index_loops(IndexLoopContinueMode::AdjacentBackEdge)
        });

        assert_eq!(recovered.len(), 3, "{recovered:#?}");
        let Stmt::For(for_stmt) = &recovered[1] else {
            panic!("expected `for (;;)`, got {recovered:#?}");
        };
        assert!(for_stmt.test.is_none() && for_stmt.update.is_none());
        let Stmt::Block(body) = for_stmt.body.as_ref() else {
            panic!("expected loop body block");
        };
        assert_eq!(body.stmts.len(), 2, "{:#?}", body.stmts);
        assert!(!stmts_contain_state_opcode_return(
            &recovered,
            OpcodeReturnScan::SkipNestedFunctions
        ));
    }

    #[test]
    fn program_rebuilds_try_region_nested_in_finally() {
        // try { a } finally { try { b } finally { c } }
        // 0: a   2: b   3: c      regions [0, , 2, 4] and [2, , 3, 4]
        let recovered = with_globals(|| {
            StateMachineProgram::from_labeled_stmts(
                vec![
                    (0, expr_ident_stmt("a")),
                    (2, expr_ident_stmt("b")),
                    (3, expr_ident_stmt("c")),
                ],
                vec![
                    [Some(0), None, Some(2), Some(4)],
                    [Some(2), None, Some(3), Some(4)],
                ],
            )
            .into_reconstructed_stmts()
        });

        assert_eq!(recovered.len(), 1, "{recovered:#?}");
        let Stmt::Try(outer) = &recovered[0] else {
            panic!("expected outer try, got {recovered:#?}");
        };
        assert_eq!(outer.block.stmts.len(), 1);
        let Some(finalizer) = &outer.finalizer else {
            panic!("expected outer finally");
        };
        assert_eq!(finalizer.stmts.len(), 1, "{:#?}", finalizer.stmts);
        let Stmt::Try(inner) = &finalizer.stmts[0] else {
            panic!(
                "expected inner try inside finally, got {:#?}",
                finalizer.stmts
            );
        };
        assert_eq!(inner.block.stmts.len(), 1);
        assert_eq!(inner.finalizer.as_ref().map(|f| f.stmts.len()), Some(1));
    }

    #[test]
    fn program_rebuilds_try_region_nested_in_try_block() {
        // try { try { a } catch { b } } finally { c }
        // 0: a   1: b   2: c      regions [0, , 2, 3] and [0, 1, , 2]
        let recovered = with_globals(|| {
            StateMachineProgram::from_labeled_stmts(
                vec![
                    (0, expr_ident_stmt("a")),
                    (1, expr_ident_stmt("b")),
                    (2, expr_ident_stmt("c")),
                ],
                vec![
                    [Some(0), None, Some(2), Some(3)],
                    [Some(0), Some(1), None, Some(2)],
                ],
            )
            .into_reconstructed_stmts()
        });

        assert_eq!(recovered.len(), 1, "{recovered:#?}");
        let Stmt::Try(outer) = &recovered[0] else {
            panic!("expected outer try, got {recovered:#?}");
        };
        assert!(outer.handler.is_none());
        assert_eq!(outer.finalizer.as_ref().map(|f| f.stmts.len()), Some(1));
        assert_eq!(outer.block.stmts.len(), 1, "{:#?}", outer.block.stmts);
        let Stmt::Try(inner) = &outer.block.stmts[0] else {
            panic!(
                "expected inner try inside the try block, got {:#?}",
                outer.block.stmts
            );
        };
        assert!(inner.handler.is_some());
        assert!(inner.finalizer.is_none());
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

    /// Catch clauses mint a fresh binding context, which needs SWC's globals.
    fn with_globals<T>(f: impl FnOnce() -> T) -> T {
        swc_core::common::GLOBALS.set(&Default::default(), f)
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
