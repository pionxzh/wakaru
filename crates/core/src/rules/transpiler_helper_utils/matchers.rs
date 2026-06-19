//! Babel/SWC transpiler-helper body-shape matchers and the per-node detection
//! dispatch (declaration scanning + inline-expression classification).
//!
//! Babel and SWC are unified here: one matcher per helper kind recognizes both
//! (and minified forms) by AST structure, ignoring names. TypeScript/tslib
//! detection lives in `ts_helpers`; the shared low-level AST predicates live in
//! the parent module.

use swc_core::atoms::Atom;
use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, BlockStmt, BlockStmtOrExpr, CallExpr,
    Callee, ComputedPropName, Decl, Expr, ForHead, Function, Ident, IdentName, IfStmt,
    KeyValueProp, Lit, MemberProp, Module, ModuleItem, Pat, Prop, PropName, PropOrSpread,
    ReturnStmt, SimpleAssignTarget, Stmt, UnaryOp, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::rules::helper_matcher::{binding_key, member_prop_name, var_declarator_binding_key};
use crate::rules::match_context::MatchContext;
use crate::utils::paren::strip_parens;

use super::*;

pub(super) fn detect_helper_from_var_decl(
    decl: &VarDeclarator,
    has_sub_helpers: bool,
    unresolved_mark: Option<Mark>,
) -> Option<(BindingKey, TranspilerHelperKind)> {
    let init = decl.init.as_ref()?;
    let key = var_declarator_binding_key(decl)?;

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
    if let Some(kind) = detect_helper_from_require(init, unresolved_mark) {
        return Some((key, kind));
    }

    // var _ird = require("@babel/runtime/helpers/interopRequireDefault").default
    if let Expr::Member(member) = init.as_ref() {
        if let Some(kind) = detect_helper_from_tslib_require_member(member, unresolved_mark) {
            return Some((key, kind));
        }
        if member_prop_name(&member.prop, "default") {
            if let Some(kind) = detect_helper_from_require(&member.obj, unresolved_mark) {
                return Some((key, kind));
            }
        }
    }

    // var _extends = Object.assign || function(target) { ... }
    // This is the Babel 6 or pre-evaluated form of the _extends polyfill.
    if let Expr::Bin(bin) = init.as_ref() {
        if bin.op == BinaryOp::LogicalOr {
            if is_object_assign_ref(&bin.left) && is_extends_polyfill_fn(&bin.right) {
                return Some((key, TranspilerHelperKind::Extends));
            }
            if let Some(kind) = detect_helper_from_expr(&bin.right, has_sub_helpers) {
                return Some((key, kind));
            }
        }
    }

    if is_typeof_polyfill_init(init) {
        return Some((key, TranspilerHelperKind::Typeof));
    }

    if let Some(kind) = generated_helper_name_kind(key.0.as_ref(), init) {
        return Some((key, kind));
    }

    None
}
fn generated_helper_name_kind(name: &str, init: &Expr) -> Option<TranspilerHelperKind> {
    match name {
        // SWC object spread helpers and esbuild object spread helpers.
        "_object_spread" | "_object_spread_props" | "__spreadValues" | "__spreadProps" => {
            matches!(init, Expr::Fn(_) | Expr::Arrow(_))
                .then_some(TranspilerHelperKind::ObjectSpread)
        }
        // SWC object rest helpers and esbuild object rest helper.
        "_object_without_properties" | "_object_without_properties_loose" | "__objRest" => {
            matches!(init, Expr::Fn(_) | Expr::Arrow(_))
                .then_some(TranspilerHelperKind::ObjectWithoutProperties)
        }
        "_tagged_template_literal" => matches!(init, Expr::Fn(_) | Expr::Arrow(_))
            .then_some(TranspilerHelperKind::TaggedTemplateLiteral),
        // Generated subhelpers used only by the spread/rest helpers above.
        "_define_property"
        | "ownKeys"
        | "__defNormalProp"
        | "__defProp"
        | "__defProps"
        | "__getOwnPropDescs"
        | "__getOwnPropSymbols"
        | "__hasOwnProp"
        | "__propIsEnum" => Some(TranspilerHelperKind::HelperDependency),
        _ => None,
    }
}
pub(super) fn generated_fn_helper_name_kind(name: &str) -> Option<TranspilerHelperKind> {
    match name {
        "_object_spread" | "_object_spread_props" => Some(TranspilerHelperKind::ObjectSpread),
        "_object_without_properties" | "_object_without_properties_loose" => {
            Some(TranspilerHelperKind::ObjectWithoutProperties)
        }
        "_tagged_template_literal" => Some(TranspilerHelperKind::TaggedTemplateLiteral),
        "_define_property" | "ownKeys" => Some(TranspilerHelperKind::HelperDependency),
        _ => None,
    }
}
fn detect_helper_from_expr(expr: &Expr, has_sub_helpers: bool) -> Option<TranspilerHelperKind> {
    match expr {
        Expr::Fn(fn_expr) => detect_helper_from_fn(&fn_expr.function, has_sub_helpers),
        Expr::Arrow(arrow) => detect_helper_from_arrow(arrow, has_sub_helpers),
        Expr::Paren(paren) => detect_helper_from_expr(&paren.expr, has_sub_helpers),
        _ => None,
    }
}
fn detect_helper_from_require(
    expr: &Expr,
    unresolved_mark: Option<Mark>,
) -> Option<TranspilerHelperKind> {
    let Expr::Call(call) = expr else { return None };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(id) = callee.as_ref() else {
        return None;
    };
    if !is_unresolved_or_unguarded_ident(id, "require", unresolved_mark) || call.args.len() != 1 {
        return None;
    }
    let Expr::Lit(Lit::Str(s)) = call.args[0].expr.as_ref() else {
        return None;
    };
    detect_helper_from_path(s.value.as_str().unwrap_or(""))
}
pub(super) fn detect_helper_from_fn(
    func: &Function,
    has_sub_helpers: bool,
) -> Option<TranspilerHelperKind> {
    if is_interop_require_default_fn(func) {
        return Some(TranspilerHelperKind::InteropRequireDefault);
    }
    if is_interop_require_wildcard_fn(func) {
        return Some(TranspilerHelperKind::InteropRequireWildcard);
    }
    if is_to_consumable_array_fn(func, has_sub_helpers) {
        return Some(TranspilerHelperKind::ToConsumableArray);
    }
    if is_extends_fn(func) {
        return Some(TranspilerHelperKind::Extends);
    }
    if is_object_spread_fn(func) {
        return Some(TranspilerHelperKind::ObjectSpread);
    }
    if is_sliced_to_array_fn(func, has_sub_helpers) {
        return Some(TranspilerHelperKind::SlicedToArray);
    }
    if is_class_call_check_fn(func) {
        return Some(TranspilerHelperKind::ClassCallCheck);
    }
    if is_possible_constructor_return_fn(func) {
        return Some(TranspilerHelperKind::PossibleConstructorReturn);
    }
    if is_assert_this_initialized_fn(func) {
        return Some(TranspilerHelperKind::AssertThisInitialized);
    }
    if is_object_without_properties_fn(func) {
        return Some(TranspilerHelperKind::ObjectWithoutProperties);
    }
    if is_inherits_fn(func) {
        return Some(TranspilerHelperKind::Inherits);
    }
    if is_call_super_fn(func) {
        return Some(TranspilerHelperKind::CallSuper);
    }
    if is_async_to_generator_fn(func) {
        return Some(TranspilerHelperKind::AsyncToGenerator);
    }
    if is_tagged_template_literal_fn(func) {
        return Some(TranspilerHelperKind::TaggedTemplateLiteral);
    }
    if is_define_property_fn(func) {
        return Some(TranspilerHelperKind::DefineProperty);
    }
    None
}
fn is_tagged_template_literal_fn(func: &Function) -> bool {
    let Some(ctx) = MatchContext::from_params(func, &["strings", "raws"]) else {
        return false;
    };
    let Some(body) = &func.body else {
        return false;
    };

    let signals = collect_tagged_template_literal_signals(body, &ctx);
    signals.slice_copy && signals.freeze_define_raw
}
#[derive(Default)]
struct TaggedTemplateLiteralSignals {
    slice_copy: bool,
    freeze_define_raw: bool,
}
fn collect_tagged_template_literal_signals(
    body: &BlockStmt,
    ctx: &MatchContext,
) -> TaggedTemplateLiteralSignals {
    struct SignalVisitor<'a> {
        ctx: &'a MatchContext,
        signals: TaggedTemplateLiteralSignals,
    }

    impl Visit for SignalVisitor<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if is_strings_slice_zero_call(call, self.ctx) {
                self.signals.slice_copy = true;
            }
            if is_freeze_define_raw_call(call, self.ctx) {
                self.signals.freeze_define_raw = true;
            }
            call.visit_children_with(self);
        }
    }

    let mut visitor = SignalVisitor {
        ctx,
        signals: TaggedTemplateLiteralSignals::default(),
    };
    body.visit_with(&mut visitor);
    visitor.signals
}
fn is_strings_slice_zero_call(call: &CallExpr, ctx: &MatchContext) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return false;
    };
    ctx.is_binding(&member.obj, "strings")
        && member_prop_name(&member.prop, "slice")
        && call.args.len() == 1
        && call.args[0].spread.is_none()
        && matches!(strip_parens(&call.args[0].expr), Expr::Lit(Lit::Num(num)) if num.value == 0.0)
}
fn is_freeze_define_raw_call(call: &CallExpr, ctx: &MatchContext) -> bool {
    if !is_object_static_call(call, "freeze") || call.args.len() != 1 {
        return false;
    }
    let Expr::Call(define_call) = strip_parens(&call.args[0].expr) else {
        return false;
    };
    if !is_object_static_call(define_call, "defineProperties") || define_call.args.len() != 2 {
        return false;
    }
    if !ctx.is_binding(&define_call.args[0].expr, "strings") {
        return false;
    }
    let Expr::Object(descriptor) = strip_parens(&define_call.args[1].expr) else {
        return false;
    };
    descriptor.props.iter().any(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return false;
        };
        let Prop::KeyValue(raw_prop) = prop.as_ref() else {
            return false;
        };
        if prop_name_as_str(&raw_prop.key) != Some("raw") {
            return false;
        }
        raw_descriptor_freezes_raws(&raw_prop.value, ctx)
    })
}
fn raw_descriptor_freezes_raws(expr: &Expr, ctx: &MatchContext) -> bool {
    let Expr::Object(raw_descriptor) = strip_parens(expr) else {
        return false;
    };
    raw_descriptor.props.iter().any(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return false;
        };
        let Prop::KeyValue(value_prop) = prop.as_ref() else {
            return false;
        };
        if prop_name_as_str(&value_prop.key) != Some("value") {
            return false;
        }
        let Expr::Call(freeze_call) = strip_parens(&value_prop.value) else {
            return false;
        };
        is_object_static_call(freeze_call, "freeze")
            && freeze_call.args.len() == 1
            && ctx.is_binding(&freeze_call.args[0].expr, "raws")
    })
}
fn is_object_static_call(call: &CallExpr, prop: &str) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return false;
    };
    is_object_member(member, prop)
}
fn is_define_property_fn(func: &Function) -> bool {
    let Some(ctx) = MatchContext::from_params(func, &["target", "key", "value"]) else {
        return false;
    };

    let Some(body) = &func.body else {
        return false;
    };
    if body.stmts.len() != 2 {
        return false;
    }

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
    is_key_or_key_normalization(left, ctx) && ctx.is_binding(right, "target")
}
fn is_key_or_key_normalization(expr: &Expr, ctx: &MatchContext) -> bool {
    let expr = strip_parens(expr);
    if ctx.is_binding(expr, "key") {
        return true;
    }

    let Expr::Assign(AssignExpr {
        op: AssignOp::Assign,
        left,
        right,
        ..
    }) = expr
    else {
        return false;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) = left else {
        return false;
    };
    if !ctx.is_ident(&ident.id, "key") {
        return false;
    }

    let Expr::Call(CallExpr { args, .. }) = right.as_ref() else {
        return false;
    };
    args.len() == 1 && args[0].spread.is_none() && ctx.is_binding(&args[0].expr, "key")
}
fn if_consequent_matches_define_property(stmt: &Stmt, ctx: &MatchContext) -> bool {
    let expr = match stmt {
        Stmt::Expr(expr_stmt) => expr_stmt.expr.as_ref(),
        Stmt::Block(BlockStmt { stmts, .. }) if stmts.len() == 1 => {
            let Stmt::Expr(expr_stmt) = &stmts[0] else {
                return false;
            };
            expr_stmt.expr.as_ref()
        }
        _ => return false,
    };
    let Expr::Call(call) = expr else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };
    if obj.sym.as_ref() != "Object" {
        return false;
    }
    if !matches!(&member.prop, MemberProp::Ident(id) if id.sym.as_ref() == "defineProperty") {
        return false;
    }
    if call.args.len() != 3 {
        return false;
    }
    if !ctx.is_binding(&call.args[0].expr, "target") {
        return false;
    }
    if !ctx.is_binding(&call.args[1].expr, "key") {
        return false;
    }

    let Expr::Object(obj_lit) = call.args[2].expr.as_ref() else {
        return false;
    };
    let mut has_value = false;
    let mut has_enumerable = false;
    let mut has_configurable = false;
    let mut has_writable = false;
    for prop in &obj_lit.props {
        let PropOrSpread::Prop(prop) = prop else {
            return false;
        };
        let Prop::KeyValue(KeyValueProp { key, value }) = prop.as_ref() else {
            return false;
        };
        let Some(name) = prop_name_ident(key) else {
            return false;
        };
        match name.as_ref() {
            "value" => has_value = ctx.is_binding(value, "value"),
            "enumerable" => {
                if !matches!(value.as_ref(), Expr::Lit(Lit::Bool(bool_lit)) if bool_lit.value) {
                    return false;
                }
                has_enumerable = true;
            }
            "configurable" => {
                if !matches!(value.as_ref(), Expr::Lit(Lit::Bool(bool_lit)) if bool_lit.value) {
                    return false;
                }
                has_configurable = true;
            }
            "writable" => {
                if !matches!(value.as_ref(), Expr::Lit(Lit::Bool(bool_lit)) if bool_lit.value) {
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
        Stmt::Expr(expr_stmt) => expr_stmt.expr.as_ref(),
        Stmt::Block(BlockStmt { stmts, .. }) if stmts.len() == 1 => {
            let Stmt::Expr(expr_stmt) = &stmts[0] else {
                return false;
            };
            expr_stmt.expr.as_ref()
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
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = left else {
        return false;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };
    if !ctx.is_ident(obj, "target") {
        return false;
    }
    let MemberProp::Computed(ComputedPropName { expr: key, .. }) = &member.prop else {
        return false;
    };
    ctx.is_binding(key, "key") && ctx.is_binding(right, "value")
}
fn prop_name_ident(key: &PropName) -> Option<Atom> {
    match key {
        PropName::Ident(IdentName { sym, .. }) => Some(sym.clone()),
        PropName::Str(s) => Some(s.value.as_str()?.into()),
        _ => None,
    }
}
/// Build an equivalent [`Function`] from a single inline callable expression
/// (`function(...) {...}` or `(...) => {...}`) so the body-shape matchers used
/// for declaration scanning can be reused at expression sites.
///
/// Returns `None` for arrows with an expression body — callers that need to
/// match those handle the expression form separately (see
/// [`classify_inline_helper_call`]).
fn function_from_inline_callable(expr: &Expr) -> Option<Function> {
    match strip_parens(expr) {
        Expr::Fn(fn_expr) => Some((*fn_expr.function).clone()),
        Expr::Arrow(arrow) => function_from_arrow(arrow),
        _ => None,
    }
}
/// Build an equivalent [`Function`] from a block-body arrow. Returns `None` for
/// expression-body arrows (there is no statement block to reuse); callers that
/// need those match the expression form separately.
fn function_from_arrow(arrow: &swc_core::ecma::ast::ArrowExpr) -> Option<Function> {
    let BlockStmtOrExpr::BlockStmt(block) = arrow.body.as_ref() else {
        return None;
    };
    Some(Function {
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
        is_generator: arrow.is_generator,
        is_async: arrow.is_async,
        type_params: None,
        return_type: None,
    })
}
/// Detect an `interopRequireDefault` arrow whose body is a bare conditional
/// expression: `(e) => e && e.__esModule ? e : { default: e }`. Block-body
/// arrows are matched by [`is_interop_require_default_fn`] after conversion to a
/// `Function`; expression-body arrows can't be converted, so they're matched
/// here. Shared by declaration scanning and inline-expression detection.
fn detect_interop_default_expr_arrow(
    arrow: &swc_core::ecma::ast::ArrowExpr,
) -> Option<TranspilerHelperKind> {
    if arrow.params.len() != 1 {
        return None;
    }
    let Pat::Ident(param) = &arrow.params[0] else {
        return None;
    };
    let BlockStmtOrExpr::Expr(expr) = arrow.body.as_ref() else {
        return None;
    };
    let mut ctx = MatchContext::new();
    let param_key = binding_key(&param.id);
    ctx.declare("obj", param_key.0, param_key.1);
    matches_ternary_expr(expr, &ctx).then_some(TranspilerHelperKind::InteropRequireDefault)
}
/// Classify an inline helper IIFE such as
/// `((e) => e && e.__esModule ? e : { default: e })(x)` by the shape of its
/// callee, reusing the same body-shape matchers that detect module-level
/// helper declarations.
///
/// Returns the detected [`TranspilerHelperKind`] together with the single
/// argument the IIFE is applied to, when the call has exactly one non-spread
/// argument. The body-shape recognition lives in one place and is shared
/// between declaration scanning and inline-expression detection.
pub(crate) fn classify_inline_helper_call(
    call: &CallExpr,
) -> Option<(TranspilerHelperKind, &Expr)> {
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let kind = classify_inline_callable(strip_parens(callee))?;
    Some((kind, call.args[0].expr.as_ref()))
}
/// Classify a single inline callable expression (arrow or function expression)
/// by helper body shape. Inline helpers are self-contained at the call site
/// and never dispatch to sibling Babel sub-helpers, so sub-helper dispatch
/// chains are not accepted here (`has_sub_helpers = false`).
pub(crate) fn classify_inline_callable(callee: &Expr) -> Option<TranspilerHelperKind> {
    // Expression-body arrows (e.g. `(e) => e && e.__esModule ? e : {default: e}`)
    // are only used by interopRequireDefault and can't be converted to a
    // Function; match them directly. Block-body callables fall through to the
    // declaration body-shape matchers.
    if let Expr::Arrow(arrow) = callee {
        if let Some(kind) = detect_interop_default_expr_arrow(arrow) {
            return Some(kind);
        }
    }

    let func = function_from_inline_callable(callee)?;
    detect_helper_from_fn(&func, false)
}
pub(super) fn detect_helper_from_arrow(
    arrow: &swc_core::ecma::ast::ArrowExpr,
    has_sub_helpers: bool,
) -> Option<TranspilerHelperKind> {
    // interopRequireDefault expression-body arrow: can't be converted to a
    // Function, so match it directly.
    if let Some(kind) = detect_interop_default_expr_arrow(arrow) {
        return Some(kind);
    }

    // Every other inlined-helper arrow has a block body. Convert it to an
    // equivalent Function and reuse the declaration body-shape matchers.
    // (extends is excluded: it has 0 params and uses `arguments`, which an arrow
    // can't express.)
    let func = function_from_arrow(arrow)?;
    if is_interop_require_default_fn(&func) {
        return Some(TranspilerHelperKind::InteropRequireDefault);
    }
    if is_to_consumable_array_fn(&func, has_sub_helpers) {
        return Some(TranspilerHelperKind::ToConsumableArray);
    }
    if is_object_spread_fn(&func) {
        return Some(TranspilerHelperKind::ObjectSpread);
    }
    if is_sliced_to_array_fn(&func, has_sub_helpers) {
        return Some(TranspilerHelperKind::SlicedToArray);
    }
    if is_object_without_properties_fn(&func) {
        return Some(TranspilerHelperKind::ObjectWithoutProperties);
    }
    None
}
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

    matches_ternary_return_block(&body.stmts, &ctx) || matches_if_return_form(&body.stmts, &ctx)
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
fn extract_single_return(stmt: &Stmt) -> Option<&Expr> {
    match stmt {
        Stmt::Return(ReturnStmt { arg: Some(arg), .. }) => Some(arg),
        Stmt::Block(block) if block.stmts.len() == 1 => extract_single_return(&block.stmts[0]),
        _ => None,
    }
}
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
            if member_prop_name(&member.prop, "__esModule") {
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
                            && (member_prop_name(&member.prop, "keys")
                                || member_prop_name(&member.prop, "getOwnPropertyDescriptor")
                                || member_prop_name(&member.prop, "defineProperty")
                                || member_prop_name(&member.prop, "getOwnPropertyNames"))
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
pub(super) fn is_class_call_check_fn(func: &Function) -> bool {
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
fn matches_negated_instanceof(ctx: &MatchContext, expr: &Expr, left: &str, right: &str) -> bool {
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
fn is_typeof_polyfill_init(expr: &Expr) -> bool {
    let Expr::Cond(cond) = expr else {
        return false;
    };

    is_typeof_symbol_test(&cond.test) && is_typeof_identity_fn(&cond.cons)
}
fn is_typeof_symbol_test(expr: &Expr) -> bool {
    let Expr::Bin(bin) = expr else { return false };
    if bin.op != BinaryOp::LogicalAnd {
        return false;
    }
    is_typeof_symbol_eq_function(&bin.left) && is_typeof_symbol_iterator_eq_symbol(&bin.right)
}
fn is_typeof_symbol_eq_function(expr: &Expr) -> bool {
    let Expr::Bin(bin) = expr else { return false };
    if !matches!(bin.op, BinaryOp::EqEq | BinaryOp::EqEqEq) {
        return false;
    }
    is_typeof_of_ident(&bin.left, "Symbol") && is_string_lit(&bin.right, "function")
}
fn is_typeof_symbol_iterator_eq_symbol(expr: &Expr) -> bool {
    let Expr::Bin(bin) = expr else { return false };
    if !matches!(bin.op, BinaryOp::EqEq | BinaryOp::EqEqEq) {
        return false;
    }
    is_typeof_of_symbol_iterator(&bin.left) && is_string_lit(&bin.right, "symbol")
}
fn is_typeof_of_ident(expr: &Expr, name: &str) -> bool {
    let Expr::Unary(unary) = expr else {
        return false;
    };
    unary.op == UnaryOp::TypeOf
        && matches!(unary.arg.as_ref(), Expr::Ident(id) if id.sym.as_ref() == name)
}
fn is_typeof_of_symbol_iterator(expr: &Expr) -> bool {
    let Expr::Unary(unary) = expr else {
        return false;
    };
    if unary.op != UnaryOp::TypeOf {
        return false;
    }
    let Expr::Member(member) = unary.arg.as_ref() else {
        return false;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };
    obj.sym.as_ref() == "Symbol"
        && matches!(&member.prop, MemberProp::Ident(id) if id.sym.as_ref() == "iterator")
}
fn is_string_lit(expr: &Expr, value: &str) -> bool {
    matches!(expr, Expr::Lit(Lit::Str(s)) if s.value.as_str() == Some(value))
}
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
                    let Some(arg) = &ret.arg else {
                        return false;
                    };
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
            let Some(arg) = &ret.arg else {
                return false;
            };
            is_typeof_of_binding(arg, &param_key)
        }
        _ => false,
    }
}
fn is_typeof_of_binding(expr: &Expr, binding: &BindingKey) -> bool {
    let Expr::Unary(unary) = expr else {
        return false;
    };
    unary.op == UnaryOp::TypeOf && expr_matches_binding(&unary.arg, binding)
}
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
pub(super) fn is_object_without_properties_fn(func: &Function) -> bool {
    let Some(mut ctx) = MatchContext::from_params(func, &["source", "excluded"]) else {
        return false;
    };

    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return false,
    };

    if body.stmts.len() < 3 {
        return false;
    }

    // Find the accumulator: the variable initialized with `{}` in loose helpers,
    // or the variable initialized by the loose helper call in spec wrappers.
    let direct_accum = find_empty_object_accumulator(&body.stmts)
        .or_else(|| find_accumulator_in_for_init(&body.stmts));
    let is_wrapper_accum = direct_accum.is_none();
    let wrapper_accum = direct_accum
        .is_none()
        .then(|| find_call_accumulator_from_source_excluded(&body.stmts, &ctx))
        .flatten();
    let Some((accum_sym, accum_ctxt)) = direct_accum.or(wrapper_accum) else {
        return false;
    };
    ctx.declare("accum", accum_sym, accum_ctxt);

    if is_wrapper_accum {
        let mut markers = BodyMarkerState::default();
        scan_stmts_for_markers(&body.stmts, &mut markers);
        if !markers.has_object_get_own_property_symbols || !markers.has_property_is_enumerable {
            return false;
        }
    }

    // Last statement must return the accumulator
    let Some(Stmt::Return(ReturnStmt { arg: Some(arg), .. })) = body.stmts.last() else {
        return false;
    };
    if !ctx.is_binding(arg, "accum") {
        return false;
    }

    for stmt in &body.stmts {
        match stmt {
            Stmt::ForIn(f) if for_in_loop_has_owp_shape(f, &ctx) => {
                return true;
            }
            Stmt::For(f) => {
                let mut checker = GuardedCopyInIfChecker {
                    ctx: &ctx,
                    found: false,
                };
                f.body.visit_with(&mut checker);
                if checker.found {
                    return true;
                }
                if for_body_has_or_guarded_copy(&f.body, &ctx) {
                    return true;
                }
                if for_body_has_and_guarded_copy(&f.body, &ctx) {
                    return true;
                }
                if for_body_has_continue_guarded_copy(&f.body, &ctx) {
                    return true;
                }
            }
            _ => {}
        }
    }

    let mut nested_checker = OwpLoopChecker {
        ctx: &ctx,
        found: false,
    };
    for stmt in &body.stmts {
        stmt.visit_with(&mut nested_checker);
        if nested_checker.found {
            return true;
        }
    }

    false
}
/// Detect `_asyncToGenerator`: single param, returns a function that calls
/// `fn.apply(this, arguments)` and constructs `new Promise(...)`.
fn is_async_to_generator_fn(func: &Function) -> bool {
    if func.params.len() != 1 {
        return false;
    }
    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return false,
    };
    // Body should have a return statement returning a function
    let has_return_fn = body.stmts.iter().any(|s| {
        if let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = s {
            matches!(arg.as_ref(), Expr::Fn(_) | Expr::Arrow(_))
        } else {
            false
        }
    });
    if !has_return_fn {
        return false;
    }
    // Look for `new Promise` somewhere in the body
    let mut finder = AsyncToGenFinder {
        found_promise: false,
        found_apply: false,
    };
    body.visit_with(&mut finder);
    finder.found_promise && finder.found_apply
}
struct AsyncToGenFinder {
    found_promise: bool,
    found_apply: bool,
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
                    return Some(binding_key(&bi.id));
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
fn computed_key_from_ident(ident: &Ident) -> ComputedKey {
    let key = binding_key(ident);
    ComputedKey::Ident(key.0, key.1)
}
struct GuardedCopyInIfChecker<'a> {
    ctx: &'a MatchContext,
    found: bool,
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
                    return Some(binding_key(&bi.id));
                }
            }
        }
    }
    None
}
fn find_call_accumulator_from_source_excluded(
    stmts: &[Stmt],
    ctx: &MatchContext,
) -> Option<(Atom, SyntaxContext)> {
    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        for decl in &var.decls {
            let Pat::Ident(bi) = &decl.name else {
                continue;
            };
            let Some(init) = &decl.init else {
                continue;
            };
            if is_call_with_source_excluded(init, ctx) {
                return Some(binding_key(&bi.id));
            }
        }
    }
    None
}
fn is_call_with_source_excluded(expr: &Expr, ctx: &MatchContext) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    if call.args.len() != 2 || call.args.iter().any(|arg| arg.spread.is_some()) {
        return false;
    }
    ctx.is_binding(&call.args[0].expr, "source") && ctx.is_binding(&call.args[1].expr, "excluded")
}
fn for_in_loop_has_owp_shape(for_in: &swc_core::ecma::ast::ForInStmt, ctx: &MatchContext) -> bool {
    let Some(loop_key) = for_in_key(&for_in.left) else {
        return false;
    };
    if !ctx.is_binding(&for_in.right, "source") {
        return false;
    }

    for_in_body_has_canonical_expr(&for_in.body, ctx, loop_key)
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
            Some(computed_key_from_ident(&binding.id))
        }
        ForHead::Pat(pat) => {
            let Pat::Ident(binding) = pat.as_ref() else {
                return None;
            };
            Some(computed_key_from_ident(&binding.id))
        }
        _ => None,
    }
}
fn copy_key_from_source_to_accum(
    assign: &swc_core::ecma::ast::AssignExpr,
    ctx: &MatchContext,
) -> Option<ComputedKey> {
    use swc_core::ecma::ast::{AssignTarget, SimpleAssignTarget};

    let AssignTarget::Simple(SimpleAssignTarget::Member(left)) = &assign.left else {
        return None;
    };
    let Expr::Ident(left_obj) = left.obj.as_ref() else {
        return None;
    };
    if !ctx.is_ident(left_obj, "accum") {
        return None;
    }
    let left_key = computed_member_key(&left.prop)?;

    let Expr::Member(right) = assign.right.as_ref() else {
        return None;
    };
    let Expr::Ident(right_obj) = right.obj.as_ref() else {
        return None;
    };
    if !ctx.is_ident(right_obj, "source") {
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
    Some(binding_key(id))
}
fn computed_member_key(prop: &MemberProp) -> Option<ComputedKey> {
    let MemberProp::Computed(computed) = prop else {
        return None;
    };
    computed_key_expr(computed.expr.as_ref())
}
fn computed_key_expr(expr: &Expr) -> Option<ComputedKey> {
    match expr {
        Expr::Ident(id) => Some(computed_key_from_ident(id)),
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
    ctx: &MatchContext,
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
    if !ctx.is_binding(&call.args[0].expr, "source") {
        return false;
    }
    let Some(key) = computed_key_expr(call.args[1].expr.as_ref()) else {
        return false;
    };
    required_key
        .as_ref()
        .is_none_or(|required| *required == key)
}
fn for_in_body_has_canonical_expr(body: &Stmt, ctx: &MatchContext, loop_key: ComputedKey) -> bool {
    let mut checker = OrGuardChecker {
        ctx,
        required_key: Some(loop_key.clone()),
        require_has_own: true,
        found: false,
    };
    body.visit_with(&mut checker);
    if checker.found {
        return true;
    }

    let mut if_checker = GuardedCopyInIfChecker { ctx, found: false };
    body.visit_with(&mut if_checker);
    if if_checker.found {
        return stmt_has_has_own_property_call(body, ctx, &Some(loop_key));
    }

    let mut continue_checker = ContinueGuardedCopyChecker {
        ctx,
        required_key: Some(loop_key),
        found: false,
    };
    body.visit_with(&mut continue_checker);
    if continue_checker.found {
        return stmt_has_has_own_property_call(body, ctx, &continue_checker.required_key);
    }
    false
}
fn for_body_has_or_guarded_copy(body: &Stmt, ctx: &MatchContext) -> bool {
    let mut checker = OrGuardChecker {
        ctx,
        required_key: None,
        require_has_own: false,
        found: false,
    };
    body.visit_with(&mut checker);
    checker.found
}
fn for_body_has_and_guarded_copy(body: &Stmt, ctx: &MatchContext) -> bool {
    let mut checker = AndGuardChecker {
        ctx,
        required_key: None,
        found: false,
    };
    body.visit_with(&mut checker);
    checker.found
}
fn for_body_has_continue_guarded_copy(body: &Stmt, ctx: &MatchContext) -> bool {
    let mut checker = ContinueGuardedCopyChecker {
        ctx,
        required_key: None,
        found: false,
    };
    body.visit_with(&mut checker);
    checker.found
}
struct OwpLoopChecker<'a> {
    ctx: &'a MatchContext,
    found: bool,
}
struct OrGuardChecker<'a> {
    ctx: &'a MatchContext,
    required_key: Option<ComputedKey>,
    require_has_own: bool,
    found: bool,
}
struct AndGuardChecker<'a> {
    ctx: &'a MatchContext,
    required_key: Option<ComputedKey>,
    found: bool,
}
struct ContinueGuardedCopyChecker<'a> {
    ctx: &'a MatchContext,
    required_key: Option<ComputedKey>,
    found: bool,
}
fn excluded_continue_keys(
    stmt: &Stmt,
    ctx: &MatchContext,
    required_key: &Option<ComputedKey>,
) -> Vec<ComputedKey> {
    let Stmt::If(if_stmt) = stmt else {
        return Vec::new();
    };
    if !stmt_is_continue(&if_stmt.cons) {
        return Vec::new();
    }
    let keys = index_guard_keys_for_polarity(&if_stmt.test, ctx, GuardPolarity::Excluded);
    filter_required_key(keys, required_key)
}
fn stmt_is_continue(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Continue(_) => true,
        Stmt::Block(block) if block.stmts.len() == 1 => matches!(block.stmts[0], Stmt::Continue(_)),
        _ => false,
    }
}
struct CopyKeyCollector<'a> {
    ctx: &'a MatchContext,
    keys: Vec<ComputedKey>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum GuardPolarity {
    Excluded,
    Included,
}
fn index_guard_keys_for_polarity(
    expr: &Expr,
    ctx: &MatchContext,
    wanted: GuardPolarity,
) -> Vec<ComputedKey> {
    index_guard_keys(expr, ctx)
        .into_iter()
        .filter_map(|(key, polarity)| (polarity == wanted).then_some(key))
        .collect()
}
fn index_guard_keys(expr: &Expr, ctx: &MatchContext) -> Vec<(ComputedKey, GuardPolarity)> {
    match unparen_expr(expr) {
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => {
            index_guard_keys(unary.arg.as_ref(), ctx)
                .into_iter()
                .map(|(key, polarity)| (key, flip_guard_polarity(polarity)))
                .collect()
        }
        Expr::Bin(bin) if bin.op == BinaryOp::LogicalAnd => {
            let mut keys = index_guard_keys(&bin.left, ctx);
            keys.extend(index_guard_keys(&bin.right, ctx));
            keys
        }
        Expr::Bin(bin) => match_index_guard_bin(bin, ctx).into_iter().collect(),
        _ => Vec::new(),
    }
}
fn match_index_guard_bin(
    bin: &swc_core::ecma::ast::BinExpr,
    ctx: &MatchContext,
) -> Option<(ComputedKey, GuardPolarity)> {
    if let Some(key) = key_from_index_of_call(&bin.left, ctx) {
        return polarity_for_index_literal_compare(bin.op, &bin.right)
            .map(|polarity| (key, polarity));
    }
    if let Some(key) = key_from_index_of_call(&bin.right, ctx) {
        return polarity_for_literal_index_compare(bin.op, &bin.left)
            .map(|polarity| (key, polarity));
    }
    None
}
fn key_from_index_of_call(expr: &Expr, ctx: &MatchContext) -> Option<ComputedKey> {
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
    if !ctx.is_binding(&member.obj, "excluded") {
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
    ctx: &MatchContext,
    guard_keys: &[ComputedKey],
) -> bool {
    let mut copy_collector = CopyKeyCollector {
        ctx,
        keys: Vec::new(),
    };
    stmt.visit_with(&mut copy_collector);
    keys_have_match(&copy_collector.keys, guard_keys)
}
fn expr_has_has_own_property_call(
    expr: &Expr,
    ctx: &MatchContext,
    required_key: &Option<ComputedKey>,
) -> bool {
    struct HasOwnCollector<'a> {
        ctx: &'a MatchContext,
        required_key: &'a Option<ComputedKey>,
        found: bool,
    }

    impl Visit for HasOwnCollector<'_> {
        fn visit_function(&mut self, _: &Function) {}
        fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if is_has_own_property_call(call, self.ctx, self.required_key) {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }

    let mut collector = HasOwnCollector {
        ctx,
        required_key,
        found: false,
    };
    expr.visit_with(&mut collector);
    collector.found
}
fn stmt_has_has_own_property_call(
    stmt: &Stmt,
    ctx: &MatchContext,
    required_key: &Option<ComputedKey>,
) -> bool {
    struct HasOwnCollector<'a> {
        ctx: &'a MatchContext,
        required_key: &'a Option<ComputedKey>,
        found: bool,
    }

    impl Visit for HasOwnCollector<'_> {
        fn visit_function(&mut self, _: &Function) {}
        fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if is_has_own_property_call(call, self.ctx, self.required_key) {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }

    let mut collector = HasOwnCollector {
        ctx,
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

    markers.has_object_assign && markers.has_apply_arguments
}
/// Check if an expression is `Object.assign`.
fn is_object_assign_ref(expr: &Expr) -> bool {
    let Expr::Member(member) = expr else {
        return false;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };
    obj.sym.as_ref() == "Object" && member_prop_name(&member.prop, "assign")
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
fn is_object_spread_fn(func: &Function) -> bool {
    let body = match func.body.as_ref() {
        Some(b) => b,
        None => return false,
    };

    let mut markers = BodyMarkerState::default();
    scan_stmts_for_markers(&body.stmts, &mut markers);

    if let Some(ctx) = MatchContext::from_params(func, &["target"]) {
        if !markers.has_arguments_ref {
            return false;
        }
        if (!markers.has_object_define_property || !markers.has_object_get_own_property_descriptor)
            && (!markers.has_object_keys || !markers.has_object_get_own_property_symbols)
        {
            return false;
        }

        return matches!(
            body.stmts.last(),
            Some(Stmt::Return(ReturnStmt { arg: Some(arg), .. }))
                if ctx.is_binding(arg, "target")
        );
    }

    if let Some(ctx) = MatchContext::from_params(func, &["target", "source"]) {
        if !markers.has_object_define_property || !markers.has_object_get_own_property_descriptor {
            return false;
        }

        return matches!(
            body.stmts.last(),
            Some(Stmt::Return(ReturnStmt { arg: Some(arg), .. }))
                if ctx.is_binding(arg, "target")
        );
    }

    false
}
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
pub(crate) struct InlineSlicedToArrayCall {
    pub(crate) source: Box<Expr>,
    pub(crate) source_ref: Option<Ident>,
    pub(crate) length: Option<usize>,
}
/// Match Babel's slicedToArray helper after Terser has inlined the helper
/// dispatcher and sub-helpers at the call site:
/// `f(source) || g(source, len) || h(source, len) || k()`.
///
/// Terser can also sink the source into a temp assignment in the first IIFE,
/// then pass that temp to the later IIFEs:
/// `f(tmp = source) || g(tmp) || h(tmp) || k()`.
pub(crate) fn extract_inline_sliced_to_array_call(expr: &Expr) -> Option<InlineSlicedToArrayCall> {
    let mut terms = Vec::new();
    collect_logical_or_terms(expr, &mut terms);
    if terms.len() != 4 {
        return None;
    }

    let first_call = as_inline_helper_call(terms[0])?;
    if first_call.args.len() != 1 {
        return None;
    }
    let (source, source_ref) = inline_sliced_source_arg(first_call.args[0].expr.as_ref())?;

    let mut length = None;
    for term in &terms[1..3] {
        let call = as_inline_helper_call(term)?;
        if !matches!(call.args.len(), 1 | 2) {
            return None;
        }
        if !inline_sliced_arg_matches_source(
            call.args[0].expr.as_ref(),
            source.as_ref(),
            &source_ref,
        ) {
            return None;
        }
        if call.args.len() == 2 {
            let term_length = inline_sliced_length_arg(call.args[1].expr.as_ref())?;
            if let (Some(existing), Some(term_length)) = (length, term_length) {
                if existing != term_length {
                    return None;
                }
            }
            length = length.or(term_length);
        }
    }

    let last_call = as_inline_helper_call(terms[3])?;
    if !last_call.args.is_empty() {
        return None;
    }

    let mut markers = BodyMarkerState::default();
    for term in &terms[..3] {
        scan_inline_helper_call_markers(as_inline_helper_call(term)?, &mut markers);
    }
    if !markers.has_array_is_array || !(markers.has_symbol_iterator || markers.has_array_from) {
        return None;
    }

    Some(InlineSlicedToArrayCall {
        source,
        source_ref,
        length,
    })
}
fn collect_logical_or_terms<'a>(expr: &'a Expr, terms: &mut Vec<&'a Expr>) {
    if let Expr::Bin(bin) = expr {
        if bin.op == BinaryOp::LogicalOr {
            collect_logical_or_terms(bin.left.as_ref(), terms);
            collect_logical_or_terms(bin.right.as_ref(), terms);
            return;
        }
    }
    terms.push(expr);
}
fn as_inline_helper_call(expr: &Expr) -> Option<&CallExpr> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let arg_count = call.args.len();
    match strip_parens(callee.as_ref()) {
        Expr::Arrow(arrow) => (arrow.params.len() == arg_count).then_some(call),
        Expr::Fn(func) => (func.function.params.len() == arg_count).then_some(call),
        _ => None,
    }
}
fn inline_sliced_source_arg(expr: &Expr) -> Option<(Box<Expr>, Option<Ident>)> {
    let expr = strip_parens(expr);
    let Expr::Assign(AssignExpr {
        op: AssignOp::Assign,
        left,
        right,
        ..
    }) = expr
    else {
        return Some((Box::new(expr.clone()), inline_sliced_expr_ident(expr)));
    };
    let AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) = left else {
        return None;
    };
    Some((right.clone(), Some(ident.id.clone())))
}
fn inline_sliced_arg_matches_source(
    expr: &Expr,
    source: &Expr,
    source_ref: &Option<Ident>,
) -> bool {
    if source_ref
        .as_ref()
        .is_some_and(|ident| inline_sliced_expr_matches_ident(expr, ident))
    {
        return true;
    }
    inline_sliced_same_expr(expr, source)
}
fn inline_sliced_length_arg(expr: &Expr) -> Option<Option<usize>> {
    match strip_parens(expr) {
        Expr::Lit(Lit::Num(num)) => Some(Some(inline_sliced_numeric_length(num.value)?)),
        Expr::Ident(_) => Some(None),
        _ => None,
    }
}
fn inline_sliced_numeric_length(value: f64) -> Option<usize> {
    if value < 0.0 || value.fract() != 0.0 || value > 64.0 {
        return None;
    }
    Some(value as usize)
}
fn inline_sliced_same_expr(left: &Expr, right: &Expr) -> bool {
    match (strip_parens(left), strip_parens(right)) {
        (Expr::Ident(left), Expr::Ident(right)) => inline_sliced_ident_matches(left, right),
        _ => false,
    }
}
fn inline_sliced_expr_ident(expr: &Expr) -> Option<Ident> {
    let Expr::Ident(id) = strip_parens(expr) else {
        return None;
    };
    Some(id.clone())
}
fn inline_sliced_expr_matches_ident(expr: &Expr, target: &Ident) -> bool {
    let Expr::Ident(id) = strip_parens(expr) else {
        return false;
    };
    inline_sliced_ident_matches(id, target)
}
fn inline_sliced_ident_matches(left: &Ident, right: &Ident) -> bool {
    left.sym == right.sym
        && (left.ctxt == right.ctxt
            || (left.ctxt == SyntaxContext::empty() && right.ctxt != SyntaxContext::empty()))
}
fn scan_inline_helper_call_markers(call: &CallExpr, markers: &mut BodyMarkerState) {
    let Callee::Expr(callee) = &call.callee else {
        return;
    };
    match strip_parens(callee.as_ref()) {
        Expr::Arrow(arrow) => {
            if let BlockStmtOrExpr::BlockStmt(block) = arrow.body.as_ref() {
                scan_stmts_for_markers(&block.stmts, markers);
            }
        }
        Expr::Fn(func) => {
            if let Some(body) = &func.function.body {
                scan_stmts_for_markers(&body.stmts, markers);
            }
        }
        _ => {}
    }
}
#[derive(Default)]
struct BodyMarkerState {
    has_array_is_array: bool,
    has_array_from: bool,
    has_array_constructor: bool,
    has_object_assign: bool,
    has_apply_arguments: bool,
    has_arguments_ref: bool,
    has_object_keys: bool,
    has_object_define_property: bool,
    has_object_get_own_property_descriptor: bool,
    has_object_get_own_property_symbols: bool,
    has_property_is_enumerable: bool,
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
                            if member_prop_name(&member.prop, "isArray") {
                                self.state.has_array_is_array = true;
                            }
                            if member_prop_name(&member.prop, "from") {
                                self.state.has_array_from = true;
                            }
                        }
                        if obj.sym.as_ref() == "Object" {
                            if member_prop_name(&member.prop, "keys") {
                                self.state.has_object_keys = true;
                            }
                            if member_prop_name(&member.prop, "assign") {
                                self.state.has_object_assign = true;
                            }
                            if member_prop_name(&member.prop, "defineProperty")
                                || member_prop_name(&member.prop, "defineProperties")
                            {
                                self.state.has_object_define_property = true;
                            }
                            if member_prop_name(&member.prop, "getOwnPropertyDescriptor")
                                || member_prop_name(&member.prop, "getOwnPropertyDescriptors")
                            {
                                self.state.has_object_get_own_property_descriptor = true;
                            }
                            if member_prop_name(&member.prop, "getOwnPropertySymbols") {
                                self.state.has_object_get_own_property_symbols = true;
                            }
                        }
                    }
                    // *.apply(this|null, arguments)
                    if member_prop_name(&member.prop, "apply")
                        && call.args.len() == 2
                        && matches!(
                            call.args[0].expr.as_ref(),
                            Expr::This(..) | Expr::Lit(Lit::Null(..))
                        )
                        && matches!(call.args[1].expr.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "arguments")
                    {
                        self.state.has_apply_arguments = true;
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
                if obj.sym.as_ref() == "Object" && member_prop_name(&member.prop, "assign") {
                    self.state.has_object_assign = true;
                }
                // Symbol.iterator
                if obj.sym.as_ref() == "Symbol" && member_prop_name(&member.prop, "iterator") {
                    self.state.has_symbol_iterator = true;
                }
            }
            if member_prop_name(&member.prop, "propertyIsEnumerable") {
                self.state.has_property_is_enumerable = true;
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
/// Check if a function body matches Babel's `_setPrototypeOf` helper:
/// `return (_setPrototypeOf = Object.setPrototypeOf ? ... : ...__proto__...)(o, p);`
pub(crate) fn is_set_prototype_of_fn(func: &Function) -> bool {
    let Some(body) = &func.body else {
        return false;
    };
    if func.params.len() != 2 {
        return false;
    }
    if body.stmts.len() > 3 {
        return false;
    }

    let mut detector = SetPrototypeOfDetector::new(func);
    body.visit_with(&mut detector);
    detector.has_object_set_prototype_of && detector.has_proto_assignment
}
struct SetPrototypeOfDetector {
    has_object_set_prototype_of: bool,
    has_proto_assignment: bool,
    param_pairs: Vec<(BindingKey, BindingKey)>,
}
fn set_prototype_param_pair(func: &Function) -> Option<(BindingKey, BindingKey)> {
    if func.params.len() != 2 {
        return None;
    }
    let Pat::Ident(object) = &func.params[0].pat else {
        return None;
    };
    let Pat::Ident(proto) = &func.params[1].pat else {
        return None;
    };
    Some((binding_key(&object.id), binding_key(&proto.id)))
}
fn is_proto_assignment_for_pair(
    assign: &swc_core::ecma::ast::AssignExpr,
    object: &BindingKey,
    proto: &BindingKey,
) -> bool {
    let swc_core::ecma::ast::AssignTarget::Simple(swc_core::ecma::ast::SimpleAssignTarget::Member(
        member,
    )) = &assign.left
    else {
        return false;
    };
    if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "__proto__") {
        return false;
    }
    let Expr::Ident(lhs_obj) = member.obj.as_ref() else {
        return false;
    };
    let Expr::Ident(rhs) = assign.right.as_ref() else {
        return false;
    };
    binding_key(lhs_obj) == *object && binding_key(rhs) == *proto
}
fn is_object_set_prototype_of_member(expr: &Expr) -> bool {
    let Expr::Member(member) = expr else {
        return false;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };
    obj.sym.as_ref() == "Object"
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "setPrototypeOf")
}
/// Check if a function body matches the `_inherits` pattern:
/// 2 params, <=5 stmts, body contains `param1.prototype = Object.create(...)`.
pub(crate) fn is_inherits_fn(func: &Function) -> bool {
    let Some(ctx) = MatchContext::from_params(func, &["sub_class", "super_class"]) else {
        return false;
    };
    let body = match &func.body {
        Some(b) => b,
        None => return false,
    };
    if body.stmts.len() > 5 {
        return false;
    }
    body.stmts
        .iter()
        .any(|s| stmt_has_prototype_object_create(s, &ctx))
}
/// Check if a statement contains `param.prototype = Object.create(...)`,
/// including inside comma/sequence expressions.
fn stmt_has_prototype_object_create(stmt: &Stmt, ctx: &MatchContext) -> bool {
    let Stmt::Expr(swc_core::ecma::ast::ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    expr_has_prototype_object_create(expr, ctx)
}
fn expr_has_prototype_object_create(expr: &Expr, ctx: &MatchContext) -> bool {
    match expr {
        Expr::Assign(assign) if assign.op == swc_core::ecma::ast::AssignOp::Assign => {
            let swc_core::ecma::ast::AssignTarget::Simple(
                swc_core::ecma::ast::SimpleAssignTarget::Member(lhs),
            ) = &assign.left
            else {
                return false;
            };
            let Expr::Ident(obj) = lhs.obj.as_ref() else {
                return false;
            };
            if !ctx.is_ident(obj, "sub_class") {
                return false;
            }
            if !matches!(&lhs.prop, MemberProp::Ident(n) if n.sym.as_ref() == "prototype") {
                return false;
            }
            is_object_create_call(&assign.right)
        }
        Expr::Seq(seq) => seq
            .exprs
            .iter()
            .any(|e| expr_has_prototype_object_create(e, ctx)),
        _ => false,
    }
}
fn is_object_create_call(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else { return false };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(m) = callee.as_ref() else {
        return false;
    };
    let Expr::Ident(obj) = m.obj.as_ref() else {
        return false;
    };
    obj.sym.as_ref() == "Object"
        && matches!(&m.prop, MemberProp::Ident(p) if p.sym.as_ref() == "create")
}
/// Check if a function body matches the `_callSuper` pattern (Babel 7.24+):
/// 2-3 params, short body (<=3 stmts), contains 3-arg `Reflect.construct` and
/// `param2.apply(param1, ...)` — the dual-path fallback pattern.
pub(crate) fn is_call_super_fn(func: &Function) -> bool {
    if func.params.len() < 2 || func.params.len() > 3 {
        return false;
    }
    let Pat::Ident(self_param) = &func.params[0].pat else {
        return false;
    };
    let Pat::Ident(super_param) = &func.params[1].pat else {
        return false;
    };
    let mut ctx = MatchContext::new();
    let self_key = binding_key(&self_param.id);
    ctx.declare("self", self_key.0, self_key.1);
    let super_key = binding_key(&super_param.id);
    ctx.declare("super_ctor", super_key.0, super_key.1);
    let body = match &func.body {
        Some(b) => b,
        None => return false,
    };
    if body.stmts.len() > 3 {
        return false;
    }
    body_has_call_super_shape(body, &ctx)
}
/// Check for both `Reflect.construct(_, _, _)` (3-arg form) AND
/// `param2.apply(param1, ...)` in the body. The dual-path pattern is the
/// structural hallmark of Babel's `_callSuper` helper:
/// `_isNR() ? Reflect.construct(o, e||[], ...) : o.apply(t, e)`
fn body_has_call_super_shape(body: &swc_core::ecma::ast::BlockStmt, ctx: &MatchContext) -> bool {
    use swc_core::ecma::ast::CallExpr;

    struct Finder<'a> {
        ctx: &'a MatchContext,
        has_reflect_construct_3: bool,
        has_param2_apply_param1: bool,
    }
    impl Visit for Finder<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Member(m) = callee.as_ref() {
                    // Check for Reflect.construct(_, _, _)
                    if let Expr::Ident(obj) = m.obj.as_ref() {
                        if obj.sym.as_ref() == "Reflect"
                            && matches!(&m.prop, MemberProp::Ident(p) if p.sym.as_ref() == "construct")
                            && call.args.len() == 3
                        {
                            self.has_reflect_construct_3 = true;
                        }
                    }
                    // Check for param2.apply(param1, ...)
                    if matches!(&m.prop, MemberProp::Ident(p) if p.sym.as_ref() == "apply") {
                        if let Expr::Ident(obj) = m.obj.as_ref() {
                            if self.ctx.is_ident(obj, "super_ctor") && !call.args.is_empty() {
                                if let Expr::Ident(first_arg) = call.args[0].expr.as_ref() {
                                    if self.ctx.is_ident(first_arg, "self") {
                                        self.has_param2_apply_param1 = true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = Finder {
        ctx,
        has_reflect_construct_3: false,
        has_param2_apply_param1: false,
    };
    body.visit_with(&mut finder);
    finder.has_reflect_construct_3 && finder.has_param2_apply_param1
}
fn is_maybe_array_like_fn(func: &Function) -> bool {
    if func.params.len() != 3 {
        return false;
    }
    let Some(body) = &func.body else {
        return false;
    };

    let first_param = match &func.params[0].pat {
        Pat::Ident(id) => &id.id,
        _ => return false,
    };
    let array_param = match &func.params[1].pat {
        Pat::Ident(id) => &id.id,
        _ => return false,
    };

    let mut markers = BodyMarkerState::default();
    scan_stmts_for_markers(&body.stmts, &mut markers);

    if !markers.has_array_is_array || !has_array_like_length_typeof_guard(&body.stmts, array_param)
    {
        return false;
    }

    has_delegate_return(&body.stmts, first_param)
}
fn has_array_like_length_typeof_guard(stmts: &[Stmt], array_param: &Ident) -> bool {
    struct Finder<'a> {
        array_param: &'a Ident,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_function(&mut self, _: &Function) {}

        fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

        fn visit_bin_expr(&mut self, bin: &BinExpr) {
            if is_number_typeof_length_check(bin, self.array_param) {
                self.found = true;
                return;
            }
            bin.visit_children_with(self);
        }
    }

    let mut finder = Finder {
        array_param,
        found: false,
    };
    for stmt in stmts {
        stmt.visit_with(&mut finder);
        if finder.found {
            return true;
        }
    }
    false
}
fn is_number_typeof_length_check(bin: &BinExpr, array_param: &Ident) -> bool {
    if !matches!(
        bin.op,
        BinaryOp::EqEq | BinaryOp::EqEqEq | BinaryOp::NotEq | BinaryOp::NotEqEq
    ) {
        return false;
    }
    (is_typeof_array_length(&bin.left, array_param) && is_number_string_literal(&bin.right))
        || (is_number_string_literal(&bin.left) && is_typeof_array_length(&bin.right, array_param))
}
fn is_typeof_array_length(expr: &Expr, array_param: &Ident) -> bool {
    let Expr::Unary(unary) = strip_parens(expr) else {
        return false;
    };
    if unary.op != UnaryOp::TypeOf {
        return false;
    }
    let Expr::Member(member) = strip_parens(unary.arg.as_ref()) else {
        return false;
    };
    if !member_prop_name(&member.prop, "length") {
        return false;
    }
    matches!(
        strip_parens(member.obj.as_ref()),
        Expr::Ident(obj) if obj.sym == array_param.sym && obj.ctxt == array_param.ctxt
    )
}
fn is_number_string_literal(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Lit(Lit::Str(s)) if s.value.as_str() == Some("number"))
}
fn has_delegate_return(stmts: &[Stmt], first_param: &Ident) -> bool {
    for stmt in stmts {
        match stmt {
            Stmt::Return(ReturnStmt { arg: Some(arg), .. })
                if is_call_to_ident(arg, first_param) =>
            {
                return true;
            }
            Stmt::If(if_stmt) => {
                if let Some(alt) = &if_stmt.alt {
                    if let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = alt.as_ref() {
                        if is_call_to_ident(arg, first_param) {
                            return true;
                        }
                    }
                    if let Stmt::Block(block) = alt.as_ref() {
                        if has_delegate_return(&block.stmts, first_param) {
                            return true;
                        }
                    }
                }
                if let Stmt::Block(block) = if_stmt.cons.as_ref() {
                    if has_delegate_return(&block.stmts, first_param) {
                        return true;
                    }
                }
            }
            Stmt::Block(block) if has_delegate_return(&block.stmts, first_param) => {
                return true;
            }
            _ => {}
        }
    }
    false
}
fn is_call_to_ident(expr: &Expr, ident: &Ident) -> bool {
    let Expr::Call(call) = expr else { return false };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    matches!(callee.as_ref(), Expr::Ident(id) if id.sym == ident.sym && id.ctxt == ident.ctxt)
}
/// Collect bindings for `_maybeArrayLike` declarations detected by body shape.
pub(crate) fn collect_maybe_array_like_bindings(module: &Module) -> HashSet<BindingKey> {
    let mut bindings = HashSet::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl)))
                if is_maybe_array_like_fn(&fn_decl.function) =>
            {
                bindings.insert(binding_key(&fn_decl.ident));
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
                for decl in &var_decl.decls {
                    let Pat::Ident(binding) = &decl.name else {
                        continue;
                    };
                    let Some(init) = &decl.init else { continue };
                    if let Expr::Fn(fn_expr) = init.as_ref() {
                        if is_maybe_array_like_fn(&fn_expr.function) {
                            bindings.insert(binding_key(&binding.id));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    bindings
}

impl Visit for AsyncToGenFinder {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::New(new_expr) = expr {
            if let Expr::Ident(id) = new_expr.callee.as_ref() {
                if id.sym.as_ref() == "Promise" {
                    self.found_promise = true;
                }
            }
        }
        if let Expr::Call(call) = expr {
            if let Some(callee) = call.callee.as_expr() {
                if let Expr::Member(member) = callee.as_ref() {
                    if let MemberProp::Ident(prop) = &member.prop {
                        if prop.sym.as_ref() == "apply" {
                            self.found_apply = true;
                        }
                    }
                }
            }
        }
        expr.visit_children_with(self);
    }
}
impl Visit for GuardedCopyInIfChecker<'_> {
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

    fn visit_if_stmt(&mut self, if_stmt: &IfStmt) {
        let included_keys =
            index_guard_keys_for_polarity(&if_stmt.test, self.ctx, GuardPolarity::Included);
        if !included_keys.is_empty()
            && stmt_contains_matching_copy(&if_stmt.cons, self.ctx, &included_keys)
        {
            self.found = true;
            return;
        }

        let excluded_keys =
            index_guard_keys_for_polarity(&if_stmt.test, self.ctx, GuardPolarity::Excluded);
        if !excluded_keys.is_empty() {
            if let Some(alt) = &if_stmt.alt {
                if stmt_contains_matching_copy(alt, self.ctx, &excluded_keys) {
                    self.found = true;
                    return;
                }
            }
        }

        if_stmt.visit_children_with(self);
    }
}
impl Visit for OwpLoopChecker<'_> {
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

    fn visit_for_in_stmt(&mut self, for_in: &swc_core::ecma::ast::ForInStmt) {
        if for_in_loop_has_owp_shape(for_in, self.ctx) {
            self.found = true;
            return;
        }
        for_in.visit_children_with(self);
    }

    fn visit_for_stmt(&mut self, for_stmt: &swc_core::ecma::ast::ForStmt) {
        let mut if_checker = GuardedCopyInIfChecker {
            ctx: self.ctx,
            found: false,
        };
        for_stmt.body.visit_with(&mut if_checker);
        if if_checker.found
            || for_body_has_or_guarded_copy(&for_stmt.body, self.ctx)
            || for_body_has_and_guarded_copy(&for_stmt.body, self.ctx)
            || for_body_has_continue_guarded_copy(&for_stmt.body, self.ctx)
        {
            self.found = true;
            return;
        }
        for_stmt.visit_children_with(self);
    }
}
impl Visit for OrGuardChecker<'_> {
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

    fn visit_bin_expr(&mut self, bin: &swc_core::ecma::ast::BinExpr) {
        if bin.op == BinaryOp::LogicalOr {
            let index_keys =
                index_guard_keys_for_polarity(&bin.left, self.ctx, GuardPolarity::Excluded);
            let index_keys = filter_required_key(index_keys, &self.required_key);
            if !index_keys.is_empty() {
                let mut copy_collector = CopyKeyCollector {
                    ctx: self.ctx,
                    keys: Vec::new(),
                };
                bin.right.visit_with(&mut copy_collector);
                let has_copy = keys_have_match(&copy_collector.keys, &index_keys);
                let has_required_has_own = !self.require_has_own
                    || expr_has_has_own_property_call(&bin.right, self.ctx, &self.required_key);
                if has_copy && has_required_has_own {
                    self.found = true;
                    return;
                }
            }
        }
        bin.visit_children_with(self);
    }
}
impl Visit for AndGuardChecker<'_> {
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

    fn visit_bin_expr(&mut self, bin: &swc_core::ecma::ast::BinExpr) {
        if bin.op == BinaryOp::LogicalAnd {
            let index_keys =
                index_guard_keys_for_polarity(&bin.left, self.ctx, GuardPolarity::Included);
            let index_keys = filter_required_key(index_keys, &self.required_key);
            if !index_keys.is_empty() {
                let mut copy_collector = CopyKeyCollector {
                    ctx: self.ctx,
                    keys: Vec::new(),
                };
                bin.right.visit_with(&mut copy_collector);
                if keys_have_match(&copy_collector.keys, &index_keys) {
                    self.found = true;
                    return;
                }
            }
        }
        bin.visit_children_with(self);
    }
}
impl Visit for ContinueGuardedCopyChecker<'_> {
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

    fn visit_block_stmt(&mut self, block: &swc_core::ecma::ast::BlockStmt) {
        let mut excluded_keys = Vec::new();
        for stmt in &block.stmts {
            let keys = excluded_continue_keys(stmt, self.ctx, &self.required_key);
            excluded_keys.extend(keys);
            if !excluded_keys.is_empty()
                && stmt_contains_matching_copy(stmt, self.ctx, &excluded_keys)
            {
                self.found = true;
                return;
            }
        }
        block.visit_children_with(self);
    }
}
impl Visit for CopyKeyCollector<'_> {
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

    fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
        if let Some(key) = copy_key_from_source_to_accum(assign, self.ctx) {
            self.keys.push(key);
        }
        assign.visit_children_with(self);
    }
}
impl SetPrototypeOfDetector {
    fn new(func: &Function) -> Self {
        let mut param_pairs = Vec::new();
        if let Some(pair) = set_prototype_param_pair(func) {
            param_pairs.push(pair);
        }
        Self {
            has_object_set_prototype_of: false,
            has_proto_assignment: false,
            param_pairs,
        }
    }
}
impl Visit for SetPrototypeOfDetector {
    fn visit_function(&mut self, func: &Function) {
        let pair = set_prototype_param_pair(func);
        if let Some(pair) = pair.clone() {
            self.param_pairs.push(pair);
        }
        func.visit_children_with(self);
        if pair.is_some() {
            self.param_pairs.pop();
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        if is_object_set_prototype_of_member(expr) {
            self.has_object_set_prototype_of = true;
        }
        expr.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
        if self
            .param_pairs
            .iter()
            .any(|(object, proto)| is_proto_assignment_for_pair(assign, object, proto))
        {
            self.has_proto_assignment = true;
        }
        assign.visit_children_with(self);
    }
}

/// Check if the module contains functions with Babel sub-helper body signals.
/// These are markers like Array.isArray, Array.from, Symbol.iterator that appear
/// in the inlined sub-helpers (arrayWithoutHoles, iterableToArray, etc.).
pub(super) fn module_has_babel_sub_helper_signals(module: &Module) -> bool {
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
