//! Whether a binding is written before it can be read, along every path
//! through a run of statements.
//!
//! The walk follows JavaScript's structured control flow on the AST; there
//! is no control-flow graph. It tracks one fact, "the binding has been
//! written on this path", which only ever goes from false to true. That
//! makes every join conservative to compute:
//! - after `if`/`else`, written only if both branches wrote it;
//! - a loop, `switch`, or labeled statement is left with the state it was
//!   entered with (after a `while`/`for` test, which runs before every exit),
//!   because a `break` can leave it before any write inside it runs, and
//!   every path inside starts from that state; a `try` keeps a write only if
//!   both its block and its handler, or its finalizer, make it;
//! - a `for` update and a `do`/`while` test are checked with the state before
//!   the body, since a `continue` can reach them before the body's writes;
//! - `return`, `throw`, `break`, and `continue` end their path, so the
//!   statements after them in the same list are not on it.
//!
//! A write counts only in the plain forms `x = value` and `var x = value`,
//! with a value that does not mention the binding. The `x = value` form
//! counts as a statement, an `if`/loop test, or an operand that always runs
//! first within one: a sequence element, or either side of a binary operator
//! other than `&&`, `||`, and `??`, whose right side may be skipped and only
//! reads (`(x = next()) !== null`, `a, (x = f()) < n`). Any other mention
//! — inside a nested function or class, in a compound assignment, in a
//! destructuring pattern, or under `with` — counts as a read where it
//! appears, and a mention inside a function declaration counts as a read
//! before any write, because the declaration is hoisted and can be called
//! from anywhere in its scope.
//!
//! The answer covers one run of the statements. A closure created during
//! the run and called during a later one is the caller's concern.

use swc_core::ecma::ast::{
    AssignOp, AssignTarget, Decl, Expr, Ident, Pat, SimpleAssignTarget, Stmt, VarDeclOrExpr,
    VarDeclarator,
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
    let mut walk = Walk {
        binding,
        read_before_write: false,
    };
    let mut written = false;
    for declarator in declarators {
        written = walk.declarator(declarator, written);
    }
    walk.stmts(stmts, written);
    !walk.read_before_write
}

struct Walk<'a> {
    binding: &'a BindingId,
    read_before_write: bool,
}

impl Walk<'_> {
    fn stmts(&mut self, stmts: &[Stmt], mut written: bool) -> bool {
        for stmt in stmts {
            written = self.stmt(stmt, written);
        }
        written
    }

    fn stmt(&mut self, stmt: &Stmt, written: bool) -> bool {
        match stmt {
            Stmt::Expr(expr_stmt) => self.effect(&expr_stmt.expr, written),
            Stmt::Block(block) => self.stmts(&block.stmts, written),
            Stmt::Decl(Decl::Var(var)) => var
                .decls
                .iter()
                .fold(written, |written, decl| self.declarator(decl, written)),
            Stmt::Decl(Decl::Fn(function)) => {
                self.read(&function.function, false);
                written
            }
            Stmt::If(if_stmt) => {
                let written = self.effect(&if_stmt.test, written);
                let cons = self.stmt(&if_stmt.cons, written);
                let alt = match &if_stmt.alt {
                    Some(alt) => self.stmt(alt, written),
                    None => written,
                };
                cons && alt
            }
            Stmt::Return(ret) => {
                self.read(&ret.arg, written);
                true
            }
            Stmt::Throw(throw) => {
                self.read(&throw.arg, written);
                true
            }
            Stmt::Break(_) | Stmt::Continue(_) => true,
            Stmt::Labeled(labeled) => {
                self.stmt(&labeled.body, written);
                written
            }
            Stmt::While(while_stmt) => {
                // The test runs before every pass of the body and before
                // every exit, a `break` included.
                let written = self.effect(&while_stmt.test, written);
                self.stmt(&while_stmt.body, written);
                written
            }
            Stmt::DoWhile(do_while) => {
                self.stmt(&do_while.body, written);
                self.read(&do_while.test, written);
                written
            }
            Stmt::For(for_stmt) => {
                let written = match &for_stmt.init {
                    Some(VarDeclOrExpr::VarDecl(var)) => var
                        .decls
                        .iter()
                        .fold(written, |written, decl| self.declarator(decl, written)),
                    Some(VarDeclOrExpr::Expr(expr)) => self.effect(expr, written),
                    None => written,
                };
                // As for `while`, the test runs before every pass and exit;
                // the update runs after a test, but a `continue` can reach it
                // before the body's writes.
                let written = match &for_stmt.test {
                    Some(test) => self.effect(test, written),
                    None => written,
                };
                self.stmt(&for_stmt.body, written);
                if let Some(update) = &for_stmt.update {
                    self.read(update, written);
                }
                written
            }
            Stmt::ForIn(for_in) => {
                self.read(&for_in.right, written);
                self.read(&for_in.left, written);
                self.stmt(&for_in.body, written);
                written
            }
            Stmt::ForOf(for_of) => {
                self.read(&for_of.right, written);
                self.read(&for_of.left, written);
                self.stmt(&for_of.body, written);
                written
            }
            Stmt::Switch(switch) => {
                self.read(&switch.discriminant, written);
                // A case is entered either directly or by falling through from
                // the one before; both paths start from at least `written`.
                for case in &switch.cases {
                    self.read(&case.test, written);
                    self.stmts(&case.cons, written);
                }
                written
            }
            Stmt::Try(try_stmt) => {
                let block = self.stmts(&try_stmt.block.stmts, written);
                // A throw can reach the handler before any write in the block.
                let handler = match &try_stmt.handler {
                    Some(handler) => {
                        self.read(&handler.param, written);
                        self.stmts(&handler.body.stmts, written)
                    }
                    None => true,
                };
                let finalizer = match &try_stmt.finalizer {
                    Some(finalizer) => self.stmts(&finalizer.stmts, written),
                    None => false,
                };
                finalizer || (block && handler) || written
            }
            // `with`, class declarations, and anything else: every mention is
            // a read where it appears.
            other => {
                self.read(other, written);
                written
            }
        }
    }

    /// `var x = value` writes; any other declarator only reads.
    fn declarator(&mut self, decl: &VarDeclarator, written: bool) -> bool {
        self.read(&decl.init, written);
        match &decl.name {
            Pat::Ident(binding) if ident_matches_binding(&binding.id, self.binding) => {
                decl.init.is_some() || written
            }
            name => {
                self.read(name, written);
                written
            }
        }
    }

    /// An expression evaluated for its effect: a statement, a sequence
    /// element, or a test.
    fn effect(&mut self, expr: &Expr, written: bool) -> bool {
        match expr {
            Expr::Paren(paren) => self.effect(&paren.expr, written),
            Expr::Seq(seq) => seq
                .exprs
                .iter()
                .fold(written, |written, expr| self.effect(expr, written)),
            // Both operands always run, left first, except that a logical
            // operator's right side may not run, so it can only read.
            Expr::Bin(bin) => {
                let written = self.effect(&bin.left, written);
                if bin.op.may_short_circuit() {
                    self.read(&bin.right, written);
                    written
                } else {
                    self.effect(&bin.right, written)
                }
            }
            Expr::Assign(assign)
                if assign.op == AssignOp::Assign
                    && matches!(
                        &assign.left,
                        AssignTarget::Simple(SimpleAssignTarget::Ident(target))
                            if ident_matches_binding(&target.id, self.binding)
                    ) =>
            {
                self.read(&assign.right, written);
                true
            }
            other => {
                self.read(other, written);
                written
            }
        }
    }

    /// Record a read before any write if `node` mentions the binding while it
    /// is still unwritten.
    fn read<N: for<'m> VisitWith<Mentions<'m>>>(&mut self, node: &N, written: bool) {
        if written || self.read_before_write {
            return;
        }
        let mut mentions = Mentions {
            binding: self.binding,
            found: false,
        };
        node.visit_with(&mut mentions);
        self.read_before_write = mentions.found;
    }
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
        assert!(!check("do { x = 1; } while (c()); use(x);"));
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
    fn labeled_break_and_switch_can_skip_writes() {
        assert!(!check("done: { if (c()) break done; x = 1; } use(x);"));
        assert!(!check("switch (k) { case 1: x = 1; break; } use(x);"));
        assert!(check("switch (k) { case 1: x = 1; use(x); }"));
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
}
