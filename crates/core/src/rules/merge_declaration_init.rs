//! Merge a bare declaration with its first assignment.
//!
//! Transpilers and minifiers frequently hoist declarations and then assign them
//! later, producing split forms like:
//!
//! ```js
//! let response;
//! response = await fetch_user(id);
//! ```
//!
//! This rule folds the bare `let`/`var` declaration into its first
//! statement-level assignment **in the same statement list**. In standard mode,
//! it only folds inert right-hand sides. Aggressive mode also folds broader
//! generated-code shapes such as `let response; response = await fetch_user(id);`.
//!
//! It runs late (after `UnDestructuring`/`SmartInline`) so it does not disturb
//! the assignment-form temporaries those rules rely on. A consequence is that
//! the merged binding keeps its `let` kind: `VarDeclToLetConst` has already run,
//! so a merged single-assignment binding is not promoted to `const`. Promoting
//! it would require either re-running const analysis here or making
//! `UnDestructuring` robust to declaration-form temps (which would let this rule
//! move earlier).
//!
//! ## Safety
//!
//! The merge only fires when these structural guards pass:
//! - the declaration is a single bare `let`/`var` binding (no initializer);
//! - the first statement-level assignment to that binding is a simple `=` in the
//!   same statement list (not nested in a branch/loop/closure);
//! - only other bare declarations appear between the declaration and that
//!   assignment (calls, branches, function declarations, and initialized
//!   declarations may observe declaration timing or closure state);
//! - the assignment's right-hand side does not reference the binding itself.
//!
//! Standard mode additionally requires an inert right-hand side so the merge
//! cannot change whether the binding is initialized while evaluating that RHS
//! (for example through a call, `await`, or direct `eval`). Aggressive mode
//! relaxes that RHS guard for readability.
//!
//! Matching is by [`BindingId`] (name + `SyntaxContext`), so same-named bindings
//! in different scopes are never conflated.

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, Decl, EmptyStmt, Expr, Ident, Pat, SimpleAssignTarget, Stmt, VarDecl,
    VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::{decl_utils::BindingId, RewriteLevel};

pub struct MergeDeclarationInit {
    level: RewriteLevel,
}

impl MergeDeclarationInit {
    pub fn new(level: RewriteLevel) -> Self {
        Self { level }
    }
}

impl Default for MergeDeclarationInit {
    fn default() -> Self {
        Self::new(RewriteLevel::Standard)
    }
}

impl VisitMut for MergeDeclarationInit {
    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        merge_stmt_list(stmts, self.level);
    }
}

fn merge_stmt_list(stmts: &mut Vec<Stmt>, level: RewriteLevel) {
    let mut i = 0;
    while i < stmts.len() {
        let Some(id) = bare_decl_binding(&stmts[i]) else {
            i += 1;
            continue;
        };
        let assignment =
            (i + 1..stmts.len()).find(|&j| assignment_target(&stmts[j]) == Some(id.clone()));
        let Some(j) = assignment else {
            i += 1;
            continue;
        };
        let between = &stmts[i + 1..j];
        if !only_bare_declarations(between)
            || slice_references(between, &id)
            || assignment_rhs_references(&stmts[j], &id)
            || (level < RewriteLevel::Aggressive
                && !assignment_rhs_is_standard_safe(&stmts[j]).unwrap_or(false))
        {
            i += 1;
            continue;
        }

        let rhs = take_assignment_rhs(&mut stmts[j]);
        let mut var = take_var_decl(&mut stmts[i]);
        var.decls[0].init = Some(rhs);
        stmts[j] = Stmt::Decl(Decl::Var(var));
        stmts.remove(i);
        // Elements shifted left by one; re-examine the same index. The merged
        // declaration now has an initializer, so it won't be matched again.
    }
}

fn only_bare_declarations(stmts: &[Stmt]) -> bool {
    stmts.iter().all(|stmt| bare_decl_binding(stmt).is_some())
}

/// The binding of a bare `let`/`var X;` (single declarator, no initializer).
fn bare_decl_binding(stmt: &Stmt) -> Option<BindingId> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.kind == VarDeclKind::Const || var.decls.len() != 1 {
        return None;
    }
    let declarator = &var.decls[0];
    if declarator.init.is_some() {
        return None;
    }
    let Pat::Ident(binding) = &declarator.name else {
        return None;
    };
    Some((binding.id.sym.clone(), binding.id.ctxt))
}

/// The binding targeted by a statement-level simple assignment `X = expr;`.
fn assignment_target(stmt: &Stmt) -> Option<BindingId> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = &*expr_stmt.expr else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &assign.left else {
        return None;
    };
    Some((binding.id.sym.clone(), binding.id.ctxt))
}

fn assignment_rhs_references(stmt: &Stmt, id: &BindingId) -> bool {
    let Some(rhs) = assignment_rhs(stmt) else {
        return false;
    };
    let mut finder = RefFinder { id, found: false };
    rhs.visit_with(&mut finder);
    finder.found
}

fn assignment_rhs_is_standard_safe(stmt: &Stmt) -> Option<bool> {
    Some(expr_is_inert_initializer(assignment_rhs(stmt)?))
}

fn assignment_rhs(stmt: &Stmt) -> Option<&Expr> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = &*expr_stmt.expr else {
        return None;
    };
    Some(assign.right.as_ref())
}

fn expr_is_inert_initializer(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(_) | Expr::Fn(_) | Expr::Arrow(_) => true,
        Expr::Object(obj) => obj.props.is_empty(),
        Expr::Array(arr) => arr.elems.is_empty(),
        Expr::Paren(paren) => expr_is_inert_initializer(&paren.expr),
        _ => false,
    }
}

/// Take the right-hand side out of a statement known to be `X = expr;`.
fn take_assignment_rhs(stmt: &mut Stmt) -> Box<Expr> {
    let taken = std::mem::replace(stmt, Stmt::Empty(EmptyStmt { span: DUMMY_SP }));
    let Stmt::Expr(expr_stmt) = taken else {
        unreachable!("assignment_target guarantees an ExprStmt")
    };
    let Expr::Assign(assign) = *expr_stmt.expr else {
        unreachable!("assignment_target guarantees an AssignExpr")
    };
    assign.right
}

/// Take the boxed `VarDecl` out of a statement known to be a bare declaration.
fn take_var_decl(stmt: &mut Stmt) -> Box<VarDecl> {
    let taken = std::mem::replace(stmt, Stmt::Empty(EmptyStmt { span: DUMMY_SP }));
    let Stmt::Decl(Decl::Var(var)) = taken else {
        unreachable!("bare_decl_binding guarantees a VarDecl")
    };
    var
}

fn slice_references(stmts: &[Stmt], id: &BindingId) -> bool {
    let mut finder = RefFinder { id, found: false };
    for stmt in stmts {
        stmt.visit_with(&mut finder);
        if finder.found {
            return true;
        }
    }
    false
}

struct RefFinder<'a> {
    id: &'a BindingId,
    found: bool,
}

impl Visit for RefFinder<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == self.id.0 && ident.ctxt == self.id.1 {
            self.found = true;
        }
    }
}
