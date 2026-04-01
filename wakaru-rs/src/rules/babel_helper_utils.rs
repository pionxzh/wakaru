use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    BinaryOp, BlockStmtOrExpr, Callee, Decl, Expr, Function, IfStmt, Lit, MemberProp, Module,
    ModuleItem, Pat, ReturnStmt, Stmt, VarDeclarator,
};

pub(crate) type BindingKey = (Atom, SyntaxContext);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BabelHelperKind {
    InteropRequireDefault,
    InteropRequireWildcard,
    ToConsumableArray,
    Extends,
    ObjectSpread,
    SlicedToArray,
}

/// Known import paths for Babel runtime helpers.
const INTEROP_DEFAULT_PATHS: &[&str] = &[
    "@babel/runtime/helpers/interopRequireDefault",
    "@babel/runtime/helpers/esm/interopRequireDefault",
];

const INTEROP_WILDCARD_PATHS: &[&str] = &[
    "@babel/runtime/helpers/interopRequireWildcard",
    "@babel/runtime/helpers/esm/interopRequireWildcard",
];

const TO_CONSUMABLE_ARRAY_PATHS: &[&str] = &[
    "@babel/runtime/helpers/toConsumableArray",
    "@babel/runtime/helpers/esm/toConsumableArray",
];

const EXTENDS_PATHS: &[&str] = &[
    "@babel/runtime/helpers/extends",
    "@babel/runtime/helpers/esm/extends",
];

const OBJECT_SPREAD_PATHS: &[&str] = &[
    "@babel/runtime/helpers/objectSpread2",
    "@babel/runtime/helpers/esm/objectSpread2",
    "@babel/runtime/helpers/objectSpread",
    "@babel/runtime/helpers/esm/objectSpread",
];

const SLICED_TO_ARRAY_PATHS: &[&str] = &[
    "@babel/runtime/helpers/slicedToArray",
    "@babel/runtime/helpers/esm/slicedToArray",
];

/// Scan module-level declarations for helper functions.
/// Detects by function body shape and by import path.
pub(crate) fn collect_helpers(module: &Module) -> HashMap<BindingKey, BabelHelperKind> {
    let mut helpers = HashMap::new();
    for item in &module.body {
        match item {
            // function _interopRequireDefault(obj) { ... }
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
                if let Some(kind) = detect_helper_from_fn(&fn_decl.function) {
                    helpers.insert(
                        (fn_decl.ident.sym.clone(), fn_decl.ident.ctxt),
                        kind,
                    );
                }
            }
            // var _ird = function(obj) { ... }  OR  var _ird = require("@babel/runtime/...")
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    if let Some((key, kind)) = detect_helper_from_var_decl(decl) {
                        helpers.insert(key, kind);
                    }
                }
            }
            _ => {}
        }
    }
    helpers
}

/// Check which helper bindings still have call-site references in the module.
/// Only counts calls (not the declaration binding itself), so it's safe to
/// use this to decide whether the declaration can be removed.
pub(crate) fn helpers_with_remaining_calls(
    module: &Module,
    helpers: &HashMap<BindingKey, BabelHelperKind>,
) -> HashSet<BindingKey> {
    use swc_core::ecma::visit::{Visit, VisitWith};

    struct CallScanner<'a> {
        helpers: &'a HashMap<BindingKey, BabelHelperKind>,
        found: HashSet<BindingKey>,
    }

    impl Visit for CallScanner<'_> {
        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Ident(id) = callee.as_ref() {
                    let key = (id.sym.clone(), id.ctxt);
                    if self.helpers.contains_key(&key) {
                        self.found.insert(key);
                    }
                }
            }
            call.visit_children_with(self);
        }
    }

    let mut scanner = CallScanner {
        helpers,
        found: HashSet::new(),
    };
    module.visit_with(&mut scanner);
    scanner.found
}

/// Remove helper declarations from the module body.
pub(crate) fn remove_helper_declarations(
    body: &mut Vec<ModuleItem>,
    helpers: &HashMap<BindingKey, BabelHelperKind>,
) {
    let mut new_body = Vec::with_capacity(body.len());
    for item in body.drain(..) {
        match &item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
                let key = (fn_decl.ident.sym.clone(), fn_decl.ident.ctxt);
                if helpers.contains_key(&key) {
                    continue;
                }
                new_body.push(item);
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                let mut var = var.clone();
                var.decls.retain(|decl| {
                    let Pat::Ident(bi) = &decl.name else { return true };
                    let key = (bi.id.sym.clone(), bi.id.ctxt);
                    !helpers.contains_key(&key)
                });
                if !var.decls.is_empty() {
                    new_body.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))));
                }
            }
            _ => new_body.push(item),
        }
    }
    *body = new_body;
}

fn detect_helper_from_var_decl(decl: &VarDeclarator) -> Option<(BindingKey, BabelHelperKind)> {
    let Pat::Ident(bi) = &decl.name else { return None };
    let init = decl.init.as_ref()?;
    let key = (bi.id.sym.clone(), bi.id.ctxt);

    // var _ird = function(obj) { ... }
    if let Expr::Fn(fn_expr) = init.as_ref() {
        if let Some(kind) = detect_helper_from_fn(&fn_expr.function) {
            return Some((key, kind));
        }
    }

    // var _ird = (obj) => { ... }
    if let Expr::Arrow(arrow) = init.as_ref() {
        if let Some(kind) = detect_helper_from_arrow(arrow) {
            return Some((key, kind));
        }
    }

    // var _ird = require("@babel/runtime/helpers/interopRequireDefault")
    if let Some(kind) = detect_helper_from_require(init) {
        return Some((key, kind));
    }

    // var _ird = require("@babel/runtime/helpers/interopRequireDefault").default
    if let Expr::Member(member) = init.as_ref() {
        if is_member_prop_name(&member.prop, "default") {
            if let Some(kind) = detect_helper_from_require(&member.obj) {
                return Some((key, kind));
            }
        }
    }

    None
}

fn detect_helper_from_require(expr: &Expr) -> Option<BabelHelperKind> {
    let Expr::Call(call) = expr else { return None };
    let Callee::Expr(callee) = &call.callee else { return None };
    let Expr::Ident(id) = callee.as_ref() else { return None };
    if id.sym.as_ref() != "require" || call.args.len() != 1 {
        return None;
    }
    let Expr::Lit(Lit::Str(s)) = call.args[0].expr.as_ref() else { return None };
    let path = s.value.as_str().unwrap_or("");
    if INTEROP_DEFAULT_PATHS.contains(&path) {
        return Some(BabelHelperKind::InteropRequireDefault);
    }
    if INTEROP_WILDCARD_PATHS.contains(&path) {
        return Some(BabelHelperKind::InteropRequireWildcard);
    }
    if TO_CONSUMABLE_ARRAY_PATHS.contains(&path) {
        return Some(BabelHelperKind::ToConsumableArray);
    }
    if EXTENDS_PATHS.contains(&path) {
        return Some(BabelHelperKind::Extends);
    }
    if OBJECT_SPREAD_PATHS.contains(&path) {
        return Some(BabelHelperKind::ObjectSpread);
    }
    if SLICED_TO_ARRAY_PATHS.contains(&path) {
        return Some(BabelHelperKind::SlicedToArray);
    }
    None
}

fn detect_helper_from_fn(func: &Function) -> Option<BabelHelperKind> {
    if is_interop_require_default_fn(func) {
        return Some(BabelHelperKind::InteropRequireDefault);
    }
    if is_interop_require_wildcard_fn(func) {
        return Some(BabelHelperKind::InteropRequireWildcard);
    }
    None
}

fn detect_helper_from_arrow(arrow: &swc_core::ecma::ast::ArrowExpr) -> Option<BabelHelperKind> {
    // interopRequireDefault: single param, body returns conditional on __esModule
    if arrow.params.len() == 1 {
        let Pat::Ident(param) = &arrow.params[0] else { return None };
        let param_sym = &param.id.sym;
        let param_ctxt = param.id.ctxt;

        match &*arrow.body {
            BlockStmtOrExpr::BlockStmt(block) => {
                if matches_ternary_return_block(&block.stmts, param_sym, param_ctxt) {
                    return Some(BabelHelperKind::InteropRequireDefault);
                }
                if matches_if_return_form(&block.stmts, param_sym, param_ctxt) {
                    return Some(BabelHelperKind::InteropRequireDefault);
                }
            }
            BlockStmtOrExpr::Expr(expr) => {
                if matches_ternary_expr(expr, param_sym, param_ctxt) {
                    return Some(BabelHelperKind::InteropRequireDefault);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// interopRequireDefault body-shape matchers
// ---------------------------------------------------------------------------

/// Match: function(obj) { return obj && obj.__esModule ? obj : { default: obj }; }
/// Or:   function(obj) { if (obj && obj.__esModule) return obj; return { default: obj }; }
fn is_interop_require_default_fn(func: &Function) -> bool {
    if func.params.len() != 1 {
        return false;
    }
    let Pat::Ident(param) = &func.params[0].pat else { return false };
    let param_sym = &param.id.sym;
    let param_ctxt = param.id.ctxt;

    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return false,
    };

    // Ternary form: return P && P.__esModule ? P : { default: P }
    if matches_ternary_return_block(&body.stmts, param_sym, param_ctxt) {
        return true;
    }
    // If-return form: if (P && P.__esModule) return P; return { default: P }
    if matches_if_return_form(&body.stmts, param_sym, param_ctxt) {
        return true;
    }

    false
}

/// Matches a block with a single return statement containing the ternary pattern.
fn matches_ternary_return_block(stmts: &[Stmt], param_sym: &Atom, param_ctxt: SyntaxContext) -> bool {
    if stmts.len() != 1 {
        return false;
    }
    let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = &stmts[0] else { return false };
    matches_ternary_expr(arg, param_sym, param_ctxt)
}

/// Matches: P && P.__esModule ? P : { default: P }
fn matches_ternary_expr(expr: &Expr, param_sym: &Atom, param_ctxt: SyntaxContext) -> bool {
    let Expr::Cond(cond) = expr else { return false };

    // test: P && P.__esModule
    if !matches_esmodule_test(&cond.test, param_sym, param_ctxt) {
        return false;
    }
    // cons: P
    if !is_same_ident(&cond.cons, param_sym, param_ctxt) {
        return false;
    }
    // alt: { default: P }
    matches_default_object(&cond.alt, param_sym, param_ctxt)
}

/// Matches: if (P && P.__esModule) return P; return { default: P };
fn matches_if_return_form(stmts: &[Stmt], param_sym: &Atom, param_ctxt: SyntaxContext) -> bool {
    if stmts.len() != 2 {
        return false;
    }
    let Stmt::If(IfStmt { test, cons, alt: None, .. }) = &stmts[0] else { return false };

    if !matches_esmodule_test(test, param_sym, param_ctxt) {
        return false;
    }

    // cons: { return P; } or return P;
    let cons_return = extract_single_return(cons);
    let Some(cons_arg) = cons_return else { return false };
    if !is_same_ident(cons_arg, param_sym, param_ctxt) {
        return false;
    }

    // second stmt: return { default: P }
    let Stmt::Return(ReturnStmt { arg: Some(alt_arg), .. }) = &stmts[1] else { return false };
    matches_default_object(alt_arg, param_sym, param_ctxt)
}

/// Matches: P && P.__esModule
fn matches_esmodule_test(expr: &Expr, param_sym: &Atom, param_ctxt: SyntaxContext) -> bool {
    let Expr::Bin(bin) = expr else { return false };
    if bin.op != BinaryOp::LogicalAnd {
        return false;
    }
    if !is_same_ident(&bin.left, param_sym, param_ctxt) {
        return false;
    }
    matches_member_prop(&bin.right, param_sym, param_ctxt, "__esModule")
}

/// Matches: { default: P } (an object literal with a single `default` property)
fn matches_default_object(expr: &Expr, param_sym: &Atom, param_ctxt: SyntaxContext) -> bool {
    let Expr::Object(obj) = expr else { return false };
    if obj.props.len() != 1 {
        return false;
    }
    let swc_core::ecma::ast::PropOrSpread::Prop(prop) = &obj.props[0] else { return false };
    let swc_core::ecma::ast::Prop::KeyValue(kv) = prop.as_ref() else { return false };

    // key must be "default"
    let key_is_default = match &kv.key {
        swc_core::ecma::ast::PropName::Ident(id) => id.sym.as_ref() == "default",
        swc_core::ecma::ast::PropName::Str(s) => s.value.as_str() == Some("default"),
        _ => false,
    };
    if !key_is_default {
        return false;
    }

    is_same_ident(&kv.value, param_sym, param_ctxt)
}

fn is_same_ident(expr: &Expr, sym: &Atom, ctxt: SyntaxContext) -> bool {
    matches!(expr, Expr::Ident(id) if id.sym == *sym && id.ctxt == ctxt)
}

fn matches_member_prop(expr: &Expr, obj_sym: &Atom, obj_ctxt: SyntaxContext, prop_name: &str) -> bool {
    let Expr::Member(member) = expr else { return false };
    if !is_same_ident(&member.obj, obj_sym, obj_ctxt) {
        return false;
    }
    is_member_prop_name(&member.prop, prop_name)
}

fn is_member_prop_name(prop: &MemberProp, name: &str) -> bool {
    match prop {
        MemberProp::Ident(id) => id.sym.as_ref() == name,
        MemberProp::Computed(c) => {
            matches!(c.expr.as_ref(), Expr::Lit(Lit::Str(s)) if s.value.as_str() == Some(name))
        }
        _ => false,
    }
}

fn extract_single_return(stmt: &Stmt) -> Option<&Expr> {
    match stmt {
        Stmt::Return(ReturnStmt { arg: Some(arg), .. }) => Some(arg),
        Stmt::Block(block) if block.stmts.len() == 1 => extract_single_return(&block.stmts[0]),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// interopRequireWildcard body-shape matcher
//
// The wildcard helper is more complex and varies across versions. We use a
// looser match: 1-2 params, body references `__esModule`, and contains
// property-copying logic (for-in or Object.keys/getOwnPropertyDescriptor).
// ---------------------------------------------------------------------------

fn is_interop_require_wildcard_fn(func: &Function) -> bool {
    if func.params.is_empty() || func.params.len() > 2 {
        return false;
    }

    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return false,
    };

    let mut has_esmodule = false;
    let mut has_property_copy = false;

    for stmt in &body.stmts {
        check_stmt_for_wildcard_markers(stmt, &mut has_esmodule, &mut has_property_copy);
    }

    has_esmodule && has_property_copy
}

fn check_stmt_for_wildcard_markers(stmt: &Stmt, has_esmodule: &mut bool, has_property_copy: &mut bool) {
    use swc_core::ecma::visit::{Visit, VisitWith};

    struct WildcardMarkerVisitor<'a> {
        has_esmodule: &'a mut bool,
        has_property_copy: &'a mut bool,
    }

    impl Visit for WildcardMarkerVisitor<'_> {
        fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
            if is_member_prop_name(&member.prop, "__esModule") {
                *self.has_esmodule = true;
            }
            member.visit_children_with(self);
        }

        fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
            // Object.keys, Object.getOwnPropertyDescriptor, etc.
            // We just look for the property-copy patterns
            let _ = ident;
        }

        fn visit_for_in_stmt(&mut self, _: &swc_core::ecma::ast::ForInStmt) {
            *self.has_property_copy = true;
        }

        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            // Look for Object.keys(...) or Object.getOwnPropertyDescriptor(...)
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Member(member) = callee.as_ref() {
                    if let Expr::Ident(obj) = member.obj.as_ref() {
                        if obj.sym.as_ref() == "Object" {
                            if is_member_prop_name(&member.prop, "keys")
                                || is_member_prop_name(&member.prop, "getOwnPropertyDescriptor")
                                || is_member_prop_name(&member.prop, "defineProperty")
                                || is_member_prop_name(&member.prop, "getOwnPropertyNames")
                            {
                                *self.has_property_copy = true;
                            }
                        }
                    }
                }
            }
            call.visit_children_with(self);
        }
    }

    let mut visitor = WildcardMarkerVisitor { has_esmodule, has_property_copy };
    stmt.visit_with(&mut visitor);
}
