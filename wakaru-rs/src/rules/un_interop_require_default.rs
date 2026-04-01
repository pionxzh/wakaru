use std::collections::{HashMap, HashSet};

use swc_core::ecma::ast::{
    AssignTarget, Callee, Expr, Lit, MemberProp, Module, Pat, SimpleAssignTarget, VarDeclarator,
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
        let all_helpers = collect_helpers(module);
        let helpers: HashMap<BindingKey, BabelHelperKind> = all_helpers
            .into_iter()
            .filter(|(_, kind)| *kind == BabelHelperKind::InteropRequireDefault)
            .collect();
        if helpers.is_empty() {
            return;
        }

        // Phase 1: Collect which bindings receive helper-wrapped values
        //          (e.g. `var _a = helper(require("x"))` → record `_a`)
        let mut affected_bindings: HashSet<BindingKey> = HashSet::new();
        let mut collector = AffectedBindingCollector {
            helpers: &helpers,
            affected: &mut affected_bindings,
        };
        collector.visit_module(module);

        // Phase 2a: Unwrap helper calls — replace `helper(arg)` with `arg`.
        //           Also handle `helper(arg).default` → `arg`.
        let mut call_unwrapper = CallUnwrapper { helpers: &helpers };
        module.visit_mut_with(&mut call_unwrapper);

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
        remove_helper_declarations(&mut module.body, &helpers);
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
    let Callee::Expr(callee) = &call.callee else { return false };
    let Expr::Ident(id) = callee.as_ref() else { return false };
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
    let Callee::Expr(callee) = &call.callee else { return None };
    let Expr::Ident(id) = callee.as_ref() else { return None };
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

fn is_default_prop(prop: &MemberProp) -> bool {
    match prop {
        MemberProp::Ident(id) => id.sym.as_ref() == "default",
        MemberProp::Computed(c) => {
            matches!(c.expr.as_ref(), Expr::Lit(Lit::Str(s)) if s.value.as_str() == Some("default"))
        }
        _ => false,
    }
}
