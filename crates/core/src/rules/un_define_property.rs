//! Detect and unwrap the Babel `_defineProperty` helper when it is inlined as
//! a local function decl.
//!
//! Shape matched:
//!
//! ```js
//! function _defineProperty(e, t, n) {
//!     if (t in e) {
//!         Object.defineProperty(e, t, {
//!             value: n,
//!             enumerable: true,
//!             configurable: true,
//!             writable: true,
//!         });
//!     } else {
//!         e[t] = n;
//!     }
//!     return e;
//! }
//! ```
//!
//! Call sites rewritten only when the call is a standalone expression
//! statement (the helper's return value is discarded). Call-in-expression
//! positions like `const x = _defineProperty(o, k, v)` are left alone —
//! rewriting them would lose the `return e` semantics.
//!
//! If all references to the helper are call sites we rewrote, the helper
//! function declaration is dropped. Exported helpers are always preserved.

use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, BlockStmt, Callee, ComputedPropName,
    Decl, Expr, ExprStmt, Function, IdentName, KeyValueProp, Lit, MemberExpr, MemberProp, Module,
    ModuleDecl, ModuleItem, Prop, PropName, PropOrSpread, ReturnStmt, SimpleAssignTarget, Stmt,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::helper_matcher::{
    binding_key, fn_decl_binding_key, import_specifier_binding_key,
    remaining_refs_outside_skipped_items, remove_fn_decls_by_binding,
    remove_import_specifiers_by_binding, BindingKey,
};
use super::match_context::MatchContext;
use super::transpiler_helper_utils::{LocalHelperContext, TranspilerHelperKind};

pub struct UnDefineProperty;

impl UnDefineProperty {
    pub(crate) fn run_with_helpers(module: &mut Module, local_helpers: &LocalHelperContext) {
        let mut helpers = find_helpers(module);
        for key in local_helpers
            .helpers_of_kind(TranspilerHelperKind::DefineProperty)
            .into_keys()
        {
            helpers.insert(key);
        }
        run_un_define_property(module, helpers);
    }
}

impl VisitMut for UnDefineProperty {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let helpers = find_helpers(module);
        run_un_define_property(module, helpers);
    }
}

fn run_un_define_property(module: &mut Module, helpers: HelperSet) {
    if helpers.is_empty() {
        return;
    }

    let mut rewriter = CallSiteRewriter {
        helpers: &helpers,
        rewrote_any: false,
    };
    module.visit_mut_with(&mut rewriter);

    if rewriter.rewrote_any {
        remove_unused_helpers(module, &helpers);
    }
}

/// Helper declaration bindings, keyed by resolver binding identity.
type HelperSet = HashSet<BindingKey>;

fn find_helpers(module: &Module) -> HelperSet {
    let mut helpers = HelperSet::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) = item else {
            continue;
        };
        if is_define_property_fn(&fn_decl.function) {
            helpers.insert(binding_key(&fn_decl.ident));
        }
    }
    helpers
}

fn is_define_property_fn(func: &Function) -> bool {
    let Some(ctx) = MatchContext::from_params(func, &["target", "key", "value"]) else {
        return false;
    };

    let Some(body) = &func.body else {
        return false;
    };
    // Body must be exactly: if/else + return e
    if body.stmts.len() != 2 {
        return false;
    }

    // Statement 0: if (t in e) { Object.defineProperty(e, t, {...}) } else { e[t] = n }
    let Stmt::If(if_stmt) = &body.stmts[0] else {
        return false;
    };
    if !is_in_check(&if_stmt.test, &ctx) {
        return false;
    }
    if !if_consequent_matches_define_property(&if_stmt.cons, &ctx) {
        return false;
    }
    let Some(alt) = &if_stmt.alt else {
        return false;
    };
    if !if_alternate_matches_direct_assign(alt, &ctx) {
        return false;
    }

    // Statement 1: return e
    let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = &body.stmts[1] else {
        return false;
    };
    ctx.is_binding(arg, "target")
}

fn is_in_check(expr: &Expr, ctx: &MatchContext) -> bool {
    let Expr::Bin(BinExpr {
        op: BinaryOp::In,
        left,
        right,
        ..
    }) = expr
    else {
        return false;
    };
    ctx.is_binding(left, "key") && ctx.is_binding(right, "target")
}

fn if_consequent_matches_define_property(stmt: &Stmt, ctx: &MatchContext) -> bool {
    // Accept either a bare ExprStmt or a BlockStmt containing exactly one
    // ExprStmt whose expression is `Object.defineProperty(e, t, {...})`.
    let expr = match stmt {
        Stmt::Expr(ExprStmt { expr, .. }) => expr.as_ref(),
        Stmt::Block(BlockStmt { stmts, .. }) if stmts.len() == 1 => {
            let Stmt::Expr(ExprStmt { expr, .. }) = &stmts[0] else {
                return false;
            };
            expr.as_ref()
        }
        _ => return false,
    };
    let Expr::Call(call) = expr else {
        return false;
    };
    // callee: Object.defineProperty
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(m) = callee.as_ref() else {
        return false;
    };
    let Expr::Ident(obj) = m.obj.as_ref() else {
        return false;
    };
    if obj.sym.as_ref() != "Object" {
        return false;
    }
    if !matches!(&m.prop, MemberProp::Ident(id) if id.sym.as_ref() == "defineProperty") {
        return false;
    }
    if call.args.len() != 3 {
        return false;
    }
    // arg 0: e, arg 1: t
    if !ctx.is_binding(&call.args[0].expr, "target") {
        return false;
    }
    if !ctx.is_binding(&call.args[1].expr, "key") {
        return false;
    }
    // arg 2: { value: n, enumerable: true, configurable: true, writable: true }
    let Expr::Object(obj_lit) = call.args[2].expr.as_ref() else {
        return false;
    };
    let mut has_value = false;
    let mut has_enumerable = false;
    let mut has_configurable = false;
    let mut has_writable = false;
    for prop in &obj_lit.props {
        let PropOrSpread::Prop(p) = prop else {
            return false;
        };
        let Prop::KeyValue(KeyValueProp { key, value }) = p.as_ref() else {
            return false;
        };
        let Some(name) = prop_name_ident(key) else {
            return false;
        };
        match name.as_ref() {
            "value" => {
                has_value = ctx.is_binding(value, "value");
            }
            "enumerable" => {
                if !matches!(value.as_ref(), Expr::Lit(Lit::Bool(b)) if b.value) {
                    return false;
                }
                has_enumerable = true;
            }
            "configurable" => {
                if !matches!(value.as_ref(), Expr::Lit(Lit::Bool(b)) if b.value) {
                    return false;
                }
                has_configurable = true;
            }
            "writable" => {
                if !matches!(value.as_ref(), Expr::Lit(Lit::Bool(b)) if b.value) {
                    return false;
                }
                has_writable = true;
            }
            _ => return false,
        }
    }
    has_value && has_enumerable && has_configurable && has_writable
}

fn if_alternate_matches_direct_assign(stmt: &Stmt, ctx: &MatchContext) -> bool {
    let expr = match stmt {
        Stmt::Expr(ExprStmt { expr, .. }) => expr.as_ref(),
        Stmt::Block(BlockStmt { stmts, .. }) if stmts.len() == 1 => {
            let Stmt::Expr(ExprStmt { expr, .. }) = &stmts[0] else {
                return false;
            };
            expr.as_ref()
        }
        _ => return false,
    };
    let Expr::Assign(AssignExpr {
        op: AssignOp::Assign,
        left,
        right,
        ..
    }) = expr
    else {
        return false;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Member(m)) = left else {
        return false;
    };
    // left: e[t]
    let Expr::Ident(obj) = m.obj.as_ref() else {
        return false;
    };
    if !ctx.is_ident(obj, "target") {
        return false;
    }
    let MemberProp::Computed(ComputedPropName { expr: key_expr, .. }) = &m.prop else {
        return false;
    };
    if !ctx.is_binding(key_expr, "key") {
        return false;
    }
    // right: n
    ctx.is_binding(right, "value")
}

fn prop_name_ident(key: &PropName) -> Option<Atom> {
    match key {
        PropName::Ident(IdentName { sym, .. }) => Some(sym.clone()),
        PropName::Str(s) => Some(s.value.as_str()?.into()),
        _ => None,
    }
}

struct CallSiteRewriter<'a> {
    helpers: &'a HelperSet,
    rewrote_any: bool,
}

impl CallSiteRewriter<'_> {
    fn try_rewrite_expr_stmt(&mut self, stmt: &mut Stmt) -> bool {
        let Stmt::Expr(ExprStmt { expr, span }) = stmt else {
            return false;
        };
        let Expr::Call(call) = expr.as_ref() else {
            return false;
        };
        let Callee::Expr(callee_expr) = &call.callee else {
            return false;
        };
        let Expr::Ident(callee_ident) = callee_expr.as_ref() else {
            return false;
        };
        if !self.helpers.contains(&binding_key(callee_ident)) {
            return false;
        }
        if call.args.len() != 3 {
            return false;
        }
        // No spread args allowed — we need all three positionally.
        if call.args.iter().any(|a| a.spread.is_some()) {
            return false;
        }

        let obj = call.args[0].expr.clone();
        let key = call.args[1].expr.clone();
        let value = call.args[2].expr.clone();

        // Build obj[key] = value
        let new_expr = Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Member(MemberExpr {
                span: DUMMY_SP,
                obj,
                prop: MemberProp::Computed(ComputedPropName {
                    span: DUMMY_SP,
                    expr: key,
                }),
            })),
            right: value,
        });
        *stmt = Stmt::Expr(ExprStmt {
            span: *span,
            expr: Box::new(new_expr),
        });
        self.rewrote_any = true;
        true
    }
}

impl VisitMut for CallSiteRewriter<'_> {
    fn visit_mut_stmt(&mut self, stmt: &mut Stmt) {
        self.try_rewrite_expr_stmt(stmt);
        stmt.visit_mut_children_with(self);
    }
}

fn remove_unused_helpers(module: &mut Module, helpers: &HelperSet) {
    let remaining = remaining_refs_outside_skipped_items(module, helpers, |item| {
        if fn_decl_binding_key(item).is_some_and(|key| helpers.contains(&key)) {
            return true;
        }
        if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item {
            if import.specifiers.len() == 1 {
                let key = import_specifier_binding_key(&import.specifiers[0]);
                return helpers.contains(&key);
            }
        }
        false
    });
    let removable: HashSet<_> = helpers.difference(&remaining).cloned().collect();
    remove_fn_decls_by_binding(module, &removable);
    remove_import_specifiers_by_binding(&mut module.body, &removable);
}
