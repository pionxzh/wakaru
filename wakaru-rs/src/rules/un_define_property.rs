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

use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, BlockStmt, Callee, ComputedPropName,
    Decl, Expr, ExprStmt, FnDecl, Function, Ident, IdentName, KeyValueProp, Lit, MemberExpr,
    MemberProp, Module, ModuleDecl, ModuleItem, Param, Pat, Prop, PropName, PropOrSpread,
    ReturnStmt, SimpleAssignTarget, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

pub struct UnDefineProperty;

impl VisitMut for UnDefineProperty {
    fn visit_mut_module(&mut self, module: &mut Module) {
        // Step 1: detect helper declarations at module top level.
        let helpers = find_helpers(module);
        if helpers.is_empty() {
            return;
        }

        // Step 2: rewrite expression-statement call sites.
        let mut rewriter = CallSiteRewriter {
            helpers: &helpers,
            rewrote_any: false,
        };
        module.visit_mut_with(&mut rewriter);

        // Step 3: if the helper has no remaining references, drop its decl.
        if rewriter.rewrote_any {
            remove_unused_helpers(module, &helpers);
        }
    }
}

/// Maps `(helper_sym, helper_ctxt) → true` so call-site and reference checks
/// can match by the resolver's binding identity.
type HelperSet = HashMap<(Atom, SyntaxContext), ()>;

fn find_helpers(module: &Module) -> HelperSet {
    let mut helpers = HelperSet::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) = item else {
            continue;
        };
        if is_define_property_fn(&fn_decl.function) {
            helpers.insert((fn_decl.ident.sym.clone(), fn_decl.ident.ctxt), ());
        }
    }
    helpers
}

fn is_define_property_fn(func: &Function) -> bool {
    if func.params.len() != 3 {
        return false;
    }
    let params: Vec<&Atom> = func
        .params
        .iter()
        .filter_map(|p| match &p.pat {
            Pat::Ident(b) => Some(&b.id.sym),
            _ => None,
        })
        .collect();
    if params.len() != 3 {
        return false;
    }
    let e = params[0];
    let t = params[1];
    let n = params[2];

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
    if !is_in_check(&if_stmt.test, t, e) {
        return false;
    }
    if !if_consequent_matches_define_property(&if_stmt.cons, e, t, n) {
        return false;
    }
    let Some(alt) = &if_stmt.alt else {
        return false;
    };
    if !if_alternate_matches_direct_assign(alt, e, t, n) {
        return false;
    }

    // Statement 1: return e
    let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = &body.stmts[1] else {
        return false;
    };
    matches!(arg.as_ref(), Expr::Ident(id) if &id.sym == e)
}

fn is_in_check(expr: &Expr, left_sym: &Atom, right_sym: &Atom) -> bool {
    let Expr::Bin(BinExpr { op: BinaryOp::In, left, right, .. }) = expr else {
        return false;
    };
    matches!(left.as_ref(), Expr::Ident(id) if &id.sym == left_sym)
        && matches!(right.as_ref(), Expr::Ident(id) if &id.sym == right_sym)
}

fn if_consequent_matches_define_property(stmt: &Stmt, e: &Atom, t: &Atom, n: &Atom) -> bool {
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
    if !matches!(call.args[0].expr.as_ref(), Expr::Ident(id) if &id.sym == e) {
        return false;
    }
    if !matches!(call.args[1].expr.as_ref(), Expr::Ident(id) if &id.sym == t) {
        return false;
    }
    // arg 2: { value: n, enumerable: true, configurable: true, writable: true }
    let Expr::Object(obj_lit) = call.args[2].expr.as_ref() else {
        return false;
    };
    let mut has_value = false;
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
                has_value = matches!(value.as_ref(), Expr::Ident(id) if &id.sym == n);
            }
            "enumerable" | "configurable" => {
                if !matches!(value.as_ref(), Expr::Lit(Lit::Bool(b)) if b.value) {
                    return false;
                }
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
    has_value && has_writable
}

fn if_alternate_matches_direct_assign(stmt: &Stmt, e: &Atom, t: &Atom, n: &Atom) -> bool {
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
    let Expr::Assign(AssignExpr { op: AssignOp::Assign, left, right, .. }) = expr else {
        return false;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Member(m)) = left else {
        return false;
    };
    // left: e[t]
    let Expr::Ident(obj) = m.obj.as_ref() else {
        return false;
    };
    if &obj.sym != e {
        return false;
    }
    let MemberProp::Computed(ComputedPropName { expr: key_expr, .. }) = &m.prop else {
        return false;
    };
    if !matches!(key_expr.as_ref(), Expr::Ident(id) if &id.sym == t) {
        return false;
    }
    // right: n
    matches!(right.as_ref(), Expr::Ident(id) if &id.sym == n)
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
        if !self
            .helpers
            .contains_key(&(callee_ident.sym.clone(), callee_ident.ctxt))
        {
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
    // Collect references to each helper in the current (post-rewrite) AST.
    let mut collector = HelperReferenceCollector {
        helpers,
        referenced: HashSet::new(),
        in_self_decl: None,
    };
    module.visit_with(&mut collector);

    module.body.retain(|item| {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) = item else {
            return true;
        };
        let key = (fn_decl.ident.sym.clone(), fn_decl.ident.ctxt);
        if !helpers.contains_key(&key) {
            return true;
        }
        // Drop the helper only if nothing outside its own body references it.
        collector.referenced.contains(&key)
    });
}

/// Counts references to helper idents anywhere except inside the helper's own
/// declaration body (the helper function referencing itself is expected and
/// shouldn't pin the decl).
struct HelperReferenceCollector<'a> {
    helpers: &'a HelperSet,
    referenced: HashSet<(Atom, SyntaxContext)>,
    in_self_decl: Option<(Atom, SyntaxContext)>,
}

impl Visit for HelperReferenceCollector<'_> {
    fn visit_fn_decl(&mut self, fn_decl: &FnDecl) {
        let key = (fn_decl.ident.sym.clone(), fn_decl.ident.ctxt);
        let is_self = self.helpers.contains_key(&key);
        let prev = if is_self {
            self.in_self_decl.replace(key)
        } else {
            self.in_self_decl.take()
        };
        fn_decl.visit_children_with(self);
        self.in_self_decl = prev;
    }

    fn visit_ident(&mut self, ident: &Ident) {
        let key = (ident.sym.clone(), ident.ctxt);
        if !self.helpers.contains_key(&key) {
            return;
        }
        // Skip occurrences inside the helper's own decl body (the binding
        // itself, or any recursive self-reference).
        if self.in_self_decl.as_ref() == Some(&key) {
            return;
        }
        self.referenced.insert(key);
    }

    fn visit_prop_name(&mut self, prop: &PropName) {
        if let PropName::Computed(c) = prop {
            c.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }
}

// Silence unused import warnings for Param (re-exported via `Pat::Ident`
// traversal) — kept imported for clarity.
#[allow(dead_code)]
fn _unused_param_marker(_: &Param) {}
