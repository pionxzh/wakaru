use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrayPat, BinExpr, BinaryOp, BindingIdent, Decl, Expr, ForHead, ForOfStmt, Ident, Lit,
    MemberExpr, MemberProp, Pat, Stmt, UpdateExpr, UpdateOp, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::RewriteLevel;

/// Convert TypeScript/Babel array-index downlevel `for` loops back to `for...of`:
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
    if init_decl.decls.is_empty() || init_decl.decls.len() > 2 {
        return None;
    }
    let idx_decl = &init_decl.decls[0];

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

    let IndexedIterable {
        access_obj,
        iterable,
        temp_sym,
    } = extract_indexed_iterable(init_decl, right)?;

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

    // --- Body: first statement must declare the element from `arr[i]` ---
    let Stmt::Block(block) = &*for_stmt.body else {
        return None;
    };
    if block.stmts.is_empty() {
        return None;
    }
    let element = extract_loop_element(&block.stmts, &access_obj, idx_sym)?;

    // --- Safety: generated index/temp bindings must not be used in remaining body statements ---
    let remaining_body = &block.stmts[element.consumed_stmts..];
    for body_stmt in remaining_body {
        if stmt_uses_ident(body_stmt, idx_sym) {
            return None;
        }
        if temp_sym
            .as_ref()
            .is_some_and(|sym| stmt_uses_ident(body_stmt, sym))
        {
            return None;
        }
        if element
            .temp_sym
            .as_ref()
            .is_some_and(|sym| stmt_uses_ident(body_stmt, sym))
        {
            return None;
        }
    }

    // Use `let` if the element variable is reassigned in the loop body, `const` otherwise
    let elem_is_reassigned = remaining_body.iter().any(|stmt| {
        element
            .bindings
            .iter()
            .any(|sym| stmt_assigns_ident(stmt, sym))
    });
    let elem_kind = if element.kind == VarDeclKind::Var {
        VarDeclKind::Var
    } else if elem_is_reassigned {
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
            name: element.pat,
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
        right: iterable,
        body: Box::new(new_body),
    })
}

struct IndexedIterable {
    access_obj: Box<Expr>,
    iterable: Box<Expr>,
    temp_sym: Option<Atom>,
}

struct LoopElement {
    pat: Pat,
    bindings: Vec<Atom>,
    kind: VarDeclKind,
    temp_sym: Option<Atom>,
    consumed_stmts: usize,
}

fn extract_indexed_iterable(init_decl: &VarDecl, length_expr: &Expr) -> Option<IndexedIterable> {
    let length_obj = extract_length_obj(length_expr)?;

    match init_decl.decls.as_slice() {
        // TypeScript: `let i = 0, arr = iterable; i < arr.length; i++`
        [_, arr_decl] => {
            let Pat::Ident(arr_binding) = &arr_decl.name else {
                return None;
            };
            let arr_sym = &arr_binding.id.sym;
            if !is_ident(&length_obj, arr_sym) {
                return None;
            }
            let iterable = arr_decl.init.clone()?;
            Some(IndexedIterable {
                access_obj: Box::new(length_obj),
                iterable,
                temp_sym: Some(arr_sym.clone()),
            })
        }
        // Babel `iterableIsArray`: `let i = 0; i < items.length; i++`
        [idx_decl] => {
            // The direct-array form only has the index declaration in `init`.
            idx_decl.init.as_ref()?;
            Some(IndexedIterable {
                access_obj: Box::new(length_obj.clone()),
                iterable: Box::new(length_obj),
                temp_sym: None,
            })
        }
        _ => None,
    }
}

fn extract_length_obj(expr: &Expr) -> Option<Expr> {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return None;
    };
    let MemberProp::Ident(length_prop) = prop else {
        return None;
    };
    if length_prop.sym.as_ref() != "length" {
        return None;
    }
    Some(*obj.clone())
}

fn extract_loop_element(stmts: &[Stmt], access_obj: &Expr, idx_sym: &Atom) -> Option<LoopElement> {
    let first_decl = stmt_as_single_var_decl(stmts.first()?)?;
    let first = &first_decl.decls[0];
    let Pat::Ident(temp_binding) = &first.name else {
        return None;
    };
    let temp_sym = &temp_binding.id.sym;
    let first_init = first.init.as_ref()?;
    if !is_index_access(first_init, access_obj, idx_sym) {
        return None;
    }

    let mut elems = Vec::new();
    let mut bindings = Vec::new();
    let mut consumed_stmts = 1;

    for stmt in &stmts[1..] {
        let Some(decl) = stmt_as_single_var_decl(stmt) else {
            break;
        };
        let declarator = &decl.decls[0];
        let expected_index = elems.len() as f64;
        let Pat::Ident(binding) = &declarator.name else {
            break;
        };
        let Some(init) = declarator.init.as_ref() else {
            break;
        };
        if !is_numeric_index_access(init, temp_sym, expected_index) {
            break;
        }

        elems.push(Some(Pat::Ident(BindingIdent {
            id: binding.id.clone(),
            type_ann: binding.type_ann.clone(),
        })));
        bindings.push(binding.id.sym.clone());
        consumed_stmts += 1;
    }

    if elems.is_empty() {
        return Some(LoopElement {
            pat: Pat::Ident(temp_binding.clone()),
            bindings: vec![temp_binding.id.sym.clone()],
            kind: first_decl.kind,
            temp_sym: None,
            consumed_stmts,
        });
    }

    Some(LoopElement {
        pat: Pat::Array(ArrayPat {
            span: DUMMY_SP,
            elems,
            optional: false,
            type_ann: None,
        }),
        bindings,
        kind: first_decl.kind,
        temp_sym: Some(temp_sym.clone()),
        consumed_stmts,
    })
}

fn stmt_as_single_var_decl(stmt: &Stmt) -> Option<&VarDecl> {
    let Stmt::Decl(Decl::Var(decl)) = stmt else {
        return None;
    };
    (decl.decls.len() == 1).then_some(decl)
}

fn is_index_access(expr: &Expr, obj_expr: &Expr, idx_sym: &Atom) -> bool {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return false;
    };
    if !same_ident_expr(obj, obj_expr) {
        return false;
    }
    let MemberProp::Computed(computed) = prop else {
        return false;
    };
    is_ident(&computed.expr, idx_sym)
}

fn is_numeric_index_access(expr: &Expr, obj_sym: &Atom, index: f64) -> bool {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return false;
    };
    if !is_ident(obj, obj_sym) {
        return false;
    }
    let MemberProp::Computed(computed) = prop else {
        return false;
    };
    matches!(&*computed.expr, Expr::Lit(Lit::Num(num)) if num.value == index)
}

fn is_zero(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(swc_core::ecma::ast::Lit::Num(n)) if n.value == 0.0)
}

fn is_ident(expr: &Expr, sym: &Atom) -> bool {
    matches!(expr, Expr::Ident(id) if &id.sym == sym)
}

fn same_ident_expr(left: &Expr, right: &Expr) -> bool {
    match (left, right) {
        (Expr::Ident(left), Expr::Ident(right)) => left.sym == right.sym && left.ctxt == right.ctxt,
        _ => false,
    }
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
