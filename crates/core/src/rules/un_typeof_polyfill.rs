use std::collections::HashSet;

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    BinaryOp, BlockStmtOrExpr, Callee, Decl, Expr, Lit, MemberProp, Module, ModuleItem, Pat, Stmt,
    UnaryExpr, UnaryOp, VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::helper_matcher::{
    binding_key, expr_matches_binding, remaining_refs_outside_var_declarators,
    remove_var_declarators_by_binding, var_declarator_binding_key, BindingKey,
};

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
        let remaining = remaining_refs_outside_var_declarators(module, &helpers, &helpers);
        let safe_to_remove: HashSet<BindingKey> = helpers.difference(&remaining).cloned().collect();
        if !safe_to_remove.is_empty() {
            remove_var_declarators_by_binding(&mut module.body, &safe_to_remove);
        }
    }
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

fn collect_typeof_helpers(module: &Module) -> HashSet<BindingKey> {
    let mut helpers = HashSet::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            if is_typeof_polyfill_decl(decl) {
                let Some(key) = var_declarator_binding_key(decl) else {
                    continue;
                };
                helpers.insert(key);
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
    let Expr::Cond(cond) = init.as_ref() else {
        return false;
    };

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
    let Expr::Unary(UnaryExpr {
        op: UnaryOp::TypeOf,
        arg,
        ..
    }) = expr
    else {
        return false;
    };
    matches!(arg.as_ref(), Expr::Ident(id) if id.sym.as_ref() == name)
}

fn is_typeof_of_symbol_iterator(expr: &Expr) -> bool {
    let Expr::Unary(UnaryExpr {
        op: UnaryOp::TypeOf,
        arg,
        ..
    }) = expr
    else {
        return false;
    };
    let Expr::Member(member) = arg.as_ref() else {
        return false;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };
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
            let Pat::Ident(param) = &arrow.params[0] else {
                return false;
            };
            let param_key = binding_key(&param.id);
            match &*arrow.body {
                BlockStmtOrExpr::Expr(body_expr) => is_typeof_of_binding(body_expr, &param_key),
                BlockStmtOrExpr::BlockStmt(block) => {
                    if block.stmts.len() != 1 {
                        return false;
                    }
                    let Stmt::Return(ret) = &block.stmts[0] else {
                        return false;
                    };
                    let Some(arg) = &ret.arg else { return false };
                    is_typeof_of_binding(arg, &param_key)
                }
            }
        }
        Expr::Fn(fn_expr) => {
            if fn_expr.function.params.len() != 1 {
                return false;
            }
            let Pat::Ident(param) = &fn_expr.function.params[0].pat else {
                return false;
            };
            let param_key = binding_key(&param.id);
            let Some(body) = &fn_expr.function.body else {
                return false;
            };
            if body.stmts.len() != 1 {
                return false;
            }
            let Stmt::Return(ret) = &body.stmts[0] else {
                return false;
            };
            let Some(arg) = &ret.arg else { return false };
            is_typeof_of_binding(arg, &param_key)
        }
        _ => false,
    }
}

fn is_typeof_of_binding(expr: &Expr, binding: &BindingKey) -> bool {
    let Expr::Unary(UnaryExpr {
        op: UnaryOp::TypeOf,
        arg,
        ..
    }) = expr
    else {
        return false;
    };
    expr_matches_binding(arg, binding)
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
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Ident(id) = callee.as_ref() else {
            return;
        };

        let key = binding_key(id);
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
