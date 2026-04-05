use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    BinaryOp, BlockStmtOrExpr, Callee, CondExpr, Decl, Expr, Lit, MemberProp, Module, ModuleItem,
    Pat, Stmt, UnaryExpr, UnaryOp, VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

type BindingKey = (Atom, SyntaxContext);

/// Detects and simplifies Babel's `_typeof` polyfill.
///
/// Pattern:
/// ```js
/// var _typeof = typeof Symbol == "function" && typeof Symbol.iterator == "symbol"
///     ? function(e) { return typeof e; }
///     : function(e) { /* Symbol polyfill */ return typeof e; };
/// ```
///
/// All calls `_typeof(expr)` are replaced with `typeof expr`, and the
/// polyfill declaration is removed.
pub struct UnTypeofPolyfill;

impl VisitMut for UnTypeofPolyfill {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let helpers = collect_typeof_helpers(module);
        if helpers.is_empty() {
            return;
        }

        let mut replacer = TypeofReplacer { helpers: &helpers };
        module.visit_mut_with(&mut replacer);

        // Remove declarations if no remaining references
        let remaining = find_remaining_refs(module, &helpers);
        let safe_to_remove: HashSet<BindingKey> = helpers
            .difference(&remaining)
            .cloned()
            .collect();
        if !safe_to_remove.is_empty() {
            remove_declarations(&mut module.body, &safe_to_remove);
        }
    }
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

fn collect_typeof_helpers(module: &Module) -> HashSet<BindingKey> {
    let mut helpers = HashSet::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else { continue };
        for decl in &var.decls {
            if is_typeof_polyfill_decl(decl) {
                let Pat::Ident(bi) = &decl.name else { continue };
                helpers.insert((bi.id.sym.clone(), bi.id.ctxt));
            }
        }
    }
    helpers
}

/// Check if a var declarator is the `_typeof` polyfill pattern:
/// `typeof Symbol == "function" && typeof Symbol.iterator == "symbol" ? fn1 : fn2`
/// where the truthy branch is `(e) => typeof e` or `function(e) { return typeof e; }`.
fn is_typeof_polyfill_decl(decl: &VarDeclarator) -> bool {
    let Some(init) = &decl.init else { return false };
    let Expr::Cond(cond) = init.as_ref() else { return false };

    // Test: typeof Symbol == "function" && typeof Symbol.iterator == "symbol"
    if !is_typeof_symbol_test(&cond.test) {
        return false;
    }

    // Consequent must be a function that returns typeof its param
    is_typeof_identity_fn(&cond.cons)
}

/// Check: `typeof Symbol == "function" && typeof Symbol.iterator == "symbol"`
fn is_typeof_symbol_test(expr: &Expr) -> bool {
    let Expr::Bin(bin) = expr else { return false };
    if bin.op != BinaryOp::LogicalAnd {
        return false;
    }
    is_typeof_symbol_eq_function(&bin.left) && is_typeof_symbol_iterator_eq_symbol(&bin.right)
}

/// Check: `typeof Symbol == "function"`
fn is_typeof_symbol_eq_function(expr: &Expr) -> bool {
    let Expr::Bin(bin) = expr else { return false };
    if !matches!(bin.op, BinaryOp::EqEq | BinaryOp::EqEqEq) {
        return false;
    }
    is_typeof_of_ident(&bin.left, "Symbol") && is_string_lit(&bin.right, "function")
}

/// Check: `typeof Symbol.iterator == "symbol"`
fn is_typeof_symbol_iterator_eq_symbol(expr: &Expr) -> bool {
    let Expr::Bin(bin) = expr else { return false };
    if !matches!(bin.op, BinaryOp::EqEq | BinaryOp::EqEqEq) {
        return false;
    }
    is_typeof_of_symbol_iterator(&bin.left) && is_string_lit(&bin.right, "symbol")
}

fn is_typeof_of_ident(expr: &Expr, name: &str) -> bool {
    let Expr::Unary(UnaryExpr { op: UnaryOp::TypeOf, arg, .. }) = expr else { return false };
    matches!(arg.as_ref(), Expr::Ident(id) if id.sym.as_ref() == name)
}

fn is_typeof_of_symbol_iterator(expr: &Expr) -> bool {
    let Expr::Unary(UnaryExpr { op: UnaryOp::TypeOf, arg, .. }) = expr else { return false };
    let Expr::Member(member) = arg.as_ref() else { return false };
    let Expr::Ident(obj) = member.obj.as_ref() else { return false };
    if obj.sym.as_ref() != "Symbol" {
        return false;
    }
    matches!(&member.prop, MemberProp::Ident(id) if id.sym.as_ref() == "iterator")
}

fn is_string_lit(expr: &Expr, value: &str) -> bool {
    matches!(expr, Expr::Lit(Lit::Str(s)) if s.value.as_str() == Some(value))
}

/// Check if an expression is `(e) => typeof e` or `function(e) { return typeof e; }`.
fn is_typeof_identity_fn(expr: &Expr) -> bool {
    match expr {
        Expr::Arrow(arrow) => {
            if arrow.params.len() != 1 {
                return false;
            }
            let Pat::Ident(param) = &arrow.params[0] else { return false };
            match &*arrow.body {
                BlockStmtOrExpr::Expr(body_expr) => {
                    is_typeof_of_binding(body_expr, &param.id.sym, param.id.ctxt)
                }
                BlockStmtOrExpr::BlockStmt(block) => {
                    if block.stmts.len() != 1 {
                        return false;
                    }
                    let Stmt::Return(ret) = &block.stmts[0] else { return false };
                    let Some(arg) = &ret.arg else { return false };
                    is_typeof_of_binding(arg, &param.id.sym, param.id.ctxt)
                }
            }
        }
        Expr::Fn(fn_expr) => {
            if fn_expr.function.params.len() != 1 {
                return false;
            }
            let Pat::Ident(param) = &fn_expr.function.params[0].pat else { return false };
            let Some(body) = &fn_expr.function.body else { return false };
            if body.stmts.len() != 1 {
                return false;
            }
            let Stmt::Return(ret) = &body.stmts[0] else { return false };
            let Some(arg) = &ret.arg else { return false };
            is_typeof_of_binding(arg, &param.id.sym, param.id.ctxt)
        }
        _ => false,
    }
}

fn is_typeof_of_binding(expr: &Expr, sym: &Atom, ctxt: SyntaxContext) -> bool {
    let Expr::Unary(UnaryExpr { op: UnaryOp::TypeOf, arg, .. }) = expr else { return false };
    matches!(arg.as_ref(), Expr::Ident(id) if id.sym == *sym && id.ctxt == ctxt)
}

// ---------------------------------------------------------------------------
// Replacement: _typeof(expr) → typeof expr
// ---------------------------------------------------------------------------

struct TypeofReplacer<'a> {
    helpers: &'a HashSet<BindingKey>,
}

impl VisitMut for TypeofReplacer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        let Callee::Expr(callee) = &call.callee else { return };
        let Expr::Ident(id) = callee.as_ref() else { return };

        let key = (id.sym.clone(), id.ctxt);
        if !self.helpers.contains(&key) {
            return;
        }

        // Must be a single-arg call: _typeof(expr)
        if call.args.len() != 1 || call.args[0].spread.is_some() {
            return;
        }

        *expr = Expr::Unary(UnaryExpr {
            span: DUMMY_SP,
            op: UnaryOp::TypeOf,
            arg: call.args[0].expr.clone(),
        });
    }
}

// ---------------------------------------------------------------------------
// Cleanup: remove declarations and find remaining references
// ---------------------------------------------------------------------------

fn find_remaining_refs(module: &Module, helpers: &HashSet<BindingKey>) -> HashSet<BindingKey> {
    use swc_core::ecma::visit::{Visit, VisitWith};

    struct RefScanner<'a> {
        helpers: &'a HashSet<BindingKey>,
        decl_bindings: HashSet<BindingKey>,
        found: HashSet<BindingKey>,
    }

    impl Visit for RefScanner<'_> {
        fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
            if let Pat::Ident(bi) = &decl.name {
                let key = (bi.id.sym.clone(), bi.id.ctxt);
                if self.decl_bindings.contains(&key) {
                    return;
                }
            }
            if let Some(init) = &decl.init {
                init.visit_with(self);
            }
        }

        fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
            let key = (ident.sym.clone(), ident.ctxt);
            if self.helpers.contains(&key) {
                self.found.insert(key);
            }
        }
    }

    let decl_bindings: HashSet<BindingKey> = helpers.clone();
    let mut scanner = RefScanner {
        helpers,
        decl_bindings,
        found: HashSet::new(),
    };
    module.visit_with(&mut scanner);
    scanner.found
}

fn remove_declarations(body: &mut Vec<ModuleItem>, helpers: &HashSet<BindingKey>) {
    for item in body.iter_mut() {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else { continue };
        var.decls.retain(|decl| {
            let Pat::Ident(bi) = &decl.name else { return true };
            let key = (bi.id.sym.clone(), bi.id.ctxt);
            !helpers.contains(&key)
        });
    }
    body.retain(|item| {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else { return true };
        !var.decls.is_empty()
    });
}
