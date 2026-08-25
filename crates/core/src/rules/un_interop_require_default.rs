use std::collections::{HashMap, HashSet};

use swc_core::ecma::ast::{
    AssignOp, AssignTarget, Callee, Decl, Expr, Lit, MemberProp, Module, ModuleItem, Pat,
    SimpleAssignTarget, Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::transpiler_helper_utils::{
    classify_inline_helper_call, remove_helper_declarations, BindingKey, LocalHelperContext,
    TranspilerHelperKind,
};

/// Detects and unwraps `interopRequireDefault` helper calls.
///
/// Transforms:
///   `var _a = _interopRequireDefault(require("a")); _a.default`
///   → `var _a = require("a"); _a`
pub struct UnInteropRequireDefault;

impl UnInteropRequireDefault {
    pub(crate) fn run_with_helpers(module: &mut Module, local_helpers: &LocalHelperContext) {
        run_un_interop_require_default(module, local_helpers);
    }
}

impl VisitMut for UnInteropRequireDefault {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_interop_require_default(module, &local_helpers);
    }
}

fn run_un_interop_require_default(module: &mut Module, local_helpers: &LocalHelperContext) {
    let mut affected_bindings: HashSet<BindingKey> = HashSet::new();

    // --- Named helper path ---
    let helpers = local_helpers.helpers_of_kind(TranspilerHelperKind::InteropRequireDefault);
    let tslib_namespaces = local_helpers.tslib_namespaces();
    let has_direct_tslib_calls =
        local_helpers.has_tslib_require_member_call(TranspilerHelperKind::InteropRequireDefault);

    if !helpers.is_empty() || !tslib_namespaces.is_empty() || has_direct_tslib_calls {
        // Phase 1: Collect which bindings receive helper-wrapped values
        let mut collector = AffectedBindingCollector {
            local_helpers,
            affected: &mut affected_bindings,
        };
        collector.visit_module(module);

        // SWC's AMD output receives dependencies as factory parameters and
        // then wraps them in standalone assignments:
        //
        //   react = interopRequireDefault(react);
        //
        // AMD extraction turns the parameter into a require-backed local. The
        // helper call is therefore the binding's generated initialization,
        // not a later semantic reassignment. Recognize only the exact,
        // unconditional top-level producer shape and remove it before the
        // ordinary reassignment check.
        let assignment_initializers =
            collect_assignment_form_initializers(module, local_helpers, &mut affected_bindings);
        if !assignment_initializers.is_empty() {
            module.body = std::mem::take(&mut module.body)
                .into_iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    (!assignment_initializers.contains(&index)).then_some(item)
                })
                .collect();
        }

        // Phase 2a: Unwrap helper calls — replace `helper(arg)` with `arg`.
        let mut call_unwrapper = CallUnwrapper { local_helpers };
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

// ---------------------------------------------------------------------------
// Phase 1: Collect affected bindings
// ---------------------------------------------------------------------------

struct AffectedBindingCollector<'a> {
    local_helpers: &'a LocalHelperContext,
    affected: &'a mut HashSet<BindingKey>,
}

impl Visit for AffectedBindingCollector<'_> {
    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        let Pat::Ident(bi) = &decl.name else { return };
        let Some(init) = &decl.init else { return };

        // var _a = helper(arg)
        if is_helper_call(init, self.local_helpers) {
            self.affected.insert((bi.id.sym.clone(), bi.id.ctxt));
        }
    }
}

fn is_helper_call(expr: &Expr, local_helpers: &LocalHelperContext) -> bool {
    let Expr::Call(call) = expr else { return false };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    local_helpers.is_helper_callee(callee, TranspilerHelperKind::InteropRequireDefault)
}

/// Collect standalone interop assignments that initialize a require-backed
/// binding in SWC AMD output. The assignment must be the binding's first use
/// outside its declaration; otherwise `.default` accesses before the wrapper
/// could refer to the unwrapped require value and must remain untouched.
fn collect_assignment_form_initializers(
    module: &Module,
    local_helpers: &LocalHelperContext,
    affected: &mut HashSet<BindingKey>,
) -> HashSet<usize> {
    let require_declarations = collect_top_level_require_declarations(module, local_helpers);
    let mut matched_indices = HashSet::new();

    for (index, item) in module.body.iter().enumerate() {
        let Some(binding) = assignment_form_initializer_binding(item, local_helpers) else {
            continue;
        };
        let Some(&declaration_index) = require_declarations.get(&binding) else {
            continue;
        };
        if declaration_index >= index {
            continue;
        }

        let used_before_initializer =
            module.body[..index]
                .iter()
                .enumerate()
                .any(|(prior_index, prior_item)| {
                    prior_index != declaration_index
                        && item_references_binding(prior_item, &binding)
                });
        if used_before_initializer {
            continue;
        }

        affected.insert(binding);
        matched_indices.insert(index);
    }

    matched_indices
}

fn collect_top_level_require_declarations(
    module: &Module,
    local_helpers: &LocalHelperContext,
) -> HashMap<BindingKey, usize> {
    let mut declarations = HashMap::new();

    for (index, item) in module.body.iter().enumerate() {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = item else {
            continue;
        };
        // AMD extraction emits one declaration per dependency. Requiring that
        // exact shape also avoids overlooking another use in the same item.
        let [declarator] = var_decl.decls.as_slice() else {
            continue;
        };
        let Pat::Ident(binding) = &declarator.name else {
            continue;
        };
        let Some(init) = &declarator.init else {
            continue;
        };
        if is_static_require_call(init, local_helpers) {
            declarations.insert((binding.id.sym.clone(), binding.id.ctxt), index);
        }
    }

    declarations
}

fn is_static_require_call(expr: &Expr, local_helpers: &LocalHelperContext) -> bool {
    let Expr::Call(call) = expr else { return false };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Ident(require) = callee.as_ref() else {
        return false;
    };
    if !local_helpers.is_unresolved_or_unguarded_ident(require, "require") {
        return false;
    }
    let [arg] = call.args.as_slice() else {
        return false;
    };
    arg.spread.is_none() && matches!(arg.expr.as_ref(), Expr::Lit(Lit::Str(_)))
}

fn assignment_form_initializer_binding(
    item: &ModuleItem,
    local_helpers: &LocalHelperContext,
) -> Option<BindingKey> {
    let ModuleItem::Stmt(Stmt::Expr(expr_stmt)) = item else {
        return None;
    };
    let Expr::Assign(assign) = expr_stmt.expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(target)) = &assign.left else {
        return None;
    };
    let Expr::Call(call) = assign.right.as_ref() else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !local_helpers.is_helper_callee(callee, TranspilerHelperKind::InteropRequireDefault) {
        return None;
    }
    let [arg] = call.args.as_slice() else {
        return None;
    };
    if arg.spread.is_some() {
        return None;
    }
    let Expr::Ident(argument) = arg.expr.as_ref() else {
        return None;
    };
    let target_key = (target.id.sym.clone(), target.id.ctxt);
    (target_key == (argument.sym.clone(), argument.ctxt)).then_some(target_key)
}

fn item_references_binding(item: &ModuleItem, binding: &BindingKey) -> bool {
    let mut finder = BindingReferenceFinder {
        binding,
        found: false,
    };
    item.visit_with(&mut finder);
    finder.found
}

struct BindingReferenceFinder<'a> {
    binding: &'a BindingKey,
    found: bool,
}

impl Visit for BindingReferenceFinder<'_> {
    fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
        if (ident.sym.clone(), ident.ctxt) == *self.binding {
            self.found = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 2a: Unwrap helper calls
// ---------------------------------------------------------------------------

struct CallUnwrapper<'a> {
    local_helpers: &'a LocalHelperContext,
}

impl VisitMut for CallUnwrapper<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        // helper(arg).default → arg
        if let Expr::Member(member) = expr {
            if is_default_prop(&member.prop) {
                if let Expr::Call(call) = member.obj.as_ref() {
                    if let Some(arg) = extract_helper_call_arg(call, self.local_helpers) {
                        *expr = arg;
                        return;
                    }
                }
            }
        }

        // helper(arg) → arg
        if let Expr::Call(call) = expr {
            if let Some(arg) = extract_helper_call_arg(call, self.local_helpers) {
                *expr = arg;
            }
        }
    }
}

fn extract_helper_call_arg(
    call: &swc_core::ecma::ast::CallExpr,
    local_helpers: &LocalHelperContext,
) -> Option<Expr> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !local_helpers.is_helper_callee(callee, TranspilerHelperKind::InteropRequireDefault) {
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
        if let AssignTarget::Simple(SimpleAssignTarget::Ident(id)) = &assign.left {
            let key = (id.id.sym.clone(), id.id.ctxt);
            if self.candidates.contains(&key) {
                self.reassigned.insert(key);
            }
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
            let Expr::Call(call) = init.as_ref() else {
                continue;
            };
            let Some((kind, inner_arg)) = classify_inline_helper_call(call) else {
                continue;
            };
            // Only strip `.default` for the default interop, not wildcard.
            match kind {
                TranspilerHelperKind::InteropRequireDefault => {
                    let key = (binding.id.sym.clone(), binding.id.ctxt);
                    affected.insert(key);
                }
                TranspilerHelperKind::InteropRequireWildcard => {}
                // Other helper IIFEs are handled by their own rules.
                _ => continue,
            }
            let inner_arg = Box::new(inner_arg.clone());
            declarator.init = Some(inner_arg);
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
