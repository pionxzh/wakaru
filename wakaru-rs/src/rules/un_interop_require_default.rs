use std::collections::{HashMap, HashSet};

use swc_core::ecma::ast::{
    ArrowExpr, AssignTarget, BlockStmtOrExpr, CallExpr, Callee, Decl, Expr, FnExpr, Lit,
    MemberExpr, MemberProp, Module, ModuleItem, Pat, SimpleAssignTarget, Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::babel_helper_utils::{
    collect_helpers, remove_helper_declarations, BabelHelperKind, BindingKey,
};

/// Detects and unwraps `interopRequireDefault` helper calls.
///
/// Transforms:
///   `var _a = _interopRequireDefault(require("a")); _a.default`
///   → `var _a = require("a"); _a`
pub struct UnInteropRequireDefault;

impl VisitMut for UnInteropRequireDefault {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let mut affected_bindings: HashSet<BindingKey> = HashSet::new();

        // --- Named helper path ---
        let all_helpers = collect_helpers(module);
        let helpers: HashMap<BindingKey, BabelHelperKind> = all_helpers
            .into_iter()
            .filter(|(_, kind)| *kind == BabelHelperKind::InteropRequireDefault)
            .collect();

        if !helpers.is_empty() {
            // Phase 1: Collect which bindings receive helper-wrapped values
            let mut collector = AffectedBindingCollector {
                helpers: &helpers,
                affected: &mut affected_bindings,
            };
            collector.visit_module(module);

            // Phase 2a: Unwrap helper calls — replace `helper(arg)` with `arg`.
            let mut call_unwrapper = CallUnwrapper { helpers: &helpers };
            module.visit_mut_with(&mut call_unwrapper);
        }

        // --- Inline IIFE interop path ---
        // Detect: `const x = ((e) => { if (e && e.__esModule) return e; return {default: e} })(require(...))`
        // Replace with: `const x = require(...)`  and record `x` as affected
        unwrap_inline_interop_iifes(module, &mut affected_bindings);

        // Phase 2b: Rewrite `.default` member access on affected bindings,
        //           but only if the binding is never reassigned.
        if !affected_bindings.is_empty() {
            let mut reassigned = HashSet::new();
            let mut checker = ReassignmentChecker {
                candidates: &affected_bindings,
                reassigned: &mut reassigned,
            };
            module.visit_with(&mut checker);
            for key in &reassigned {
                affected_bindings.remove(key);
            }
        }
        if !affected_bindings.is_empty() {
            let mut ref_rewriter = DefaultRefRewriter {
                affected: &affected_bindings,
            };
            module.visit_mut_with(&mut ref_rewriter);
        }

        // Phase 3: Remove helper declarations.
        if !helpers.is_empty() {
            remove_helper_declarations(&mut module.body, &helpers);
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 1: Collect affected bindings
// ---------------------------------------------------------------------------

struct AffectedBindingCollector<'a> {
    helpers: &'a HashMap<BindingKey, BabelHelperKind>,
    affected: &'a mut HashSet<BindingKey>,
}

impl Visit for AffectedBindingCollector<'_> {
    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        let Pat::Ident(bi) = &decl.name else { return };
        let Some(init) = &decl.init else { return };

        // var _a = helper(arg)
        if is_helper_call(init, self.helpers) {
            self.affected.insert((bi.id.sym.clone(), bi.id.ctxt));
        }
    }
}

fn is_helper_call(expr: &Expr, helpers: &HashMap<BindingKey, BabelHelperKind>) -> bool {
    let Expr::Call(call) = expr else { return false };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Ident(id) = callee.as_ref() else {
        return false;
    };
    helpers.contains_key(&(id.sym.clone(), id.ctxt))
}

// ---------------------------------------------------------------------------
// Phase 2a: Unwrap helper calls
// ---------------------------------------------------------------------------

struct CallUnwrapper<'a> {
    helpers: &'a HashMap<BindingKey, BabelHelperKind>,
}

impl VisitMut for CallUnwrapper<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        // helper(arg).default → arg
        if let Expr::Member(member) = expr {
            if is_default_prop(&member.prop) {
                if let Expr::Call(call) = member.obj.as_ref() {
                    if let Some(arg) = extract_helper_call_arg(call, self.helpers) {
                        *expr = arg;
                        return;
                    }
                }
            }
        }

        // helper(arg) → arg
        if let Expr::Call(call) = expr {
            if let Some(arg) = extract_helper_call_arg(call, self.helpers) {
                *expr = arg;
            }
        }
    }
}

fn extract_helper_call_arg(
    call: &swc_core::ecma::ast::CallExpr,
    helpers: &HashMap<BindingKey, BabelHelperKind>,
) -> Option<Expr> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(id) = callee.as_ref() else {
        return None;
    };
    if !helpers.contains_key(&(id.sym.clone(), id.ctxt)) {
        return None;
    }
    if call.args.len() != 1 {
        return None;
    }
    Some(*call.args[0].expr.clone())
}

// ---------------------------------------------------------------------------
// Phase 2b (pre): Check for reassignment of affected bindings
// ---------------------------------------------------------------------------

struct ReassignmentChecker<'a> {
    candidates: &'a HashSet<BindingKey>,
    reassigned: &'a mut HashSet<BindingKey>,
}

impl Visit for ReassignmentChecker<'_> {
    fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
        match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(id)) => {
                let key = (id.id.sym.clone(), id.id.ctxt);
                if self.candidates.contains(&key) {
                    self.reassigned.insert(key);
                }
            }
            _ => {}
        }
        assign.visit_children_with(self);
    }
}

// ---------------------------------------------------------------------------
// Phase 2b: Rewrite .default references on affected bindings
// ---------------------------------------------------------------------------

struct DefaultRefRewriter<'a> {
    affected: &'a HashSet<BindingKey>,
}

impl VisitMut for DefaultRefRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        // x.default → x  (or x["default"] → x, already normalized by UnBracketNotation)
        if let Expr::Member(member) = expr {
            if is_default_prop(&member.prop) {
                if let Expr::Ident(obj) = member.obj.as_ref() {
                    if self.affected.contains(&(obj.sym.clone(), obj.ctxt)) {
                        *expr = Expr::Ident(obj.clone());
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Inline IIFE interop detection and unwrapping
// ---------------------------------------------------------------------------

/// Detect and unwrap inline interop IIFEs:
/// ```js
/// const x = ((e) => {
///     if (e && e.__esModule) { return e; }
///     return { default: e };
/// })(require("./module.js"));
/// ```
/// → `const x = require("./module.js")`
fn unwrap_inline_interop_iifes(module: &mut Module, affected: &mut HashSet<BindingKey>) {
    for item in &mut module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = item else {
            continue;
        };
        for declarator in &mut var_decl.decls {
            let Pat::Ident(binding) = &declarator.name else {
                continue;
            };
            let Some(init) = &declarator.init else {
                continue;
            };
            if let Some((inner_arg, kind)) = extract_inline_interop_arg_with_kind(init) {
                // Only strip `.default` for the default interop, not wildcard
                if kind == InteropKind::Default {
                    let key = (binding.id.sym.clone(), binding.id.ctxt);
                    affected.insert(key);
                }
                declarator.init = Some(inner_arg);
            }
        }
    }
}

/// Extract the IIFE argument and interop kind from an inline interop expression.
fn extract_inline_interop_arg_with_kind(expr: &Expr) -> Option<(Box<Expr>, InteropKind)> {
    let Expr::Call(CallExpr {
        callee: Callee::Expr(callee),
        args,
        ..
    }) = expr
    else {
        return None;
    };
    if args.len() != 1 || args[0].spread.is_some() {
        return None;
    }

    // Unwrap parens around the callee
    let callee = strip_parens_expr(callee);

    let body_stmts = match callee {
        Expr::Arrow(ArrowExpr { body, params, .. }) if params.len() == 1 => match &**body {
            BlockStmtOrExpr::BlockStmt(block) => &block.stmts,
            _ => return None,
        },
        Expr::Fn(FnExpr { function, .. }) if function.params.len() == 1 => {
            function.body.as_ref()?.stmts.as_slice()
        }
        _ => return None,
    };

    let kind = classify_interop_body(body_stmts)?;
    Some((args[0].expr.clone(), kind))
}

#[derive(PartialEq)]
enum InteropKind {
    /// `if (e.__esModule) return e; return { default: e }` — strips `.default`
    Default,
    /// `if (e.__esModule) return e; ... t.default = e; return t` — namespace import, no `.default` strip
    Wildcard,
}

/// Check if the function body matches an __esModule interop pattern.
fn classify_interop_body(stmts: &[Stmt]) -> Option<InteropKind> {
    if stmts.is_empty() {
        return None;
    }

    // First statement must be: if (e && e.__esModule) { return e; }
    let Stmt::If(if_stmt) = &stmts[0] else {
        return None;
    };
    if !is_esmodule_check(&if_stmt.test) {
        return None;
    }

    if stmts.len() == 2 {
        // Default pattern: return { default: e }
        let Stmt::Return(ret) = &stmts[1] else {
            return None;
        };
        let Some(arg) = &ret.arg else {
            return None;
        };
        let Expr::Object(obj) = &**arg else {
            return None;
        };
        if obj.props.len() != 1 {
            return None;
        }
        let swc_core::ecma::ast::PropOrSpread::Prop(prop) = &obj.props[0] else {
            return None;
        };
        let swc_core::ecma::ast::Prop::KeyValue(kv) = &**prop else {
            return None;
        };
        if matches!(&kv.key, swc_core::ecma::ast::PropName::Ident(id) if id.sym.as_ref() == "default")
        {
            return Some(InteropKind::Default);
        }
        return None;
    }

    // Wildcard pattern (3+ stmts): copies all props, sets .default, returns namespace object.
    // Require: penultimate stmt is `t.default = e` (the defining feature of wildcard interop).
    if stmts.len() >= 3 {
        let Stmt::Return(ret) = stmts.last()? else {
            return None;
        };
        if ret.arg.is_none() {
            return None;
        }
        // Check penultimate: must be `X.default = Y`
        let penultimate = &stmts[stmts.len() - 2];
        if is_default_assignment(penultimate) {
            return Some(InteropKind::Wildcard);
        }
    }

    None
}

/// Check if expression matches `e && e.__esModule`
fn is_esmodule_check(expr: &Expr) -> bool {
    let Expr::Bin(bin) = expr else {
        return false;
    };
    if bin.op != swc_core::ecma::ast::BinaryOp::LogicalAnd {
        return false;
    }
    // right must be X.__esModule
    let Expr::Member(MemberExpr {
        prop: MemberProp::Ident(prop),
        ..
    }) = &*bin.right
    else {
        return false;
    };
    prop.sym.as_ref() == "__esModule"
}

fn strip_parens_expr(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(p) => strip_parens_expr(&p.expr),
        _ => expr,
    }
}

/// Check if a statement is `X.default = Y` (the wildcard interop's namespace default assignment).
fn is_default_assignment(stmt: &Stmt) -> bool {
    let Stmt::Expr(swc_core::ecma::ast::ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    let Expr::Assign(assign) = &**expr else {
        return false;
    };
    let swc_core::ecma::ast::AssignTarget::Simple(swc_core::ecma::ast::SimpleAssignTarget::Member(
        member,
    )) = &assign.left
    else {
        return false;
    };
    is_default_prop(&member.prop)
}

fn is_default_prop(prop: &MemberProp) -> bool {
    match prop {
        MemberProp::Ident(id) => id.sym.as_ref() == "default",
        MemberProp::Computed(c) => {
            matches!(c.expr.as_ref(), Expr::Lit(Lit::Str(s)) if s.value.as_str() == Some("default"))
        }
        _ => false,
    }
}
