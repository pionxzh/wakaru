use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, BlockStmtOrExpr, Expr, FnExpr, Function, Ident, Pat, ReturnStmt, Stmt,
    ThisExpr,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

pub struct ArrowFunction;

impl VisitMut for ArrowFunction {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Fn(fn_expr) = expr {
            if let Some(arrow) = try_convert_to_arrow(fn_expr) {
                *expr = Expr::Arrow(arrow);
            }
        }
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        arrow.visit_mut_children_with(self);

        // Simplify block body `{ return expr; }` to expression body `expr`
        // This makes the rule idempotent with UnCurlyBraces which expands
        // expression bodies to block bodies.
        simplify_arrow_body(arrow);
    }
}

/// If the arrow has body `{ return expr; }`, simplify to expression body `expr`.
/// Does not simplify if the return value is an object literal (ambiguous with block).
fn simplify_arrow_body(arrow: &mut ArrowExpr) {
    let BlockStmtOrExpr::BlockStmt(block) = arrow.body.as_ref() else {
        return;
    };

    if block.stmts.len() != 1 {
        return;
    }

    let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = &block.stmts[0] else {
        return;
    };

    // Don't simplify if the return value is an object literal
    // (would be ambiguous with block statement)
    if matches!(arg.as_ref(), Expr::Object(_)) {
        return;
    }

    let expr = arg.clone();
    *arrow.body = BlockStmtOrExpr::Expr(expr);
}

fn try_convert_to_arrow(fn_expr: &mut FnExpr) -> Option<ArrowExpr> {
    let func = &fn_expr.function;

    // Don't convert generators
    if func.is_generator {
        return None;
    }

    // Must have a body
    let body = func.body.as_ref()?;

    // Check for this or arguments usage (don't recurse into nested functions)
    let mut checker = HasThisOrArguments(false);
    body.visit_with(&mut checker);
    if checker.0 {
        return None;
    }

    // Check if the function name is used inside (self-referential)
    if let Some(ident) = &fn_expr.ident {
        let mut name_checker = HasIdentRef { sym: ident.sym.clone(), found: false };
        body.visit_with(&mut name_checker);
        if name_checker.found {
            return None;
        }
    }

    // Convert params: Vec<Param> -> Vec<Pat>
    let params: Vec<Pat> = fn_expr.function.params.iter()
        .map(|p| p.pat.clone())
        .collect();

    // Build the arrow body
    let arrow_body = build_arrow_body(&fn_expr.function);

    Some(ArrowExpr {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        params,
        body: Box::new(arrow_body),
        is_async: fn_expr.function.is_async,
        is_generator: false,
        type_params: fn_expr.function.type_params.clone(),
        return_type: fn_expr.function.return_type.clone(),
    })
}

/// Build the arrow body:
/// - If body is `{ return expr; }` → `expr` (expression body)
/// - Otherwise → keep as block body
fn build_arrow_body(func: &Function) -> BlockStmtOrExpr {
    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return BlockStmtOrExpr::BlockStmt(Default::default()),
    };

    // Single statement that is `return expr;`
    if body.stmts.len() == 1 {
        if let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = &body.stmts[0] {
            // Don't simplify if the return value is an object literal
            // (would be ambiguous with block statement)
            if !matches!(arg.as_ref(), Expr::Object(_)) {
                return BlockStmtOrExpr::Expr(arg.clone());
            }
        }
    }

    BlockStmtOrExpr::BlockStmt(body.clone())
}

// ============================================================
// Visitor: check for `this` or `arguments` (not in nested fns)
// ============================================================

struct HasThisOrArguments(bool);

impl Visit for HasThisOrArguments {
    fn visit_this_expr(&mut self, _: &ThisExpr) {
        self.0 = true;
    }

    fn visit_ident(&mut self, id: &Ident) {
        if id.sym == "arguments" {
            self.0 = true;
        }
    }

    // Don't recurse into nested functions — they have their own this/arguments
    fn visit_function(&mut self, _: &Function) {}

    // Don't recurse into arrow expressions either (they capture this from outer,
    // but they don't have their own `arguments`)
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
}

// ============================================================
// Visitor: check if a given ident name is referenced in body
// ============================================================

struct HasIdentRef {
    sym: swc_core::atoms::Atom,
    found: bool,
}

impl Visit for HasIdentRef {
    fn visit_ident(&mut self, id: &Ident) {
        if id.sym == self.sym {
            self.found = true;
        }
    }
}
