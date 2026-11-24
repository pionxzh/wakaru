//! Fold the temporaries Babel's regenerator transform spills an expression
//! into when a later operand of it yields, back into that expression.
//!
//! To keep evaluation order across a `yield` (an `await` before async
//! recovery), regenerator stores every operand evaluated before the yield in
//! a temporary and builds the expression from the temporaries after it:
//!
//! ```js
//! // source
//! f(a, await b)
//! // Babel 7.23 (`_ctx.tN` slots, lowered to locals by the decoder)
//! t0 = f; t1 = a; t2 = yield b; t0(t1, t2);
//! // Babel 7.29 (function-scoped `_t`, `_t2`, ... locals)
//! _t = f; _t2 = a; _t(_t2, yield b);
//! ```
//!
//! Substituting each temporary's value into its single read gives back the
//! source expression with the same evaluation order. The fold proves that from
//! syntax only:
//!
//! - the temporary is a binding the decoder made from a `_ctx.tN` slot, or a
//!   `var` declared without initializer in the generator body and named like
//!   Babel's own temporaries (`_t`, `_tN`); its only references are that
//!   declaration, one plain `=` write, and one read;
//! - the writes being folded are the statements immediately before the
//!   consumer, so nothing runs between the last write and the consumer;
//! - in the consumer's evaluation order, the reads come in the same order as
//!   the writes, and nothing but those reads (and literals) is evaluated before
//!   the last of them. Substitution then evaluates the values in the order and
//!   at the point the writes did, and the rest of the consumer after them;
//! - a read inside a nested function, a conditional operand, or any
//!   expression form not listed in `SpillScan::expr` is never folded.
//!
//! A write and its read are adjacent statements of one statement list, so no
//! try boundary or loop back-edge separates them. Every fold also needs a
//! yield in the folded expression: regenerator spills only operands that a
//! later yield would otherwise reorder, and the requirement keeps the fold off
//! ordinary single-use locals.
//!
//! One read shape is not a plain substitution: regenerator lowers a method call
//! `obj.m(a, await b)` to `t0 = obj; …; t0.m.call(t0, t1, t2)`. That form reads
//! `t0` twice and is recovered as `obj.m(a, await b)`. The source reads `obj.m`
//! before the arguments, the lowered form after them; the recovery restores
//! the source order (`pure_getters` in docs/rewrite-assumptions.md).

use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    BinaryOp, CallExpr, Callee, Decl, Expr, ExprOrSpread, Ident, Lit, MemberExpr, MemberProp,
    Number, Pat, Prop, PropName, PropOrSpread, SeqExpr, Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::collections::{HashMap, HashSet};
use crate::utils::paren::strip_parens;

use super::eval_utils::node_has_dynamic_scope_construct;
use super::helper_matcher::member_prop_name;
use super::transpiler_helper_utils::BindingKey;

/// Fold await-spill temporaries in a decoded generator body. `slot_locals`
/// are the locals the decoder made from `_ctx.tN` slots.
pub(super) fn fold_await_spill_temps(stmts: &mut Vec<Stmt>, slot_locals: &HashSet<BindingKey>) {
    if node_has_dynamic_scope_construct(stmts.as_slice()) || mentions_arguments(stmts) {
        return;
    }

    let mut candidates: HashSet<BindingKey> = slot_locals.clone();
    for stmt in stmts.iter() {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        for decl in &var.decls {
            if let Pat::Ident(binding) = &decl.name {
                if decl.init.is_none() && is_babel_spill_temp_name(&binding.id.sym) {
                    candidates.insert((binding.id.sym.clone(), binding.id.ctxt));
                }
            }
        }
    }
    if candidates.is_empty() {
        return;
    }

    let mut counter = ReferenceCounter {
        candidates: &candidates,
        refs: HashMap::default(),
        declarators: HashMap::default(),
    };
    stmts.visit_with(&mut counter);
    // A temp is declared exactly once, by a declarator without initializer,
    // so its references are that declaration, the write, and the reads.
    let refs: HashMap<BindingKey, usize> = counter
        .refs
        .into_iter()
        .filter(|(key, _)| counter.declarators.get(key) == Some(&1))
        .collect();

    let mut folded = HashSet::default();
    fold_in_list(stmts, &refs, &mut folded);
    if folded.is_empty() {
        return;
    }

    stmts.retain_mut(|stmt| {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            return true;
        };
        var.decls.retain(|decl| !declares_any(decl, &folded));
        !var.decls.is_empty()
    });
}

/// Babel 7.29 names its spill temporaries `_t`, `_t2`, `_t3`, ...
fn is_babel_spill_temp_name(name: &Atom) -> bool {
    let Some(rest) = name.strip_prefix("_t") else {
        return false;
    };
    rest.bytes().all(|b| b.is_ascii_digit())
}

fn declares_any(decl: &VarDeclarator, keys: &HashSet<BindingKey>) -> bool {
    matches!(&decl.name, Pat::Ident(binding) if keys.contains(&(binding.id.sym.clone(), binding.id.ctxt)))
}

fn mentions_arguments(stmts: &[Stmt]) -> bool {
    struct Finder(bool);
    impl Visit for Finder {
        fn visit_ident(&mut self, ident: &Ident) {
            if &*ident.sym == "arguments" {
                self.0 = true;
            }
        }
    }
    let mut finder = Finder(false);
    stmts.visit_with(&mut finder);
    finder.0
}

struct ReferenceCounter<'a> {
    candidates: &'a HashSet<BindingKey>,
    refs: HashMap<BindingKey, usize>,
    declarators: HashMap<BindingKey, usize>,
}

impl Visit for ReferenceCounter<'_> {
    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        if let Pat::Ident(binding) = &decl.name {
            let key = (binding.id.sym.clone(), binding.id.ctxt);
            if self.candidates.contains(&key) {
                // An initializer is a second write the fold does not model.
                let count = if decl.init.is_some() { 2 } else { 1 };
                *self.declarators.entry(key).or_default() += count;
            }
        }
        decl.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        let key = (ident.sym.clone(), ident.ctxt);
        if self.candidates.contains(&key) {
            *self.refs.entry(key).or_default() += 1;
        }
    }
}

fn fold_in_list(
    stmts: &mut Vec<Stmt>,
    refs: &HashMap<BindingKey, usize>,
    folded: &mut HashSet<BindingKey>,
) {
    let mut out: Vec<Stmt> = Vec::with_capacity(stmts.len());
    // The temp writes at the end of `out`, in order.
    let mut run: Vec<BindingKey> = Vec::new();

    for mut stmt in stmts.drain(..) {
        fold_in_nested_lists(&mut stmt, refs, folded);

        if !run.is_empty() {
            let values: Vec<&Expr> = out[out.len() - run.len()..]
                .iter()
                .map(|stmt| {
                    temp_write(stmt, refs)
                        .expect("run entries are temp writes")
                        .1
                })
                .collect();
            if let Some(count) = try_fold_into(&mut stmt, &run, &values, refs, folded) {
                out.truncate(out.len() - count);
                run.truncate(run.len() - count);
            }
        }

        match temp_write(&stmt, refs) {
            Some((key, _)) => run.push(key),
            None => run.clear(),
        }
        out.push(stmt);
    }

    *stmts = out;
}

fn fold_in_nested_lists(
    stmt: &mut Stmt,
    refs: &HashMap<BindingKey, usize>,
    folded: &mut HashSet<BindingKey>,
) {
    match stmt {
        Stmt::Block(block) => fold_in_list(&mut block.stmts, refs, folded),
        Stmt::If(if_stmt) => {
            fold_in_nested_lists(&mut if_stmt.cons, refs, folded);
            if let Some(alt) = &mut if_stmt.alt {
                fold_in_nested_lists(alt, refs, folded);
            }
        }
        Stmt::Try(try_stmt) => {
            fold_in_list(&mut try_stmt.block.stmts, refs, folded);
            if let Some(handler) = &mut try_stmt.handler {
                fold_in_list(&mut handler.body.stmts, refs, folded);
            }
            if let Some(finalizer) = &mut try_stmt.finalizer {
                fold_in_list(&mut finalizer.stmts, refs, folded);
            }
        }
        Stmt::While(s) => fold_in_nested_lists(&mut s.body, refs, folded),
        Stmt::DoWhile(s) => fold_in_nested_lists(&mut s.body, refs, folded),
        Stmt::For(s) => fold_in_nested_lists(&mut s.body, refs, folded),
        Stmt::ForIn(s) => fold_in_nested_lists(&mut s.body, refs, folded),
        Stmt::ForOf(s) => fold_in_nested_lists(&mut s.body, refs, folded),
        Stmt::Labeled(s) => fold_in_nested_lists(&mut s.body, refs, folded),
        Stmt::Switch(s) => {
            for case in &mut s.cases {
                fold_in_list(&mut case.cons, refs, folded);
            }
        }
        _ => {}
    }
}

/// `temp = value;` for a candidate temp.
fn temp_write<'a>(
    stmt: &'a Stmt,
    refs: &HashMap<BindingKey, usize>,
) -> Option<(BindingKey, &'a Expr)> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr_stmt.expr.as_ref() else {
        return None;
    };
    if assign.op != swc_core::ecma::ast::AssignOp::Assign {
        return None;
    }
    let ident = assign.left.as_simple()?.as_ident()?;
    let key = (ident.sym.clone(), ident.ctxt);
    refs.contains_key(&key)
        .then_some((key, assign.right.as_ref()))
}

/// Try to fold a suffix of `run` into `stmt`. Returns how many temp writes
/// were folded.
fn try_fold_into(
    stmt: &mut Stmt,
    run: &[BindingKey],
    values: &[&Expr],
    refs: &HashMap<BindingKey, usize>,
    folded: &mut HashSet<BindingKey>,
) -> Option<usize> {
    let run_set: HashSet<BindingKey> = run.iter().cloned().collect();
    let mut candidate = stmt.clone();

    // `t.m.call(t, ...)` → `t.m(...)` for a run temp read exactly there.
    let mut receivers = HashSet::default();
    {
        let mut rewriter = MethodCallRewriter {
            run: &run_set,
            refs,
            receivers: &mut receivers,
        };
        for expr in consumer_exprs_mut(&mut candidate)? {
            expr.visit_mut_with(&mut rewriter);
        }
    }

    let mut scan = SpillScan {
        run: &run_set,
        receivers: &receivers,
        events: Vec::new(),
    };
    scan.stmt(&candidate)?;
    let events = scan.events;

    let reads: Vec<&BindingKey> = events
        .iter()
        .filter_map(|event| match event {
            SpillEvent::Read(key) => Some(key),
            _ => None,
        })
        .collect();
    let count = reads.len();
    if count == 0 || count > run.len() {
        return None;
    }
    let suffix = &run[run.len() - count..];
    if reads.iter().zip(suffix).any(|(read, key)| *read != key) {
        return None;
    }
    for key in suffix {
        let expected = if receivers.contains(key) { 4 } else { 3 };
        if refs.get(key) != Some(&expected) {
            return None;
        }
    }
    let last_read = events
        .iter()
        .rposition(|event| matches!(event, SpillEvent::Read(_)))?;
    if events[..last_read]
        .iter()
        .any(|event| !matches!(event, SpillEvent::Read(_)))
    {
        return None;
    }

    let suffix_values = &values[values.len() - count..];
    let yields_after_reads = events[last_read..]
        .iter()
        .any(|event| matches!(event, SpillEvent::Yield));
    if !yields_after_reads && !suffix_values.iter().any(|value| contains_yield(value)) {
        return None;
    }

    let substitutions: HashMap<BindingKey, Expr> = suffix
        .iter()
        .cloned()
        .zip(suffix_values.iter().map(|value| (*value).clone()))
        .collect();
    let mut substituter = Substituter {
        values: &substitutions,
    };
    for expr in consumer_exprs_mut(&mut candidate)? {
        expr.visit_mut_with(&mut substituter);
    }

    folded.extend(suffix.iter().cloned());
    *stmt = candidate;
    Some(count)
}

/// The expressions of a consumer statement the fold may rewrite, in
/// evaluation order.
fn consumer_exprs_mut(stmt: &mut Stmt) -> Option<Vec<&mut Expr>> {
    Some(match stmt {
        Stmt::Expr(expr_stmt) => vec![expr_stmt.expr.as_mut()],
        Stmt::Return(ret) => ret.arg.as_deref_mut().into_iter().collect(),
        Stmt::Throw(throw) => vec![throw.arg.as_mut()],
        Stmt::If(if_stmt) => vec![if_stmt.test.as_mut()],
        Stmt::Decl(Decl::Var(var)) => var
            .decls
            .iter_mut()
            .filter_map(|decl| decl.init.as_deref_mut())
            .collect(),
        _ => return None,
    })
}

struct MethodCallRewriter<'a> {
    run: &'a HashSet<BindingKey>,
    refs: &'a HashMap<BindingKey, usize>,
    receivers: &'a mut HashSet<BindingKey>,
}

impl VisitMut for MethodCallRewriter<'_> {
    fn visit_mut_function(&mut self, _: &mut swc_core::ecma::ast::Function) {}

    fn visit_mut_arrow_expr(&mut self, _: &mut swc_core::ecma::ast::ArrowExpr) {}

    fn visit_mut_class(&mut self, _: &mut swc_core::ecma::ast::Class) {}

    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        call.visit_mut_children_with(self);
        let Some(key) = bound_method_call_receiver(call) else {
            return;
        };
        if !self.run.contains(&key) || self.refs.get(&key) != Some(&4) {
            return;
        }
        if !self.receivers.insert(key) {
            return;
        }
        let Callee::Expr(callee) = &mut call.callee else {
            return;
        };
        let Expr::Member(call_member) = callee.as_mut() else {
            return;
        };
        let method = std::mem::replace(call_member.obj.as_mut(), Expr::Invalid(Default::default()));
        **callee = method;
        call.args.remove(0);
    }
}

/// `t.m.call(t, ...)` with a non-computed `m` and a non-spread first
/// argument: the receiver temp `t`.
fn bound_method_call_receiver(call: &CallExpr) -> Option<BindingKey> {
    let callee = call.callee.as_expr()?;
    let Expr::Member(call_member) = callee.as_ref() else {
        return None;
    };
    if !member_prop_name(&call_member.prop, "call") {
        return None;
    }
    let Expr::Member(method) = call_member.obj.as_ref() else {
        return None;
    };
    if !matches!(method.prop, MemberProp::Ident(_)) {
        return None;
    }
    let Expr::Ident(receiver) = method.obj.as_ref() else {
        return None;
    };
    let first = call.args.first()?;
    if first.spread.is_some() {
        return None;
    }
    let Expr::Ident(this_arg) = first.expr.as_ref() else {
        return None;
    };
    if this_arg.sym != receiver.sym || this_arg.ctxt != receiver.ctxt {
        return None;
    }
    Some((receiver.sym.clone(), receiver.ctxt))
}

enum SpillEvent {
    /// A read of a temp of the current run.
    Read(BindingKey),
    /// A suspension point.
    Yield,
    /// Anything else that may have or observe an effect.
    Other,
}

struct SpillScan<'a> {
    run: &'a HashSet<BindingKey>,
    receivers: &'a HashSet<BindingKey>,
    events: Vec<SpillEvent>,
}

impl SpillScan<'_> {
    fn stmt(&mut self, stmt: &Stmt) -> Option<()> {
        match stmt {
            Stmt::Expr(expr_stmt) => self.expr(&expr_stmt.expr),
            Stmt::Return(ret) => {
                if let Some(arg) = &ret.arg {
                    self.expr(arg);
                }
            }
            Stmt::Throw(throw) => self.expr(&throw.arg),
            Stmt::If(if_stmt) => self.expr(&if_stmt.test),
            Stmt::Decl(Decl::Var(var)) => {
                for decl in &var.decls {
                    if let Some(init) = &decl.init {
                        self.expr(init);
                    }
                    self.other();
                }
            }
            _ => return None,
        }
        // The statement's own effect (the return, the throw, the branch)
        // comes after its operands.
        self.other();
        Some(())
    }

    fn other(&mut self) {
        self.events.push(SpillEvent::Other);
    }

    fn ident(&mut self, ident: &Ident) {
        let key = (ident.sym.clone(), ident.ctxt);
        if self.run.contains(&key) {
            self.events.push(SpillEvent::Read(key));
        } else {
            self.other();
        }
    }

    fn args(&mut self, args: &[ExprOrSpread]) {
        for arg in args {
            self.expr(&arg.expr);
            if arg.spread.is_some() {
                self.other();
            }
        }
    }

    fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Ident(ident) => self.ident(ident),
            Expr::Lit(Lit::Str(_) | Lit::Num(_) | Lit::Bool(_) | Lit::Null(_) | Lit::BigInt(_)) => {
            }
            Expr::Paren(paren) => self.expr(&paren.expr),
            Expr::Seq(seq) => {
                for expr in &seq.exprs {
                    self.expr(expr);
                }
            }
            Expr::Array(array) => {
                for elem in array.elems.iter().flatten() {
                    self.expr(&elem.expr);
                    if elem.spread.is_some() {
                        self.other();
                    }
                }
            }
            Expr::Object(object) => {
                for prop in &object.props {
                    match prop {
                        PropOrSpread::Spread(spread) => {
                            self.expr(&spread.expr);
                            self.other();
                        }
                        PropOrSpread::Prop(prop) => match prop.as_ref() {
                            // Never a regenerator temp; a temp read here is
                            // not seen and so never folded.
                            Prop::Shorthand(_) => self.other(),
                            Prop::KeyValue(kv) => {
                                self.prop_name(&kv.key);
                                self.expr(&kv.value);
                            }
                            // Defining a method reads nothing; a read in its
                            // body is not seen and so never folded.
                            Prop::Method(method) => self.prop_name(&method.key),
                            Prop::Getter(getter) => self.prop_name(&getter.key),
                            Prop::Setter(setter) => self.prop_name(&setter.key),
                            Prop::Assign(_) => self.other(),
                        },
                    }
                }
            }
            Expr::Call(call) => {
                match &call.callee {
                    Callee::Expr(callee) => match callee.as_ref() {
                        // A method call on a receiver temp: reading the
                        // method is the source's own order (see module docs).
                        Expr::Member(MemberExpr {
                            obj,
                            prop: MemberProp::Ident(_),
                            ..
                        }) if matches!(obj.as_ref(), Expr::Ident(id)
                            if self.receivers.contains(&(id.sym.clone(), id.ctxt))) =>
                        {
                            self.expr(obj);
                        }
                        callee => self.expr(callee),
                    },
                    _ => self.other(),
                }
                self.args(&call.args);
                self.other();
            }
            Expr::New(new) => {
                self.expr(&new.callee);
                if let Some(args) = &new.args {
                    self.args(args);
                }
                self.other();
            }
            Expr::Member(member) => {
                self.expr(&member.obj);
                if let MemberProp::Computed(computed) = &member.prop {
                    self.expr(&computed.expr);
                }
                self.other();
            }
            Expr::Unary(unary) => {
                self.expr(&unary.arg);
                self.other();
            }
            Expr::Bin(bin) => {
                self.expr(&bin.left);
                if matches!(
                    bin.op,
                    BinaryOp::LogicalAnd | BinaryOp::LogicalOr | BinaryOp::NullishCoalescing
                ) {
                    // The right operand may not run: reads there are not seen.
                    self.other();
                } else {
                    self.expr(&bin.right);
                    self.other();
                }
            }
            Expr::Cond(cond) => {
                self.expr(&cond.test);
                self.other();
            }
            Expr::Yield(yield_expr) => {
                if let Some(arg) = &yield_expr.arg {
                    self.expr(arg);
                }
                self.events.push(SpillEvent::Yield);
            }
            Expr::Await(await_expr) => {
                self.expr(&await_expr.arg);
                self.events.push(SpillEvent::Yield);
            }
            Expr::Tpl(tpl) => {
                for expr in &tpl.exprs {
                    self.expr(expr);
                }
                self.other();
            }
            Expr::Assign(assign) => {
                if assign.op == swc_core::ecma::ast::AssignOp::Assign
                    && assign.left.as_simple().and_then(|s| s.as_ident()).is_some()
                {
                    self.expr(&assign.right);
                }
                self.other();
            }
            // Functions, classes, and every other form: nothing inside is
            // seen, so no read there is folded.
            _ => self.other(),
        }
    }

    fn prop_name(&mut self, name: &PropName) {
        if let PropName::Computed(computed) = name {
            self.expr(&computed.expr);
            self.other();
        }
    }
}

fn contains_yield(expr: &Expr) -> bool {
    struct Finder(bool);
    impl Visit for Finder {
        fn visit_function(&mut self, _: &swc_core::ecma::ast::Function) {}
        fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}
        fn visit_class(&mut self, _: &swc_core::ecma::ast::Class) {}
        fn visit_yield_expr(&mut self, _: &swc_core::ecma::ast::YieldExpr) {
            self.0 = true;
        }
        fn visit_await_expr(&mut self, _: &swc_core::ecma::ast::AwaitExpr) {
            self.0 = true;
        }
    }
    let mut finder = Finder(false);
    expr.visit_with(&mut finder);
    finder.0
}

struct Substituter<'a> {
    values: &'a HashMap<BindingKey, Expr>,
}

impl Substituter<'_> {
    fn value_for(&self, expr: &Expr) -> Option<Expr> {
        let Expr::Ident(ident) = expr else {
            return None;
        };
        self.values.get(&(ident.sym.clone(), ident.ctxt)).cloned()
    }
}

impl VisitMut for Substituter<'_> {
    fn visit_mut_function(&mut self, _: &mut swc_core::ecma::ast::Function) {}

    fn visit_mut_arrow_expr(&mut self, _: &mut swc_core::ecma::ast::ArrowExpr) {}

    fn visit_mut_class(&mut self, _: &mut swc_core::ecma::ast::Class) {}

    fn visit_mut_callee(&mut self, callee: &mut Callee) {
        if let Callee::Expr(expr) = callee {
            if let Some(value) = self.value_for(expr) {
                // `t()` calls with no receiver; `o.m` as callee would pass `o`.
                **expr = if matches!(
                    strip_parens(&value),
                    Expr::Member(_) | Expr::SuperProp(_) | Expr::OptChain(_)
                ) {
                    Expr::Seq(SeqExpr {
                        span: DUMMY_SP,
                        exprs: vec![
                            Box::new(Expr::Lit(Lit::Num(Number {
                                span: DUMMY_SP,
                                value: 0.0,
                                raw: None,
                            }))),
                            Box::new(value),
                        ],
                    })
                } else {
                    value
                };
                return;
            }
        }
        callee.visit_mut_children_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Some(value) = self.value_for(expr) {
            *expr = value;
            return;
        }
        expr.visit_mut_children_with(self);
    }
}
