use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, BindingIdent, BlockStmt, BlockStmtOrExpr, CallExpr, Callee, Decl, Expr,
    ExprOrSpread, Function, Ident, Lit, Pat, Param, Stmt, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnIife;

impl VisitMut for UnIife {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Call(call_expr) = expr {
            process_iife(call_expr);
        }
    }
}

fn process_iife(call: &mut CallExpr) {
    match &mut call.callee {
        Callee::Expr(callee_expr) => {
            match callee_expr.as_mut() {
                Expr::Fn(fn_expr) => {
                    process_fn_iife(&mut fn_expr.function, &mut call.args);
                }
                Expr::Arrow(arrow_expr) => {
                    process_arrow_iife(arrow_expr, &mut call.args);
                }
                Expr::Paren(paren) => {
                    match paren.expr.as_mut() {
                        Expr::Fn(fn_expr) => {
                            process_fn_iife(&mut fn_expr.function, &mut call.args);
                        }
                        Expr::Arrow(arrow_expr) => {
                            process_arrow_iife(arrow_expr, &mut call.args);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn process_fn_iife(function: &mut Function, args: &mut Vec<ExprOrSpread>) {
    let Some(body) = &mut function.body else {
        return;
    };
    let uses_arguments = body_uses_arguments(&body.stmts);
    process_params_and_args(&mut function.params, args, body, uses_arguments);
}

fn process_arrow_iife(arrow: &mut ArrowExpr, args: &mut Vec<ExprOrSpread>) {
    let BlockStmtOrExpr::BlockStmt(body) = arrow.body.as_mut() else {
        return;
    };
    let uses_arguments = body_uses_arguments(&body.stmts);
    // Arrow functions use Vec<Pat> not Vec<Param>
    process_arrow_params_and_args(&mut arrow.params, args, body, uses_arguments);
}

fn process_params_and_args(
    params: &mut Vec<Param>,
    args: &mut Vec<ExprOrSpread>,
    body: &mut BlockStmt,
    uses_arguments: bool,
) {
    // (idx, old_name, new_name, new_ctxt)
    let mut renames: Vec<(usize, Atom, Atom, SyntaxContext)> = Vec::new();
    // (idx, param_name, param_ctxt, lit)
    let mut const_inserts: Vec<(usize, Atom, SyntaxContext, Lit)> = Vec::new();

    let param_count = params.len();
    let arg_count = args.len();

    for i in 0..param_count {
        let param_id = match pat_ident(&params[i].pat) {
            Some(id) => id,
            None => continue,
        };
        let param_sym = param_id.sym.clone();
        let param_ctxt = param_id.ctxt;

        // Only process single-char params
        if param_sym.len() != 1 {
            continue;
        }

        if i >= arg_count {
            continue;
        }

        let arg_expr = &args[i].expr;

        match arg_expr.as_ref() {
            Expr::Ident(ident) => {
                // Arg is an identifier - rename param if arg name is longer
                if ident.sym.len() > 1 && ident.sym != param_sym {
                    renames.push((i, param_sym, ident.sym.clone(), ident.ctxt));
                }
            }
            Expr::Lit(lit) => {
                // Arg is a literal - inline as const if no `arguments` usage
                if !uses_arguments {
                    const_inserts.push((i, param_sym, param_ctxt, lit.clone()));
                }
            }
            _ => {}
        }
    }

    // Apply renames (just rename in-place, no removal needed)
    for (i, old_sym, new_sym, new_ctxt) in &renames {
        // Rename the param itself, use the arg's ctxt to avoid conflicts
        if let Pat::Ident(bi) = &mut params[*i].pat {
            bi.id.sym = new_sym.clone();
            bi.id.ctxt = *new_ctxt;
        }
        // Rename all uses in the body
        let mut renamer = RenameIdent {
            old_sym: old_sym.clone(),
            new_sym: new_sym.clone(),
            new_ctxt: *new_ctxt,
        };
        body.visit_mut_with(&mut renamer);
    }

    // Process const_inserts in reverse index order so removal doesn't shift indices
    const_inserts.sort_by(|a, b| b.0.cmp(&a.0));

    let mut const_stmts: Vec<(usize, Stmt)> = Vec::new();

    for (i, param_sym, param_ctxt, lit) in const_inserts {
        // Remove the param and its corresponding arg
        params.remove(i);
        args.remove(i);

        // Create: const param_sym = lit; (using param's original ctxt)
        let const_stmt = make_const_decl(param_sym, param_ctxt, lit);
        const_stmts.push((i, const_stmt));
    }

    // Insert const declarations at the TOP of the body, in original index order.
    // const_stmts is currently in reverse order (r, g, o). Reverse to get (o, g, r).
    // Then prepend the entire batch to body.stmts.
    const_stmts.reverse(); // Now ordered by ascending original index: (o, g, r)
    let old_body = std::mem::take(&mut body.stmts);
    body.stmts = const_stmts.into_iter().map(|(_, s)| s).collect();
    body.stmts.extend(old_body);
}

fn process_arrow_params_and_args(
    params: &mut Vec<Pat>,
    args: &mut Vec<ExprOrSpread>,
    body: &mut BlockStmt,
    uses_arguments: bool,
) {
    // (idx, old_name, new_name, new_ctxt)
    let mut renames: Vec<(usize, Atom, Atom, SyntaxContext)> = Vec::new();
    // (idx, param_name, param_ctxt, lit)
    let mut const_inserts: Vec<(usize, Atom, SyntaxContext, Lit)> = Vec::new();

    let param_count = params.len();
    let arg_count = args.len();

    for i in 0..param_count {
        let param_id = match pat_ident(&params[i]) {
            Some(id) => id,
            None => continue,
        };
        let param_sym = param_id.sym.clone();
        let param_ctxt = param_id.ctxt;

        if param_sym.len() != 1 {
            continue;
        }

        if i >= arg_count {
            continue;
        }

        let arg_expr = &args[i].expr;

        match arg_expr.as_ref() {
            Expr::Ident(ident) => {
                if ident.sym.len() > 1 && ident.sym != param_sym {
                    renames.push((i, param_sym, ident.sym.clone(), ident.ctxt));
                }
            }
            Expr::Lit(lit) => {
                if !uses_arguments {
                    const_inserts.push((i, param_sym, param_ctxt, lit.clone()));
                }
            }
            _ => {}
        }
    }

    for (i, old_sym, new_sym, new_ctxt) in &renames {
        if let Pat::Ident(bi) = &mut params[*i] {
            bi.id.sym = new_sym.clone();
            bi.id.ctxt = *new_ctxt;
        }
        let mut renamer = RenameIdent {
            old_sym: old_sym.clone(),
            new_sym: new_sym.clone(),
            new_ctxt: *new_ctxt,
        };
        body.visit_mut_with(&mut renamer);
    }

    const_inserts.sort_by(|a, b| b.0.cmp(&a.0));
    let mut const_stmts: Vec<(usize, Stmt)> = Vec::new();

    for (i, param_sym, param_ctxt, lit) in const_inserts {
        params.remove(i);
        args.remove(i);
        let const_stmt = make_const_decl(param_sym, param_ctxt, lit);
        const_stmts.push((i, const_stmt));
    }

    // Insert const declarations at the TOP of the body, in original index order.
    const_stmts.reverse(); // Now ordered by ascending original index
    let old_body = std::mem::take(&mut body.stmts);
    body.stmts = const_stmts.into_iter().map(|(_, s)| s).collect();
    body.stmts.extend(old_body);
}

fn pat_ident(pat: &Pat) -> Option<&Ident> {
    if let Pat::Ident(BindingIdent { id, .. }) = pat {
        Some(id)
    } else {
        None
    }
}


fn make_const_decl(name: Atom, binding_ctxt: SyntaxContext, lit: Lit) -> Stmt {
    Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent {
                id: Ident {
                    span: DUMMY_SP,
                    ctxt: binding_ctxt,
                    sym: name,
                    optional: false,
                },
                type_ann: None,
            }),
            init: Some(Box::new(Expr::Lit(lit))),
            definite: false,
        }],
    })))
}

struct RenameIdent {
    old_sym: Atom,
    new_sym: Atom,
    new_ctxt: SyntaxContext,
}

impl VisitMut for RenameIdent {
    fn visit_mut_ident(&mut self, ident: &mut Ident) {
        if ident.sym == self.old_sym {
            ident.sym = self.new_sym.clone();
            ident.ctxt = self.new_ctxt;
        }
    }
}

struct ArgumentsChecker {
    found: bool,
}

impl swc_core::ecma::visit::Visit for ArgumentsChecker {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym.as_ref() == "arguments" {
            self.found = true;
        }
    }
}

fn body_uses_arguments(stmts: &[Stmt]) -> bool {
    use swc_core::ecma::visit::Visit;
    let mut checker = ArgumentsChecker { found: false };
    for stmt in stmts {
        checker.visit_stmt(stmt);
        if checker.found {
            return true;
        }
    }
    checker.found
}
