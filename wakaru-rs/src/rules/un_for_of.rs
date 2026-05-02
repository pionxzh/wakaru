use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    BinExpr, BinaryOp, Decl, Expr, ForHead, ForOfStmt, Ident, MemberExpr, MemberProp, Pat, Stmt,
    UpdateExpr, UpdateOp, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::RewriteLevel;

/// Convert TypeScript-style downlevel `for` loops back to `for...of`:
///
/// ```js
/// for (let i = 0, arr = expr; i < arr.length; i++) {
///     const elem = arr[i];
///     // body...
/// }
/// // →
/// for (const elem of expr) {
///     // body...
/// }
/// ```
pub struct UnForOf {
    level: RewriteLevel,
}

impl UnForOf {
    pub fn new(level: RewriteLevel) -> Self {
        Self { level }
    }
}

impl Default for UnForOf {
    fn default() -> Self {
        Self::new(RewriteLevel::Standard)
    }
}

impl VisitMut for UnForOf {
    fn visit_mut_stmt(&mut self, stmt: &mut Stmt) {
        if self.level < RewriteLevel::Standard {
            return;
        }
        stmt.visit_mut_children_with(self);

        if let Some(for_of) = try_convert_for_of(stmt) {
            *stmt = Stmt::ForOf(for_of);
        }
    }
}

fn try_convert_for_of(stmt: &Stmt) -> Option<ForOfStmt> {
    let Stmt::For(for_stmt) = stmt else {
        return None;
    };

    // --- Init: `let i = 0, arr = <iterable>` ---
    let Some(swc_core::ecma::ast::VarDeclOrExpr::VarDecl(init_decl)) = &for_stmt.init else {
        return None;
    };
    if init_decl.decls.len() != 2 {
        return None;
    }
    let idx_decl = &init_decl.decls[0];
    let arr_decl = &init_decl.decls[1];

    // Index must be initialized to 0
    let Pat::Ident(idx_binding) = &idx_decl.name else {
        return None;
    };
    let idx_sym = &idx_binding.id.sym;
    let Some(idx_init) = &idx_decl.init else {
        return None;
    };
    if !is_zero(idx_init) {
        return None;
    }

    // Arr must have an initializer (the iterable expression)
    let Pat::Ident(arr_binding) = &arr_decl.name else {
        return None;
    };
    let arr_sym = &arr_binding.id.sym;
    let Some(iterable) = &arr_decl.init else {
        return None;
    };

    // --- Test: `i < arr.length` ---
    let Some(test) = &for_stmt.test else {
        return None;
    };
    let Expr::Bin(BinExpr {
        op: BinaryOp::Lt,
        left,
        right,
        ..
    }) = &**test
    else {
        return None;
    };
    if !is_ident(left, idx_sym) {
        return None;
    }
    // right must be arr.length
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**right else {
        return None;
    };
    if !is_ident(obj, arr_sym) {
        return None;
    }
    let MemberProp::Ident(length_prop) = prop else {
        return None;
    };
    if length_prop.sym.as_ref() != "length" {
        return None;
    }

    // --- Update: `i++` ---
    let Some(update) = &for_stmt.update else {
        return None;
    };
    let Expr::Update(UpdateExpr {
        op: UpdateOp::PlusPlus,
        arg,
        ..
    }) = &**update
    else {
        return None;
    };
    if !is_ident(arg, idx_sym) {
        return None;
    }

    // --- Body: first statement must be `const elem = arr[i]` ---
    let Stmt::Block(block) = &*for_stmt.body else {
        return None;
    };
    if block.stmts.is_empty() {
        return None;
    }
    let Stmt::Decl(Decl::Var(elem_decl)) = &block.stmts[0] else {
        return None;
    };
    if elem_decl.decls.len() != 1 {
        return None;
    }
    let elem_declarator = &elem_decl.decls[0];
    let Pat::Ident(elem_binding) = &elem_declarator.name else {
        return None;
    };
    let elem_sym = &elem_binding.id.sym;
    let Some(elem_init) = &elem_declarator.init else {
        return None;
    };
    // elem_init must be arr[i]
    let Expr::Member(MemberExpr {
        obj: elem_obj,
        prop: elem_prop,
        ..
    }) = &**elem_init
    else {
        return None;
    };
    if !is_ident(elem_obj, arr_sym) {
        return None;
    }
    let MemberProp::Computed(computed) = elem_prop else {
        return None;
    };
    if !is_ident(&computed.expr, idx_sym) {
        return None;
    }

    // --- Safety: idx and arr must not be used in remaining body statements ---
    let remaining_body = &block.stmts[1..];
    for body_stmt in remaining_body {
        if stmt_uses_ident(body_stmt, idx_sym) || stmt_uses_ident(body_stmt, arr_sym) {
            return None;
        }
    }

    // Use `let` if the element variable is reassigned in the loop body, `const` otherwise
    let elem_is_reassigned = remaining_body.iter().any(|s| stmt_assigns_ident(s, elem_sym));
    let elem_kind = if elem_is_reassigned {
        VarDeclKind::Let
    } else {
        VarDeclKind::Const
    };

    // --- Build for...of ---
    let for_of_left = ForHead::VarDecl(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: elem_kind,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(elem_binding.clone()),
            init: None,
            definite: false,
        }],
    }));

    let new_body = Stmt::Block(swc_core::ecma::ast::BlockStmt {
        span: DUMMY_SP,
        ctxt: Default::default(),
        stmts: remaining_body.to_vec(),
    });

    Some(ForOfStmt {
        span: for_stmt.span,
        is_await: false,
        left: for_of_left,
        right: iterable.clone(),
        body: Box::new(new_body),
    })
}

fn is_zero(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(swc_core::ecma::ast::Lit::Num(n)) if n.value == 0.0)
}

fn is_ident(expr: &Expr, sym: &Atom) -> bool {
    matches!(expr, Expr::Ident(id) if &id.sym == sym)
}

/// Check if a statement assigns to an identifier by name (e.g. `elem = ...`).
fn stmt_assigns_ident(stmt: &Stmt, sym: &Atom) -> bool {
    use swc_core::ecma::ast::{AssignTarget, SimpleAssignTarget};
    use swc_core::ecma::visit::Visit;

    struct AssignFinder {
        sym: Atom,
        found: bool,
    }

    impl Visit for AssignFinder {
        fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
            if let AssignTarget::Simple(SimpleAssignTarget::Ident(id)) = &assign.left {
                if id.sym == self.sym {
                    self.found = true;
                }
            }
        }

        fn visit_update_expr(&mut self, update: &UpdateExpr) {
            if let Expr::Ident(id) = &*update.arg {
                if id.sym == self.sym {
                    self.found = true;
                }
            }
        }
    }

    let mut finder = AssignFinder {
        sym: sym.clone(),
        found: false,
    };
    finder.visit_stmt(stmt);
    finder.found
}

/// Check if a statement references an identifier by name.
fn stmt_uses_ident(stmt: &Stmt, sym: &Atom) -> bool {
    use swc_core::ecma::visit::Visit;

    struct IdentFinder {
        sym: Atom,
        found: bool,
    }

    impl Visit for IdentFinder {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym == self.sym {
                self.found = true;
            }
        }
    }

    let mut finder = IdentFinder {
        sym: sym.clone(),
        found: false,
    };
    finder.visit_stmt(stmt);
    finder.found
}
