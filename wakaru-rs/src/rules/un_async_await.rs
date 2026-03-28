use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    AwaitExpr, BlockStmt, CatchClause, Expr, ExprStmt, Function, Ident, Pat, Stmt, SwitchCase,
    TryStmt, YieldExpr,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith, VisitWith};

pub struct UnAsyncAwait;

impl VisitMut for UnAsyncAwait {
    fn visit_mut_function(&mut self, func: &mut Function) {
        // Recurse into children first
        func.visit_mut_children_with(self);

        let body = match func.body.as_mut() {
            Some(b) => b,
            None => return,
        };

        // Try __generator transform first (makes function a generator)
        if try_transform_generator(body) {
            func.is_generator = true;
            return;
        }

        // Try __awaiter transform (makes function async).
        // After extracting the inner body, also run the generator transform
        // in case the inner function was a __generator state machine.
        if try_transform_awaiter(body) {
            try_transform_generator(body);
            func.is_async = true;
        }
    }
}

// ============================================================
// __generator state-machine → function*
// ============================================================

fn try_transform_generator(body: &mut BlockStmt) -> bool {
    // Find: return __generator(this, function(_a) { switch(_a.label) { ... } })
    let return_idx = body.stmts.iter().position(|s| is_generator_return(s));
    let return_idx = match return_idx {
        Some(i) => i,
        None => return false,
    };

    let ret_stmt = body.stmts.remove(return_idx);
    let (state_name, cases) = match extract_generator_args(ret_stmt) {
        Some(x) => x,
        None => return false,
    };

    // Build new statements from the state machine
    let new_stmts = decode_state_machine(state_name, cases);

    // Insert new statements where the return was
    body.stmts.splice(return_idx..return_idx, new_stmts);
    true
}

fn is_generator_return(stmt: &Stmt) -> bool {
    let Stmt::Return(ret) = stmt else {
        return false;
    };
    let Some(arg) = &ret.arg else { return false };
    let Expr::Call(call) = arg.as_ref() else {
        return false;
    };
    callee_name(call) == Some("__generator".into())
}

fn extract_generator_args(stmt: Stmt) -> Option<(Atom, Vec<SwitchCase>)> {
    let Stmt::Return(ret) = stmt else { return None };
    let arg = *ret.arg?;
    let Expr::Call(mut call) = arg else {
        return None;
    };
    if callee_name(&call) != Some("__generator".into()) {
        return None;
    }
    if call.args.len() < 2 {
        return None;
    }

    let fn_arg = *call.args.remove(1).expr;
    let Expr::Fn(fn_expr) = fn_arg else {
        return None;
    };
    let state_name: Atom = fn_expr.function.params.first().and_then(|p| {
        if let Pat::Ident(bi) = &p.pat {
            Some(bi.id.sym.clone())
        } else {
            None
        }
    })?;
    let body = fn_expr.function.body?;
    // First stmt should be a switch
    let switch = body.stmts.into_iter().next()?;
    let Stmt::Switch(sw) = switch else {
        return None;
    };
    Some((state_name, sw.cases))
}

/// Decode the state machine into a flat list of statements.
///
/// Phase 1: Collect (label_idx, Stmt) pairs in case order, decoding opcodes.
/// Phase 2: Merge `_a.sent()` usages with the previous yield:
///   - standalone `_a.sent();` → drop
///   - `v = _a.sent()` → pop prev `yield X;`, push `v = yield X;`
/// Phase 3: Group by label and reconstruct try/catch/finally blocks.
fn decode_state_machine(state_name: Atom, cases: Vec<SwitchCase>) -> Vec<Stmt> {
    let mut trys: Vec<[Option<usize>; 4]> = Vec::new();
    // (label_idx, stmt) pairs
    let mut flat: Vec<(usize, Stmt)> = Vec::new();

    for case in &cases {
        let idx = match numeric_case_test(case) {
            Some(n) => n as usize,
            None => continue,
        };

        for stmt in &case.cons {
            if let Some(region) = extract_trys_push(&state_name, stmt) {
                trys.push(region);
                continue;
            }
            if is_state_label_assign(&state_name, stmt) {
                continue;
            }

            if let Some(decoded) = decode_return_opcode(stmt) {
                if let Some(s) = decoded {
                    flat.push((idx, s));
                }
                continue;
            }

            flat.push((idx, stmt.clone()));
        }
    }

    // Phase 2: merge _a.sent() with previous yield
    let mut output: Vec<(usize, Stmt)> = Vec::new();
    for (idx, stmt) in flat {
        if is_standalone_sent(&state_name, &stmt) {
            // Standalone _a.sent(); — the caller discards the yielded value. Drop.
            continue;
        }
        if stmt_uses_sent(&state_name, &stmt) {
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
            // Pop the previous yield and embed it into this assignment/expression.
            let merged = if let Some((_, prev)) = output.last() {
                extract_yield_from_stmt(prev).map(|(arg, delegate)| {
                    let yield_expr = Box::new(Expr::Yield(YieldExpr {
                        span: DUMMY_SP,
                        delegate,
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
                // No previous yield — replace sent with undefined
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

    // Phase 3: group by label index
    let max_label = output.iter().map(|(i, _)| *i).max().unwrap_or(0);
    let mut label_stmts: Vec<Vec<Stmt>> = vec![vec![]; max_label + 1];
    for (idx, stmt) in output {
        label_stmts[idx].push(stmt);
    }

    reconstruct_with_regions(label_stmts, &trys)
}

fn is_catch_label(label_idx: usize, trys: &[[Option<usize>; 4]]) -> bool {
    trys.iter().any(|region| region[1] == Some(label_idx))
}

/// If `stmt` is `ExprStmt(yield X)`, return `(X, delegate)`.
fn extract_yield_from_stmt(stmt: &Stmt) -> Option<(Box<Expr>, bool)> {
    if let Stmt::Expr(ExprStmt { expr, .. }) = stmt {
        if let Expr::Yield(y) = expr.as_ref() {
            let arg = y.arg.clone().unwrap_or_else(|| {
                Box::new(Expr::Ident(Ident::new_no_ctxt(
                    "undefined".into(),
                    DUMMY_SP,
                )))
            });
            return Some((arg, y.delegate));
        }
    }
    None
}

fn is_standalone_sent(state_name: &Atom, stmt: &Stmt) -> bool {
    if let Stmt::Expr(ExprStmt { expr, .. }) = stmt {
        if let Expr::Call(call) = expr.as_ref() {
            if let Some(mem) = call.callee.as_expr().and_then(|e| e.as_member()) {
                if let Expr::Ident(id) = mem.obj.as_ref() {
                    if id.sym == *state_name && is_ident_prop(&mem.prop, "sent") {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn numeric_case_test(case: &SwitchCase) -> Option<f64> {
    let test = case.test.as_ref()?;
    if let Expr::Lit(swc_core::ecma::ast::Lit::Num(n)) = test.as_ref() {
        Some(n.value)
    } else {
        None
    }
}

fn extract_trys_push(state_name: &Atom, stmt: &Stmt) -> Option<[Option<usize>; 4]> {
    // _a.trys.push([s, c, f, n])
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    let Expr::Member(callee_mem) = &**call.callee.as_expr()? else {
        return None;
    };
    let Expr::Member(outer_mem) = callee_mem.obj.as_ref() else {
        return None;
    };
    let Expr::Ident(obj_id) = outer_mem.obj.as_ref() else {
        return None;
    };
    if obj_id.sym != *state_name {
        return None;
    }
    if !is_ident_prop(&outer_mem.prop, "trys") {
        return None;
    }
    if !is_ident_prop(&callee_mem.prop, "push") {
        return None;
    }
    if call.args.len() != 1 {
        return None;
    }
    let Expr::Array(arr) = call.args[0].expr.as_ref() else {
        return None;
    };
    if arr.elems.len() != 4 {
        return None;
    }
    let region: [Option<usize>; 4] = std::array::from_fn(|i| {
        arr.elems[i].as_ref().and_then(|e| {
            if let Expr::Lit(swc_core::ecma::ast::Lit::Num(n)) = e.expr.as_ref() {
                Some(n.value as usize)
            } else {
                None
            }
        })
    });
    Some(region)
}

fn is_state_label_assign(state_name: &Atom, stmt: &Stmt) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return false;
    };
    let Some(left_expr) = assign.left.as_simple().and_then(|s| s.as_member()) else {
        return false;
    };
    let Expr::Ident(id) = left_expr.obj.as_ref() else {
        return false;
    };
    id.sym == *state_name && is_ident_prop(&left_expr.prop, "label")
}

/// Returns `Some(Some(stmt))` if an opcode-based return was decoded,
/// `Some(None)` to drop the statement, or `None` if not a return opcode.
fn decode_return_opcode(stmt: &Stmt) -> Option<Option<Stmt>> {
    let Stmt::Return(ret) = stmt else { return None };
    let arg = ret.arg.as_ref()?;
    let Expr::Array(arr) = arg.as_ref() else {
        return None;
    };
    if arr.elems.is_empty() {
        return None;
    }
    let opcode = match arr.elems[0].as_ref()?.expr.as_ref() {
        Expr::Lit(swc_core::ecma::ast::Lit::Num(n)) => n.value as u32,
        _ => return None,
    };
    let argument = arr
        .elems
        .get(1)
        .and_then(|e| e.as_ref())
        .map(|e| e.expr.clone());

    match opcode {
        2 => {
            // return(value?)
            let s = argument.map(|a| {
                Stmt::Return(swc_core::ecma::ast::ReturnStmt {
                    span: DUMMY_SP,
                    arg: Some(a),
                })
            });
            Some(s)
        }
        4 => {
            // yield(value)
            let expr = argument.unwrap_or_else(|| {
                Box::new(Expr::Ident(Ident::new_no_ctxt(
                    "undefined".into(),
                    DUMMY_SP,
                )))
            });
            Some(Some(Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: Box::new(Expr::Yield(YieldExpr {
                    span: DUMMY_SP,
                    delegate: false,
                    arg: Some(expr),
                })),
            })))
        }
        5 => {
            // yield*(value)
            let expr = argument.unwrap_or_else(|| {
                Box::new(Expr::Ident(Ident::new_no_ctxt(
                    "undefined".into(),
                    DUMMY_SP,
                )))
            });
            Some(Some(Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: Box::new(Expr::Yield(YieldExpr {
                    span: DUMMY_SP,
                    delegate: true,
                    arg: Some(expr),
                })),
            })))
        }
        0 | 1 | 3 | 6 | 7 => Some(None), // skip
        _ => Some(Some(stmt.clone())),
    }
}

fn stmt_uses_sent(state_name: &Atom, stmt: &Stmt) -> bool {
    struct Finder {
        state_name: Atom,
        found: bool,
    }
    impl swc_core::ecma::visit::Visit for Finder {
        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if let Some(mem) = call.callee.as_expr().and_then(|e| e.as_member()) {
                if let Expr::Ident(id) = mem.obj.as_ref() {
                    if id.sym == self.state_name && is_ident_prop(&mem.prop, "sent") {
                        self.found = true;
                        return;
                    }
                }
            }
            call.visit_children_with(self);
        }
    }
    let mut f = Finder {
        state_name: state_name.clone(),
        found: false,
    };
    swc_core::ecma::visit::VisitWith::visit_with(stmt, &mut f);
    f.found
}

struct SentReplacer {
    state_name: Atom,
    replacement: Box<Expr>,
}

impl VisitMut for SentReplacer {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Call(call) = expr {
            if let Some(mem) = call.callee.as_expr().and_then(|e| e.as_member()) {
                if let Expr::Ident(id) = mem.obj.as_ref() {
                    if id.sym == self.state_name && is_ident_prop(&mem.prop, "sent") {
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
        // Check if i is the start of a protected region
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

            // Skip past the whole protected region
            i = next.unwrap_or(n);
        } else {
            // Check if i is inside any region (but not the start) → already handled
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
// __awaiter wrapper → async function
// ============================================================

fn try_transform_awaiter(body: &mut BlockStmt) -> bool {
    // Find: return __awaiter(this, void0, void0, function*() { ... })
    let return_idx = body.stmts.iter().position(|s| is_awaiter_return(s));
    let return_idx = match return_idx {
        Some(i) => i,
        None => return false,
    };

    let ret_stmt = body.stmts.remove(return_idx);
    let inner_stmts = match extract_awaiter_body(ret_stmt) {
        Some(s) => s,
        None => return false,
    };

    // Replace yield with await in the extracted statements
    let mut inner_stmts = inner_stmts;
    replace_yield_with_await(&mut inner_stmts);

    // Splice the inner statements in place of the return
    body.stmts.splice(return_idx..return_idx, inner_stmts);
    true
}

fn is_awaiter_return(stmt: &Stmt) -> bool {
    let Stmt::Return(ret) = stmt else {
        return false;
    };
    let Some(arg) = &ret.arg else { return false };
    let Expr::Call(call) = arg.as_ref() else {
        return false;
    };
    callee_name(call) == Some("__awaiter".into())
}

fn extract_awaiter_body(stmt: Stmt) -> Option<Vec<Stmt>> {
    let Stmt::Return(ret) = stmt else { return None };
    let arg = *ret.arg?;
    let Expr::Call(mut call) = arg else {
        return None;
    };
    if callee_name(&call) != Some("__awaiter".into()) {
        return None;
    }
    if call.args.len() < 4 {
        return None;
    }

    let gen_fn_arg = *call.args.remove(3).expr;
    let Expr::Fn(fn_expr) = gen_fn_arg else {
        return None;
    };
    let body = fn_expr.function.body?;
    Some(body.stmts)
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
// Helpers
// ============================================================

fn callee_name(call: &swc_core::ecma::ast::CallExpr) -> Option<Atom> {
    match &**call.callee.as_expr()? {
        Expr::Ident(id) => Some(id.sym.clone()),
        _ => None,
    }
}

fn is_ident_prop(prop: &swc_core::ecma::ast::MemberProp, name: &str) -> bool {
    matches!(prop, swc_core::ecma::ast::MemberProp::Ident(n) if n.sym.as_str() == name)
}
