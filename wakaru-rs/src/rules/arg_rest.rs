use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrowExpr, BindingIdent, BlockStmt, Callee, Expr, Function, Ident, MemberExpr, MemberProp,
    Param, Pat, RestPat, Stmt, VarDeclOrExpr,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

/// Replaces `arguments[N]` / `arguments.length` patterns in parameter-less functions
/// with a rest parameter `...args` and rewrites all accesses to use `args`.
///
/// Only fires when:
/// - The function has no formal parameters (empty param list)
/// - The function does not already have a rest parameter
/// - All `arguments` usages are via subscript (`arguments[expr]`) or `.length`
pub struct ArgRest;

impl VisitMut for ArgRest {
    fn visit_mut_function(&mut self, func: &mut Function) {
        // Recurse first so inner functions are processed independently
        func.visit_mut_children_with(self);

        // Skip if already has rest params
        if func.params.iter().any(|p| matches!(p.pat, Pat::Rest(_))) {
            return;
        }

        // Only handle parameter-less functions: when formal params exist, arguments[i]
        // for i < params.len() refers to the named param, not the rest array.
        if !func.params.is_empty() {
            return;
        }

        let Some(body) = &func.body else { return };

        let mut checker = ArgumentsChecker::default();
        body.visit_with(&mut checker);

        if !checker.has_any || checker.has_unsafe {
            return;
        }

        // Use the copy variable's name when a Babel copy loop is present; it is
        // already unique within the scope and avoids any naming conflict.
        let rest_name: Atom = detect_copy_var_name(body).unwrap_or_else(|| "args".into());
        func.params.push(make_rest_param(rest_name.clone()));

        // Rewrite `arguments` → `args` in the body
        if let Some(body) = &mut func.body {
            body.visit_mut_with(&mut ArgumentsRewriter { name: rest_name });
        }
    }
}

/// Scan `body` for the Babel rest-args copy pattern **before** `arguments` is rewritten.
/// Returns the copy variable's name (e.g. `i`, `r`, `t`) so it can be reused as the
/// rest param — this avoids any naming conflicts because minified copy vars are already
/// unique within their enclosing scope.
///
/// Pattern matched (3-declarator for-init, `arguments.length` as source):
/// ```text
/// for (var len = arguments.length, copy = Array(len), idx = 0; …) …
/// ```
fn detect_copy_var_name(body: &BlockStmt) -> Option<Atom> {
    body.stmts.iter().find_map(|stmt| {
        let Stmt::For(for_stmt) = stmt else { return None };
        let Some(VarDeclOrExpr::VarDecl(init)) = &for_stmt.init else { return None };
        if init.decls.len() != 3 {
            return None;
        }

        // Decl 0: len = arguments.length
        let d0 = &init.decls[0];
        let Pat::Ident(BindingIdent { id: len_id, .. }) = &d0.name else { return None };
        let Expr::Member(m) = d0.init.as_deref()? else { return None };
        let Expr::Ident(src) = m.obj.as_ref() else { return None };
        if src.sym != "arguments" {
            return None;
        }
        if !matches!(&m.prop, MemberProp::Ident(p) if p.sym == "length") {
            return None;
        }
        let len_sym = len_id.sym.clone();

        // Decl 1: copy = Array(len) or new Array(len)
        let d1 = &init.decls[1];
        let Pat::Ident(BindingIdent { id: copy_id, .. }) = &d1.name else { return None };

        let is_array_ctor = |sym: &Atom| sym == "Array";
        let one_len_arg = |args: &[swc_core::ecma::ast::ExprOrSpread]| -> bool {
            args.len() == 1
                && args[0].spread.is_none()
                && matches!(args[0].expr.as_ref(), Expr::Ident(id) if id.sym == len_sym)
        };

        match d1.init.as_deref()? {
            Expr::Call(call) => {
                let Callee::Expr(callee) = &call.callee else { return None };
                let Expr::Ident(id) = callee.as_ref() else { return None };
                if !is_array_ctor(&id.sym) || !one_len_arg(&call.args) {
                    return None;
                }
            }
            Expr::New(new_expr) => {
                let Expr::Ident(id) = new_expr.callee.as_ref() else { return None };
                if !is_array_ctor(&id.sym) {
                    return None;
                }
                let args = new_expr.args.as_deref().unwrap_or(&[]);
                if !one_len_arg(args) {
                    return None;
                }
            }
            _ => return None,
        }

        Some(copy_id.sym.clone())
    })
}

fn make_rest_param(name: Atom) -> Param {
    Param {
        span: DUMMY_SP,
        decorators: vec![],
        pat: Pat::Rest(RestPat {
            span: DUMMY_SP,
            dot3_token: DUMMY_SP,
            arg: Box::new(Pat::Ident(BindingIdent {
                id: Ident::new_no_ctxt(name, DUMMY_SP),
                type_ann: None,
            })),
            type_ann: None,
        }),
    }
}

// ============================================================
// Visitor: classify all `arguments` usages as safe or unsafe
// ============================================================

#[derive(Default)]
struct ArgumentsChecker {
    has_any: bool,
    has_unsafe: bool,
}

impl Visit for ArgumentsChecker {
    fn visit_member_expr(&mut self, expr: &MemberExpr) {
        if is_arguments_ident(&expr.obj) {
            self.has_any = true;
            match &expr.prop {
                // arguments[expr] — any subscript access is safe; the rest array
                // supports arbitrary indexing the same way.
                MemberProp::Computed(_) => {}
                // arguments.length — safe; rest array has .length
                MemberProp::Ident(i) if i.sym == "length" => {}
                // arguments.callee, arguments.anything_else — unsafe
                _ => {
                    self.has_unsafe = true;
                }
            }
            // Don't recurse: we've handled the `arguments` object reference and
            // don't want visit_ident to fire for the inner `arguments` ident.
            return;
        }
        expr.visit_children_with(self);
    }

    fn visit_ident(&mut self, id: &Ident) {
        // Any bare `arguments` reference that wasn't caught as a safe member
        // access above (e.g. passed as a value, spread, etc.) is unsafe.
        if id.sym == "arguments" {
            self.has_any = true;
            self.has_unsafe = true;
        }
    }

    // Don't descend into nested functions — they have their own `arguments`
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
}

fn is_arguments_ident(expr: &Expr) -> bool {
    matches!(expr, Expr::Ident(id) if id.sym == "arguments")
}

// ============================================================
// VisitMut: rewrite `arguments` → rest param name in member exprs
// ============================================================

struct ArgumentsRewriter {
    name: Atom,
}

impl VisitMut for ArgumentsRewriter {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Member(member) = expr {
            if is_arguments_ident(&member.obj) {
                *member.obj = Expr::Ident(Ident::new_no_ctxt(self.name.clone(), DUMMY_SP));
                // Don't recurse — we've already rewritten this node
                return;
            }
        }
        expr.visit_mut_children_with(self);
    }

    // Don't descend into nested functions
    fn visit_mut_function(&mut self, _: &mut Function) {}
    fn visit_mut_arrow_expr(&mut self, _: &mut ArrowExpr) {}
}
