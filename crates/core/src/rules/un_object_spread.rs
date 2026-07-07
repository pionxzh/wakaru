use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BinaryOp, BlockStmt, BlockStmtOrExpr, CallExpr, Callee, Decl, Expr,
    ForInStmt, Function, Ident, ImportSpecifier, Lit, MemberExpr, MemberProp, Module, ModuleDecl,
    ModuleItem, ObjectLit, Pat, Prop, PropName, PropOrSpread, ReturnStmt, SimpleAssignTarget,
    SpreadElement, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::facts::{HelperKind, ModuleFactsMap, TypeScriptHelperKind};

use super::cross_module_helper_refs::{
    collect_cross_module_helper_refs, collect_cross_module_ts_helper_refs,
    cross_module_member_helper_kind, cross_module_ts_member_helper,
};
use super::helper_matcher::{
    binding_key, member_prop_name, remaining_refs_outside_declarations, remove_fn_decls_by_binding,
    remove_var_declarators_by_binding, static_member_prop_name, var_declarator_binding_key,
};
use super::transpiler_helper_utils::{
    tslib_member_helper_kind, BindingKey, LocalHelperContext, TranspilerHelperKind,
};

use crate::utils::paren::strip_parens;

/// Detects and replaces `_extends` and `_objectSpread2` helper calls with
/// object spread syntax.
///
/// Both `_extends` and `_objectSpread2` mutate and return their first argument
/// (like Object.assign). Only transform when the first arg is a safe fresh object
/// literal target, which guarantees no mutation/identity side effects:
///   `_extends({}, obj1, obj2)` → `{ ...obj1, ...obj2 }`
///   `_extends({ a: 1 }, obj1)` → `{ a: 1, ...obj1 }`
///   `_objectSpread2({}, y)` → `{ ...y }`
///   `_extends(target, source)` → left as-is (mutation semantics)
///   `_objectSpread2(existing, {a: 1})` → left as-is (mutation semantics)
pub struct UnObjectSpread<'a> {
    module_facts: Option<&'a ModuleFactsMap>,
    current_filename: Option<&'a str>,
    unresolved_mark: Option<Mark>,
}

impl UnObjectSpread<'_> {
    pub fn new() -> Self {
        Self {
            module_facts: None,
            current_filename: None,
            unresolved_mark: None,
        }
    }

    pub fn new_with_mark(unresolved_mark: Mark) -> Self {
        Self {
            module_facts: None,
            current_filename: None,
            unresolved_mark: Some(unresolved_mark),
        }
    }
}

impl<'a> UnObjectSpread<'a> {
    pub fn new_with_facts(module_facts: &'a ModuleFactsMap) -> Self {
        Self {
            module_facts: Some(module_facts),
            current_filename: None,
            unresolved_mark: None,
        }
    }

    pub(crate) fn run_with_helpers(
        module: &mut Module,
        unresolved_mark: Mark,
        local_helpers: &LocalHelperContext,
        module_facts: Option<&ModuleFactsMap>,
        current_filename: Option<&str>,
    ) {
        run_un_object_spread(
            module,
            Some(unresolved_mark),
            local_helpers,
            module_facts,
            current_filename,
        );
    }
}

impl VisitMut for UnObjectSpread<'_> {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = self.unresolved_mark.map_or_else(
            || LocalHelperContext::collect(module),
            |mark| LocalHelperContext::collect_with_mark(module, mark),
        );
        run_un_object_spread(
            module,
            self.unresolved_mark,
            &local_helpers,
            self.module_facts,
            self.current_filename,
        );
    }
}

fn run_un_object_spread(
    module: &mut Module,
    unresolved_mark: Option<Mark>,
    local_helper_context: &LocalHelperContext,
    module_facts: Option<&ModuleFactsMap>,
    current_filename: Option<&str>,
) {
    let esbuild_aliases = collect_esbuild_object_builtin_aliases(module, unresolved_mark);
    let esbuild_define_normal_prop_helpers = if esbuild_aliases.has_spread_values_signals() {
        collect_esbuild_define_normal_prop_helpers(module, &esbuild_aliases)
    } else {
        HashSet::new()
    };
    let mut local_helpers: HashMap<BindingKey, TranspilerHelperKind> = local_helper_context
        .helpers()
        .iter()
        .filter(|(_, kind)| {
            **kind == TranspilerHelperKind::Extends
                || **kind == TranspilerHelperKind::ObjectSpread
                || **kind == TranspilerHelperKind::DefineProperty
                || **kind == TranspilerHelperKind::HelperDependency
        })
        .map(|(key, kind)| (key.clone(), *kind))
        .collect();
    let uninitialized_object_spread_stubs = collect_uninitialized_object_spread_stubs(module);
    let mangled_esbuild_helpers = collect_mangled_esbuild_object_spread_helpers(
        module,
        &esbuild_aliases,
        &esbuild_define_normal_prop_helpers,
    );
    local_helpers.extend(uninitialized_object_spread_stubs.clone());
    local_helpers.extend(mangled_esbuild_helpers.clone());
    let mut helpers = local_helpers.clone();
    if let Some(module_facts) = module_facts {
        helpers.extend(collect_cross_module_object_spread_helpers(
            module,
            module_facts,
            current_filename,
        ));
    }
    let cross_module_helper_refs = module_facts
        .map(|facts| {
            collect_cross_module_helper_refs(module, facts, current_filename, |kind| {
                matches!(
                    kind,
                    TranspilerHelperKind::Extends | TranspilerHelperKind::ObjectSpread
                )
            })
        })
        .unwrap_or_default();
    helpers.extend(
        cross_module_helper_refs
            .direct
            .iter()
            .map(|(key, kind)| (key.clone(), *kind)),
    );
    let cross_module_ts_assign_refs = module_facts
        .map(|facts| {
            collect_cross_module_ts_helper_refs(
                module,
                facts,
                current_filename,
                TypeScriptHelperKind::Assign,
            )
        })
        .unwrap_or_default();
    helpers.extend(
        cross_module_ts_assign_refs
            .direct
            .iter()
            .map(|key| (key.clone(), TranspilerHelperKind::Extends)),
    );
    let swc_numeric_helper_namespaces =
        collect_swc_numeric_helper_namespaces(module, unresolved_mark);
    let tslib_namespaces = local_helper_context.tslib_namespaces();
    let has_inline_object_spread_call = has_inline_object_spread_call(
        module,
        &esbuild_aliases,
        &esbuild_define_normal_prop_helpers,
    );
    if helpers.is_empty()
        && cross_module_ts_assign_refs.namespaces.is_empty()
        && swc_numeric_helper_namespaces.is_empty()
        && cross_module_helper_refs.namespaces.is_empty()
        && tslib_namespaces.is_empty()
        && !has_inline_object_spread_call
    {
        return;
    }
    let mut replacer = SpreadReplacer {
        helpers: &helpers,
        cross_module_helper_namespaces: &cross_module_helper_refs.namespaces,
        cross_module_ts_assign_namespaces: &cross_module_ts_assign_refs.namespaces,
        swc_numeric_helper_namespaces: &swc_numeric_helper_namespaces,
        tslib_namespaces,
        esbuild_aliases: &esbuild_aliases,
        esbuild_define_normal_prop_helpers: &esbuild_define_normal_prop_helpers,
    };
    module.visit_mut_with(&mut replacer);
    remove_unused_numeric_helper_namespace_decls(module, &swc_numeric_helper_namespaces);

    // Only remove root helpers whose calls were fully transformed. Dependencies
    // referenced by retained helpers must stay with those helpers.
    let local_root_helpers: HashMap<BindingKey, TranspilerHelperKind> = local_helpers
        .iter()
        .filter(|(_, kind)| {
            matches!(
                kind,
                TranspilerHelperKind::Extends | TranspilerHelperKind::ObjectSpread
            )
        })
        .map(|(key, kind)| (key.clone(), *kind))
        .collect();
    let mut helper_dependency_cleanup_candidates: HashSet<_> =
        uninitialized_object_spread_stubs.keys().cloned().collect();
    helper_dependency_cleanup_candidates.extend(
        mangled_esbuild_helpers
            .iter()
            .filter(|(_, kind)| **kind == TranspilerHelperKind::HelperDependency)
            .map(|(key, _)| key.clone()),
    );
    helper_dependency_cleanup_candidates.extend(esbuild_define_normal_prop_helpers.iter().cloned());
    local_helper_context.remove_helpers_with_dependencies(module, local_root_helpers);
    remove_unused_helper_dependency_decls(module, &helper_dependency_cleanup_candidates);
    remove_unused_esbuild_object_builtin_aliases(module, &esbuild_aliases);
}

impl Default for UnObjectSpread<'_> {
    fn default() -> Self {
        Self::new()
    }
}

fn collect_uninitialized_object_spread_stubs(
    module: &Module,
) -> HashMap<BindingKey, TranspilerHelperKind> {
    let mut helpers = HashMap::new();

    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            if decl.init.is_some() {
                continue;
            }
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            if matches!(binding.id.sym.as_ref(), "__spreadValues" | "__spreadProps") {
                helpers.insert(
                    (binding.id.sym.clone(), binding.id.ctxt),
                    TranspilerHelperKind::HelperDependency,
                );
            }
        }
    }

    helpers
}

// esbuild object-spread detection is intentionally kept in this rule rather than
// in the central `transpiler_helper_utils` scanner. The Babel/SWC matchers there
// classify a function purely by its own AST shape (`fn(&Function) -> bool`), but
// esbuild's `__spreadValues` / `__spreadProps` can only be recognized against
// module-wide *state*: esbuild mangles `Object.defineProperty`,
// `Object.prototype.hasOwnProperty`, etc. into local aliases, and the spread
// helpers are matched relative to those aliases (`EsbuildObjectBuiltinAliases`)
// and the `__defNormalProp` helper. Threading that bundler-specific state through
// the generic scanner would couple it to esbuild internals, so per
// docs/helper-detection.md this stateful detection stays rule-local.
#[derive(Default)]
struct EsbuildObjectBuiltinAliases {
    define_property: HashSet<BindingKey>,
    define_properties: HashSet<BindingKey>,
    get_own_property_descriptors: HashSet<BindingKey>,
    get_own_property_symbols: HashSet<BindingKey>,
    has_own_property: HashSet<BindingKey>,
    property_is_enumerable: HashSet<BindingKey>,
}

impl EsbuildObjectBuiltinAliases {
    fn has_spread_values_signals(&self) -> bool {
        !self.define_property.is_empty()
            && !self.get_own_property_symbols.is_empty()
            && !self.has_own_property.is_empty()
            && !self.property_is_enumerable.is_empty()
    }

    fn has_spread_props_signals(&self) -> bool {
        !self.define_properties.is_empty() && !self.get_own_property_descriptors.is_empty()
    }

    fn dependency_keys(&self) -> impl Iterator<Item = BindingKey> + '_ {
        self.define_property
            .iter()
            .chain(&self.define_properties)
            .chain(&self.get_own_property_descriptors)
            .chain(&self.get_own_property_symbols)
            .chain(&self.has_own_property)
            .chain(&self.property_is_enumerable)
            .cloned()
    }
}

fn remove_unused_esbuild_object_builtin_aliases(
    module: &mut Module,
    aliases: &EsbuildObjectBuiltinAliases,
) {
    let candidates: HashSet<_> = aliases.dependency_keys().collect();
    if candidates.is_empty() {
        return;
    }
    let remaining = remaining_refs_outside_declarations(module, &candidates, &candidates);
    let removable: HashSet<_> = candidates
        .into_iter()
        .filter(|key| !remaining.contains(key))
        .collect();
    if !removable.is_empty() {
        remove_var_declarators_by_binding(&mut module.body, &removable);
    }
}

fn remove_unused_helper_dependency_decls(module: &mut Module, candidates: &HashSet<BindingKey>) {
    if candidates.is_empty() {
        return;
    }
    let remaining = remaining_refs_outside_declarations(module, candidates, candidates);
    let removable: HashSet<_> = candidates
        .iter()
        .filter(|key| !remaining.contains(*key))
        .cloned()
        .collect();
    if removable.is_empty() {
        return;
    }
    remove_fn_decls_by_binding(module, &removable);
    remove_var_declarators_by_binding(&mut module.body, &removable);
}

fn collect_mangled_esbuild_object_spread_helpers(
    module: &Module,
    aliases: &EsbuildObjectBuiltinAliases,
    define_normal_prop_helpers: &HashSet<BindingKey>,
) -> HashMap<BindingKey, TranspilerHelperKind> {
    if !aliases.has_spread_values_signals() && !aliases.has_spread_props_signals() {
        return HashMap::new();
    }

    let mut helpers = HashMap::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Some(key) = var_declarator_binding_key(decl) else {
                continue;
            };
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            if (aliases.has_spread_values_signals()
                && !define_normal_prop_helpers.is_empty()
                && esbuild_spread_values_helper_matches(init, define_normal_prop_helpers, aliases))
                || (aliases.has_spread_props_signals()
                    && esbuild_spread_props_helper_matches(init, aliases))
            {
                helpers.insert(key, TranspilerHelperKind::ObjectSpread);
            }
        }
    }

    if helpers.is_empty() {
        return helpers;
    }

    helpers.extend(
        aliases
            .dependency_keys()
            .map(|key| (key, TranspilerHelperKind::HelperDependency)),
    );
    helpers.extend(
        define_normal_prop_helpers
            .iter()
            .cloned()
            .map(|key| (key, TranspilerHelperKind::HelperDependency)),
    );
    helpers
}

fn collect_esbuild_object_builtin_aliases(
    module: &Module,
    unresolved_mark: Option<Mark>,
) -> EsbuildObjectBuiltinAliases {
    let mut aliases = EsbuildObjectBuiltinAliases::default();

    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Some(key) = var_declarator_binding_key(decl) else {
                continue;
            };
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            match object_builtin_alias_kind(init, unresolved_mark) {
                Some("defineProperty") => {
                    aliases.define_property.insert(key);
                }
                Some("defineProperties") => {
                    aliases.define_properties.insert(key);
                }
                Some("getOwnPropertyDescriptors") => {
                    aliases.get_own_property_descriptors.insert(key);
                }
                Some("getOwnPropertySymbols") => {
                    aliases.get_own_property_symbols.insert(key);
                }
                Some("hasOwnProperty") => {
                    aliases.has_own_property.insert(key);
                }
                Some("propertyIsEnumerable") => {
                    aliases.property_is_enumerable.insert(key);
                }
                _ => {}
            }
        }
    }

    aliases
}

fn object_builtin_alias_kind(expr: &Expr, unresolved_mark: Option<Mark>) -> Option<&'static str> {
    let is_global_object = |obj: &Ident| {
        obj.sym.as_ref() == "Object" && unresolved_mark.is_none_or(|mark| obj.ctxt.outer() == mark)
    };
    let Expr::Member(member) = strip_parens(expr) else {
        return None;
    };
    if let Expr::Ident(obj) = member.obj.as_ref() {
        if is_global_object(obj) {
            match static_member_prop_name(&member.prop) {
                Some("defineProperty") => return Some("defineProperty"),
                Some("defineProperties") => return Some("defineProperties"),
                Some("getOwnPropertyDescriptors") => return Some("getOwnPropertyDescriptors"),
                Some("getOwnPropertySymbols") => return Some("getOwnPropertySymbols"),
                _ => {}
            }
        }
    }

    let Expr::Member(proto_member) = member.obj.as_ref() else {
        return None;
    };
    let Expr::Ident(obj) = proto_member.obj.as_ref() else {
        return None;
    };
    if !is_global_object(obj) || !member_prop_name(&proto_member.prop, "prototype") {
        return None;
    }
    match static_member_prop_name(&member.prop) {
        Some("hasOwnProperty") => Some("hasOwnProperty"),
        Some("propertyIsEnumerable") => Some("propertyIsEnumerable"),
        _ => None,
    }
}

fn collect_esbuild_define_normal_prop_helpers(
    module: &Module,
    aliases: &EsbuildObjectBuiltinAliases,
) -> HashSet<BindingKey> {
    let mut helpers = HashSet::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Some(key) = var_declarator_binding_key(decl) else {
                continue;
            };
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            if esbuild_define_normal_prop_helper_matches(init, aliases) {
                helpers.insert(key);
            }
        }
    }
    helpers
}

fn esbuild_define_normal_prop_helper_matches(
    expr: &Expr,
    aliases: &EsbuildObjectBuiltinAliases,
) -> bool {
    let Some((target, key, value, body)) = helper_three_param_body(expr) else {
        return false;
    };
    let mut marker = DefineNormalPropMarker {
        target,
        key,
        value,
        define_property_aliases: &aliases.define_property,
        saw_define_property: false,
        saw_fallback_assign: false,
    };
    body.visit_with(&mut marker);
    marker.saw_define_property && marker.saw_fallback_assign
}

fn esbuild_spread_values_helper_matches(
    expr: &Expr,
    define_normal_prop_helpers: &HashSet<BindingKey>,
    aliases: &EsbuildObjectBuiltinAliases,
) -> bool {
    let Some((target, source, block)) = helper_two_param_block(expr) else {
        return false;
    };
    if !block_returns_binding(block, target) {
        return false;
    }

    let mut marker = SpreadValuesMarker {
        target,
        source,
        define_normal_prop_helpers,
        has_own_property_aliases: &aliases.has_own_property,
        saw_for_in_source: false,
        saw_has_own_call: false,
        saw_define_normal_prop_call: false,
    };
    block.visit_with(&mut marker);
    marker.saw_for_in_source && marker.saw_has_own_call && marker.saw_define_normal_prop_call
}

fn esbuild_spread_props_helper_matches(expr: &Expr, aliases: &EsbuildObjectBuiltinAliases) -> bool {
    let Some((target, source, body)) = helper_two_param_body(expr) else {
        return false;
    };
    spread_props_expr_matches(body, target, source, aliases)
}

fn helper_three_param_body(expr: &Expr) -> Option<(&Ident, &Ident, &Ident, &Expr)> {
    match strip_parens(expr) {
        Expr::Arrow(arrow) => {
            if arrow.params.len() != 3 {
                return None;
            }
            let target = pat_ident(&arrow.params[0])?;
            let key = pat_ident(&arrow.params[1])?;
            let value = pat_ident(&arrow.params[2])?;
            Some((target, key, value, arrow_body_expr(&arrow.body)?))
        }
        _ => None,
    }
}

fn helper_two_param_body(expr: &Expr) -> Option<(&Ident, &Ident, &Expr)> {
    match strip_parens(expr) {
        Expr::Arrow(arrow) => {
            if arrow.params.len() != 2 {
                return None;
            }
            let target = pat_ident(&arrow.params[0])?;
            let source = pat_ident(&arrow.params[1])?;
            Some((target, source, arrow_body_expr(&arrow.body)?))
        }
        Expr::Fn(fn_expr) => {
            let (target, source) = function_two_param_idents(&fn_expr.function)?;
            let body = function_single_return_expr(&fn_expr.function)?;
            Some((target, source, body))
        }
        _ => None,
    }
}

fn helper_two_param_block(expr: &Expr) -> Option<(&Ident, &Ident, &BlockStmt)> {
    match strip_parens(expr) {
        Expr::Arrow(arrow) => {
            if arrow.params.len() != 2 {
                return None;
            }
            let target = pat_ident(&arrow.params[0])?;
            let source = pat_ident(&arrow.params[1])?;
            let BlockStmtOrExpr::BlockStmt(block) = arrow.body.as_ref() else {
                return None;
            };
            Some((target, source, block))
        }
        Expr::Fn(fn_expr) => {
            let (target, source) = function_two_param_idents(&fn_expr.function)?;
            Some((target, source, fn_expr.function.body.as_ref()?))
        }
        _ => None,
    }
}

fn arrow_body_expr(body: &BlockStmtOrExpr) -> Option<&Expr> {
    match body {
        BlockStmtOrExpr::Expr(expr) => Some(expr),
        BlockStmtOrExpr::BlockStmt(block) => {
            if block.stmts.len() != 1 {
                return None;
            }
            let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = &block.stmts[0] else {
                return None;
            };
            Some(arg)
        }
    }
}

fn function_two_param_idents(func: &Function) -> Option<(&Ident, &Ident)> {
    if func.params.len() != 2 {
        return None;
    }
    Some((
        pat_ident(&func.params[0].pat)?,
        pat_ident(&func.params[1].pat)?,
    ))
}

fn function_single_return_expr(func: &Function) -> Option<&Expr> {
    let body = func.body.as_ref()?;
    if body.stmts.len() != 1 {
        return None;
    }
    let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = &body.stmts[0] else {
        return None;
    };
    Some(arg)
}

fn block_returns_binding(block: &BlockStmt, binding: &Ident) -> bool {
    matches!(
        block.stmts.last(),
        Some(Stmt::Return(ReturnStmt { arg: Some(arg), .. }))
            if is_binding_ref(arg, binding)
    )
}

fn spread_props_expr_matches(
    expr: &Expr,
    target: &Ident,
    source: &Ident,
    aliases: &EsbuildObjectBuiltinAliases,
) -> bool {
    // esbuild only ever emits `__spreadProps` wrapping a `__spreadValues`
    // result, so the values-family aliases are always in the preamble. Without
    // that corroboration the bare two-alias shape is just as likely a
    // user-written descriptor-copy utility, whose defineProperties semantics
    // (mutates target, preserves accessors) object spread would break.
    if !aliases.has_spread_values_signals() {
        return false;
    }
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    if call.args.len() != 2 || call.args.iter().any(|arg| arg.spread.is_some()) {
        return false;
    }
    if !callee_matches_alias_or_object_member(
        &call.callee,
        &aliases.define_properties,
        "defineProperties",
    ) || !is_binding_ref(&call.args[0].expr, target)
    {
        return false;
    }

    let Expr::Call(descs_call) = strip_parens(&call.args[1].expr) else {
        return false;
    };
    descs_call.args.len() == 1
        && descs_call.args[0].spread.is_none()
        && is_binding_ref(&descs_call.args[0].expr, source)
        && callee_matches_alias_or_object_member(
            &descs_call.callee,
            &aliases.get_own_property_descriptors,
            "getOwnPropertyDescriptors",
        )
}

struct DefineNormalPropMarker<'a> {
    target: &'a Ident,
    key: &'a Ident,
    value: &'a Ident,
    define_property_aliases: &'a HashSet<BindingKey>,
    saw_define_property: bool,
    saw_fallback_assign: bool,
}

impl Visit for DefineNormalPropMarker<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if call.args.len() >= 2
            && call.args[0].spread.is_none()
            && call.args[1].spread.is_none()
            && is_binding_ref(&call.args[0].expr, self.target)
            && is_binding_ref(&call.args[1].expr, self.key)
            && callee_matches_alias_or_object_member(
                &call.callee,
                self.define_property_aliases,
                "defineProperty",
            )
        {
            self.saw_define_property = true;
        }
        call.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
        if assign.op == AssignOp::Assign
            && is_binding_ref(&assign.right, self.value)
            && assign_target_matches_computed_member(&assign.left, self.target, self.key)
        {
            self.saw_fallback_assign = true;
        }
        assign.visit_children_with(self);
    }
}

struct SpreadValuesMarker<'a> {
    target: &'a Ident,
    source: &'a Ident,
    define_normal_prop_helpers: &'a HashSet<BindingKey>,
    has_own_property_aliases: &'a HashSet<BindingKey>,
    saw_for_in_source: bool,
    saw_has_own_call: bool,
    saw_define_normal_prop_call: bool,
}

impl Visit for SpreadValuesMarker<'_> {
    fn visit_for_in_stmt(&mut self, for_in: &ForInStmt) {
        if expr_is_source_or_default(&for_in.right, self.source) {
            self.saw_for_in_source = true;
        }
        for_in.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if call.args.len() >= 2
            && call.args[0].spread.is_none()
            && call.args[1].spread.is_none()
            && is_binding_ref(&call.args[0].expr, self.target)
            && callee_matches_binding_set(&call.callee, self.define_normal_prop_helpers)
        {
            self.saw_define_normal_prop_call = true;
        }
        if call
            .args
            .first()
            .is_some_and(|arg| arg.spread.is_none() && is_binding_ref(&arg.expr, self.source))
            && callee_is_alias_call_method(&call.callee, self.has_own_property_aliases)
        {
            self.saw_has_own_call = true;
        }
        call.visit_children_with(self);
    }
}

fn expr_is_source_or_default(expr: &Expr, source: &Ident) -> bool {
    if is_binding_ref(expr, source) {
        return true;
    }
    let Expr::Bin(bin) = strip_parens(expr) else {
        return false;
    };
    bin.op == BinaryOp::LogicalOr
        && is_binding_ref(&bin.left, source)
        && matches!(
            strip_parens(&bin.right),
            Expr::Assign(assign)
                if assign.op == AssignOp::Assign
                    && assign_target_matches_ident(&assign.left, source)
                    && matches!(strip_parens(&assign.right), Expr::Object(obj) if obj.props.is_empty())
        )
}

fn assign_target_matches_ident(target: &AssignTarget, ident: &Ident) -> bool {
    matches!(
        target,
        AssignTarget::Simple(SimpleAssignTarget::Ident(binding))
            if ident_matches(&binding.id, ident)
    )
}

fn assign_target_matches_computed_member(target: &AssignTarget, obj: &Ident, prop: &Ident) -> bool {
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = target else {
        return false;
    };
    member_matches_computed_binding(member, obj, prop)
}

fn member_matches_computed_binding(member: &MemberExpr, obj: &Ident, prop: &Ident) -> bool {
    if !matches!(member.obj.as_ref(), Expr::Ident(id) if ident_matches(id, obj)) {
        return false;
    }
    let swc_core::ecma::ast::MemberProp::Computed(computed) = &member.prop else {
        return false;
    };
    is_binding_ref(&computed.expr, prop)
}

fn callee_matches_alias_or_object_member(
    callee: &Callee,
    aliases: &HashSet<BindingKey>,
    member_name: &str,
) -> bool {
    let Callee::Expr(expr) = callee else {
        return false;
    };
    match strip_parens(expr) {
        Expr::Ident(id) => aliases.contains(&binding_key(id)),
        Expr::Member(member) => {
            matches!(member.obj.as_ref(), Expr::Ident(obj) if obj.sym.as_ref() == "Object")
                && member_prop_name(&member.prop, member_name)
        }
        _ => false,
    }
}

fn callee_matches_binding_set(callee: &Callee, bindings: &HashSet<BindingKey>) -> bool {
    let Callee::Expr(expr) = callee else {
        return false;
    };
    matches!(strip_parens(expr), Expr::Ident(id) if bindings.contains(&binding_key(id)))
}

fn callee_is_alias_call_method(callee: &Callee, aliases: &HashSet<BindingKey>) -> bool {
    let Callee::Expr(expr) = callee else {
        return false;
    };
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    member_prop_name(&member.prop, "call")
        && matches!(member.obj.as_ref(), Expr::Ident(id) if aliases.contains(&binding_key(id)))
}

fn pat_ident(pat: &Pat) -> Option<&Ident> {
    let Pat::Ident(binding) = pat else {
        return None;
    };
    Some(&binding.id)
}

fn collect_cross_module_object_spread_helpers(
    module: &Module,
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
) -> HashMap<BindingKey, TranspilerHelperKind> {
    let mut helpers = HashMap::new();

    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        let source = str_to_atom(&import.src.value);

        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Default(default) => {
                    if let Some(kind) = module_helper_export_kind(
                        module_facts,
                        current_filename,
                        &source,
                        "default",
                    ) {
                        helpers.insert((default.local.sym.clone(), default.local.ctxt), kind);
                    }
                }
                ImportSpecifier::Named(named) => {
                    let imported = named
                        .imported
                        .as_ref()
                        .map(export_name_to_atom)
                        .unwrap_or_else(|| named.local.sym.clone());
                    if let Some(kind) = module_helper_export_kind(
                        module_facts,
                        current_filename,
                        &source,
                        imported.as_ref(),
                    ) {
                        helpers.insert((named.local.sym.clone(), named.local.ctxt), kind);
                    }
                }
                ImportSpecifier::Namespace(_) => {}
            }
        }
    }

    helpers
}

fn module_helper_export_kind(
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
    source: &Atom,
    exported: &str,
) -> Option<TranspilerHelperKind> {
    module_facts
        .get_from(current_filename, source.as_ref())
        .and_then(|facts| {
            facts
                .helper_exports
                .iter()
                .find(|helper| helper.exported.as_ref() == exported)
                .and_then(|helper| helper_kind_to_transpiler(helper.kind))
        })
}

fn helper_kind_to_transpiler(kind: HelperKind) -> Option<TranspilerHelperKind> {
    match kind {
        HelperKind::Extends => Some(TranspilerHelperKind::Extends),
        HelperKind::ObjectSpread => Some(TranspilerHelperKind::ObjectSpread),
        _ => None,
    }
}

fn collect_swc_numeric_helper_namespaces(
    module: &Module,
    unresolved_mark: Option<Mark>,
) -> HashSet<BindingKey> {
    let mut namespaces = HashSet::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            if decl
                .init
                .as_deref()
                .is_some_and(|init| is_numeric_require_call(init, unresolved_mark))
            {
                namespaces.insert((binding.id.sym.clone(), binding.id.ctxt));
            }
        }
    }
    namespaces
}

fn is_numeric_require_call(expr: &Expr, unresolved_mark: Option<Mark>) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return false;
    }
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !matches!(callee.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "require" && unresolved_mark.is_none_or(|mark| id.ctxt.outer() == mark))
    {
        return false;
    }
    matches!(call.args[0].expr.as_ref(), Expr::Lit(Lit::Num(_)))
}

fn remove_unused_numeric_helper_namespace_decls(
    module: &mut Module,
    namespaces: &HashSet<BindingKey>,
) {
    if namespaces.is_empty() {
        return;
    }
    let unused: HashSet<_> = namespaces
        .iter()
        .filter(|(sym, ctxt)| {
            let ident = Ident::new(sym.clone(), DUMMY_SP, *ctxt);
            !ident_used_in_module(&module.body, &ident)
        })
        .cloned()
        .collect();
    if unused.is_empty() {
        return;
    }
    module.body.retain_mut(|item| {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            return true;
        };
        var.decls.retain(|decl| {
            let Pat::Ident(binding) = &decl.name else {
                return true;
            };
            !unused.contains(&(binding.id.sym.clone(), binding.id.ctxt))
        });
        !var.decls.is_empty()
    });
}

fn ident_used_in_module(body: &[ModuleItem], target: &Ident) -> bool {
    struct Finder<'a> {
        target: &'a Ident,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_binding_ident(&mut self, _: &swc_core::ecma::ast::BindingIdent) {}

        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym == self.target.sym && ident.ctxt == self.target.ctxt {
                self.found = true;
            }
        }
    }

    let mut finder = Finder {
        target,
        found: false,
    };
    for item in body {
        item.visit_with(&mut finder);
        if finder.found {
            return true;
        }
    }
    false
}

struct SpreadReplacer<'a> {
    helpers: &'a HashMap<BindingKey, TranspilerHelperKind>,
    cross_module_helper_namespaces: &'a HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
    cross_module_ts_assign_namespaces: &'a HashMap<BindingKey, HashSet<String>>,
    swc_numeric_helper_namespaces: &'a HashSet<BindingKey>,
    tslib_namespaces: &'a HashSet<BindingKey>,
    esbuild_aliases: &'a EsbuildObjectBuiltinAliases,
    esbuild_define_normal_prop_helpers: &'a HashSet<BindingKey>,
}

impl VisitMut for SpreadReplacer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        if !self.is_object_spread_callee(callee) {
            return;
        }

        if call.args.is_empty() {
            return;
        }

        // Both _extends and _objectSpread2 mutate their first argument.
        // Only transform when the first arg is a safe fresh object literal
        // target, otherwise mutation/identity semantics are lost.
        let Expr::Object(first_obj) = call.args[0].expr.as_ref() else {
            return;
        };
        if call.args[0].spread.is_some() || !is_safe_to_inline_props(&first_obj.props) {
            return;
        }

        // Merge all arguments into a single object expression.
        // - Object literal args: flatten their properties
        // - Everything else: wrap as spread element
        let mut properties: Vec<PropOrSpread> = first_obj.props.clone();

        for arg in &call.args[1..] {
            if arg.spread.is_some() {
                properties.push(PropOrSpread::Spread(SpreadElement {
                    dot3_token: DUMMY_SP,
                    expr: arg.expr.clone(),
                }));
                continue;
            }

            match arg.expr.as_ref() {
                Expr::Object(obj) if is_safe_to_inline_props(&obj.props) => {
                    properties.extend(obj.props.iter().cloned());
                }
                _ => {
                    properties.push(PropOrSpread::Spread(SpreadElement {
                        dot3_token: DUMMY_SP,
                        expr: arg.expr.clone(),
                    }));
                }
            }
        }

        *expr = Expr::Object(ObjectLit {
            span: DUMMY_SP,
            props: properties,
        });
    }
}

impl SpreadReplacer<'_> {
    fn is_object_spread_callee(&self, callee: &Expr) -> bool {
        match strip_parens(callee) {
            Expr::Ident(id) => {
                let key = (id.sym.clone(), id.ctxt);
                matches!(
                    self.helpers.get(&key),
                    Some(TranspilerHelperKind::Extends | TranspilerHelperKind::ObjectSpread)
                )
            }
            Expr::Member(_) => {
                matches!(
                    tslib_member_helper_kind(callee, self.tslib_namespaces),
                    Some(TranspilerHelperKind::Extends | TranspilerHelperKind::ObjectSpread)
                ) || matches!(
                    cross_module_member_helper_kind(callee, self.cross_module_helper_namespaces),
                    Some(TranspilerHelperKind::Extends | TranspilerHelperKind::ObjectSpread)
                ) || cross_module_ts_member_helper(callee, self.cross_module_ts_assign_namespaces)
                    || is_swc_numeric_object_spread_member(
                        callee,
                        self.swc_numeric_helper_namespaces,
                    )
            }
            expr => is_inline_object_spread_helper(
                expr,
                self.esbuild_aliases,
                self.esbuild_define_normal_prop_helpers,
            ),
        }
    }
}

fn is_swc_numeric_object_spread_member(expr: &Expr, namespaces: &HashSet<BindingKey>) -> bool {
    let Expr::Member(member) = expr else {
        return false;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };
    namespaces.contains(&binding_key(obj)) && static_member_prop_name(&member.prop) == Some("pi")
}

fn has_inline_object_spread_call(
    module: &Module,
    esbuild_aliases: &EsbuildObjectBuiltinAliases,
    esbuild_define_normal_prop_helpers: &HashSet<BindingKey>,
) -> bool {
    struct Finder<'a> {
        esbuild_aliases: &'a EsbuildObjectBuiltinAliases,
        esbuild_define_normal_prop_helpers: &'a HashSet<BindingKey>,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if self.found {
                return;
            }
            if let Callee::Expr(callee) = &call.callee {
                if is_inline_object_spread_helper(
                    callee.as_ref(),
                    self.esbuild_aliases,
                    self.esbuild_define_normal_prop_helpers,
                ) {
                    self.found = true;
                    return;
                }
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = Finder {
        esbuild_aliases,
        esbuild_define_normal_prop_helpers,
        found: false,
    };
    module.visit_with(&mut finder);
    finder.found
}

fn is_inline_object_spread_helper(
    expr: &Expr,
    esbuild_aliases: &EsbuildObjectBuiltinAliases,
    esbuild_define_normal_prop_helpers: &HashSet<BindingKey>,
) -> bool {
    match strip_parens(expr) {
        Expr::Fn(fn_expr) => {
            function_matches_inline_object_spread(
                &fn_expr.function,
                esbuild_aliases,
                esbuild_define_normal_prop_helpers,
            ) || function_matches_arguments_object_spread(&fn_expr.function)
        }
        Expr::Arrow(arrow) => {
            let Some((target, source)) = arrow_param_pair(&arrow.params) else {
                return false;
            };
            match arrow.body.as_ref() {
                BlockStmtOrExpr::BlockStmt(block) => {
                    block_matches_inline_object_spread(
                        &block.stmts,
                        target,
                        source,
                        esbuild_define_normal_prop_helpers,
                    ) || block_single_return_expr(&block.stmts).is_some_and(|expr| {
                        spread_props_expr_matches(expr, target, source, esbuild_aliases)
                    })
                }
                BlockStmtOrExpr::Expr(expr) => expr_matches_inline_object_spread(
                    expr,
                    target,
                    source,
                    esbuild_aliases,
                    esbuild_define_normal_prop_helpers,
                ),
            }
        }
        _ => false,
    }
}

fn function_matches_inline_object_spread(
    function: &Function,
    esbuild_aliases: &EsbuildObjectBuiltinAliases,
    esbuild_define_normal_prop_helpers: &HashSet<BindingKey>,
) -> bool {
    let Some(body) = &function.body else {
        return false;
    };
    let params: Vec<_> = function.params.iter().map(|param| &param.pat).collect();
    let Some((target, source)) = pat_pair(&params) else {
        return false;
    };
    block_matches_inline_object_spread(
        &body.stmts,
        target,
        source,
        esbuild_define_normal_prop_helpers,
    ) || block_single_return_expr(&body.stmts)
        .is_some_and(|expr| spread_props_expr_matches(expr, target, source, esbuild_aliases))
}

fn block_single_return_expr(stmts: &[Stmt]) -> Option<&Expr> {
    let [Stmt::Return(ReturnStmt { arg: Some(arg), .. })] = stmts else {
        return None;
    };
    Some(arg)
}

fn function_matches_arguments_object_spread(function: &Function) -> bool {
    let Some(body) = &function.body else {
        return false;
    };
    let [param] = function.params.as_slice() else {
        return false;
    };
    let Pat::Ident(target) = &param.pat else {
        return false;
    };
    let [copy_stmt, Stmt::Return(ret)] = body.stmts.as_slice() else {
        return false;
    };
    if !ret
        .arg
        .as_deref()
        .is_some_and(|arg| is_binding_ref(arg, &target.id))
    {
        return false;
    }
    if !matches!(copy_stmt, Stmt::For(_)) {
        return false;
    }

    let mut marker = ArgumentsSpreadMarker::new(&target.id);
    copy_stmt.visit_with(&mut marker);
    marker.is_match()
}

struct ArgumentsSpreadMarker<'a> {
    target: &'a Ident,
    saw_arguments_ref: bool,
    saw_target_write: bool,
    saw_unsafe_target_write: bool,
    saw_object_define_property: bool,
    saw_object_get_own_property_descriptor: bool,
}

impl<'a> ArgumentsSpreadMarker<'a> {
    fn new(target: &'a Ident) -> Self {
        Self {
            target,
            saw_arguments_ref: false,
            saw_target_write: false,
            saw_unsafe_target_write: false,
            saw_object_define_property: false,
            saw_object_get_own_property_descriptor: false,
        }
    }

    fn is_match(&self) -> bool {
        self.saw_arguments_ref
            && self.saw_target_write
            && !self.saw_unsafe_target_write
            && self.saw_object_define_property
            && self.saw_object_get_own_property_descriptor
    }
}

impl Visit for ArgumentsSpreadMarker<'_> {
    fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
        if let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left {
            if matches!(member.obj.as_ref(), Expr::Ident(ident) if ident_matches(ident, self.target))
            {
                if matches!(member.prop, MemberProp::Computed(_)) {
                    self.saw_target_write = true;
                } else {
                    self.saw_unsafe_target_write = true;
                }
            }
        }
        assign.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Member(member) = strip_parens(callee.as_ref()) {
                if let Expr::Ident(obj) = member.obj.as_ref() {
                    if obj.sym.as_ref() == "Object" {
                        match static_member_prop_name(&member.prop) {
                            Some("defineProperty" | "defineProperties") => {
                                self.saw_object_define_property = true;
                                if call
                                    .args
                                    .first()
                                    .is_some_and(|arg| is_binding_ref(&arg.expr, self.target))
                                {
                                    self.saw_target_write = true;
                                }
                            }
                            Some("getOwnPropertyDescriptor" | "getOwnPropertyDescriptors") => {
                                self.saw_object_get_own_property_descriptor = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym.as_ref() == "arguments" {
            self.saw_arguments_ref = true;
        }
    }
}

fn block_matches_inline_object_spread(
    stmts: &[Stmt],
    target: &Ident,
    source: &Ident,
    esbuild_define_normal_prop_helpers: &HashSet<BindingKey>,
) -> bool {
    if !matches!(
        stmts.last(),
        Some(Stmt::Return(ret)) if ret.arg.as_deref().is_some_and(|arg| is_binding_ref(arg, target))
    ) {
        return false;
    }

    let mut marker = InlineSpreadMarker::new(target, source, esbuild_define_normal_prop_helpers);
    stmts.visit_with(&mut marker);
    marker.is_match()
}

fn expr_matches_inline_object_spread(
    expr: &Expr,
    target: &Ident,
    source: &Ident,
    esbuild_aliases: &EsbuildObjectBuiltinAliases,
    esbuild_define_normal_prop_helpers: &HashSet<BindingKey>,
) -> bool {
    let mut marker = InlineSpreadMarker::new(target, source, esbuild_define_normal_prop_helpers);
    expr.visit_with(&mut marker);
    marker.is_match() || spread_props_expr_matches(expr, target, source, esbuild_aliases)
}

fn arrow_param_pair(params: &[Pat]) -> Option<(&Ident, &Ident)> {
    let refs: Vec<_> = params.iter().collect();
    pat_pair(&refs)
}

fn pat_pair<'a>(params: &[&'a Pat]) -> Option<(&'a Ident, &'a Ident)> {
    let [Pat::Ident(target), Pat::Ident(source)] = params else {
        return None;
    };
    Some((&target.id, &source.id))
}

struct InlineSpreadMarker<'a> {
    target: &'a Ident,
    source: &'a Ident,
    esbuild_define_normal_prop_helpers: &'a HashSet<BindingKey>,
    saw_generated_spread_helper: bool,
    saw_target_helper_arg: bool,
    saw_source_ref: bool,
}

impl<'a> InlineSpreadMarker<'a> {
    fn new(
        target: &'a Ident,
        source: &'a Ident,
        esbuild_define_normal_prop_helpers: &'a HashSet<BindingKey>,
    ) -> Self {
        Self {
            target,
            source,
            esbuild_define_normal_prop_helpers,
            saw_generated_spread_helper: false,
            saw_target_helper_arg: false,
            saw_source_ref: false,
        }
    }

    fn is_match(&self) -> bool {
        self.saw_generated_spread_helper && self.saw_target_helper_arg && self.saw_source_ref
    }
}

impl Visit for InlineSpreadMarker<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident_matches(ident, self.source) {
            self.saw_source_ref = true;
        }
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if is_generated_spread_helper_callee(&call.callee, self.esbuild_define_normal_prop_helpers)
        {
            self.saw_generated_spread_helper = true;
            if call
                .args
                .first()
                .is_some_and(|arg| arg.spread.is_none() && is_binding_ref(&arg.expr, self.target))
            {
                self.saw_target_helper_arg = true;
            }
        }

        call.visit_children_with(self);
    }
}

fn is_generated_spread_helper_callee(
    callee: &Callee,
    esbuild_define_normal_prop_helpers: &HashSet<BindingKey>,
) -> bool {
    let Callee::Expr(expr) = callee else {
        return false;
    };
    match strip_parens(expr) {
        Expr::Ident(id) => {
            matches!(id.sym.as_ref(), "__defNormalProp" | "__defProps")
                || esbuild_define_normal_prop_helpers.contains(&binding_key(id))
        }
        Expr::Member(member) => match &member.prop {
            swc_core::ecma::ast::MemberProp::Ident(prop) => {
                matches!(prop.sym.as_ref(), "defineProperty" | "defineProperties")
            }
            swc_core::ecma::ast::MemberProp::PrivateName(_)
            | swc_core::ecma::ast::MemberProp::Computed(_) => false,
        },
        _ => false,
    }
}

fn is_binding_ref(expr: &Expr, binding: &Ident) -> bool {
    matches!(strip_parens(expr), Expr::Ident(id) if ident_matches(id, binding))
}

fn ident_matches(left: &Ident, right: &Ident) -> bool {
    left.sym == right.sym && left.ctxt == right.ctxt
}

fn is_safe_to_inline_props(props: &[PropOrSpread]) -> bool {
    props.iter().all(is_safe_to_inline_prop)
}

fn is_safe_to_inline_prop(prop: &PropOrSpread) -> bool {
    match prop {
        PropOrSpread::Spread(_) => true,
        PropOrSpread::Prop(prop) => match prop.as_ref() {
            Prop::Shorthand(ident) => ident.sym != "__proto__",
            Prop::KeyValue(kv) => !is_bare_proto_name(&kv.key),
            Prop::Assign(assign) => assign.key.sym != "__proto__",
            Prop::Getter(_) | Prop::Setter(_) | Prop::Method(_) => false,
        },
    }
}

fn is_bare_proto_name(name: &PropName) -> bool {
    match name {
        PropName::Ident(ident) => ident.sym == "__proto__",
        PropName::Str(value) => value.value == "__proto__",
        PropName::Num(_) | PropName::BigInt(_) | PropName::Computed(_) => false,
    }
}

fn export_name_to_atom(name: &swc_core::ecma::ast::ModuleExportName) -> Atom {
    match name {
        swc_core::ecma::ast::ModuleExportName::Ident(id) => id.sym.clone(),
        swc_core::ecma::ast::ModuleExportName::Str(s) => str_to_atom(&s.value),
    }
}

fn str_to_atom(value: &swc_core::atoms::Wtf8Atom) -> Atom {
    Atom::from(value.as_str().unwrap_or(""))
}
