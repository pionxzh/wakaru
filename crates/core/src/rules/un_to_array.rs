use std::collections::HashSet;

use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, AssignTargetPat, Callee, Decl, Expr, ImportSpecifier, Lit,
    Module, ModuleDecl, ModuleExportName, ModuleItem, Pat, Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::helper_matcher::{
    binding_key, remaining_refs_outside_declarations, remove_fn_decls_by_binding,
    remove_import_specifiers_by_binding, remove_var_declarators_by_binding,
};
use super::transpiler_helper_utils::BindingKey;

/// Known import paths for the array-rest `toArray` helper.
///
/// Distinct from `toConsumableArray` (spread, `[...x]`) and `slicedToArray`
/// (fixed-count destructuring): `toArray` lowers array destructuring with a
/// **rest** element, `const [a, ...b] = x`.
const TO_ARRAY_PATHS: &[&str] = &[
    "@babel/runtime/helpers/toArray",
    "@babel/runtime/helpers/esm/toArray",
    "@swc/helpers/_/_to_array",
];

/// Unwraps the `toArray` helper around an array-destructuring source:
/// `[a, ...b] = _toArray(x)` becomes `[a, ...b] = x`.
///
/// Babel/swc emit this helper *only* as the source of array destructuring with a
/// rest element (babel `plugin-transform-destructuring` picks `toArray` when the
/// element count is unbounded, `slicedToArray` when it is fixed). The helper body
/// is `arrayWithHoles(x) || iterableToArray(x) || unsupportedIterableToArray(x)
/// || nonIterableRest()` in both toolchains. Array destructuring already drives
/// the iterator protocol, so once `UnDestructuring` has rebuilt the `[a, ...b]`
/// pattern the wrapper is redundant. This rule only fires when the assignment
/// target is an array pattern — the proof that the value is being destructured.
///
/// ## Assumption: `rest_source_is_iterable`
///
/// `toArray(x)` is slightly more permissive than native `[a, ...b] = x`: its
/// `unsupportedIterableToArray` branch also accepts a *non-iterable array-like*
/// (e.g. `{ 0: …, length: n }`), which native rest destructuring would reject
/// with a `TypeError`. Stripping the wrapper therefore assumes `x` is a genuine
/// iterable — which holds by construction, because the original pre-lowering
/// source was native `[a, ...b] = x`; the array-like tolerance is the compiler's
/// polyfill, not author intent. (When babel opts into array-like sources via the
/// `arrayLikeIsIterable` assumption it emits a *different* helper,
/// `maybeArrayLike`, which this rule does not match.) This is the same recovery
/// contract `UnSlicedToArray` and `UnToConsumableArray` already rely on.
///
/// Runs after `UnDestructuring`, which builds the `[a, ...b]` pattern; this rule
/// then strips the now-redundant helper call and drops its import.
pub struct UnToArray;

impl UnToArray {
    pub fn new() -> Self {
        Self
    }
}

impl Default for UnToArray {
    fn default() -> Self {
        Self::new()
    }
}

impl VisitMut for UnToArray {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let helpers = collect_to_array_bindings(module);
        if !helpers.is_empty() {
            let mut replacer = ToArrayUnwrapper { helpers: &helpers };
            module.visit_mut_with(&mut replacer);

            let remaining = remaining_refs_outside_declarations(module, &helpers, &helpers);
            let removable: HashSet<BindingKey> = helpers.difference(&remaining).cloned().collect();
            if !removable.is_empty() {
                remove_import_specifiers_by_binding(&mut module.body, &removable);
                remove_fn_decls_by_binding(module, &removable);
                remove_var_declarators_by_binding(&mut module.body, &removable);
            }
        }

        module.visit_mut_with(&mut MaybeArrayLikeUnwrapper);
    }
}

struct MaybeArrayLikeUnwrapper;

impl VisitMut for MaybeArrayLikeUnwrapper {
    fn visit_mut_var_declarator(&mut self, declarator: &mut VarDeclarator) {
        declarator.visit_mut_children_with(self);
        if !is_array_rest_pat(&declarator.name) {
            return;
        }
        if let Some(init) = &declarator.init {
            if let Some(arg) = unwrap_maybe_array_like(init) {
                declarator.init = Some(arg);
            }
        }
    }

    fn visit_mut_assign_expr(&mut self, assign: &mut AssignExpr) {
        assign.visit_mut_children_with(self);
        if assign.op != AssignOp::Assign {
            return;
        }
        if !is_array_rest_assign_target(&assign.left) {
            return;
        }
        if let Some(arg) = unwrap_maybe_array_like(&assign.right) {
            assign.right = arg;
        }
    }
}

struct ToArrayUnwrapper<'a> {
    helpers: &'a HashSet<BindingKey>,
}

impl ToArrayUnwrapper<'_> {
    /// `_toArray(arg)` -> `arg`.
    fn unwrap(&self, expr: &Expr) -> Option<Box<Expr>> {
        let Expr::Call(call) = expr else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Ident(id) = callee.as_ref() else {
            return None;
        };
        if !self.helpers.contains(&binding_key(id)) {
            return None;
        }
        if call.args.len() != 1 || call.args[0].spread.is_some() {
            return None;
        }
        Some(call.args[0].expr.clone())
    }
}

/// `_maybeArrayLike(helperRef, source)` -> `source` on an array-rest init.
///
/// Babel emits this wrapper when the `arrayLikeIsIterable` assumption is on.
/// The wrapper adds array-like tolerance that native destructuring doesn't need,
/// so stripping it is safe under the same `rest_source_is_iterable` assumption.
fn unwrap_maybe_array_like(expr: &Expr) -> Option<Box<Expr>> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(id) = callee.as_ref() else {
        return None;
    };
    if !matches!(id.sym.as_ref(), "_maybeArrayLike" | "_maybe_array_like") {
        return None;
    }
    if call.args.len() != 2 || call.args.iter().any(|a| a.spread.is_some()) {
        return None;
    }
    if !matches!(call.args[0].expr.as_ref(), Expr::Ident(_)) {
        return None;
    }
    Some(call.args[1].expr.clone())
}

impl VisitMut for ToArrayUnwrapper<'_> {
    fn visit_mut_var_declarator(&mut self, declarator: &mut VarDeclarator) {
        declarator.visit_mut_children_with(self);

        if !matches!(declarator.name, Pat::Array(_)) {
            return;
        }
        if let Some(init) = &declarator.init {
            if let Some(arg) = self.unwrap(init) {
                declarator.init = Some(arg);
            }
        }
    }

    fn visit_mut_assign_expr(&mut self, assign: &mut AssignExpr) {
        assign.visit_mut_children_with(self);

        if assign.op != AssignOp::Assign {
            return;
        }
        if !matches!(assign.left, AssignTarget::Pat(AssignTargetPat::Array(_))) {
            return;
        }
        if let Some(arg) = self.unwrap(&assign.right) {
            assign.right = arg;
        }
    }
}

fn is_array_rest_pat(pat: &Pat) -> bool {
    let Pat::Array(arr) = pat else { return false };
    arr.elems.iter().any(|e| matches!(e, Some(Pat::Rest(_))))
}

fn is_array_rest_assign_target(target: &AssignTarget) -> bool {
    let AssignTarget::Pat(AssignTargetPat::Array(arr)) = target else {
        return false;
    };
    arr.elems.iter().any(|e| matches!(e, Some(Pat::Rest(_))))
}

fn collect_to_array_bindings(module: &Module) -> HashSet<BindingKey> {
    let mut bindings = HashSet::new();

    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                if import.type_only {
                    continue;
                }
                let path = import.src.value.as_str().unwrap_or("");
                if !TO_ARRAY_PATHS.contains(&path) {
                    continue;
                }
                for specifier in &import.specifiers {
                    match specifier {
                        // import _toArray from "@babel/runtime/helpers/toArray"
                        ImportSpecifier::Default(default) => {
                            bindings.insert(binding_key(&default.local));
                        }
                        // import { _ as _to_array } from "@swc/helpers/_/_to_array"
                        ImportSpecifier::Named(named) if named_import_is_helper(named) => {
                            bindings.insert(binding_key(&named.local));
                        }
                        _ => {}
                    }
                }
            }
            // var _toArray = require("@babel/runtime/helpers/toArray")
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for declarator in &var.decls {
                    if let Some(init) = &declarator.init {
                        if init_requires_to_array(init) {
                            if let Pat::Ident(binding) = &declarator.name {
                                bindings.insert(binding_key(&binding.id));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    bindings
}

/// The swc helper export is always named `_`; accept the bare or aliased form.
fn named_import_is_helper(named: &swc_core::ecma::ast::ImportNamedSpecifier) -> bool {
    match &named.imported {
        Some(ModuleExportName::Ident(id)) => id.sym.as_ref() == "_",
        Some(ModuleExportName::Str(s)) => s.value.as_str() == Some("_"),
        None => named.local.sym.as_ref() == "_",
    }
}

/// `require("...toArray")` or `require("...toArray").default`.
fn init_requires_to_array(init: &Expr) -> bool {
    let call = match init {
        Expr::Call(call) => call,
        Expr::Member(member) => {
            let Expr::Call(call) = member.obj.as_ref() else {
                return false;
            };
            call
        }
        _ => return false,
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Ident(id) = callee.as_ref() else {
        return false;
    };
    if id.sym.as_ref() != "require" || call.args.len() != 1 {
        return false;
    }
    let Expr::Lit(Lit::Str(path)) = call.args[0].expr.as_ref() else {
        return false;
    };
    path.value
        .as_str()
        .is_some_and(|path| TO_ARRAY_PATHS.contains(&path))
}
