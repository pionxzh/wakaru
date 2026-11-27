//! Whether a binding is written before it can be read, along every path
//! through a run of statements.
//!
//! The walk follows JavaScript's structured control flow and evaluation
//! order on the AST; there is no control-flow graph. It tracks one fact, "the
//! binding has been written on this path", which only ever goes from false to
//! true, so once it holds nothing later can read an older value and the walk
//! of that path stops. A point no path reaches (after `return`, `throw`,
//! `break`, or `continue`) counts as written. Joins:
//! - `if`/`else` and `?:` keep a write both branches make; the right side of
//!   `&&`, `||`, and `??` may not run, so it only reads;
//! - a loop is left with the state after its head (a `while`/`for` test runs
//!   before every exit), since its body may not run; a `do`/`while` body
//!   counts when it holds no `break` or `continue`;
//! - a `switch` keeps a write only with a `default` case, when the last case
//!   and every `break` out of it come after one; a case can be entered
//!   directly, so falling through never helps;
//! - a labeled block keeps a write unless a `break` leaves it first;
//! - a `try` keeps a write its block and handler, or its finalizer, make; a
//!   throw can reach the handler before any write in the block.
//!
//! Expressions are walked in evaluation order: operands, call callee then
//! arguments, member object then computed key, array elements, object
//! properties. A write counts only as `x = value` or `var x = value`. Any
//! other mention — a compound assignment, a destructuring target, `with`, or
//! a nested function or class created before the write — counts as a read
//! where it appears. A mention inside a function declaration counts as a read
//! before any write, because the declaration is hoisted and can be called from
//! anywhere in its scope.
//!
//! The answer covers one run of the statements. A closure created during
//! the run and called during a later one is the caller's concern.

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    ArrowExpr, AssignOp, AssignTarget, BreakStmt, Callee, Class, ContinueStmt, Decl, Expr,
    ExprOrSpread, FnDecl, ForStmt, Function, Ident, MemberProp, Pat, Prop, PropName, PropOrSpread,
    SimpleAssignTarget, Stmt, SwitchStmt, TryStmt, VarDeclOrExpr, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::{ident_matches_binding, BindingId};

/// Whether `binding` is written before any read on every path that runs
/// `declarators` (the rest of a declaration's declarator list) and then
/// `stmts`, starting from a state where it has not been written.
pub(crate) fn written_before_read(
    declarators: &[VarDeclarator],
    stmts: &[Stmt],
    binding: &BindingId,
) -> bool {
    if hoisted_function_mentions(stmts, binding) {
        return false;
    }
    let mut walk = Walk::new(binding);
    let written = walk.declarators(declarators, false);
    walk.stmts(stmts, written);
    !walk.read_before_write
}

/// [`written_before_read`] for an uninitialized `var` in the head of `for
/// (var x, …; test; update) body`: the head's later `declarators`, then the
/// loop.
pub(crate) fn for_head_written_before_read(
    declarators: &[VarDeclarator],
    for_stmt: &ForStmt,
    binding: &BindingId,
) -> bool {
    if hoisted_function_mentions(std::slice::from_ref(&*for_stmt.body), binding) {
        return false;
    }
    let mut walk = Walk::new(binding);
    let written = walk.declarators(declarators, false);
    walk.for_after_init(for_stmt, written);
    !walk.read_before_write
}

struct Walk<'a> {
    binding: &'a BindingId,
    /// `break` targets inside the walked code, innermost last.
    targets: Vec<BreakTarget>,
    read_before_write: bool,
}

struct BreakTarget {
    /// `None` for a loop or `switch`, the target of an unlabeled `break`.
    label: Option<Atom>,
    /// Leaving a loop is harmless: every path out passed its head, and the
    /// walk takes the state after the head as the loop's result.
    is_loop: bool,
    /// A `break` reached this target before the binding was written.
    broken_unwritten: bool,
}

impl<'a> Walk<'a> {
    fn new(binding: &'a BindingId) -> Self {
        Self {
            binding,
            targets: Vec::new(),
            read_before_write: false,
        }
    }

    /// Anything not modeled: a mention counts as a read, never as a write.
    fn read<N: for<'m> VisitWith<Mentions<'m>>>(&mut self, node: &N, written: bool) -> bool {
        if !written && !self.read_before_write && mentions(node, self.binding) {
            self.read_before_write = true;
        }
        written
    }

    fn with_target(
        &mut self,
        label: Option<Atom>,
        is_loop: bool,
        walk: impl FnOnce(&mut Self) -> bool,
    ) -> bool {
        self.targets.push(BreakTarget {
            label,
            is_loop,
            broken_unwritten: false,
        });
        let written = walk(self);
        let target = self.targets.pop().expect("pushed above");
        written && !target.broken_unwritten
    }

    /// Only reached while the binding is unwritten. A `break` whose target is
    /// outside the walked code leaves it, so no later read in it follows.
    fn break_unwritten(&mut self, label: Option<&Ident>) {
        let label = label.map(|label| label.sym.clone());
        if let Some(target) = self.targets.iter_mut().rev().find(|t| t.label == label) {
            if !target.is_loop {
                target.broken_unwritten = true;
            }
        }
    }

    fn stmts(&mut self, stmts: &[Stmt], written: bool) -> bool {
        stmts
            .iter()
            .fold(written, |written, stmt| self.stmt(stmt, written))
    }

    fn stmt(&mut self, stmt: &Stmt, written: bool) -> bool {
        if written {
            return true;
        }
        match stmt {
            Stmt::Expr(expr_stmt) => self.expr(&expr_stmt.expr, false),
            Stmt::Block(block) => self.stmts(&block.stmts, false),
            Stmt::Empty(_) => false,
            Stmt::Decl(Decl::Var(var)) => self.declarators(&var.decls, false),
            // Checked up front: the declaration is hoisted.
            Stmt::Decl(Decl::Fn(_)) => false,
            Stmt::If(if_stmt) => {
                let written = self.expr(&if_stmt.test, false);
                let cons = self.stmt(&if_stmt.cons, written);
                let alt = match &if_stmt.alt {
                    Some(alt) => self.stmt(alt, written),
                    None => written,
                };
                cons && alt
            }
            Stmt::Switch(switch) => self.switch(switch),
            Stmt::Labeled(labeled) => {
                let is_loop = matches!(
                    *labeled.body,
                    Stmt::For(_)
                        | Stmt::ForIn(_)
                        | Stmt::ForOf(_)
                        | Stmt::While(_)
                        | Stmt::DoWhile(_)
                );
                self.with_target(Some(labeled.label.sym.clone()), is_loop, |walk| {
                    walk.stmt(&labeled.body, false)
                })
            }
            Stmt::While(while_stmt) => {
                let written = self.expr(&while_stmt.test, false);
                self.with_target(None, true, |walk| walk.stmt(&while_stmt.body, written));
                written
            }
            Stmt::DoWhile(do_while) => {
                // The body runs once before the test, unless a `break` or
                // `continue` leaves it early.
                let body = self.with_target(None, true, |walk| walk.stmt(&do_while.body, false));
                let body = body && !contains_jump(&do_while.body, true);
                self.expr(&do_while.test, body)
            }
            Stmt::For(for_stmt) => {
                let written = match &for_stmt.init {
                    Some(VarDeclOrExpr::VarDecl(var)) => self.declarators(&var.decls, false),
                    Some(VarDeclOrExpr::Expr(init)) => self.expr(init, false),
                    None => false,
                };
                self.for_after_init(for_stmt, written)
            }
            Stmt::ForIn(for_in) => {
                self.read(&for_in.right, false);
                self.read(&for_in.left, false);
                self.with_target(None, true, |walk| walk.stmt(&for_in.body, false));
                false
            }
            Stmt::ForOf(for_of) => {
                self.read(&for_of.right, false);
                self.read(&for_of.left, false);
                self.with_target(None, true, |walk| walk.stmt(&for_of.body, false));
                false
            }
            Stmt::Try(try_stmt) => self.try_stmt(try_stmt),
            Stmt::Return(ret) => {
                if let Some(arg) = &ret.arg {
                    self.expr(arg, false);
                }
                true
            }
            Stmt::Throw(throw) => {
                self.expr(&throw.arg, false);
                true
            }
            // `continue` reaches a loop head, which the walk already passed.
            Stmt::Continue(_) => true,
            Stmt::Break(break_stmt) => {
                self.break_unwritten(break_stmt.label.as_ref());
                true
            }
            // `with`, class declarations, and anything else. A `break` hidden
            // in one could skip a write in an enclosing `switch` or labeled
            // block.
            other => {
                if self.targets.iter().any(|t| !t.is_loop) && contains_jump(other, false) {
                    self.read_before_write = true;
                }
                self.read(other, false)
            }
        }
    }

    /// The test, then the body and update, which may not run.
    fn for_after_init(&mut self, for_stmt: &ForStmt, written: bool) -> bool {
        let written = match &for_stmt.test {
            Some(test) => self.expr(test, written),
            None => written,
        };
        self.with_target(None, true, |walk| {
            walk.stmt(&for_stmt.body, written);
            if let Some(update) = &for_stmt.update {
                // A `continue` can reach the update before the body's writes.
                walk.expr(update, written);
            }
            written
        })
    }

    fn switch(&mut self, switch: &SwitchStmt) -> bool {
        if self.expr(&switch.discriminant, false) {
            return true;
        }
        // Case tests run before any case body, and only up to the match.
        for case in &switch.cases {
            if let Some(test) = &case.test {
                self.expr(test, false);
            }
        }
        let has_default = switch.cases.iter().any(|case| case.test.is_none());
        let ends_written = self.with_target(None, false, |walk| {
            // Every case can be entered directly. Only the last one falls out
            // of the switch; the others leave it through `break`, which the
            // target records.
            switch
                .cases
                .iter()
                .fold(true, |_, case| walk.stmts(&case.cons, false))
        });
        // Without `default`, no case may run at all.
        ends_written && has_default
    }

    fn try_stmt(&mut self, try_stmt: &TryStmt) -> bool {
        let block = self.stmts(&try_stmt.block.stmts, false);
        let handler = match &try_stmt.handler {
            Some(handler) => {
                self.read(&handler.param, false);
                self.stmts(&handler.body.stmts, false)
            }
            None => true,
        };
        let finalizer = match &try_stmt.finalizer {
            Some(finalizer) => self.stmts(&finalizer.stmts, false),
            None => false,
        };
        finalizer || (block && handler)
    }

    fn declarators(&mut self, decls: &[VarDeclarator], written: bool) -> bool {
        decls
            .iter()
            .fold(written, |written, decl| self.declarator(decl, written))
    }

    /// `var x = value` writes; any other declarator only reads.
    fn declarator(&mut self, decl: &VarDeclarator, written: bool) -> bool {
        if written {
            return true;
        }
        let written = match &decl.init {
            Some(init) => self.expr(init, false),
            None => false,
        };
        match &decl.name {
            Pat::Ident(binding) if ident_matches_binding(&binding.id, self.binding) => {
                decl.init.is_some() || written
            }
            name => self.read(name, written),
        }
    }

    fn args(&mut self, args: &[ExprOrSpread], written: bool) -> bool {
        args.iter()
            .fold(written, |written, arg| self.expr(&arg.expr, written))
    }

    fn expr(&mut self, expr: &Expr, written: bool) -> bool {
        if written {
            return true;
        }
        match expr {
            Expr::Ident(ident) => self.read(ident, false),
            Expr::Paren(paren) => self.expr(&paren.expr, false),
            Expr::Seq(seq) => seq
                .exprs
                .iter()
                .fold(false, |written, expr| self.expr(expr, written)),
            Expr::Assign(assign) => {
                let writes_binding = assign.op == AssignOp::Assign
                    && matches!(
                        &assign.left,
                        AssignTarget::Simple(SimpleAssignTarget::Ident(target))
                            if ident_matches_binding(&target.id, self.binding)
                    );
                if writes_binding {
                    // The value runs before the write.
                    self.expr(&assign.right, false);
                    return true;
                }
                // Another target: its object and key run first; a compound
                // assignment or a destructuring target reads.
                let written = self.read(&assign.left, false);
                self.expr(&assign.right, written)
            }
            Expr::Bin(bin) => {
                let left = self.expr(&bin.left, false);
                let right = self.expr(&bin.right, left);
                if bin.op.may_short_circuit() {
                    left
                } else {
                    right
                }
            }
            Expr::Cond(cond) => {
                let written = self.expr(&cond.test, false);
                let cons = self.expr(&cond.cons, written);
                let alt = self.expr(&cond.alt, written);
                cons && alt
            }
            Expr::Call(call) => {
                let written = match &call.callee {
                    Callee::Expr(callee) => self.expr(callee, false),
                    _ => false,
                };
                self.args(&call.args, written)
            }
            Expr::New(new) => {
                let written = self.expr(&new.callee, false);
                match &new.args {
                    Some(args) => self.args(args, written),
                    None => written,
                }
            }
            Expr::Member(member) => {
                let written = self.expr(&member.obj, false);
                match &member.prop {
                    MemberProp::Computed(prop) => self.expr(&prop.expr, written),
                    _ => written,
                }
            }
            Expr::Unary(unary) => self.expr(&unary.arg, false),
            Expr::Array(array) => array
                .elems
                .iter()
                .flatten()
                .fold(false, |written, elem| self.expr(&elem.expr, written)),
            Expr::Object(object) => object.props.iter().fold(false, |written, prop| match prop {
                PropOrSpread::Spread(spread) => self.expr(&spread.expr, written),
                PropOrSpread::Prop(prop) => match &**prop {
                    Prop::KeyValue(kv) => {
                        let written = match &kv.key {
                            PropName::Computed(key) => self.expr(&key.expr, written),
                            _ => written,
                        };
                        self.expr(&kv.value, written)
                    }
                    // A shorthand reads; methods and accessors are closures.
                    other => self.read(other, written),
                },
            }),
            // Template literals, updates, closures, optional chains, and
            // anything else.
            other => self.read(other, false),
        }
    }
}

/// Whether a function declaration in `stmts` (outside nested functions)
/// mentions the binding.
fn hoisted_function_mentions(stmts: &[Stmt], binding: &BindingId) -> bool {
    struct Finder<'a> {
        binding: &'a BindingId,
        found: bool,
    }
    impl Visit for Finder<'_> {
        fn visit_fn_decl(&mut self, decl: &FnDecl) {
            self.found |= mentions(&decl.function, self.binding);
        }
        fn visit_function(&mut self, _: &Function) {}
        fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
        fn visit_class(&mut self, _: &Class) {}
    }
    let mut finder = Finder {
        binding,
        found: false,
    };
    stmts.visit_with(&mut finder);
    finder.found
}

/// Whether `stmt` holds a `break` (or, with `continues`, a `continue`)
/// outside nested functions, whatever its target.
fn contains_jump(stmt: &Stmt, continues: bool) -> bool {
    struct JumpFinder {
        continues: bool,
        found: bool,
    }
    impl Visit for JumpFinder {
        fn visit_break_stmt(&mut self, _: &BreakStmt) {
            self.found = true;
        }
        fn visit_continue_stmt(&mut self, _: &ContinueStmt) {
            self.found |= self.continues;
        }
        fn visit_function(&mut self, _: &Function) {}
        fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
        fn visit_class(&mut self, _: &Class) {}
    }
    let mut finder = JumpFinder {
        continues,
        found: false,
    };
    stmt.visit_with(&mut finder);
    finder.found
}

fn mentions<N: for<'m> VisitWith<Mentions<'m>>>(node: &N, binding: &BindingId) -> bool {
    let mut mentions = Mentions {
        binding,
        found: false,
    };
    node.visit_with(&mut mentions);
    mentions.found
}

struct Mentions<'a> {
    binding: &'a BindingId,
    found: bool,
}

impl Visit for Mentions<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident_matches_binding(ident, self.binding) {
            self.found = true;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
    use swc_core::ecma::ast::{Module, ModuleItem};
    use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::VisitMutWith;

    /// Parse `function f() { var x; <body> }` and ask about `x` after its
    /// declaration.
    fn check(body: &str) -> bool {
        GLOBALS.set(&Default::default(), || {
            let source = format!("function f() {{ var x; {body} }}");
            let cm: Lrc<SourceMap> = Default::default();
            let fm = cm.new_source_file(FileName::Anon.into(), source);
            let lexer = Lexer::new(
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                StringInput::from(&*fm),
                None,
            );
            let mut module: Module = Parser::new_from(lexer).parse_module().unwrap();
            module.visit_mut_with(&mut resolver(Mark::new(), Mark::new(), false));
            let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function))) = &module.body[0] else {
                unreachable!()
            };
            let stmts = &function.function.body.as_ref().unwrap().stmts;
            let Stmt::Decl(Decl::Var(var)) = &stmts[0] else {
                unreachable!()
            };
            let Pat::Ident(x) = &var.decls[0].name else {
                unreachable!()
            };
            written_before_read(&[], &stmts[1..], &super::super::binding_id(&x.id))
        })
    }

    #[test]
    fn plain_write_then_read() {
        assert!(check("x = 1; use(x);"));
        assert!(check("a(), x = 1; use(x);"));
        assert!(check("if (x = next()) use(x);"));
    }

    #[test]
    fn read_before_write() {
        assert!(!check("use(x); x = 1;"));
        assert!(!check("x = x + 1;"));
        assert!(!check("x += 1;"));
        assert!(!check("ok() && (x = 1); use(x);"));
    }

    #[test]
    fn branches_join() {
        assert!(check("if (c) { x = 1; use(x); }"));
        assert!(check("if (c) x = 1; else x = 2; use(x);"));
        assert!(!check("if (c) x = 1; use(x);"));
        assert!(check("if (c) { x = 1; } else { return; } use(x);"));
    }

    #[test]
    fn loops_leave_with_their_entry_state() {
        assert!(!check("while (c()) { x = 1; } use(x);"));
        // A do/while body runs once, unless a jump leaves it early.
        assert!(check("do { x = 1; } while (c()); use(x);"));
        assert!(!check(
            "do { if (c()) continue; x = 1; } while (d()); use(x);"
        ));
        assert!(!check("for (;;) { if (c()) break; x = 1; } use(x);"));
        assert!(!check("for (; c(); x++) { if (d()) continue; x = 1; }"));
        assert!(check("while (x = next()) use(x); use(x);"));
        assert!(check("for (; (x = next()); ) use(x);"));
        assert!(check("x = 0; while (c()) { use(x); x = 1; }"));
        assert!(!check("while (c()) { use(x); x = 1; }"));
    }

    #[test]
    fn writes_inside_operands_that_always_run() {
        assert!(check("while ((x = next()) !== null) use(x); use(x);"));
        assert!(check("while (a(), (x = next()) < n) use(x);"));
        assert!(check("if (n < (x = next())) use(x);"));
        // The left operand runs first, so it reads before the write.
        assert!(!check("if (x < (x = next())) use(x);"));
        // A logical operator's right side may be skipped.
        assert!(!check("if (ok() && (x = next())) {} use(x);"));
        assert!(!check("if (a() || (x = next()) !== null) {} use(x);"));
        assert!(check("if ((x = next()) || fallback()) use(x);"));
    }

    #[test]
    fn labeled_break_and_switch() {
        assert!(!check("done: { if (c()) break done; x = 1; } use(x);"));
        assert!(!check("switch (k) { case 1: x = 1; break; } use(x);"));
        assert!(check("switch (k) { case 1: x = 1; use(x); }"));
        assert!(check("done: { x = 1; if (c()) break done; } use(x);"));
        // With `default`, every way out of the switch passes a write.
        assert!(check(
            "switch (k) { case 1: x = 1; break; case 2: prep(); case 3: x = 3; break; default: x = 0; } use(x);"
        ));
        assert!(!check(
            "switch (k) { case 1: x = 1; break; case 2: break; default: x = 0; } use(x);"
        ));
        assert!(!check("switch (k) { case 1: x = 1; break; } use(x);"));
        // A `break` in an unmodeled statement could skip the write.
        assert!(!check(
            "switch (k) { default: with (o) { if (c) break; } x = 1; } use(x);"
        ));
    }

    #[test]
    fn try_paths() {
        assert!(!check("try { risky(); x = 1; } catch (e) {} use(x);"));
        assert!(check("try { x = 1; } catch (e) { x = 2; } use(x);"));
        assert!(check("try { risky(); } finally { x = 1; } use(x);"));
        assert!(!check("try { x = risky(); } catch (e) { use(x); }"));
    }

    #[test]
    fn nested_functions_and_hoisting() {
        assert!(!check("peek(); x = 1; function peek() { use(x); }"));
        assert!(!check("const get = () => x; x = 1;"));
        assert!(check("x = 1; const get = () => x;"));
    }

    #[test]
    fn evaluation_order_inside_expressions() {
        assert!(check("out.push(x = make()); use(x);"));
        assert!(check("c() ? (x = 1) : (x = 2); use(x);"));
        assert!(!check("c() ? (x = 1) : 0; use(x);"));
        assert!(check("(x = make()).init(x);"));
        assert!(check("if (!(x = next())) return; use(x);"));
        assert!(check(
            "if ((x = a.b) == null || (x = x.c) == null) return; use(x);"
        ));
        assert!(check("sink({ flag: (x = load()) != null && x, last: x });"));
        assert!(check("f(a, (x = g(), x++, x));"));
        // The callee and earlier arguments run before the write.
        assert!(!check("f(x, x = 1);"));
        assert!(!check("x.m(x = 1);"));
        assert!(!check("o[x] = (x = 1);"));
        // A compound assignment or destructuring target reads.
        assert!(!check("[x] = pair; use(x);"));
    }

    #[test]
    fn redeclaration_with_a_value_writes() {
        assert!(check("var x = 1; use(x);"));
        assert!(!check("var y = x; x = 1;"));
    }
}
