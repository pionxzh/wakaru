use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    BinaryOp, BlockStmtOrExpr, Callee, Decl, Expr, ForHead, Function, IfStmt, Lit, MemberProp,
    Module, ModuleItem, Pat, ReturnStmt, Stmt, UnaryOp, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::match_context::MatchContext;

pub(crate) type BindingKey = (Atom, SyntaxContext);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BabelHelperKind {
    InteropRequireDefault,
    InteropRequireWildcard,
    ToConsumableArray,
    Extends,
    ObjectSpread,
    SlicedToArray,
    ClassCallCheck,
    PossibleConstructorReturn,
    AssertThisInitialized,
    ObjectWithoutProperties,
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

const OBJECT_WITHOUT_PROPERTIES_PATHS: &[&str] = &[
    "@babel/runtime/helpers/objectWithoutProperties",
    "@babel/runtime/helpers/esm/objectWithoutProperties",
    "@babel/runtime/helpers/objectWithoutPropertiesLoose",
    "@babel/runtime/helpers/esm/objectWithoutPropertiesLoose",
];

/// Scan module-level declarations for helper functions.
/// Detects by function body shape and by import path.
pub(crate) fn collect_helpers(module: &Module) -> HashMap<BindingKey, BabelHelperKind> {
    // Phase 1: scan all module-level function bodies for Babel sub-helper markers.
    // The Babel 7+ pattern uses a thin dispatcher (`return f(x) || g(x) || h(x) || k()`)
    // that delegates to sub-helpers defined in the same module. We only accept OR-chain
    // dispatchers when the module also contains functions with Array.isArray, Array.from,
    // or Symbol.iterator — signals that Babel sub-helpers are present.
    let has_sub_helpers = module_has_babel_sub_helper_signals(module);

    let mut helpers = HashMap::new();
    for item in &module.body {
        match item {
            // function _interopRequireDefault(obj) { ... }
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
                if let Some(kind) = detect_helper_from_fn(&fn_decl.function, has_sub_helpers) {
                    helpers.insert((fn_decl.ident.sym.clone(), fn_decl.ident.ctxt), kind);
                }
            }
            // var _ird = function(obj) { ... }  OR  var _ird = require("@babel/runtime/...")
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    if let Some((key, kind)) = detect_helper_from_var_decl(decl, has_sub_helpers) {
                        helpers.insert(key, kind);
                    }
                }
            }
            _ => {}
        }
    }
    helpers
}

/// Collect helpers of a specific kind from module-level declarations.
pub(crate) fn collect_helpers_of_kind(
    module: &Module,
    kind: BabelHelperKind,
) -> HashMap<BindingKey, BabelHelperKind> {
    let all = collect_helpers(module);
    all.into_iter().filter(|(_, k)| *k == kind).collect()
}

/// Collect only ClassCallCheck helpers from module-level declarations.
pub(crate) fn collect_class_call_check_helpers(
    module: &Module,
) -> HashMap<BindingKey, BabelHelperKind> {
    let all = collect_helpers(module);
    all.into_iter()
        .filter(|(_, kind)| *kind == BabelHelperKind::ClassCallCheck)
        .collect()
}

/// Check if the module contains functions with Babel sub-helper body signals.
/// These are markers like Array.isArray, Array.from, Symbol.iterator that appear
/// in the inlined sub-helpers (arrayWithoutHoles, iterableToArray, etc.).
fn module_has_babel_sub_helper_signals(module: &Module) -> bool {
    for item in &module.body {
        let func = match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => Some(&*fn_decl.function),
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                var.decls.iter().find_map(|d| match d.init.as_deref() {
                    Some(Expr::Fn(f)) => Some(&*f.function),
                    _ => None,
                })
            }
            _ => None,
        };
        if let Some(func) = func {
            if let Some(body) = &func.body {
                let mut markers = BodyMarkerState::default();
                scan_stmts_for_markers(&body.stmts, &mut markers);
                if markers.has_array_is_array
                    || markers.has_array_from
                    || markers.has_symbol_iterator
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Check which helper bindings still have references in the module body,
/// excluding the declaration binding itself (VarDeclarator name / FnDecl ident).
/// Catches both remaining calls and aliasing (`var f = helper`).
pub(crate) fn helpers_with_remaining_refs(
    module: &Module,
    helpers: &HashMap<BindingKey, BabelHelperKind>,
) -> HashSet<BindingKey> {
    use swc_core::ecma::visit::{Visit, VisitWith};

    // First, collect the declaration spans/positions so we can exclude them
    let mut decl_idents: HashSet<BindingKey> = HashSet::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
                let key = (fn_decl.ident.sym.clone(), fn_decl.ident.ctxt);
                if helpers.contains_key(&key) {
                    decl_idents.insert(key);
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    let Pat::Ident(bi) = &decl.name else { continue };
                    let key = (bi.id.sym.clone(), bi.id.ctxt);
                    if helpers.contains_key(&key) {
                        decl_idents.insert(key);
                    }
                }
            }
            _ => {}
        }
    }

    // Scan for references, skipping entire helper declarations (name + body).
    // Self-references inside a helper body (e.g. `_extends = Object.assign || ...`)
    // should not count as external usage.
    struct RefScanner<'a> {
        helpers: &'a HashMap<BindingKey, BabelHelperKind>,
        decl_idents: &'a HashSet<BindingKey>,
        found: HashSet<BindingKey>,
    }

    impl Visit for RefScanner<'_> {
        fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
            // If this var declarator IS a helper, skip it entirely (name + init)
            if let Pat::Ident(bi) = &decl.name {
                let key = (bi.id.sym.clone(), bi.id.ctxt);
                if self.decl_idents.contains(&key) {
                    return;
                }
            }
            // Otherwise skip just the binding name, scan the init
            if let Some(init) = &decl.init {
                init.visit_with(self);
            }
        }

        fn visit_fn_decl(&mut self, fn_decl: &swc_core::ecma::ast::FnDecl) {
            let key = (fn_decl.ident.sym.clone(), fn_decl.ident.ctxt);
            // If this fn decl IS a helper, skip it entirely (name + body)
            if self.decl_idents.contains(&key) {
                return;
            }
            // Otherwise skip just the name, scan the body
            fn_decl.function.visit_with(self);
        }

        fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
            let key = (ident.sym.clone(), ident.ctxt);
            if self.helpers.contains_key(&key) {
                self.found.insert(key);
            }
        }
    }

    let mut scanner = RefScanner {
        helpers,
        decl_idents: &decl_idents,
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
                    let Pat::Ident(bi) = &decl.name else {
                        return true;
                    };
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

fn detect_helper_from_var_decl(
    decl: &VarDeclarator,
    has_sub_helpers: bool,
) -> Option<(BindingKey, BabelHelperKind)> {
    let Pat::Ident(bi) = &decl.name else {
        return None;
    };
    let init = decl.init.as_ref()?;
    let key = (bi.id.sym.clone(), bi.id.ctxt);

    // var _ird = function(obj) { ... }
    if let Expr::Fn(fn_expr) = init.as_ref() {
        if let Some(kind) = detect_helper_from_fn(&fn_expr.function, has_sub_helpers) {
            return Some((key, kind));
        }
    }

    // var _ird = (obj) => { ... }
    if let Expr::Arrow(arrow) = init.as_ref() {
        if let Some(kind) = detect_helper_from_arrow(arrow, has_sub_helpers) {
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

    // var _extends = Object.assign || function(target) { ... }
    // This is the Babel 6 or pre-evaluated form of the _extends polyfill.
    if let Expr::Bin(bin) = init.as_ref() {
        if bin.op == BinaryOp::LogicalOr
            && is_object_assign_ref(&bin.left)
            && is_extends_polyfill_fn(&bin.right)
        {
            return Some((key, BabelHelperKind::Extends));
        }
    }

    None
}

fn detect_helper_from_require(expr: &Expr) -> Option<BabelHelperKind> {
    let Expr::Call(call) = expr else { return None };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(id) = callee.as_ref() else {
        return None;
    };
    if id.sym.as_ref() != "require" || call.args.len() != 1 {
        return None;
    }
    let Expr::Lit(Lit::Str(s)) = call.args[0].expr.as_ref() else {
        return None;
    };
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
    if OBJECT_WITHOUT_PROPERTIES_PATHS.contains(&path) {
        return Some(BabelHelperKind::ObjectWithoutProperties);
    }
    None
}

fn detect_helper_from_fn(func: &Function, has_sub_helpers: bool) -> Option<BabelHelperKind> {
    if is_interop_require_default_fn(func) {
        return Some(BabelHelperKind::InteropRequireDefault);
    }
    if is_interop_require_wildcard_fn(func) {
        return Some(BabelHelperKind::InteropRequireWildcard);
    }
    if is_to_consumable_array_fn(func, has_sub_helpers) {
        return Some(BabelHelperKind::ToConsumableArray);
    }
    if is_extends_fn(func) {
        return Some(BabelHelperKind::Extends);
    }
    if is_object_spread_fn(func) {
        return Some(BabelHelperKind::ObjectSpread);
    }
    if is_sliced_to_array_fn(func, has_sub_helpers) {
        return Some(BabelHelperKind::SlicedToArray);
    }
    if is_class_call_check_fn(func) {
        return Some(BabelHelperKind::ClassCallCheck);
    }
    if is_possible_constructor_return_fn(func) {
        return Some(BabelHelperKind::PossibleConstructorReturn);
    }
    if is_assert_this_initialized_fn(func) {
        return Some(BabelHelperKind::AssertThisInitialized);
    }
    if is_object_without_properties_fn(func) {
        return Some(BabelHelperKind::ObjectWithoutProperties);
    }
    None
}

fn detect_helper_from_arrow(
    arrow: &swc_core::ecma::ast::ArrowExpr,
    has_sub_helpers: bool,
) -> Option<BabelHelperKind> {
    // interopRequireDefault: single param, body returns conditional on __esModule
    if arrow.params.len() == 1 {
        let Pat::Ident(param) = &arrow.params[0] else {
            return None;
        };
        let mut ctx = MatchContext::new();
        ctx.declare("obj", param.id.sym.clone(), param.id.ctxt);

        match &*arrow.body {
            BlockStmtOrExpr::BlockStmt(block) => {
                if matches_ternary_return_block(&block.stmts, &ctx) {
                    return Some(BabelHelperKind::InteropRequireDefault);
                }
                if matches_if_return_form(&block.stmts, &ctx) {
                    return Some(BabelHelperKind::InteropRequireDefault);
                }
            }
            BlockStmtOrExpr::Expr(expr) => {
                if matches_ternary_expr(expr, &ctx) {
                    return Some(BabelHelperKind::InteropRequireDefault);
                }
            }
        }
    }

    // Convert arrow to equivalent Function shape and try the general matchers.
    // Only for block-body arrows (the common case for inlined helpers).
    if let BlockStmtOrExpr::BlockStmt(block) = &*arrow.body {
        let func = Function {
            params: arrow
                .params
                .iter()
                .map(|p| swc_core::ecma::ast::Param {
                    span: DUMMY_SP,
                    decorators: vec![],
                    pat: p.clone(),
                })
                .collect(),
            decorators: vec![],
            span: DUMMY_SP,
            ctxt: Default::default(),
            body: Some(block.clone()),
            is_generator: false,
            is_async: arrow.is_async,
            type_params: None,
            return_type: None,
        };
        if is_to_consumable_array_fn(&func, has_sub_helpers) {
            return Some(BabelHelperKind::ToConsumableArray);
        }
        if is_object_spread_fn(&func) {
            return Some(BabelHelperKind::ObjectSpread);
        }
        if is_sliced_to_array_fn(&func, has_sub_helpers) {
            return Some(BabelHelperKind::SlicedToArray);
        }
        if is_object_without_properties_fn(&func) {
            return Some(BabelHelperKind::ObjectWithoutProperties);
        }
        // Note: extends has 0 params and uses `arguments`, which arrows can't do.
    }

    None
}

// ---------------------------------------------------------------------------
// interopRequireDefault body-shape matchers
// ---------------------------------------------------------------------------

/// Match: function(obj) { return obj && obj.__esModule ? obj : { default: obj }; }
/// Or:   function(obj) { if (obj && obj.__esModule) return obj; return { default: obj }; }
fn is_interop_require_default_fn(func: &Function) -> bool {
    let Some(ctx) = MatchContext::from_params(func, &["obj"]) else {
        return false;
    };

    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return false,
    };

    matches_ternary_return_block(&body.stmts, &ctx)
        || matches_if_return_form(&body.stmts, &ctx)
}

fn matches_ternary_return_block(stmts: &[Stmt], ctx: &MatchContext) -> bool {
    if stmts.len() != 1 {
        return false;
    }
    let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = &stmts[0] else {
        return false;
    };
    matches_ternary_expr(arg, ctx)
}

/// Matches: obj && obj.__esModule ? obj : { default: obj }
fn matches_ternary_expr(expr: &Expr, ctx: &MatchContext) -> bool {
    let Expr::Cond(cond) = expr else { return false };

    matches_esmodule_test(&cond.test, ctx)
        && ctx.is_binding(&cond.cons, "obj")
        && matches_default_object(&cond.alt, ctx)
}

/// Matches: if (obj && obj.__esModule) return obj; return { default: obj };
fn matches_if_return_form(stmts: &[Stmt], ctx: &MatchContext) -> bool {
    if stmts.len() != 2 {
        return false;
    }
    let Stmt::If(IfStmt {
        test,
        cons,
        alt: None,
        ..
    }) = &stmts[0]
    else {
        return false;
    };

    if !matches_esmodule_test(test, ctx) {
        return false;
    }

    let Some(cons_arg) = extract_single_return(cons) else {
        return false;
    };
    if !ctx.is_binding(cons_arg, "obj") {
        return false;
    }

    let Stmt::Return(ReturnStmt {
        arg: Some(alt_arg), ..
    }) = &stmts[1]
    else {
        return false;
    };
    matches_default_object(alt_arg, ctx)
}

/// Matches: obj && obj.__esModule
fn matches_esmodule_test(expr: &Expr, ctx: &MatchContext) -> bool {
    let Expr::Bin(bin) = expr else { return false };
    if bin.op != BinaryOp::LogicalAnd {
        return false;
    }
    ctx.is_binding(&bin.left, "obj") && ctx.is_member_of(&bin.right, "obj", "__esModule")
}

/// Matches: { default: obj }
fn matches_default_object(expr: &Expr, ctx: &MatchContext) -> bool {
    let Expr::Object(obj) = expr else {
        return false;
    };
    if obj.props.len() != 1 {
        return false;
    }
    let swc_core::ecma::ast::PropOrSpread::Prop(prop) = &obj.props[0] else {
        return false;
    };
    let swc_core::ecma::ast::Prop::KeyValue(kv) = prop.as_ref() else {
        return false;
    };

    let key_is_default = match &kv.key {
        swc_core::ecma::ast::PropName::Ident(id) => id.sym.as_ref() == "default",
        swc_core::ecma::ast::PropName::Str(s) => s.value.as_str() == Some("default"),
        _ => false,
    };
    if !key_is_default {
        return false;
    }

    ctx.is_binding(&kv.value, "obj")
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

fn check_stmt_for_wildcard_markers(
    stmt: &Stmt,
    has_esmodule: &mut bool,
    has_property_copy: &mut bool,
) {
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
                        if obj.sym.as_ref() == "Object"
                            && (is_member_prop_name(&member.prop, "keys")
                                || is_member_prop_name(&member.prop, "getOwnPropertyDescriptor")
                                || is_member_prop_name(&member.prop, "defineProperty")
                                || is_member_prop_name(&member.prop, "getOwnPropertyNames"))
                        {
                            *self.has_property_copy = true;
                        }
                    }
                }
            }
            call.visit_children_with(self);
        }
    }

    let mut visitor = WildcardMarkerVisitor {
        has_esmodule,
        has_property_copy,
    };
    stmt.visit_with(&mut visitor);
}

// ---------------------------------------------------------------------------
// toConsumableArray body-shape matcher
//
// Babel 7+: function(arr) { return f(arr) || g(arr) || h(arr) || k(); }
//   where the sub-helpers reference Array.isArray / Array.from
// Babel 6:  function(arr) { if (Array.isArray(arr)) { ... } else { return Array.from(arr); } }
//
// Key signal: 1 param, body references both Array.isArray and Array.from.
// ---------------------------------------------------------------------------

fn is_to_consumable_array_fn(func: &Function, has_sub_helpers: bool) -> bool {
    if func.params.len() != 1 {
        return false;
    }

    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return false,
    };

    let mut markers = BodyMarkerState::default();
    scan_stmts_for_markers(&body.stmts, &mut markers);

    // Babel 6 form: Array.isArray + Array.from (or Array(len) constructor).
    // Must be a short function (≤4 statements) to avoid matching unrelated
    // utility functions that happen to use both Array.isArray and Array.from.
    if markers.has_array_is_array
        && (markers.has_array_from || markers.has_array_constructor)
        && body.stmts.len() <= 4
    {
        return true;
    }

    // Babel 7+ form: single return of logical-OR chain calling sub-helpers.
    // Pattern: return f(arr) || g(arr) || h(arr) || nonIterableSpread()
    // Only accepted when the module also contains Babel sub-helpers (functions
    // with Array.isArray, Array.from, etc.) to avoid false positives on
    // normal fallback chains.
    if has_sub_helpers && body.stmts.len() == 1 {
        if let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = &body.stmts[0] {
            if is_babel_helper_or_chain(arg) {
                return true;
            }
        }
    }

    false
}

// ---------------------------------------------------------------------------
// extends body-shape matcher
//
// function _extends() {
//   _extends = Object.assign || function(target) { ... for-in ... };
//   return _extends.apply(this, arguments);
// }
// Or minified: function() { return n = Object.assign || ..., n.apply(this, arguments); }
//
// Key signal: 0 params, references Object.assign, has .apply(this, arguments).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// classCallCheck body-shape matcher
//
// function _classCallCheck(instance, Constructor) {
//   if (!(instance instanceof Constructor)) {
//     throw new TypeError("Cannot call a class as a function");
//   }
// }
//
// Key signal: 2 params, single if statement with !(param1 instanceof param2),
// throws TypeError.
// ---------------------------------------------------------------------------

fn is_class_call_check_fn(func: &Function) -> bool {
    let Some(ctx) = MatchContext::from_params(func, &["instance", "constructor"]) else {
        return false;
    };

    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return false,
    };

    if body.stmts.len() != 1 {
        return false;
    }
    let Stmt::If(if_stmt) = &body.stmts[0] else {
        return false;
    };

    if !matches_negated_instanceof(&ctx, &if_stmt.test, "instance", "constructor") {
        return false;
    }

    matches_throw_type_error(&if_stmt.cons)
}

/// Match `!(left instanceof right)` with optional parens around the instanceof.
fn matches_negated_instanceof(
    ctx: &MatchContext,
    expr: &Expr,
    left: &str,
    right: &str,
) -> bool {
    let Expr::Unary(unary) = expr else {
        return false;
    };
    if unary.op != UnaryOp::Bang {
        return false;
    }
    let inner = match unary.arg.as_ref() {
        Expr::Paren(p) => p.expr.as_ref(),
        other => other,
    };
    let Expr::Bin(bin) = inner else { return false };
    bin.op == BinaryOp::InstanceOf
        && ctx.is_binding(&bin.left, left)
        && ctx.is_binding(&bin.right, right)
}

/// Match `throw new TypeError(...)` — bare or wrapped in a block.
fn matches_throw_type_error(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Throw(throw) => is_new_type_error(&throw.arg),
        Stmt::Block(block) if block.stmts.len() == 1 => {
            if let Stmt::Throw(throw) = &block.stmts[0] {
                is_new_type_error(&throw.arg)
            } else {
                false
            }
        }
        _ => false,
    }
}

fn is_new_type_error(expr: &Expr) -> bool {
    let Expr::New(new_expr) = expr else {
        return false;
    };
    matches!(new_expr.callee.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "TypeError")
}

// ---------------------------------------------------------------------------
// possibleConstructorReturn body-shape matcher
//
// function _possibleConstructorReturn(self, call) {
//   if (!self) throw new ReferenceError("this hasn't been initialised...");
//   if (!call || typeof call != "object" && typeof call != "function") return self;
//   return call;
// }
//
// Key signal: 2 params, first stmt throws ReferenceError on !param1,
// second stmt tests typeof param2, returns param1 or param2.
// ---------------------------------------------------------------------------

fn is_possible_constructor_return_fn(func: &Function) -> bool {
    let Some(ctx) = MatchContext::from_params(func, &["self", "call"]) else {
        return false;
    };

    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return false,
    };

    if body.stmts.len() < 2 {
        return false;
    }

    // First statement: if (!self) { throw new ReferenceError(...) }
    let Stmt::If(first_if) = &body.stmts[0] else {
        return false;
    };
    let Expr::Unary(unary) = first_if.test.as_ref() else {
        return false;
    };
    if unary.op != UnaryOp::Bang {
        return false;
    }
    if !ctx.is_binding(&unary.arg, "self") {
        return false;
    }
    let Some(throw_expr) = extract_throw_arg(&first_if.cons) else {
        return false;
    };
    if !is_new_reference_error(throw_expr) {
        return false;
    }

    // Last statement must be a return.
    // 3-stmt form: if-throw, if-return-self, return-call
    // 2-stmt form: if-throw, return-ternary
    let Stmt::Return(ReturnStmt {
        arg: Some(ret_arg), ..
    }) = body.stmts.last().unwrap()
    else {
        return false;
    };

    if body.stmts.len() >= 3 {
        return ctx.is_binding(ret_arg, "call");
    }

    true
}

// ---------------------------------------------------------------------------
// _assertThisInitialized
//
// function p(e) {
//     if (e === undefined) {
//         throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
//     }
//     return e;
// }
//
// Key signal: 1 param, throws ReferenceError with the Babel-specific message,
// returns param. We match on the error message text to avoid false positives.
// ---------------------------------------------------------------------------

fn is_assert_this_initialized_fn(func: &Function) -> bool {
    let Some(ctx) = MatchContext::from_params(func, &["self"]) else {
        return false;
    };

    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return false,
    };

    if body.stmts.len() != 2 {
        return false;
    }

    // First statement: if (...) { throw new ReferenceError("this hasn't been initialised...") }
    let Stmt::If(if_stmt) = &body.stmts[0] else {
        return false;
    };
    if if_stmt.alt.is_some() {
        return false;
    }

    let throw_expr = extract_throw_arg(&if_stmt.cons);
    let Some(throw_expr) = throw_expr else {
        return false;
    };
    if !is_new_reference_error_with_babel_message(throw_expr) {
        return false;
    }

    // Second statement: return self
    let Stmt::Return(ReturnStmt {
        arg: Some(ret_arg), ..
    }) = &body.stmts[1]
    else {
        return false;
    };
    ctx.is_binding(ret_arg, "self")
}

/// Extract the throw argument from a bare throw or a block-wrapped throw.
fn extract_throw_arg(stmt: &Stmt) -> Option<&Expr> {
    match stmt {
        Stmt::Throw(throw) => Some(&*throw.arg),
        Stmt::Block(block) if block.stmts.len() == 1 => match &block.stmts[0] {
            Stmt::Throw(throw) => Some(&*throw.arg),
            _ => None,
        },
        _ => None,
    }
}

fn is_new_reference_error_with_babel_message(expr: &Expr) -> bool {
    let Expr::New(new_expr) = expr else {
        return false;
    };
    let Expr::Ident(id) = new_expr.callee.as_ref() else {
        return false;
    };
    if id.sym.as_ref() != "ReferenceError" {
        return false;
    }
    let Some(args) = &new_expr.args else {
        return false;
    };
    if args.len() != 1 {
        return false;
    }
    let Expr::Lit(Lit::Str(s)) = args[0].expr.as_ref() else {
        return false;
    };
    s.value
        .as_str()
        .is_some_and(|v| v.contains("this hasn't been initialised"))
}

fn is_new_reference_error(expr: &Expr) -> bool {
    let Expr::New(new_expr) = expr else {
        return false;
    };
    let Expr::Ident(id) = new_expr.callee.as_ref() else {
        return false;
    };
    id.sym.as_ref() == "ReferenceError"
}

// ---------------------------------------------------------------------------
// _objectWithoutProperties / _objectWithoutPropertiesLoose
//
// Both variants take (source, excluded_keys_array) and return a new object
// with the excluded keys filtered out. Two body shapes exist:
//
// Variant A (for-in + hasOwnProperty):
//   var t = {}; for (var k in s) { excl.indexOf(k)...; t[k] = s[k]; } return t;
//
// Variant B (Object.keys + for loop):
//   if (s == null) return {};
//   var t = {}; var keys = Object.keys(s);
//   for (i = 0; i < keys.length; i++) { excl.indexOf(...)...; t[k] = s[k]; }
//   return t;
//
// Key signal: 2 params, body uses `.indexOf` on the second param,
// initializes an empty object, and returns it.
// ---------------------------------------------------------------------------

fn is_object_without_properties_fn(func: &Function) -> bool {
    if func.params.len() != 2 {
        return false;
    }
    let Pat::Ident(param1) = &func.params[0].pat else {
        return false;
    };
    let Pat::Ident(param2) = &func.params[1].pat else {
        return false;
    };

    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return false,
    };

    if body.stmts.len() < 3 {
        return false;
    }

    // Find the accumulator: the variable initialized with `{}`.
    // Check both top-level statements and for-loop init expressions,
    // since minified code often has `for(var o={},i=Object.keys(e),...)`
    let Some((accum_sym, accum_ctxt)) = find_empty_object_accumulator(&body.stmts)
        .or_else(|| find_accumulator_in_for_init(&body.stmts))
    else {
        return false;
    };

    // Last statement must return the accumulator
    let returns_accum = matches!(
        body.stmts.last(),
        Some(Stmt::Return(ReturnStmt { arg: Some(arg), .. }))
            if matches!(arg.as_ref(), Expr::Ident(id) if id.sym == accum_sym && id.ctxt == accum_ctxt)
    );
    if !returns_accum {
        return false;
    }

    // Match known Babel loop shapes. Two variants:
    //
    // Variant A (for-in + hasOwnProperty):
    //   for (var k in source) {
    //       excluded.indexOf(k) >= 0 || Object.prototype.hasOwnProperty.call(source, k) && (accum[k] = source[k]);
    //   }
    //
    // Variant B (Object.keys + for loop with indexOf on param2 in loop body):
    //   for (var o={},i=Object.keys(e),r=0; r<i.length; r++) {
    //       ... if (!(t.indexOf(n) >= 0)) { o[n] = e[n]; } ...
    //   }
    //
    // For variant B, the accumulator may be in the for-init (minified form)
    // rather than a separate statement. The indexOf + accumulator-return is
    // sufficient because the for-init accumulator is a very specific pattern.
    for stmt in &body.stmts {
        match stmt {
            Stmt::ForIn(f)
                if for_in_loop_has_owp_shape(
                    f,
                    (&param1.id.sym, param1.id.ctxt),
                    (&param2.id.sym, param2.id.ctxt),
                    (&accum_sym, accum_ctxt),
                ) =>
            {
                return true;
            }
            Stmt::For(f) => {
                let mut checker = GuardedCopyInIfChecker::new(
                    (&param1.id.sym, param1.id.ctxt),
                    (&param2.id.sym, param2.id.ctxt),
                    (&accum_sym, accum_ctxt),
                );
                f.body.visit_with(&mut checker);
                if checker.found {
                    return true;
                }
                // Minified form: expression-level || guard instead of if-statement.
                // e.g. `t.indexOf(n) >= 0 || (o[n] = e[n])`
                if for_body_has_or_guarded_copy(
                    &f.body,
                    (&param1.id.sym, param1.id.ctxt),
                    (&param2.id.sym, param2.id.ctxt),
                    (&accum_sym, accum_ctxt),
                ) {
                    return true;
                }
            }
            _ => {}
        }
    }

    false
}

/// Find the binding (name + context) of the variable initialized with `{}`.
fn find_empty_object_accumulator(stmts: &[Stmt]) -> Option<(Atom, SyntaxContext)> {
    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        for decl in &var.decls {
            let Pat::Ident(bi) = &decl.name else {
                continue;
            };
            if let Some(init) = &decl.init {
                if matches!(init.as_ref(), Expr::Object(obj) if obj.props.is_empty()) {
                    return Some((bi.id.sym.clone(), bi.id.ctxt));
                }
            }
        }
    }
    None
}

#[derive(Clone, PartialEq, Eq)]
enum ComputedKey {
    Ident(Atom, SyntaxContext),
    Member {
        obj: Atom,
        obj_ctxt: SyntaxContext,
        prop: Atom,
        prop_ctxt: SyntaxContext,
    },
}

struct GuardedCopyInIfChecker<'a> {
    param1_sym: &'a Atom,
    param1_ctxt: SyntaxContext,
    param2_sym: &'a Atom,
    param2_ctxt: SyntaxContext,
    accum_sym: &'a Atom,
    accum_ctxt: SyntaxContext,
    found: bool,
}

impl<'a> GuardedCopyInIfChecker<'a> {
    fn new(
        param1: (&'a Atom, SyntaxContext),
        param2: (&'a Atom, SyntaxContext),
        accum: (&'a Atom, SyntaxContext),
    ) -> Self {
        Self {
            param1_sym: param1.0,
            param1_ctxt: param1.1,
            param2_sym: param2.0,
            param2_ctxt: param2.1,
            accum_sym: accum.0,
            accum_ctxt: accum.1,
            found: false,
        }
    }
}

impl Visit for GuardedCopyInIfChecker<'_> {
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

    fn visit_if_stmt(&mut self, if_stmt: &IfStmt) {
        let included_keys = index_guard_keys_for_polarity(
            &if_stmt.test,
            self.param2_sym,
            self.param2_ctxt,
            GuardPolarity::Included,
        );
        if !included_keys.is_empty()
            && stmt_contains_matching_copy(
                &if_stmt.cons,
                (self.param1_sym, self.param1_ctxt),
                (self.accum_sym, self.accum_ctxt),
                &included_keys,
            )
        {
            self.found = true;
            return;
        }

        let excluded_keys = index_guard_keys_for_polarity(
            &if_stmt.test,
            self.param2_sym,
            self.param2_ctxt,
            GuardPolarity::Excluded,
        );
        if !excluded_keys.is_empty() {
            if let Some(alt) = &if_stmt.alt {
                if stmt_contains_matching_copy(
                    alt,
                    (self.param1_sym, self.param1_ctxt),
                    (self.accum_sym, self.accum_ctxt),
                    &excluded_keys,
                ) {
                    self.found = true;
                    return;
                }
            }
        }

        if_stmt.visit_children_with(self);
    }
}

/// Find accumulator inside a for-loop's init (e.g. `for(var o={},i=Object.keys(e);...)`).
fn find_accumulator_in_for_init(stmts: &[Stmt]) -> Option<(Atom, SyntaxContext)> {
    for stmt in stmts {
        let Stmt::For(for_stmt) = stmt else { continue };
        let Some(swc_core::ecma::ast::VarDeclOrExpr::VarDecl(var)) = &for_stmt.init else {
            continue;
        };
        for decl in &var.decls {
            let Pat::Ident(bi) = &decl.name else {
                continue;
            };
            if let Some(init) = &decl.init {
                if matches!(init.as_ref(), Expr::Object(obj) if obj.props.is_empty()) {
                    return Some((bi.id.sym.clone(), bi.id.ctxt));
                }
            }
        }
    }
    None
}

fn for_in_loop_has_owp_shape(
    for_in: &swc_core::ecma::ast::ForInStmt,
    param1: (&Atom, SyntaxContext),
    param2: (&Atom, SyntaxContext),
    accum: (&Atom, SyntaxContext),
) -> bool {
    let Some(loop_key) = for_in_key(&for_in.left) else {
        return false;
    };
    let Expr::Ident(source) = for_in.right.as_ref() else {
        return false;
    };
    if source.sym != *param1.0 || source.ctxt != param1.1 {
        return false;
    }

    for_in_body_has_canonical_expr(&for_in.body, param1, param2, accum, loop_key)
}

fn for_in_key(left: &ForHead) -> Option<ComputedKey> {
    match left {
        ForHead::VarDecl(var) => {
            if var.decls.len() != 1 || var.decls[0].init.is_some() {
                return None;
            }
            let Pat::Ident(binding) = &var.decls[0].name else {
                return None;
            };
            Some(ComputedKey::Ident(binding.id.sym.clone(), binding.id.ctxt))
        }
        ForHead::Pat(pat) => {
            let Pat::Ident(binding) = pat.as_ref() else {
                return None;
            };
            Some(ComputedKey::Ident(binding.id.sym.clone(), binding.id.ctxt))
        }
        _ => None,
    }
}

fn copy_key_from_source_to_accum(
    assign: &swc_core::ecma::ast::AssignExpr,
    source_sym: &Atom,
    source_ctxt: SyntaxContext,
    accum_sym: &Atom,
    accum_ctxt: SyntaxContext,
) -> Option<ComputedKey> {
    use swc_core::ecma::ast::{AssignTarget, SimpleAssignTarget};

    let AssignTarget::Simple(SimpleAssignTarget::Member(left)) = &assign.left else {
        return None;
    };
    let Expr::Ident(left_obj) = left.obj.as_ref() else {
        return None;
    };
    if left_obj.sym != *accum_sym || left_obj.ctxt != accum_ctxt {
        return None;
    }
    let left_key = computed_member_key(&left.prop)?;

    let Expr::Member(right) = assign.right.as_ref() else {
        return None;
    };
    let Expr::Ident(right_obj) = right.obj.as_ref() else {
        return None;
    };
    if right_obj.sym != *source_sym || right_obj.ctxt != source_ctxt {
        return None;
    }
    let right_key = computed_member_key(&right.prop)?;
    if left_key == right_key {
        Some(left_key)
    } else {
        None
    }
}

fn computed_ident_key(prop: &MemberProp) -> Option<(Atom, SyntaxContext)> {
    let MemberProp::Computed(computed) = prop else {
        return None;
    };
    let Expr::Ident(id) = computed.expr.as_ref() else {
        return None;
    };
    Some((id.sym.clone(), id.ctxt))
}

fn computed_member_key(prop: &MemberProp) -> Option<ComputedKey> {
    let MemberProp::Computed(computed) = prop else {
        return None;
    };
    computed_key_expr(computed.expr.as_ref())
}

fn computed_key_expr(expr: &Expr) -> Option<ComputedKey> {
    match expr {
        Expr::Ident(id) => Some(ComputedKey::Ident(id.sym.clone(), id.ctxt)),
        Expr::Member(member) => {
            let Expr::Ident(obj) = member.obj.as_ref() else {
                return None;
            };
            let (prop, prop_ctxt) = computed_ident_key(&member.prop)?;
            Some(ComputedKey::Member {
                obj: obj.sym.clone(),
                obj_ctxt: obj.ctxt,
                prop,
                prop_ctxt,
            })
        }
        _ => None,
    }
}

fn is_has_own_property_call(
    call: &swc_core::ecma::ast::CallExpr,
    source_sym: &Atom,
    source_ctxt: SyntaxContext,
    required_key: &Option<ComputedKey>,
) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(call_member) = callee.as_ref() else {
        return false;
    };
    if !matches!(&call_member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "call") {
        return false;
    }
    let Expr::Member(has_own_member) = call_member.obj.as_ref() else {
        return false;
    };
    if !matches!(&has_own_member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "hasOwnProperty")
    {
        return false;
    }
    if call.args.len() < 2 {
        return false;
    }
    let Expr::Ident(source) = call.args[0].expr.as_ref() else {
        return false;
    };
    if source.sym != *source_sym || source.ctxt != source_ctxt {
        return false;
    }
    let Some(key) = computed_key_expr(call.args[1].expr.as_ref()) else {
        return false;
    };
    required_key
        .as_ref()
        .is_none_or(|required| *required == key)
}

fn for_in_body_has_canonical_expr(
    body: &Stmt,
    param1: (&Atom, SyntaxContext),
    param2: (&Atom, SyntaxContext),
    accum: (&Atom, SyntaxContext),
    loop_key: ComputedKey,
) -> bool {
    // Shape 1 (expression): indexOf(k) >= 0 || hasOwn.call(e, k) && (accum[k] = e[k])
    let mut checker = OrGuardChecker {
        param1,
        param2,
        accum,
        required_key: Some(loop_key.clone()),
        require_has_own: true,
        found: false,
    };
    body.visit_with(&mut checker);
    if checker.found {
        return true;
    }

    // Shape 2 (if-statement): if (hasOwn && indexOf < 0) { accum[k] = source[k]; }
    let mut if_checker = GuardedCopyInIfChecker::new(
        (param1.0, param1.1),
        (param2.0, param2.1),
        (accum.0, accum.1),
    );
    body.visit_with(&mut if_checker);
    if if_checker.found {
        return stmt_has_has_own_property_call(body, param1.0, param1.1, &Some(loop_key));
    }
    false
}

/// Check for `excluded.indexOf(key) >= 0 || (accum[key] = source[key])` patterns
/// in expression statements (minified OWP form without if-statements).
fn for_body_has_or_guarded_copy(
    body: &Stmt,
    param1: (&Atom, SyntaxContext),
    param2: (&Atom, SyntaxContext),
    accum: (&Atom, SyntaxContext),
) -> bool {
    let mut checker = OrGuardChecker {
        param1,
        param2,
        accum,
        required_key: None,
        require_has_own: false,
        found: false,
    };
    body.visit_with(&mut checker);
    checker.found
}

struct OrGuardChecker<'a> {
    param1: (&'a Atom, SyntaxContext),
    param2: (&'a Atom, SyntaxContext),
    accum: (&'a Atom, SyntaxContext),
    required_key: Option<ComputedKey>,
    require_has_own: bool,
    found: bool,
}

impl Visit for OrGuardChecker<'_> {
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

    fn visit_bin_expr(&mut self, bin: &swc_core::ecma::ast::BinExpr) {
        if bin.op == BinaryOp::LogicalOr {
            let index_keys = index_guard_keys_for_polarity(
                &bin.left,
                self.param2.0,
                self.param2.1,
                GuardPolarity::Excluded,
            );
            let index_keys = filter_required_key(index_keys, &self.required_key);
            if !index_keys.is_empty() {
                let mut copy_collector = CopyKeyCollector::new(self.param1, self.accum);
                bin.right.visit_with(&mut copy_collector);
                let has_copy = keys_have_match(&copy_collector.keys, &index_keys);
                let has_required_has_own = !self.require_has_own
                    || expr_has_has_own_property_call(
                        &bin.right,
                        self.param1.0,
                        self.param1.1,
                        &self.required_key,
                    );
                if has_copy && has_required_has_own {
                    self.found = true;
                    return;
                }
            }
        }
        bin.visit_children_with(self);
    }
}

struct CopyKeyCollector<'a> {
    param1_sym: &'a Atom,
    param1_ctxt: SyntaxContext,
    accum_sym: &'a Atom,
    accum_ctxt: SyntaxContext,
    keys: Vec<ComputedKey>,
}

impl<'a> CopyKeyCollector<'a> {
    fn new(param1: (&'a Atom, SyntaxContext), accum: (&'a Atom, SyntaxContext)) -> Self {
        Self {
            param1_sym: param1.0,
            param1_ctxt: param1.1,
            accum_sym: accum.0,
            accum_ctxt: accum.1,
            keys: Vec::new(),
        }
    }
}

impl Visit for CopyKeyCollector<'_> {
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

    fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
        if let Some(key) = copy_key_from_source_to_accum(
            assign,
            self.param1_sym,
            self.param1_ctxt,
            self.accum_sym,
            self.accum_ctxt,
        ) {
            self.keys.push(key);
        }
        assign.visit_children_with(self);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GuardPolarity {
    Excluded,
    Included,
}

fn index_guard_keys_for_polarity(
    expr: &Expr,
    param2_sym: &Atom,
    param2_ctxt: SyntaxContext,
    wanted: GuardPolarity,
) -> Vec<ComputedKey> {
    index_guard_keys(expr, param2_sym, param2_ctxt)
        .into_iter()
        .filter_map(|(key, polarity)| (polarity == wanted).then_some(key))
        .collect()
}

fn index_guard_keys(
    expr: &Expr,
    param2_sym: &Atom,
    param2_ctxt: SyntaxContext,
) -> Vec<(ComputedKey, GuardPolarity)> {
    match unparen_expr(expr) {
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => {
            index_guard_keys(unary.arg.as_ref(), param2_sym, param2_ctxt)
                .into_iter()
                .map(|(key, polarity)| (key, flip_guard_polarity(polarity)))
                .collect()
        }
        Expr::Bin(bin) if bin.op == BinaryOp::LogicalAnd => {
            let mut keys = index_guard_keys(&bin.left, param2_sym, param2_ctxt);
            keys.extend(index_guard_keys(&bin.right, param2_sym, param2_ctxt));
            keys
        }
        Expr::Bin(bin) => match_index_guard_bin(bin, param2_sym, param2_ctxt)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

fn match_index_guard_bin(
    bin: &swc_core::ecma::ast::BinExpr,
    param2_sym: &Atom,
    param2_ctxt: SyntaxContext,
) -> Option<(ComputedKey, GuardPolarity)> {
    if let Some(key) = key_from_index_of_call(&bin.left, param2_sym, param2_ctxt) {
        return polarity_for_index_literal_compare(bin.op, &bin.right)
            .map(|polarity| (key, polarity));
    }
    if let Some(key) = key_from_index_of_call(&bin.right, param2_sym, param2_ctxt) {
        return polarity_for_literal_index_compare(bin.op, &bin.left)
            .map(|polarity| (key, polarity));
    }
    None
}

fn key_from_index_of_call(
    expr: &Expr,
    param2_sym: &Atom,
    param2_ctxt: SyntaxContext,
) -> Option<ComputedKey> {
    let Expr::Call(call) = unparen_expr(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "indexOf") {
        return None;
    }
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return None;
    };
    if obj.sym != *param2_sym || obj.ctxt != param2_ctxt {
        return None;
    }
    let first_arg = call.args.first()?;
    computed_key_expr(first_arg.expr.as_ref())
}

fn polarity_for_index_literal_compare(op: BinaryOp, right: &Expr) -> Option<GuardPolarity> {
    if is_number_literal(right, 0.0) {
        match op {
            BinaryOp::GtEq => Some(GuardPolarity::Excluded),
            BinaryOp::Lt => Some(GuardPolarity::Included),
            _ => None,
        }
    } else if is_number_literal(right, -1.0) {
        match op {
            BinaryOp::Gt | BinaryOp::NotEq | BinaryOp::NotEqEq => Some(GuardPolarity::Excluded),
            BinaryOp::LtEq | BinaryOp::EqEq | BinaryOp::EqEqEq => Some(GuardPolarity::Included),
            _ => None,
        }
    } else {
        None
    }
}

fn polarity_for_literal_index_compare(op: BinaryOp, left: &Expr) -> Option<GuardPolarity> {
    if is_number_literal(left, 0.0) {
        match op {
            BinaryOp::LtEq => Some(GuardPolarity::Excluded),
            BinaryOp::Gt => Some(GuardPolarity::Included),
            _ => None,
        }
    } else if is_number_literal(left, -1.0) {
        match op {
            BinaryOp::Lt | BinaryOp::NotEq | BinaryOp::NotEqEq => Some(GuardPolarity::Excluded),
            BinaryOp::GtEq | BinaryOp::EqEq | BinaryOp::EqEqEq => Some(GuardPolarity::Included),
            _ => None,
        }
    } else {
        None
    }
}

fn is_number_literal(expr: &Expr, expected: f64) -> bool {
    match unparen_expr(expr) {
        Expr::Lit(Lit::Num(num)) => num.value == expected,
        Expr::Unary(unary) if unary.op == UnaryOp::Minus => {
            matches!(unary.arg.as_ref(), Expr::Lit(Lit::Num(num)) if -num.value == expected)
        }
        _ => false,
    }
}

fn unparen_expr(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => unparen_expr(&paren.expr),
        _ => expr,
    }
}

fn flip_guard_polarity(polarity: GuardPolarity) -> GuardPolarity {
    match polarity {
        GuardPolarity::Excluded => GuardPolarity::Included,
        GuardPolarity::Included => GuardPolarity::Excluded,
    }
}

fn filter_required_key(
    keys: Vec<ComputedKey>,
    required_key: &Option<ComputedKey>,
) -> Vec<ComputedKey> {
    keys.into_iter()
        .filter(|key| required_key.as_ref().is_none_or(|required| required == key))
        .collect()
}

fn keys_have_match(copy_keys: &[ComputedKey], guard_keys: &[ComputedKey]) -> bool {
    copy_keys
        .iter()
        .any(|copy_key| guard_keys.iter().any(|guard_key| guard_key == copy_key))
}

fn stmt_contains_matching_copy(
    stmt: &Stmt,
    param1: (&Atom, SyntaxContext),
    accum: (&Atom, SyntaxContext),
    guard_keys: &[ComputedKey],
) -> bool {
    let mut copy_collector = CopyKeyCollector::new(param1, accum);
    stmt.visit_with(&mut copy_collector);
    keys_have_match(&copy_collector.keys, guard_keys)
}

fn expr_has_has_own_property_call(
    expr: &Expr,
    source_sym: &Atom,
    source_ctxt: SyntaxContext,
    required_key: &Option<ComputedKey>,
) -> bool {
    struct HasOwnCollector<'a> {
        source_sym: &'a Atom,
        source_ctxt: SyntaxContext,
        required_key: &'a Option<ComputedKey>,
        found: bool,
    }

    impl Visit for HasOwnCollector<'_> {
        fn visit_function(&mut self, _: &Function) {}
        fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if is_has_own_property_call(call, self.source_sym, self.source_ctxt, self.required_key)
            {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }

    let mut collector = HasOwnCollector {
        source_sym,
        source_ctxt,
        required_key,
        found: false,
    };
    expr.visit_with(&mut collector);
    collector.found
}

fn stmt_has_has_own_property_call(
    stmt: &Stmt,
    source_sym: &Atom,
    source_ctxt: SyntaxContext,
    required_key: &Option<ComputedKey>,
) -> bool {
    struct HasOwnCollector<'a> {
        source_sym: &'a Atom,
        source_ctxt: SyntaxContext,
        required_key: &'a Option<ComputedKey>,
        found: bool,
    }

    impl Visit for HasOwnCollector<'_> {
        fn visit_function(&mut self, _: &Function) {}
        fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if is_has_own_property_call(call, self.source_sym, self.source_ctxt, self.required_key)
            {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }

    let mut collector = HasOwnCollector {
        source_sym,
        source_ctxt,
        required_key,
        found: false,
    };
    stmt.visit_with(&mut collector);
    collector.found
}

fn is_extends_fn(func: &Function) -> bool {
    if !func.params.is_empty() {
        return false;
    }

    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return false,
    };

    let mut markers = BodyMarkerState::default();
    scan_stmts_for_markers(&body.stmts, &mut markers);

    markers.has_object_assign && markers.has_apply_this_arguments
}

// ---------------------------------------------------------------------------
// _extends polyfill form: Object.assign || function(target) { for-in arguments }
//
// var _extends = Object.assign || function(target) {
//   for (var i = 1; i < arguments.length; i++) {
//     var source = arguments[i];
//     for (var key in source) {
//       if (Object.prototype.hasOwnProperty.call(source, key))
//         target[key] = source[key];
//     }
//   }
//   return target;
// };
//
// Key signal: 1 param, references `arguments`, loops with for-in, returns param.
// ---------------------------------------------------------------------------

/// Check if an expression is `Object.assign`.
fn is_object_assign_ref(expr: &Expr) -> bool {
    let Expr::Member(member) = expr else {
        return false;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };
    obj.sym.as_ref() == "Object" && is_member_prop_name(&member.prop, "assign")
}

/// Check if an expression is the inline polyfill function for _extends.
/// Shape: function(target) { for (...; i < arguments.length; ...) { for (key in source) { ... } } return target; }
fn is_extends_polyfill_fn(expr: &Expr) -> bool {
    let func = match expr {
        Expr::Fn(fn_expr) => &fn_expr.function,
        _ => return false,
    };

    let Some(ctx) = MatchContext::from_params(func, &["target"]) else {
        return false;
    };

    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return false,
    };

    let mut markers = BodyMarkerState::default();
    scan_stmts_for_markers(&body.stmts, &mut markers);
    if !markers.has_arguments_ref {
        return false;
    }

    matches!(
        body.stmts.last(),
        Some(Stmt::Return(ReturnStmt { arg: Some(arg), .. }))
            if ctx.is_binding(arg, "target")
    )
}

// ---------------------------------------------------------------------------
// objectSpread / objectSpread2 body-shape matcher
//
// function _objectSpread2(target) {
//   for (var i = 1; i < arguments.length; i++) { ... Object.defineProperty ... }
//   return target;
// }
//
// Key signal: 1 param, references `arguments`, contains Object.defineProperty
// or Object.getOwnPropertyDescriptor/getOwnPropertyDescriptors, returns param.
// ---------------------------------------------------------------------------

fn is_object_spread_fn(func: &Function) -> bool {
    let Some(ctx) = MatchContext::from_params(func, &["target"]) else {
        return false;
    };

    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return false,
    };

    let mut markers = BodyMarkerState::default();
    scan_stmts_for_markers(&body.stmts, &mut markers);

    if !markers.has_arguments_ref {
        return false;
    }
    if !markers.has_object_define_property || !markers.has_object_get_own_property_descriptor {
        return false;
    }

    matches!(
        body.stmts.last(),
        Some(Stmt::Return(ReturnStmt { arg: Some(arg), .. }))
            if ctx.is_binding(arg, "target")
    )
}

// ---------------------------------------------------------------------------
// slicedToArray body-shape matcher
//
// Babel 7+: function(arr, i) { return f(arr) || g(arr, i) || h(arr, i) || k(); }
// Babel 6:  function(arr, i) { if (Array.isArray(arr)) { ... } else if (Symbol.iterator in ...) { ... } ... }
//
// Key signal: 2 params, body references Symbol.iterator or is a logical-OR
// chain of sub-helper calls with 2 params.
// ---------------------------------------------------------------------------

fn is_sliced_to_array_fn(func: &Function, has_sub_helpers: bool) -> bool {
    if func.params.len() != 2 {
        return false;
    }

    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return false,
    };

    let mut markers = BodyMarkerState::default();
    scan_stmts_for_markers(&body.stmts, &mut markers);

    // Babel 6: references both Symbol.iterator AND Array.isArray
    // (the helper always has both: Array.isArray check + iterator protocol fallback)
    if markers.has_symbol_iterator && markers.has_array_is_array {
        return true;
    }

    // Babel 7+ form: single return of logical-OR chain calling sub-helpers.
    // Only accepted when the module also contains Babel sub-helpers.
    if has_sub_helpers && body.stmts.len() == 1 {
        if let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = &body.stmts[0] {
            if is_babel_helper_or_chain(arg) {
                return true;
            }
        }
    }

    false
}

// ---------------------------------------------------------------------------
// Shared body scanning infrastructure
// ---------------------------------------------------------------------------

#[derive(Default)]
struct BodyMarkerState {
    has_array_is_array: bool,
    has_array_from: bool,
    has_array_constructor: bool,
    has_object_assign: bool,
    has_apply_this_arguments: bool,
    has_arguments_ref: bool,
    has_object_define_property: bool,
    has_object_get_own_property_descriptor: bool,
    has_symbol_iterator: bool,
}

fn scan_stmts_for_markers(stmts: &[Stmt], state: &mut BodyMarkerState) {
    use swc_core::ecma::visit::{Visit, VisitWith};

    struct MarkerVisitor<'a> {
        state: &'a mut BodyMarkerState,
    }

    impl Visit for MarkerVisitor<'_> {
        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Member(member) = callee.as_ref() {
                    // Array.isArray, Array.from
                    if let Expr::Ident(obj) = member.obj.as_ref() {
                        if obj.sym.as_ref() == "Array" {
                            if is_member_prop_name(&member.prop, "isArray") {
                                self.state.has_array_is_array = true;
                            }
                            if is_member_prop_name(&member.prop, "from") {
                                self.state.has_array_from = true;
                            }
                        }
                        if obj.sym.as_ref() == "Object" {
                            if is_member_prop_name(&member.prop, "assign") {
                                self.state.has_object_assign = true;
                            }
                            if is_member_prop_name(&member.prop, "defineProperty")
                                || is_member_prop_name(&member.prop, "defineProperties")
                            {
                                self.state.has_object_define_property = true;
                            }
                            if is_member_prop_name(&member.prop, "getOwnPropertyDescriptor")
                                || is_member_prop_name(&member.prop, "getOwnPropertyDescriptors")
                            {
                                self.state.has_object_get_own_property_descriptor = true;
                            }
                        }
                    }
                    // *.apply(this, arguments)
                    if is_member_prop_name(&member.prop, "apply")
                        && call.args.len() == 2
                        && matches!(call.args[0].expr.as_ref(), Expr::This(..))
                        && matches!(call.args[1].expr.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "arguments")
                    {
                        self.state.has_apply_this_arguments = true;
                    }
                }
                // new Array(len) constructor
                if let Expr::Ident(id) = callee.as_ref() {
                    if id.sym.as_ref() == "Array" {
                        self.state.has_array_constructor = true;
                    }
                }
            }
            call.visit_children_with(self);
        }

        fn visit_new_expr(&mut self, expr: &swc_core::ecma::ast::NewExpr) {
            // new Array(len)
            if let Expr::Ident(id) = expr.callee.as_ref() {
                if id.sym.as_ref() == "Array" {
                    self.state.has_array_constructor = true;
                }
            }
            expr.visit_children_with(self);
        }

        fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
            if ident.sym.as_ref() == "arguments" {
                self.state.has_arguments_ref = true;
            }
        }

        fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
            if let Expr::Ident(obj) = member.obj.as_ref() {
                // Object.assign (as reference, not just as call)
                if obj.sym.as_ref() == "Object" && is_member_prop_name(&member.prop, "assign") {
                    self.state.has_object_assign = true;
                }
                // Symbol.iterator
                if obj.sym.as_ref() == "Symbol" && is_member_prop_name(&member.prop, "iterator") {
                    self.state.has_symbol_iterator = true;
                }
            }
            member.visit_children_with(self);
        }
    }

    let mut visitor = MarkerVisitor { state };
    for stmt in stmts {
        stmt.visit_with(&mut visitor);
    }
}

/// Detect the Babel 7+ helper delegation pattern:
///   `return f(x) || g(x) || h(x) || nonIterableThrow()`
///
/// Key distinguishing feature: the **rightmost** (last evaluated) term is always
/// a 0-arg call (e.g. `_nonIterableSpread()`, `_nonIterableRest()`) that throws
/// a TypeError. Normal fallback chains don't end with a no-arg throwing call.
///
/// Requires at least 3 call terms total.
fn is_babel_helper_or_chain(expr: &Expr) -> bool {
    // The rightmost term of a left-associative || chain is the right child
    // of the outermost BinExpr. Check it's a 0-arg call first.
    let Expr::Bin(outermost) = expr else {
        return false;
    };
    if outermost.op != BinaryOp::LogicalOr {
        return false;
    }
    // Rightmost term must be a 0-arg call (the "throw" helper)
    let Expr::Call(rightmost_call) = outermost.right.as_ref() else {
        return false;
    };
    if !rightmost_call.args.is_empty() {
        return false;
    }

    // Now count all call terms in the chain (including the rightmost)
    let mut call_count = 1; // already counted rightmost
    let mut current: &Expr = &outermost.left;
    loop {
        match current {
            Expr::Bin(bin) if bin.op == BinaryOp::LogicalOr => {
                if matches!(bin.right.as_ref(), Expr::Call(..)) {
                    call_count += 1;
                }
                current = &bin.left;
            }
            Expr::Call(..) => {
                call_count += 1;
                break;
            }
            _ => break,
        }
    }
    call_count >= 3
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::common::{
        sync::Lrc, FileName, Globals, SourceMap, GLOBALS,
    };
    use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};

    fn parse_first_function(code: &str) -> Function {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(Lrc::new(FileName::Anon), code.to_string());
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax::default()),
            Default::default(),
            StringInput::from(&*fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);
        let module = parser.parse_module().expect("failed to parse");
        for item in &module.body {
            if let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) = item {
                return *fn_decl.function.clone();
            }
        }
        panic!("no function declaration found in source");
    }

    #[test]
    fn class_call_check_canonical() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _c(a, b) {
                    if (!(a instanceof b)) {
                        throw new TypeError("Cannot call a class as a function");
                    }
                }"#,
            );
            assert!(is_class_call_check_fn(&f));
        });
    }

    #[test]
    fn class_call_check_no_block_wrapping() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _c(a, b) {
                    if (!(a instanceof b))
                        throw new TypeError("Cannot call a class as a function");
                }"#,
            );
            assert!(is_class_call_check_fn(&f));
        });
    }

    #[test]
    fn class_call_check_with_parens() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _c(a, b) {
                    if (!(a instanceof b)) {
                        throw new TypeError("Cannot call a class as a function");
                    }
                }"#,
            );
            assert!(is_class_call_check_fn(&f));
        });
    }

    #[test]
    fn class_call_check_rejects_wrong_param_count() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _c(a) {
                    if (!(a instanceof Foo)) {
                        throw new TypeError("nope");
                    }
                }"#,
            );
            assert!(!is_class_call_check_fn(&f));
        });
    }

    #[test]
    fn class_call_check_rejects_swapped_operands() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _c(a, b) {
                    if (!(b instanceof a)) {
                        throw new TypeError("nope");
                    }
                }"#,
            );
            assert!(!is_class_call_check_fn(&f));
        });
    }

    #[test]
    fn class_call_check_rejects_non_instanceof() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _c(a, b) {
                    if (!(a === b)) {
                        throw new TypeError("nope");
                    }
                }"#,
            );
            assert!(!is_class_call_check_fn(&f));
        });
    }

    #[test]
    fn class_call_check_rejects_no_throw() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _c(a, b) {
                    if (!(a instanceof b)) {
                        console.log("bad");
                    }
                }"#,
            );
            assert!(!is_class_call_check_fn(&f));
        });
    }

    #[test]
    fn class_call_check_rejects_multiple_stmts() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _c(a, b) {
                    var x = 1;
                    if (!(a instanceof b)) {
                        throw new TypeError("nope");
                    }
                }"#,
            );
            assert!(!is_class_call_check_fn(&f));
        });
    }
}
