use swc_core::atoms::Atom;
use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignOp, AwaitExpr, BlockStmt, CatchClause, Decl, Expr, ExprStmt, Function, Ident, Lit,
    MemberExpr, MemberProp, Module, ModuleItem, Pat, ReturnStmt, Stmt, SwitchCase, WhileStmt,
    YieldExpr,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith, VisitWith};

use super::babel_helper_utils::{
    collect_helpers, helpers_with_remaining_refs, BabelHelperKind, BindingKey,
};

pub struct UnRegenerator {
    unresolved_mark: Mark,
}

impl UnRegenerator {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }
}

impl VisitMut for UnRegenerator {
    fn visit_mut_module(&mut self, module: &mut Module) {
        // Phase 1: Detect _asyncToGenerator helper bindings (scope-aware)
        let helpers = collect_helpers(module);
        let async_to_gen_bindings: Vec<BindingKey> = helpers
            .iter()
            .filter(|(_, kind)| **kind == BabelHelperKind::AsyncToGenerator)
            .map(|((sym, ctxt), _)| (sym.clone(), *ctxt))
            .collect();

        // Phase 2: Transform functions containing regeneratorRuntime.wrap()
        // and _asyncToGenerator() calls. Track consumed mark bindings.
        let mut consumed_marks: Vec<BindingKey> = Vec::new();
        let mut transformer = FunctionTransformer {
            unresolved_mark: self.unresolved_mark,
            async_to_gen_bindings: &async_to_gen_bindings,
            consumed_marks: &mut consumed_marks,
        };
        module.visit_mut_with(&mut transformer);

        // Phase 3: Remove only the mark declarations that were consumed
        remove_consumed_mark_declarations(module, &consumed_marks);

        // Phase 4: Remove _asyncToGenerator helper if no longer referenced
        if !async_to_gen_bindings.is_empty() {
            let remaining = helpers_with_remaining_refs(module, &helpers);
            let to_remove: Vec<_> = helpers
                .iter()
                .filter(|(key, kind)| {
                    **kind == BabelHelperKind::AsyncToGenerator && !remaining.contains(key)
                })
                .map(|((sym, ctxt), _)| (sym.clone(), *ctxt))
                .collect();
            remove_helper_decls(module, &to_remove);
        }
    }
}

struct FunctionTransformer<'a> {
    unresolved_mark: Mark,
    async_to_gen_bindings: &'a [BindingKey],
    consumed_marks: &'a mut Vec<BindingKey>,
}

impl VisitMut for FunctionTransformer<'_> {
    fn visit_mut_function(&mut self, func: &mut Function) {
        // Try _asyncToGenerator BEFORE recursing — the inner function hasn't
        // been transformed yet, so we can still detect the full pattern.
        if let Some(body) = func.body.as_mut() {
            if try_transform_async_to_generator(
                body,
                self.async_to_gen_bindings,
                self.unresolved_mark,
            ) {
                func.is_async = true;
                // Still recurse into the (now-rewritten) body for nested cases
                func.visit_mut_children_with(self);
                return;
            }
        }

        func.visit_mut_children_with(self);

        let body = match func.body.as_mut() {
            Some(b) => b,
            None => return,
        };

        // Try regeneratorRuntime.wrap() transform
        if let Some(mark_key) = try_transform_regenerator_wrap(body) {
            func.is_generator = true;
            if let Some(key) = mark_key {
                self.consumed_marks.push(key);
            }
        }
    }
}

// ============================================================
// regeneratorRuntime.wrap() → function*
// ============================================================

/// Returns the consumed mark binding key (sym + ctxt) on success.
fn try_transform_regenerator_wrap(body: &mut BlockStmt) -> Option<Option<BindingKey>> {
    let return_idx = body.stmts.iter().position(is_regenerator_wrap_return)?;

    // P1-1: Pre-check for nested control flow before extracting.
    if has_nested_control_flow_in_stmt(&body.stmts[return_idx]) {
        return None;
    }

    // Extract the mark binding key (2nd arg to .wrap()) before consuming
    let mark_name = extract_wrap_mark_key(&body.stmts[return_idx]);

    let ret_stmt = body.stmts.remove(return_idx);
    let (state_name, cases) = extract_wrap_args(ret_stmt)?;

    let new_stmts = decode_babel_state_machine(&state_name, cases);
    body.stmts.splice(return_idx..return_idx, new_stmts);
    Some(mark_name)
}

/// Extract the mark binding key (sym + ctxt) from the 2nd argument of .wrap(fn, markIdent, ...)
fn extract_wrap_mark_key(stmt: &Stmt) -> Option<BindingKey> {
    let Stmt::Return(ret) = stmt else { return None };
    let arg = ret.arg.as_ref()?;
    let Expr::Call(call) = arg.as_ref() else {
        return None;
    };
    if call.args.len() < 2 {
        return None;
    }
    let Expr::Ident(id) = call.args[1].expr.as_ref() else {
        return None;
    };
    Some((id.sym.clone(), id.ctxt))
}

/// Check if the regenerator.wrap() state machine contains nested control flow
/// (if/else blocks with state transitions) that we can't safely linearize.
fn has_nested_control_flow_in_stmt(stmt: &Stmt) -> bool {
    let Stmt::Return(ret) = stmt else {
        return false;
    };
    let Some(arg) = &ret.arg else { return false };
    let Expr::Call(call) = arg.as_ref() else {
        return false;
    };
    if call.args.is_empty() {
        return false;
    }
    // If .wrap() has a 4th argument, it's a trys array — try/catch we can't decode yet
    if call.args.len() >= 4 {
        if let Some(arg4) = call.args.get(3) {
            if matches!(arg4.expr.as_ref(), Expr::Array(_)) {
                return true;
            }
        }
    }
    let fn_expr = &call.args[0].expr;
    let cases = match fn_expr.as_ref() {
        Expr::Fn(f) => {
            let param_name = match f.function.params.first().map(|p| &p.pat) {
                Some(Pat::Ident(bi)) => bi.id.sym.clone(),
                _ => return false,
            };
            let Some(body) = &f.function.body else {
                return false;
            };
            extract_switch_cases_ref(body).map(|c| (param_name, c))
        }
        Expr::Arrow(a) => {
            let param_name = match a.params.first() {
                Some(Pat::Ident(bi)) => bi.id.sym.clone(),
                _ => return false,
            };
            match a.body.as_ref() {
                swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(body) => {
                    extract_switch_cases_ref(body).map(|c| (param_name, c))
                }
                _ => None,
            }
        }
        _ => None,
    };
    let Some((state_name, cases)) = cases else {
        return false;
    };
    // Check each case's top-level statements for nested blocks that
    // contain state machine operations (_ctx.next or break)
    for case in cases {
        for stmt in &case.cons {
            if has_state_ops_in_nested_block(&state_name, stmt) {
                return true;
            }
        }
    }
    // Check for _ctx.catch() calls — signals try/catch we can't decode
    for case in cases {
        if case_uses_catch(&state_name, case) {
            return true;
        }
    }
    false
}

/// Check if a statement contains _ctx.next assignments or break statements
/// inside nested blocks (if/else, block statements, etc.) — not at the top level.
fn has_state_ops_in_nested_block(state_name: &Atom, stmt: &Stmt) -> bool {
    match stmt {
        Stmt::If(if_stmt) => {
            has_state_ops_deep(state_name, &if_stmt.cons)
                || if_stmt
                    .alt
                    .as_ref()
                    .is_some_and(|alt| has_state_ops_deep(state_name, alt))
        }
        Stmt::Block(block) => block
            .stmts
            .iter()
            .any(|s| has_state_ops_deep(state_name, s)),
        _ => false,
    }
}

fn has_state_ops_deep(state_name: &Atom, stmt: &Stmt) -> bool {
    struct Finder {
        state_name: Atom,
        found: bool,
    }
    impl swc_core::ecma::visit::Visit for Finder {
        fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
            if let Some(left_member) = assign.left.as_simple().and_then(|s| s.as_member()) {
                if is_ident_with_name(&left_member.obj, &self.state_name)
                    && is_member_prop(&left_member.prop, "next")
                {
                    self.found = true;
                    return;
                }
            }
            assign.visit_children_with(self);
        }
        fn visit_break_stmt(&mut self, _: &swc_core::ecma::ast::BreakStmt) {
            self.found = true;
        }
    }
    let mut f = Finder {
        state_name: state_name.clone(),
        found: false,
    };
    stmt.visit_with(&mut f);
    f.found
}

/// Check if a switch case contains `_ctx.catch(...)` calls — signals try/catch.
fn case_uses_catch(state_name: &Atom, case: &SwitchCase) -> bool {
    struct CatchFinder {
        state_name: Atom,
        found: bool,
    }
    impl swc_core::ecma::visit::Visit for CatchFinder {
        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if let Some(callee) = call.callee.as_expr() {
                if let Expr::Member(member) = callee.as_ref() {
                    if is_ident_with_name(&member.obj, &self.state_name)
                        && is_member_prop(&member.prop, "catch")
                    {
                        self.found = true;
                        return;
                    }
                }
            }
            call.visit_children_with(self);
        }
    }
    let mut f = CatchFinder {
        state_name: state_name.clone(),
        found: false,
    };
    for stmt in &case.cons {
        stmt.visit_with(&mut f);
        if f.found {
            return true;
        }
    }
    false
}

fn extract_switch_cases_ref(body: &BlockStmt) -> Option<&[SwitchCase]> {
    for stmt in &body.stmts {
        match stmt {
            Stmt::While(while_stmt) => {
                if let Stmt::Block(block) = while_stmt.body.as_ref() {
                    for inner in &block.stmts {
                        if let Stmt::Switch(sw) = inner {
                            return Some(&sw.cases);
                        }
                    }
                }
            }
            Stmt::For(for_stmt) => {
                if let Stmt::Block(block) = for_stmt.body.as_ref() {
                    for inner in &block.stmts {
                        if let Stmt::Switch(sw) = inner {
                            return Some(&sw.cases);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn is_regenerator_wrap_return(stmt: &Stmt) -> bool {
    let Stmt::Return(ret) = stmt else {
        return false;
    };
    let Some(arg) = &ret.arg else { return false };
    is_wrap_call(arg)
}

/// Check if expr is `<something>.wrap(stateMachineFn, ...)`
/// where stateMachineFn contains the distinctive `while(true) { switch(param.prev = param.next) }` pattern.
fn is_wrap_call(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let Some(callee_expr) = call.callee.as_expr() else {
        return false;
    };
    let Expr::Member(member) = callee_expr.as_ref() else {
        return false;
    };
    if !is_member_prop(&member.prop, "wrap") {
        return false;
    }
    if call.args.is_empty() {
        return false;
    }
    // Validate that the first argument is a state machine function
    is_state_machine_fn(&call.args[0].expr)
}

/// Check if an expression is a state machine function:
/// `function(param) { while(true) { switch(param.prev = param.next) { ... } } }`
/// or arrow: `(param) => { while(true) { switch(...) { ... } } }`
fn is_state_machine_fn(expr: &Expr) -> bool {
    match expr {
        Expr::Fn(fn_expr) => {
            if fn_expr.function.params.len() != 1 {
                return false;
            }
            let param_name = match &fn_expr.function.params[0].pat {
                Pat::Ident(bi) => &bi.id.sym,
                _ => return false,
            };
            let Some(body) = &fn_expr.function.body else {
                return false;
            };
            has_state_machine_structure(body, param_name)
        }
        Expr::Arrow(arrow) => {
            if arrow.params.len() != 1 {
                return false;
            }
            let param_name = match &arrow.params[0] {
                Pat::Ident(bi) => &bi.id.sym,
                _ => return false,
            };
            match arrow.body.as_ref() {
                swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(body) => {
                    has_state_machine_structure(body, param_name)
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// Check for `while(true) { switch(param.prev = param.next) { ... case "end": ... } }`
/// or `for(;;) { switch(...) { ... } }`
fn has_state_machine_structure(body: &BlockStmt, param_name: &Atom) -> bool {
    // Look for a while(true) or for(;;) loop containing the switch
    for stmt in &body.stmts {
        match stmt {
            Stmt::While(while_stmt) => {
                if !is_true_expr(&while_stmt.test) {
                    continue;
                }
                if let Stmt::Block(block) = while_stmt.body.as_ref() {
                    if has_state_switch(block, param_name) {
                        return true;
                    }
                }
            }
            Stmt::For(for_stmt) => {
                // for(;;) — init, test, update all None
                if for_stmt.init.is_some() || for_stmt.test.is_some() || for_stmt.update.is_some() {
                    continue;
                }
                if let Stmt::Block(block) = for_stmt.body.as_ref() {
                    if has_state_switch(block, param_name) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

fn is_true_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(Lit::Bool(b)) => b.value,
        Expr::Lit(Lit::Num(n)) => n.value != 0.0,
        _ => false,
    }
}

/// Check if a block contains `switch(param.prev = param.next) { ... }`
fn has_state_switch(block: &BlockStmt, param_name: &Atom) -> bool {
    for stmt in &block.stmts {
        if let Stmt::Switch(sw) = stmt {
            // Discriminant should be: param.prev = param.next
            if is_prev_assign_next(&sw.discriminant, param_name) {
                return true;
            }
        }
    }
    false
}

/// Check for `param.prev = param.next`
fn is_prev_assign_next(expr: &Expr, param_name: &Atom) -> bool {
    let Expr::Assign(assign) = expr else {
        return false;
    };
    if assign.op != AssignOp::Assign {
        return false;
    }
    // Left: param.prev
    let Some(left_member) = assign.left.as_simple().and_then(|s| s.as_member()) else {
        return false;
    };
    if !is_ident_with_name(&left_member.obj, param_name)
        || !is_member_prop(&left_member.prop, "prev")
    {
        return false;
    }
    // Right: param.next
    let Expr::Member(right_member) = assign.right.as_ref() else {
        return false;
    };
    is_ident_with_name(&right_member.obj, param_name) && is_member_prop(&right_member.prop, "next")
}

fn extract_wrap_args(stmt: Stmt) -> Option<(Atom, Vec<SwitchCase>)> {
    let Stmt::Return(ret) = stmt else { return None };
    let arg = *ret.arg?;
    let Expr::Call(call) = arg else { return None };
    let callee_expr = call.callee.as_expr()?;
    let Expr::Member(member) = callee_expr.as_ref() else {
        return None;
    };
    if !is_member_prop(&member.prop, "wrap") {
        return None;
    }
    if call.args.is_empty() {
        return None;
    }

    let fn_arg = *call.args.into_iter().next()?.expr;
    extract_state_machine_parts(fn_arg)
}

fn extract_state_machine_parts(expr: Expr) -> Option<(Atom, Vec<SwitchCase>)> {
    match expr {
        Expr::Fn(fn_expr) => {
            let param_name = match &fn_expr.function.params.first()?.pat {
                Pat::Ident(bi) => bi.id.sym.clone(),
                _ => return None,
            };
            let body = fn_expr.function.body?;
            let cases = extract_switch_cases_from_body(body)?;
            Some((param_name, cases))
        }
        Expr::Arrow(arrow) => {
            let param_name = match &arrow.params.first()? {
                Pat::Ident(bi) => bi.id.sym.clone(),
                _ => return None,
            };
            let body = match *arrow.body {
                swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(b) => b,
                _ => return None,
            };
            let cases = extract_switch_cases_from_body(body)?;
            Some((param_name, cases))
        }
        _ => None,
    }
}

fn extract_switch_cases_from_body(body: BlockStmt) -> Option<Vec<SwitchCase>> {
    // Find the while(true) or for(;;) loop, then the switch inside
    for stmt in body.stmts {
        match stmt {
            Stmt::While(while_stmt) => {
                if let Stmt::Block(block) = *while_stmt.body {
                    for inner in block.stmts {
                        if let Stmt::Switch(sw) = inner {
                            return Some(sw.cases);
                        }
                    }
                }
            }
            Stmt::For(for_stmt) => {
                if let Stmt::Block(block) = *for_stmt.body {
                    for inner in block.stmts {
                        if let Stmt::Switch(sw) = inner {
                            return Some(sw.cases);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

// ============================================================
// Babel state machine decoder
// ============================================================

fn decode_babel_state_machine(state_name: &Atom, cases: Vec<SwitchCase>) -> Vec<Stmt> {
    let mut trys: Vec<[Option<usize>; 4]> = Vec::new();
    // Collect (label_idx, stmt) pairs
    let mut flat: Vec<(usize, Stmt)> = Vec::new();

    for case in &cases {
        let idx = match case_label_index(case) {
            Some(n) => n,
            None => continue, // skip "end" case
        };

        let stmts = &case.cons;
        let mut i = 0;
        while i < stmts.len() {
            let stmt = &stmts[i];

            // Skip _ctx.next = N assignments (state transitions)
            if is_next_assign(state_name, stmt) {
                i += 1;
                continue;
            }

            // Skip _ctx.label = N (tslib-style, shouldn't appear but be safe)
            if is_label_assign(state_name, stmt) {
                i += 1;
                continue;
            }

            // Handle _ctx.trys.push([...]) for try/catch regions
            if let Some(region) = extract_trys_push(state_name, stmt) {
                trys.push(region);
                i += 1;
                continue;
            }

            // Handle return statements (yields, abrupt returns, stop)
            if let Stmt::Return(ret) = stmt {
                if let Some(decoded) = decode_return(state_name, ret) {
                    match decoded {
                        DecodedReturn::Return(expr) => {
                            flat.push((
                                idx,
                                Stmt::Return(ReturnStmt {
                                    span: DUMMY_SP,
                                    arg: Some(expr),
                                }),
                            ));
                        }
                        DecodedReturn::ReturnVoid => {
                            flat.push((
                                idx,
                                Stmt::Return(ReturnStmt {
                                    span: DUMMY_SP,
                                    arg: None,
                                }),
                            ));
                        }
                        DecodedReturn::Throw(expr) => {
                            flat.push((
                                idx,
                                Stmt::Throw(swc_core::ecma::ast::ThrowStmt {
                                    span: DUMMY_SP,
                                    arg: expr,
                                }),
                            ));
                        }
                        DecodedReturn::Stop => {} // end of generator, drop
                        DecodedReturn::CommaYield(expr) => {
                            // return _ctx.next = N, value → yield value
                            flat.push((
                                idx,
                                Stmt::Expr(ExprStmt {
                                    span: DUMMY_SP,
                                    expr: Box::new(Expr::Yield(YieldExpr {
                                        span: DUMMY_SP,
                                        delegate: false,
                                        arg: Some(expr),
                                    })),
                                }),
                            ));
                        }
                    }
                    i += 1;
                    continue;
                }
                // Plain return with non-pattern expression: treat as yield
                if let Some(arg) = &ret.arg {
                    if !is_stop_call(state_name, arg) {
                        flat.push((
                            idx,
                            Stmt::Expr(ExprStmt {
                                span: DUMMY_SP,
                                expr: Box::new(Expr::Yield(YieldExpr {
                                    span: DUMMY_SP,
                                    delegate: false,
                                    arg: Some(arg.clone()),
                                })),
                            }),
                        ));
                        i += 1;
                        continue;
                    }
                }
                i += 1;
                continue;
            }

            // Handle break — if the last _ctx.next pointed back to case 0 this is a loop,
            // otherwise it's just a goto (skip it)
            if matches!(stmt, Stmt::Break(_)) {
                i += 1;
                continue;
            }

            // Regular statement — emit as-is
            flat.push((idx, stmt.clone()));
            i += 1;
        }
    }

    // Phase 2: merge _ctx.sent with previous yield
    let mut output: Vec<(usize, Stmt)> = Vec::new();
    for (idx, stmt) in flat {
        if is_standalone_sent(state_name, &stmt) {
            continue;
        }
        if stmt_uses_sent(state_name, &stmt) {
            if is_catch_label(idx, &trys) {
                let mut replacer = SentReplacer {
                    state_name: state_name.clone(),
                    replacement: Box::new(Expr::Ident(Ident::new_no_ctxt(
                        "error".into(),
                        DUMMY_SP,
                    ))),
                };
                let mut s = stmt;
                s.visit_mut_with(&mut replacer);
                output.push((idx, s));
                continue;
            }
            let merged = if let Some((_, prev)) = output.last() {
                extract_yield_from_stmt(prev).map(|arg| {
                    let yield_expr = Box::new(Expr::Yield(YieldExpr {
                        span: DUMMY_SP,
                        delegate: false,
                        arg: Some(arg),
                    }));
                    let mut replacer = SentReplacer {
                        state_name: state_name.clone(),
                        replacement: yield_expr,
                    };
                    let mut s = stmt.clone();
                    s.visit_mut_with(&mut replacer);
                    s
                })
            } else {
                None
            };
            if let Some(merged_stmt) = merged {
                output.pop();
                output.push((idx, merged_stmt));
            } else {
                let mut replacer = SentReplacer {
                    state_name: state_name.clone(),
                    replacement: Box::new(Expr::Ident(Ident::new_no_ctxt(
                        "undefined".into(),
                        DUMMY_SP,
                    ))),
                };
                let mut s = stmt;
                s.visit_mut_with(&mut replacer);
                output.push((idx, s));
            }
        } else {
            output.push((idx, stmt));
        }
    }

    // Phase 3: Detect infinite loops (case 0 → ... → goto 0 pattern)
    let has_back_edge_to_zero = detect_back_edge_to_zero(state_name, &cases);

    // Phase 4: Group by label and reconstruct try/catch/finally
    let max_label = output.iter().map(|(i, _)| *i).max().unwrap_or(0);
    let mut label_stmts: Vec<Vec<Stmt>> = vec![vec![]; max_label + 1];
    for (idx, stmt) in output {
        if idx <= max_label {
            label_stmts[idx].push(stmt);
        }
    }

    let mut result = reconstruct_with_regions(label_stmts, &trys);

    // Wrap in while(true) if we detected a back-edge to case 0
    if has_back_edge_to_zero && !result.is_empty() {
        result = vec![Stmt::While(WhileStmt {
            span: DUMMY_SP,
            test: Box::new(Expr::Lit(Lit::Bool(swc_core::ecma::ast::Bool {
                span: DUMMY_SP,
                value: true,
            }))),
            body: Box::new(Stmt::Block(BlockStmt {
                span: DUMMY_SP,
                ctxt: Default::default(),
                stmts: result,
            })),
        })];
    }

    result
}

fn detect_back_edge_to_zero(state_name: &Atom, cases: &[SwitchCase]) -> bool {
    for case in cases {
        let idx = match case_label_index(case) {
            Some(n) => n,
            None => continue,
        };
        if idx == 0 {
            continue;
        }
        for stmt in &case.cons {
            if is_next_assign_to(state_name, stmt, 0) {
                return true;
            }
            // Check comma operator: return _ctx.next = 0, ...
            if let Stmt::Return(ret) = stmt {
                if let Some(arg) = &ret.arg {
                    if is_comma_next_assign_to(state_name, arg, 0) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn is_next_assign_to(state_name: &Atom, stmt: &Stmt, target: usize) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return false;
    };
    if assign.op != AssignOp::Assign {
        return false;
    }
    let Some(left_member) = assign.left.as_simple().and_then(|s| s.as_member()) else {
        return false;
    };
    if !is_ident_with_name(&left_member.obj, state_name)
        || !is_member_prop(&left_member.prop, "next")
    {
        return false;
    }
    if let Expr::Lit(Lit::Num(n)) = assign.right.as_ref() {
        return n.value as usize == target;
    }
    false
}

fn is_comma_next_assign_to(state_name: &Atom, expr: &Expr, target: usize) -> bool {
    let Expr::Seq(seq) = expr else {
        return false;
    };
    for e in &seq.exprs {
        if let Expr::Assign(assign) = e.as_ref() {
            if assign.op != AssignOp::Assign {
                continue;
            }
            let Some(left_member) = assign.left.as_simple().and_then(|s| s.as_member()) else {
                continue;
            };
            if is_ident_with_name(&left_member.obj, state_name)
                && is_member_prop(&left_member.prop, "next")
            {
                if let Expr::Lit(Lit::Num(n)) = assign.right.as_ref() {
                    if n.value as usize == target {
                        return true;
                    }
                }
            }
        }
    }
    false
}

enum DecodedReturn {
    Return(Box<Expr>),
    ReturnVoid,
    Throw(Box<Expr>),
    Stop,
    CommaYield(Box<Expr>),
}

fn decode_return(state_name: &Atom, ret: &ReturnStmt) -> Option<DecodedReturn> {
    let arg = ret.arg.as_ref()?;

    // return _ctx.stop()
    if is_stop_call(state_name, arg) {
        return Some(DecodedReturn::Stop);
    }

    // return _ctx.abrupt("return", value)
    if let Some(decoded) = decode_abrupt(state_name, arg) {
        return Some(decoded);
    }

    // return (_ctx.next = N, value) — comma operator form
    if let Expr::Seq(seq) = arg.as_ref() {
        if seq.exprs.len() >= 2 {
            // Check if first expression is _ctx.next = N
            if is_next_assign_expr(state_name, &seq.exprs[0]) {
                let value = seq.exprs.last().unwrap().clone();
                return Some(DecodedReturn::CommaYield(value));
            }
        }
    }

    None
}

fn is_stop_call(state_name: &Atom, expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let Some(callee) = call.callee.as_expr() else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    is_ident_with_name(&member.obj, state_name) && is_member_prop(&member.prop, "stop")
}

fn decode_abrupt(state_name: &Atom, expr: &Expr) -> Option<DecodedReturn> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let callee = call.callee.as_expr()?;
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    if !is_ident_with_name(&member.obj, state_name) || !is_member_prop(&member.prop, "abrupt") {
        return None;
    }
    if call.args.is_empty() {
        return None;
    }
    let Expr::Lit(Lit::Str(kind)) = call.args[0].expr.as_ref() else {
        return None;
    };
    let kind_str = kind.value.as_str().unwrap_or("");
    match kind_str {
        "return" => {
            if call.args.len() >= 2 {
                Some(DecodedReturn::Return(call.args[1].expr.clone()))
            } else {
                Some(DecodedReturn::ReturnVoid)
            }
        }
        "throw" => {
            if call.args.len() >= 2 {
                Some(DecodedReturn::Throw(call.args[1].expr.clone()))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_next_assign(state_name: &Atom, stmt: &Stmt) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    is_next_assign_expr(state_name, expr)
}

fn is_next_assign_expr(state_name: &Atom, expr: &Expr) -> bool {
    let Expr::Assign(assign) = expr else {
        return false;
    };
    if assign.op != AssignOp::Assign {
        return false;
    }
    let Some(left_member) = assign.left.as_simple().and_then(|s| s.as_member()) else {
        return false;
    };
    is_ident_with_name(&left_member.obj, state_name) && is_member_prop(&left_member.prop, "next")
}

fn is_label_assign(state_name: &Atom, stmt: &Stmt) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return false;
    };
    if assign.op != AssignOp::Assign {
        return false;
    }
    let Some(left_member) = assign.left.as_simple().and_then(|s| s.as_member()) else {
        return false;
    };
    is_ident_with_name(&left_member.obj, state_name) && is_member_prop(&left_member.prop, "label")
}

fn case_label_index(case: &SwitchCase) -> Option<usize> {
    let test = case.test.as_ref()?;
    if let Expr::Lit(Lit::Num(n)) = test.as_ref() {
        Some(n.value as usize)
    } else {
        None // "end" case
    }
}

fn extract_trys_push(state_name: &Atom, stmt: &Stmt) -> Option<[Option<usize>; 4]> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    let callee_expr = call.callee.as_expr()?;
    let Expr::Member(callee_mem) = callee_expr.as_ref() else {
        return None;
    };
    let Expr::Member(outer_mem) = callee_mem.obj.as_ref() else {
        return None;
    };
    if !is_ident_with_name(&outer_mem.obj, state_name) {
        return None;
    }
    if !is_member_prop(&outer_mem.prop, "trys") {
        return None;
    }
    if !is_member_prop(&callee_mem.prop, "push") {
        return None;
    }
    if call.args.len() != 1 {
        return None;
    }
    let Expr::Array(arr) = call.args[0].expr.as_ref() else {
        return None;
    };
    if arr.elems.len() < 2 {
        return None;
    }
    let mut region = [None; 4];
    for (i, elem) in arr.elems.iter().enumerate().take(4) {
        region[i] = elem.as_ref().and_then(|e| {
            if let Expr::Lit(Lit::Num(n)) = e.expr.as_ref() {
                Some(n.value as usize)
            } else {
                None
            }
        });
    }
    Some(region)
}

fn is_standalone_sent(state_name: &Atom, stmt: &Stmt) -> bool {
    if let Stmt::Expr(ExprStmt { expr, .. }) = stmt {
        return is_sent_access(state_name, expr);
    }
    false
}

fn is_sent_access(state_name: &Atom, expr: &Expr) -> bool {
    // _ctx.sent (property access, not method call like tslib)
    if let Expr::Member(member) = expr {
        return is_ident_with_name(&member.obj, state_name) && is_member_prop(&member.prop, "sent");
    }
    // Also handle _ctx.sent() (some versions use method call)
    if let Expr::Call(call) = expr {
        if let Some(callee) = call.callee.as_expr() {
            if let Expr::Member(member) = callee.as_ref() {
                return is_ident_with_name(&member.obj, state_name)
                    && is_member_prop(&member.prop, "sent");
            }
        }
    }
    false
}

fn stmt_uses_sent(state_name: &Atom, stmt: &Stmt) -> bool {
    struct Finder {
        state_name: Atom,
        found: bool,
    }
    impl swc_core::ecma::visit::Visit for Finder {
        fn visit_member_expr(&mut self, member: &MemberExpr) {
            if is_ident_with_name(&member.obj, &self.state_name)
                && is_member_prop(&member.prop, "sent")
            {
                self.found = true;
                return;
            }
            member.visit_children_with(self);
        }
    }
    let mut f = Finder {
        state_name: state_name.clone(),
        found: false,
    };
    stmt.visit_with(&mut f);
    f.found
}

fn extract_yield_from_stmt(stmt: &Stmt) -> Option<Box<Expr>> {
    if let Stmt::Expr(ExprStmt { expr, .. }) = stmt {
        if let Expr::Yield(y) = expr.as_ref() {
            return y.arg.clone();
        }
    }
    None
}

fn is_catch_label(label_idx: usize, trys: &[[Option<usize>; 4]]) -> bool {
    trys.iter().any(|region| region[1] == Some(label_idx))
}

struct SentReplacer {
    state_name: Atom,
    replacement: Box<Expr>,
}

impl VisitMut for SentReplacer {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        // Replace _ctx.sent property access
        if let Expr::Member(member) = expr {
            if is_ident_with_name(&member.obj, &self.state_name)
                && is_member_prop(&member.prop, "sent")
            {
                *expr = *self.replacement.clone();
                return;
            }
        }
        // Replace _ctx.sent() method call
        if let Expr::Call(call) = expr {
            if let Some(callee) = call.callee.as_expr() {
                if let Expr::Member(member) = callee.as_ref() {
                    if is_ident_with_name(&member.obj, &self.state_name)
                        && is_member_prop(&member.prop, "sent")
                    {
                        *expr = *self.replacement.clone();
                        return;
                    }
                }
            }
        }
        expr.visit_mut_children_with(self);
    }
}

fn reconstruct_with_regions(label_stmts: Vec<Vec<Stmt>>, trys: &[[Option<usize>; 4]]) -> Vec<Stmt> {
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

            result.push(Stmt::Try(Box::new(swc_core::ecma::ast::TryStmt {
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

// ============================================================
// _asyncToGenerator → async function
// ============================================================

fn try_transform_async_to_generator(
    body: &mut BlockStmt,
    async_to_gen_bindings: &[BindingKey],
    _unresolved_mark: Mark,
) -> bool {
    let return_idx = body
        .stmts
        .iter()
        .position(|s| is_async_to_gen_return(s, async_to_gen_bindings));
    let return_idx = match return_idx {
        Some(i) => i,
        None => return false,
    };

    // Pre-check: validate the pattern is extractable before removing the stmt.
    if !can_extract_async_to_gen(&body.stmts[return_idx]) {
        return false;
    }

    let ret_stmt = body.stmts.remove(return_idx);
    let inner_stmts = match extract_async_to_gen_body(ret_stmt, async_to_gen_bindings) {
        Some(s) => s,
        None => unreachable!("can_extract_async_to_gen passed but extract failed"),
    };

    let mut inner_stmts = inner_stmts;
    replace_yield_with_await(&mut inner_stmts);

    body.stmts.splice(return_idx..return_idx, inner_stmts);
    true
}

fn is_async_to_gen_return(stmt: &Stmt, async_to_gen_bindings: &[BindingKey]) -> bool {
    let Stmt::Return(ret) = stmt else {
        return false;
    };
    let Some(arg) = &ret.arg else { return false };
    is_async_to_gen_call(arg, async_to_gen_bindings)
}

/// Non-destructive check: can we extract the async body from this statement?
/// Validates the same conditions as extract_async_to_gen_body without consuming the AST.
fn can_extract_async_to_gen(stmt: &Stmt) -> bool {
    let Stmt::Return(ret) = stmt else {
        return false;
    };
    let Some(arg) = &ret.arg else { return false };
    let Expr::Call(outer_call) = arg.as_ref() else {
        return false;
    };
    // Outer IIFE must have no arguments
    if !outer_call.args.is_empty() {
        return false;
    }
    let Some(outer_callee) = outer_call.callee.as_expr() else {
        return false;
    };
    let Expr::Call(inner_call) = outer_callee.as_ref() else {
        return false;
    };
    if inner_call.args.len() != 1 {
        return false;
    }
    let gen_fn = &inner_call.args[0].expr;
    match gen_fn.as_ref() {
        Expr::Fn(fn_expr) => {
            // Inner generator must have no params
            if !fn_expr.function.params.is_empty() {
                return false;
            }
            if fn_expr.function.is_generator {
                return true;
            }
            // Non-generator: must contain regenerator.wrap and pass nested-CF check
            fn_expr.function.body.as_ref().is_some_and(|body| {
                body.stmts
                    .iter()
                    .any(|s| is_regenerator_wrap_return(s) && !has_nested_control_flow_in_stmt(s))
            })
        }
        Expr::Call(mark_call) => {
            // regeneratorRuntime.mark(function _callee() { ... })
            let Some(callee) = mark_call.callee.as_expr() else {
                return false;
            };
            let Expr::Member(member) = callee.as_ref() else {
                return false;
            };
            if !is_member_prop(&member.prop, "mark") || mark_call.args.len() != 1 {
                return false;
            }
            let Expr::Fn(fn_expr) = mark_call.args[0].expr.as_ref() else {
                return false;
            };
            if !fn_expr.function.params.is_empty() {
                return false;
            }
            fn_expr.function.body.as_ref().is_some_and(|body| {
                body.stmts
                    .iter()
                    .any(|s| is_regenerator_wrap_return(s) && !has_nested_control_flow_in_stmt(s))
            })
        }
        _ => false,
    }
}

/// Check for `_asyncToGenerator(fn)()` — IIFE pattern with scope-aware matching
fn is_async_to_gen_call(expr: &Expr, async_to_gen_bindings: &[BindingKey]) -> bool {
    let Expr::Call(outer_call) = expr else {
        return false;
    };
    let Some(outer_callee) = outer_call.callee.as_expr() else {
        return false;
    };
    let Expr::Call(inner_call) = outer_callee.as_ref() else {
        return false;
    };
    let Some(inner_callee) = inner_call.callee.as_expr() else {
        return false;
    };
    let Expr::Ident(id) = inner_callee.as_ref() else {
        return false;
    };
    async_to_gen_bindings
        .iter()
        .any(|(sym, ctxt)| id.sym == *sym && id.ctxt == *ctxt)
}

fn extract_async_to_gen_body(
    stmt: Stmt,
    async_to_gen_bindings: &[BindingKey],
) -> Option<Vec<Stmt>> {
    let Stmt::Return(ret) = stmt else { return None };
    let arg = *ret.arg?;
    // _asyncToGenerator(fn)()
    let Expr::Call(outer_call) = arg else {
        return None;
    };
    // P1-2: Bail out if the outer IIFE call has arguments
    if !outer_call.args.is_empty() {
        return None;
    }
    let Expr::Call(mut inner_call) = *outer_call.callee.expect_expr() else {
        return None;
    };
    let inner_callee = inner_call.callee.as_expr()?;
    let Expr::Ident(id) = inner_callee.as_ref() else {
        return None;
    };
    // P1-3: Scope-aware matching — check both sym and SyntaxContext
    if !async_to_gen_bindings
        .iter()
        .any(|(sym, ctxt)| id.sym == *sym && id.ctxt == *ctxt)
    {
        return None;
    }
    if inner_call.args.len() != 1 {
        return None;
    }

    let gen_fn_arg = *inner_call.args.remove(0).expr;

    // The argument could be:
    // 1. function*() { ... } — native generator
    // 2. regeneratorRuntime.mark(function _callee() { ... }) — babel wrapped
    match gen_fn_arg {
        Expr::Fn(fn_expr) => {
            // P1-2: Bail out if inner generator has params — real Babel output
            // never has params here (they're on the outer function via closure)
            if !fn_expr.function.params.is_empty() {
                return None;
            }
            if fn_expr.function.is_generator {
                // Native generator: just extract body
                return fn_expr.function.body.map(|b| b.stmts);
            }
            // Non-generator function that contains regeneratorRuntime.wrap
            let mut body = fn_expr.function.body?;
            if try_transform_regenerator_wrap(&mut body).is_some() {
                return Some(body.stmts);
            }
            None
        }
        Expr::Call(mark_call) => {
            // regeneratorRuntime.mark(function _callee() { ... })
            let callee_expr = mark_call.callee.as_expr()?;
            let Expr::Member(member) = callee_expr.as_ref() else {
                return None;
            };
            if !is_member_prop(&member.prop, "mark") {
                return None;
            }
            if mark_call.args.len() != 1 {
                return None;
            }
            let inner_fn = *mark_call.args.into_iter().next()?.expr;
            let Expr::Fn(fn_expr) = inner_fn else {
                return None;
            };
            let mut body = fn_expr.function.body?;
            if try_transform_regenerator_wrap(&mut body).is_some() {
                return Some(body.stmts);
            }
            None
        }
        _ => None,
    }
}

fn replace_yield_with_await(stmts: &mut Vec<Stmt>) {
    struct YieldToAwait;
    impl VisitMut for YieldToAwait {
        fn visit_mut_expr(&mut self, expr: &mut Expr) {
            if let Expr::Yield(y) = expr {
                let arg = y.arg.take().unwrap_or_else(|| {
                    Box::new(Expr::Ident(Ident::new_no_ctxt(
                        "undefined".into(),
                        DUMMY_SP,
                    )))
                });
                *expr = Expr::Await(AwaitExpr {
                    span: DUMMY_SP,
                    arg,
                });
                return;
            }
            expr.visit_mut_children_with(self);
        }
    }
    let mut v = YieldToAwait;
    for s in stmts.iter_mut() {
        s.visit_mut_with(&mut v);
    }
}

// ============================================================
// Module-level cleanup: remove regeneratorRuntime.mark() decls
// ============================================================

/// Remove only the mark declarations whose bindings were consumed by
/// successful `.wrap()` transforms. Only removes `var x = <expr>.mark(fn)`
/// where `(x.sym, x.ctxt)` matches a consumed mark key.
fn remove_consumed_mark_declarations(module: &mut Module, consumed_marks: &[BindingKey]) {
    if consumed_marks.is_empty() {
        return;
    }
    module.body.retain_mut(|item| {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            return true;
        };
        var.decls.retain(|decl| {
            let Pat::Ident(bi) = &decl.name else {
                return true;
            };
            if !consumed_marks
                .iter()
                .any(|(sym, ctxt)| bi.id.sym == *sym && bi.id.ctxt == *ctxt)
            {
                return true;
            }
            // Only remove if the initializer is a .mark() call
            let Some(init) = &decl.init else {
                return true;
            };
            let Expr::Call(call) = init.as_ref() else {
                return true;
            };
            let Some(callee) = call.callee.as_expr() else {
                return true;
            };
            let Expr::Member(member) = callee.as_ref() else {
                return true;
            };
            !is_member_prop(&member.prop, "mark")
        });
        !var.decls.is_empty()
    });
}

fn remove_helper_decls(module: &mut Module, to_remove: &[(Atom, swc_core::common::SyntaxContext)]) {
    if to_remove.is_empty() {
        return;
    }
    module.body.retain_mut(|item| match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
            var.decls.retain(|decl| {
                if let Pat::Ident(bi) = &decl.name {
                    return !to_remove
                        .iter()
                        .any(|(sym, ctxt)| bi.id.sym == *sym && bi.id.ctxt == *ctxt);
                }
                true
            });
            !var.decls.is_empty()
        }
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => !to_remove
            .iter()
            .any(|(sym, ctxt)| fn_decl.ident.sym == *sym && fn_decl.ident.ctxt == *ctxt),
        _ => true,
    });
}

// ============================================================
// Shared helpers
// ============================================================

fn is_ident_with_name(expr: &Expr, name: &Atom) -> bool {
    matches!(expr, Expr::Ident(id) if id.sym == *name)
}

fn is_member_prop(prop: &MemberProp, name: &str) -> bool {
    match prop {
        MemberProp::Ident(id) => id.sym.as_str() == name,
        MemberProp::Computed(c) => {
            if let Expr::Lit(Lit::Str(s)) = c.expr.as_ref() {
                s.value.as_str() == Some(name)
            } else {
                false
            }
        }
        _ => false,
    }
}
