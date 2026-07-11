use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::util::take::Take;
use swc_core::common::{Mark, Span, Spanned};
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, AssignPat, AssignPatProp, AssignTarget, AssignTargetPat,
    BinaryOp, BindingIdent, BlockStmtOrExpr, Bool, CallExpr, Callee, ComputedPropName, CondExpr,
    Decl, Expr, ExprStmt, FnDecl, FnExpr, Function, Ident, ImportSpecifier, JSXElementName,
    KeyValuePatProp, Lit, MemberExpr, MemberProp, Module, ModuleDecl, ModuleItem, ObjectPat,
    ObjectPatProp, Pat, PropName, PropOrSpread, RestPat, SimpleAssignTarget, Stmt, VarDecl,
    VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::facts::{ModuleFactsMap, TypeScriptHelperKind};
use crate::utils::paren::strip_parens;

use super::cross_module_helper_refs::{
    collect_cross_module_helper_refs, collect_cross_module_ts_helper_refs,
    cross_module_member_helper_kind,
};
use super::helper_matcher::{
    binding_key, member_prop_name, remaining_refs_outside_declarations,
    remove_fn_decls_from_body_by_binding, remove_import_specifiers_by_binding,
    remove_var_declarators_by_binding, static_member_prop_name, var_declarator_binding_key,
};
use super::transpiler_helper_utils::{
    tslib_member_helper_kind, BindingKey, LocalHelperContext, TranspilerHelperKind,
};

/// Convert inline `_objectWithoutPropertiesLoose` IIFEs to object rest destructuring.
///
/// ```js
/// const rest = ((e, t) => {
///     const n = {};
///     for (const r in e) {
///         t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
///     }
///     return n;
/// })(obj, ["a", "b"]);
/// // →
/// const { a, b, ...rest } = obj;
/// ```
pub struct UnObjectRest<'a> {
    unresolved_mark: Mark,
    module_facts: Option<&'a ModuleFactsMap>,
    current_filename: Option<&'a str>,
}

impl UnObjectRest<'_> {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self {
            unresolved_mark,
            module_facts: None,
            current_filename: None,
        }
    }
}

impl<'a> UnObjectRest<'a> {
    pub fn new_with_facts(unresolved_mark: Mark, module_facts: &'a ModuleFactsMap) -> Self {
        Self {
            unresolved_mark,
            module_facts: Some(module_facts),
            current_filename: None,
        }
    }

    pub(crate) fn run_with_helpers(
        module: &mut swc_core::ecma::ast::Module,
        unresolved_mark: Mark,
        local_helpers: &LocalHelperContext,
        module_facts: Option<&ModuleFactsMap>,
        current_filename: Option<&str>,
    ) {
        run_un_object_rest(
            module,
            unresolved_mark,
            local_helpers,
            module_facts,
            current_filename,
        );
    }
}

impl UnObjectRest<'_> {
    fn has_owp_iife_candidate(module: &swc_core::ecma::ast::Module) -> bool {
        struct Scan {
            found: bool,
        }
        impl Visit for Scan {
            fn visit_call_expr(&mut self, call: &CallExpr) {
                if self.found {
                    return;
                }
                if call.args.len() == 2
                    && call.args.iter().all(|a| a.spread.is_none())
                    && matches!(call.args[1].expr.as_ref(), Expr::Array(_) | Expr::Ident(_))
                {
                    let callee_is_fn = if let Callee::Expr(e) = &call.callee {
                        match strip_parens(e) {
                            Expr::Arrow(a) => a.params.len() == 2,
                            Expr::Fn(f) => f.function.params.len() == 2,
                            _ => false,
                        }
                    } else {
                        false
                    };
                    if callee_is_fn {
                        self.found = true;
                        return;
                    }
                }
                call.visit_children_with(self);
            }
        }
        let mut scan = Scan { found: false };
        module.visit_with(&mut scan);
        scan.found
    }
}

impl VisitMut for UnObjectRest<'_> {
    fn visit_mut_module(&mut self, module: &mut swc_core::ecma::ast::Module) {
        let local_helpers = LocalHelperContext::collect_with_mark(module, self.unresolved_mark);
        run_un_object_rest(
            module,
            self.unresolved_mark,
            &local_helpers,
            self.module_facts,
            self.current_filename,
        );
    }
}

fn run_un_object_rest(
    module: &mut swc_core::ecma::ast::Module,
    unresolved_mark: Mark,
    local_helpers: &LocalHelperContext,
    module_facts: Option<&ModuleFactsMap>,
    current_filename: Option<&str>,
) {
    // Collect named OWP helpers (function declarations detected by transpiler_helper_utils)
    let mut local_named_helpers =
        local_helpers.helpers_of_kind(TranspilerHelperKind::ObjectWithoutProperties);
    let esbuild_rest_aliases = collect_esbuild_object_rest_builtin_aliases(module, unresolved_mark);
    let mangled_esbuild_rest_helpers =
        collect_mangled_esbuild_object_rest_helpers(module, &esbuild_rest_aliases);
    local_named_helpers.extend(mangled_esbuild_rest_helpers.clone());
    let mut named_helpers = local_named_helpers.clone();
    let mut cross_module_helpers = module_facts
        .map(|facts| {
            collect_cross_module_helper_refs(module, facts, current_filename, |kind| {
                kind == TranspilerHelperKind::ObjectWithoutProperties
            })
        })
        .unwrap_or_default();
    named_helpers.extend(
        cross_module_helpers
            .direct
            .iter()
            .map(|(key, kind)| (key.clone(), *kind)),
    );
    if let Some(module_facts) = module_facts {
        let ts_rest_refs = collect_cross_module_ts_helper_refs(
            module,
            module_facts,
            current_filename,
            TypeScriptHelperKind::Rest,
        );
        named_helpers.extend(
            ts_rest_refs
                .direct
                .iter()
                .map(|key| (key.clone(), TranspilerHelperKind::ObjectWithoutProperties)),
        );
        for (namespace, members) in ts_rest_refs.namespaces {
            cross_module_helpers
                .namespaces
                .entry(namespace)
                .or_default()
                .extend(
                    members
                        .into_iter()
                        .map(|name| (name, TranspilerHelperKind::ObjectWithoutProperties)),
                );
        }
    }
    let tslib_namespaces = local_helpers.tslib_namespaces();
    let swc_numeric_helper_namespaces =
        collect_swc_numeric_helper_namespaces(module, unresolved_mark);

    let (property_key_helpers, property_key_typeof_helpers) =
        collect_property_key_coercion_helpers(module, local_helpers, unresolved_mark);
    let computed_context = ComputedObjectRestContext {
        named_helpers: &named_helpers,
        tslib_namespaces,
        swc_numeric_helper_namespaces: &swc_numeric_helper_namespaces,
        cross_module_namespaces: &cross_module_helpers.namespaces,
        property_key_helpers: &property_key_helpers,
        property_key_typeof_helpers: &property_key_typeof_helpers,
        unresolved_mark,
    };
    let collapsed_key_aliases = recover_computed_object_rest(module, &computed_context);
    remove_unused_computed_key_aliases(module, &collapsed_key_aliases);
    let property_key_cleanup_helpers = property_key_helpers
        .union(&property_key_typeof_helpers)
        .cloned()
        .collect();
    remove_unused_property_key_helpers(module, &property_key_cleanup_helpers);

    if named_helpers.is_empty()
        && cross_module_helpers.namespaces.is_empty()
        && tslib_namespaces.is_empty()
        && swc_numeric_helper_namespaces.is_empty()
        && !UnObjectRest::has_owp_iife_candidate(module)
    {
        return;
    }

    // Process inner scopes first (function bodies, etc.) with helpers available
    let exclusion_arrays = collect_exclusion_arrays_from_module_items(&module.body);
    let mut processor = ObjectRestProcessor {
        named_helpers: &named_helpers,
        tslib_namespaces,
        swc_numeric_helper_namespaces: &swc_numeric_helper_namespaces,
        cross_module_namespaces: &cross_module_helpers.namespaces,
        exclusion_arrays: exclusion_arrays.clone(),
        unresolved_mark,
    };
    module.visit_mut_children_with(&mut processor);
    reattach_elided_object_rest_in_module_items(
        &mut module.body,
        &named_helpers,
        tslib_namespaces,
        &cross_module_helpers.namespaces,
        unresolved_mark,
    );

    // Process module-level statements
    let mut new_body = Vec::with_capacity(module.body.len());
    let mut recent_stmts: Vec<Stmt> = Vec::new();
    let mut exclusion_arrays = exclusion_arrays;

    let items = std::mem::take(&mut module.body);
    for (index, item) in items.iter().cloned().enumerate() {
        let ModuleItem::Stmt(ref stmt) = item else {
            recent_stmts.clear();
            new_body.push(item);
            continue;
        };

        let extraction = try_extract_owp_iife(stmt, &exclusion_arrays).or_else(|| {
            try_extract_owp_named_call(
                stmt,
                &named_helpers,
                tslib_namespaces,
                &swc_numeric_helper_namespaces,
                &cross_module_helpers.namespaces,
                &exclusion_arrays,
            )
        });

        if let Some((rest_binding, declaration_kind, source, excluded_keys, before, after)) =
            extraction
        {
            let future_jsx_tag_bindings = jsx_tag_bindings_in_module_items(&items[index + 1..]);
            if has_jsx_tag_default_pair(
                &recent_stmts,
                &source,
                &excluded_keys,
                &future_jsx_tag_bindings,
                unresolved_mark,
            ) {
                collect_exclusion_arrays_from_stmt(stmt, &mut exclusion_arrays);
                recent_stmts.push(stmt.clone());
                new_body.push(item);
                continue;
            }
            let mut inline_accesses = declarators_to_accesses(&before, &source, &excluded_keys);
            let preceding_scan =
                scan_preceding_detailed(&recent_stmts, &source, &excluded_keys, unresolved_mark);
            for _ in 0..preceding_scan.absorbed {
                recent_stmts.pop();
                new_body.pop();
            }
            if let Some(source_init) = preceding_scan.source_init.clone() {
                let source_init_stmt = build_source_init_stmt(source_init);
                recent_stmts.push(source_init_stmt.clone());
                new_body.push(ModuleItem::Stmt(source_init_stmt));
            }
            let mut preceding_accesses = preceding_scan.accesses;
            preceding_accesses.append(&mut inline_accesses);
            let scope_names = collect_scope_names_module(&new_body);
            let original_span = stmt.span();
            let new_stmt = build_rest_destructuring(
                original_span,
                declaration_kind,
                &rest_binding,
                &source,
                &excluded_keys,
                &preceding_accesses,
                &scope_names,
            );
            recent_stmts.push(new_stmt.clone());
            new_body.push(ModuleItem::Stmt(new_stmt));
            if !after.is_empty() {
                let after_stmt = Stmt::Decl(Decl::Var(Box::new(VarDecl {
                    span: original_span,
                    ctxt: Default::default(),
                    kind: declaration_kind,
                    declare: false,
                    decls: after,
                })));
                recent_stmts.push(after_stmt.clone());
                new_body.push(ModuleItem::Stmt(after_stmt));
            }
            continue;
        }

        if let Some((rest_binding, source, excluded_keys)) = try_extract_owp_named_assignment(
            stmt,
            &named_helpers,
            tslib_namespaces,
            &swc_numeric_helper_namespaces,
            &cross_module_helpers.namespaces,
            &exclusion_arrays,
        ) {
            let original_span = stmt.span();
            let preceding_scan =
                scan_preceding_detailed(&recent_stmts, &source, &excluded_keys, unresolved_mark);
            let scope_names = collect_scope_names_module(&new_body);
            if preceding_scan.absorbed > 0 {
                if let Some(new_stmt) = build_rest_assignment(
                    original_span,
                    &rest_binding,
                    &source,
                    &excluded_keys,
                    &preceding_scan.accesses,
                    &scope_names,
                ) {
                    for _ in 0..preceding_scan.absorbed {
                        recent_stmts.pop();
                        new_body.pop();
                    }
                    if let Some(source_init) = preceding_scan.source_init {
                        let source_init_stmt = build_source_init_stmt(source_init);
                        recent_stmts.push(source_init_stmt.clone());
                        new_body.push(ModuleItem::Stmt(source_init_stmt));
                    }
                    recent_stmts.push(new_stmt.clone());
                    new_body.push(ModuleItem::Stmt(new_stmt));
                    continue;
                }
            }
        }

        collect_exclusion_arrays_from_stmt(stmt, &mut exclusion_arrays);
        recent_stmts.push(stmt.clone());
        new_body.push(item);
    }
    module.body = new_body;
    remove_unused_exclusion_array_decls(&mut module.body, &exclusion_arrays);
    remove_unused_numeric_helper_namespace_decls(&mut module.body, &swc_numeric_helper_namespaces);

    // Remove named helper declarations if all call sites were replaced
    if !local_named_helpers.is_empty() {
        let import_bindings = super::helper_matcher::collect_import_binding_keys(module);
        let define_property_helpers: HashMap<BindingKey, TranspilerHelperKind> = local_helpers
            .helpers_of_kind(TranspilerHelperKind::DefineProperty)
            .into_iter()
            .filter(|(key, _)| !import_bindings.contains(key))
            .collect();
        let root_helpers = local_named_helpers
            .into_iter()
            .chain(define_property_helpers)
            .collect::<HashMap<_, _>>();
        local_helpers.remove_helpers_with_dependencies(module, root_helpers);
        // Only sweep the collected Object aliases when a mangled esbuild rest
        // helper was actually matched; a recovered Babel helper alone must not
        // trigger removal of unrelated unused aliases.
        if !mangled_esbuild_rest_helpers.is_empty() {
            remove_unused_esbuild_object_rest_builtin_aliases(module, &esbuild_rest_aliases);
        }
    }
}

fn remove_unused_property_key_helpers(module: &mut Module, helpers: &HashSet<BindingKey>) {
    if helpers.is_empty() {
        return;
    }
    let remaining = remaining_refs_outside_declarations(module, helpers, helpers);
    let removable: HashSet<_> = helpers.difference(&remaining).cloned().collect();
    if removable.is_empty() {
        return;
    }
    remove_fn_decls_from_body_by_binding(&mut module.body, &removable);
    remove_var_declarators_by_binding(&mut module.body, &removable);
    remove_import_specifiers_by_binding(&mut module.body, &removable);
}

// This semantic matcher stays rule-local because the shared helper registry
// intentionally classifies these paths as lifecycle-only HelperDependency.
// UnObjectRest is the only rule that rewrites calls based on ToPropertyKey
// identity, and it needs the producer-specific wrapper shapes below as well as
// import provenance.
const PROPERTY_KEY_HELPER_PATHS: &[&str] = &[
    "@babel/runtime/helpers/toPropertyKey",
    "@babel/runtime/helpers/esm/toPropertyKey",
    "@swc/helpers/_/_to_property_key",
];

fn collect_property_key_coercion_helpers(
    module: &Module,
    local_helpers: &LocalHelperContext,
    unresolved_mark: Mark,
) -> (HashSet<BindingKey>, HashSet<BindingKey>) {
    let mut typeof_helpers: HashSet<_> = local_helpers
        .helpers_of_kind(TranspilerHelperKind::Typeof)
        .into_keys()
        .collect();
    collect_property_key_typeof_helpers(module, unresolved_mark, &mut typeof_helpers);
    let mut helpers = HashSet::new();

    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import))
                if PROPERTY_KEY_HELPER_PATHS.contains(&import.src.value.as_str().unwrap_or("")) =>
            {
                for specifier in &import.specifiers {
                    let local = match specifier {
                        ImportSpecifier::Named(specifier) => &specifier.local,
                        ImportSpecifier::Default(specifier) => &specifier.local,
                        ImportSpecifier::Namespace(specifier) => &specifier.local,
                    };
                    helpers.insert(binding_key(local));
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(decl)))
                if property_key_coercion_function_matches(
                    &decl.function,
                    &typeof_helpers,
                    unresolved_mark,
                ) =>
            {
                helpers.insert(binding_key(&decl.ident));
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                collect_property_key_var_helpers(
                    var,
                    &typeof_helpers,
                    unresolved_mark,
                    &mut helpers,
                );
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => match &export.decl {
                Decl::Fn(decl)
                    if property_key_coercion_function_matches(
                        &decl.function,
                        &typeof_helpers,
                        unresolved_mark,
                    ) =>
                {
                    helpers.insert(binding_key(&decl.ident));
                }
                Decl::Var(var) => collect_property_key_var_helpers(
                    var,
                    &typeof_helpers,
                    unresolved_mark,
                    &mut helpers,
                ),
                _ => {}
            },
            _ => {}
        }
    }

    (helpers, typeof_helpers)
}

fn collect_property_key_typeof_helpers(
    module: &Module,
    unresolved_mark: Mark,
    helpers: &mut HashSet<BindingKey>,
) {
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import))
                if matches!(
                    import.src.value.as_str(),
                    Some("@babel/runtime/helpers/typeof")
                        | Some("@babel/runtime/helpers/esm/typeof")
                        | Some("@swc/helpers/_/_type_of")
                ) =>
            {
                for specifier in &import.specifiers {
                    let local = match specifier {
                        ImportSpecifier::Named(specifier) => &specifier.local,
                        ImportSpecifier::Default(specifier) => &specifier.local,
                        ImportSpecifier::Namespace(specifier) => &specifier.local,
                    };
                    helpers.insert(binding_key(local));
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(decl)))
                if swc_typeof_function_matches(&decl.function, unresolved_mark) =>
            {
                helpers.insert(binding_key(&decl.ident));
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    let Some(key) = var_declarator_binding_key(decl) else {
                        continue;
                    };
                    if decl
                        .init
                        .as_deref()
                        .is_some_and(|init| swc_typeof_callable_matches(init, unresolved_mark))
                    {
                        helpers.insert(key);
                    }
                }
            }
            _ => {}
        }
    }
}

fn swc_typeof_callable_matches(expr: &Expr, unresolved_mark: Mark) -> bool {
    match strip_parens(expr) {
        Expr::Fn(function) => swc_typeof_function_matches(&function.function, unresolved_mark),
        Expr::Arrow(arrow) if arrow.params.len() == 1 => {
            let Pat::Ident(param) = &arrow.params[0] else {
                return false;
            };
            let BlockStmtOrExpr::BlockStmt(block) = arrow.body.as_ref() else {
                return false;
            };
            swc_typeof_block_matches(&block.stmts, &param.id, unresolved_mark)
        }
        _ => false,
    }
}

fn swc_typeof_function_matches(function: &Function, unresolved_mark: Mark) -> bool {
    if function.params.len() != 1 {
        return false;
    }
    let Pat::Ident(param) = &function.params[0].pat else {
        return false;
    };
    function
        .body
        .as_ref()
        .is_some_and(|body| swc_typeof_block_matches(&body.stmts, &param.id, unresolved_mark))
}

fn swc_typeof_block_matches(stmts: &[Stmt], param: &Ident, unresolved_mark: Mark) -> bool {
    let Some(Stmt::Return(return_stmt)) = stmts.last() else {
        return false;
    };
    let Some(Expr::Cond(cond)) = return_stmt.arg.as_deref().map(strip_parens) else {
        return false;
    };
    if !matches!(cond.cons.as_ref(), Expr::Lit(Lit::Str(value)) if value.value.as_str() == Some("symbol"))
    {
        return false;
    }
    if !matches!(strip_parens(&cond.alt), Expr::Unary(unary) if unary.op == swc_core::ecma::ast::UnaryOp::TypeOf && matches!(strip_parens(&unary.arg), Expr::Ident(id) if binding_key(id) == binding_key(param)))
    {
        return false;
    }

    let mut signals = SwcTypeofTestSignals {
        param: binding_key(param),
        unresolved_mark,
        saw_symbol_typeof: false,
        saw_constructor_symbol: false,
    };
    cond.test.visit_with(&mut signals);
    signals.saw_symbol_typeof && signals.saw_constructor_symbol
}

struct SwcTypeofTestSignals {
    param: BindingKey,
    unresolved_mark: Mark,
    saw_symbol_typeof: bool,
    saw_constructor_symbol: bool,
}

impl Visit for SwcTypeofTestSignals {
    fn visit_bin_expr(&mut self, binary: &swc_core::ecma::ast::BinExpr) {
        if matches!(binary.op, BinaryOp::NotEq | BinaryOp::NotEqEq) {
            let left_symbol_typeof = is_global_symbol_typeof(&binary.left, self.unresolved_mark);
            let right_symbol_typeof = is_global_symbol_typeof(&binary.right, self.unresolved_mark);
            let left_undefined = matches!(binary.left.as_ref(), Expr::Lit(Lit::Str(value)) if value.value.as_str() == Some("undefined"));
            let right_undefined = matches!(binary.right.as_ref(), Expr::Lit(Lit::Str(value)) if value.value.as_str() == Some("undefined"));
            self.saw_symbol_typeof |=
                (left_symbol_typeof && right_undefined) || (right_symbol_typeof && left_undefined);
        }
        if matches!(binary.op, BinaryOp::EqEq | BinaryOp::EqEqEq) {
            self.saw_constructor_symbol |= constructor_symbol_comparison_matches(
                &binary.left,
                &binary.right,
                &self.param,
                self.unresolved_mark,
            ) || constructor_symbol_comparison_matches(
                &binary.right,
                &binary.left,
                &self.param,
                self.unresolved_mark,
            );
        }
        binary.visit_children_with(self);
    }
}

fn is_global_symbol_typeof(expr: &Expr, unresolved_mark: Mark) -> bool {
    matches!(
        strip_parens(expr),
        Expr::Unary(unary)
            if unary.op == swc_core::ecma::ast::UnaryOp::TypeOf
                && matches!(strip_parens(&unary.arg), Expr::Ident(id) if id.sym.as_ref() == "Symbol" && id.ctxt.outer() == unresolved_mark)
    )
}

fn constructor_symbol_comparison_matches(
    member_expr: &Expr,
    symbol_expr: &Expr,
    param: &BindingKey,
    unresolved_mark: Mark,
) -> bool {
    let Expr::Member(member) = strip_parens(member_expr) else {
        return false;
    };
    member_prop_name(&member.prop, "constructor")
        && matches!(strip_parens(member.obj.as_ref()), Expr::Ident(id) if binding_key(id) == *param)
        && matches!(strip_parens(symbol_expr), Expr::Ident(id) if id.sym.as_ref() == "Symbol" && id.ctxt.outer() == unresolved_mark)
}

fn collect_property_key_var_helpers(
    var: &VarDecl,
    typeof_helpers: &HashSet<BindingKey>,
    unresolved_mark: Mark,
    helpers: &mut HashSet<BindingKey>,
) {
    for decl in &var.decls {
        let Some(key) = var_declarator_binding_key(decl) else {
            continue;
        };
        let Some(init) = decl.init.as_deref() else {
            continue;
        };
        if property_key_coercion_callable_matches(init, typeof_helpers, unresolved_mark)
            || property_key_runtime_require_matches(init, unresolved_mark)
        {
            helpers.insert(key);
        }
    }
}

fn property_key_runtime_require_matches(expr: &Expr, unresolved_mark: Mark) -> bool {
    let expr = strip_parens(expr);
    let call = match expr {
        Expr::Call(call) => call,
        Expr::Member(member) if member_prop_name(&member.prop, "default") => {
            let Expr::Call(call) = strip_parens(member.obj.as_ref()) else {
                return false;
            };
            call
        }
        _ => return false,
    };
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return false;
    }
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !matches!(strip_parens(callee), Expr::Ident(id) if id.sym.as_ref() == "require" && id.ctxt.outer() == unresolved_mark)
    {
        return false;
    }
    matches!(
        call.args[0].expr.as_ref(),
        Expr::Lit(Lit::Str(path))
            if path.value.as_str().is_some_and(|path| PROPERTY_KEY_HELPER_PATHS.contains(&path))
    )
}

fn property_key_coercion_callable_matches(
    expr: &Expr,
    typeof_helpers: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) -> bool {
    match strip_parens(expr) {
        Expr::Arrow(arrow) => {
            if arrow.params.len() != 1 {
                return false;
            }
            let Pat::Ident(param) = &arrow.params[0] else {
                return false;
            };
            match arrow.body.as_ref() {
                BlockStmtOrExpr::Expr(expr) => {
                    property_key_cond_candidate(expr, typeof_helpers, unresolved_mark)
                        .is_some_and(|candidate| candidate == binding_key(&param.id))
                }
                BlockStmtOrExpr::BlockStmt(block) => property_key_block_matches(
                    &block.stmts,
                    &param.id,
                    typeof_helpers,
                    unresolved_mark,
                ),
            }
        }
        Expr::Fn(function) => property_key_coercion_function_matches(
            &function.function,
            typeof_helpers,
            unresolved_mark,
        ),
        _ => false,
    }
}

fn property_key_coercion_function_matches(
    function: &Function,
    typeof_helpers: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) -> bool {
    if function.params.len() != 1 {
        return false;
    }
    let Pat::Ident(param) = &function.params[0].pat else {
        return false;
    };
    let Some(body) = &function.body else {
        return false;
    };
    property_key_block_matches(&body.stmts, &param.id, typeof_helpers, unresolved_mark)
}

fn property_key_block_matches(
    stmts: &[Stmt],
    param: &Ident,
    typeof_helpers: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) -> bool {
    let Some(Stmt::Return(return_stmt)) = stmts.last() else {
        return false;
    };
    let Some(returned) = return_stmt.arg.as_deref() else {
        return false;
    };
    let Some(candidate) = property_key_cond_candidate(returned, typeof_helpers, unresolved_mark)
    else {
        return false;
    };
    if candidate == binding_key(param) {
        return true;
    }

    stmts[..stmts.len() - 1].iter().any(|stmt| {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            return false;
        };
        var.decls.iter().any(|decl| {
            let Some(binding) = var_declarator_binding_key(decl) else {
                return false;
            };
            binding == candidate
                && decl
                    .init
                    .as_deref()
                    .is_some_and(|init| property_key_candidate_init_matches(init, param))
        })
    })
}

fn property_key_candidate_init_matches(expr: &Expr, param: &Ident) -> bool {
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    call.args.len() == 2
        && call.args.iter().all(|arg| arg.spread.is_none())
        && matches!(strip_parens(call.args[0].expr.as_ref()), Expr::Ident(id) if binding_key(id) == binding_key(param))
        && matches!(call.args[1].expr.as_ref(), Expr::Lit(Lit::Str(value)) if value.value.as_str() == Some("string"))
}

fn property_key_cond_candidate(
    expr: &Expr,
    typeof_helpers: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) -> Option<BindingKey> {
    let Expr::Cond(cond) = strip_parens(expr) else {
        return None;
    };
    let candidate = property_key_symbol_test_candidate(&cond.test, typeof_helpers)?;
    if !matches!(strip_parens(&cond.cons), Expr::Ident(id) if binding_key(id) == candidate) {
        return None;
    }
    property_key_string_fallback_matches(&cond.alt, &candidate, unresolved_mark)
        .then_some(candidate)
}

fn property_key_symbol_test_candidate(
    expr: &Expr,
    typeof_helpers: &HashSet<BindingKey>,
) -> Option<BindingKey> {
    let Expr::Bin(binary) = strip_parens(expr) else {
        return None;
    };
    if !matches!(binary.op, BinaryOp::EqEq | BinaryOp::EqEqEq) {
        return None;
    }
    if matches!(binary.right.as_ref(), Expr::Lit(Lit::Str(value)) if value.value.as_str() == Some("symbol"))
    {
        return property_key_type_test_candidate(&binary.left, typeof_helpers);
    }
    if matches!(binary.left.as_ref(), Expr::Lit(Lit::Str(value)) if value.value.as_str() == Some("symbol"))
    {
        return property_key_type_test_candidate(&binary.right, typeof_helpers);
    }
    None
}

fn property_key_type_test_candidate(
    expr: &Expr,
    typeof_helpers: &HashSet<BindingKey>,
) -> Option<BindingKey> {
    match strip_parens(expr) {
        Expr::Unary(unary) if unary.op == swc_core::ecma::ast::UnaryOp::TypeOf => {
            let Expr::Ident(candidate) = strip_parens(&unary.arg) else {
                return None;
            };
            Some(binding_key(candidate))
        }
        Expr::Call(call) if call.args.len() == 1 && call.args[0].spread.is_none() => {
            let Callee::Expr(callee) = &call.callee else {
                return None;
            };
            let Expr::Ident(helper) = strip_parens(callee) else {
                return None;
            };
            if !typeof_helpers.contains(&binding_key(helper)) {
                return None;
            }
            let Expr::Ident(candidate) = strip_parens(call.args[0].expr.as_ref()) else {
                return None;
            };
            Some(binding_key(candidate))
        }
        _ => None,
    }
}

fn property_key_string_fallback_matches(
    expr: &Expr,
    candidate: &BindingKey,
    unresolved_mark: Mark,
) -> bool {
    match strip_parens(expr) {
        Expr::Bin(binary) if binary.op == BinaryOp::Add => {
            let candidate_on_left = matches!(strip_parens(&binary.left), Expr::Ident(id) if binding_key(id) == *candidate)
                && matches!(binary.right.as_ref(), Expr::Lit(Lit::Str(value)) if value.value.as_str() == Some(""));
            let candidate_on_right = matches!(strip_parens(&binary.right), Expr::Ident(id) if binding_key(id) == *candidate)
                && matches!(binary.left.as_ref(), Expr::Lit(Lit::Str(value)) if value.value.as_str() == Some(""));
            candidate_on_left || candidate_on_right
        }
        Expr::Call(call) if call.args.len() == 1 && call.args[0].spread.is_none() => {
            let Callee::Expr(callee) = &call.callee else {
                return false;
            };
            matches!(strip_parens(callee), Expr::Ident(id) if id.sym.as_ref() == "String" && id.ctxt.outer() == unresolved_mark)
                && matches!(strip_parens(call.args[0].expr.as_ref()), Expr::Ident(id) if binding_key(id) == *candidate)
        }
        _ => false,
    }
}

struct ComputedObjectRestContext<'a> {
    named_helpers: &'a HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &'a HashSet<BindingKey>,
    swc_numeric_helper_namespaces: &'a HashSet<BindingKey>,
    cross_module_namespaces: &'a HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
    property_key_helpers: &'a HashSet<BindingKey>,
    property_key_typeof_helpers: &'a HashSet<BindingKey>,
    unresolved_mark: Mark,
}

fn recover_computed_object_rest(
    module: &mut Module,
    context: &ComputedObjectRestContext<'_>,
) -> HashSet<BindingKey> {
    let mut collapsed_key_aliases = HashSet::new();
    let mut processor = ComputedObjectRestProcessor {
        context,
        collapsed_key_aliases: &mut collapsed_key_aliases,
    };
    module.visit_mut_children_with(&mut processor);
    recover_computed_object_rest_in_module_items(
        &mut module.body,
        context,
        &mut collapsed_key_aliases,
    );
    collapsed_key_aliases
}

struct ComputedObjectRestProcessor<'a, 'b, 'c> {
    context: &'a ComputedObjectRestContext<'b>,
    collapsed_key_aliases: &'c mut HashSet<BindingKey>,
}

impl VisitMut for ComputedObjectRestProcessor<'_, '_, '_> {
    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        recover_computed_object_rest_in_stmts(stmts, self.context, self.collapsed_key_aliases);
    }
}

fn recover_computed_object_rest_in_module_items(
    items: &mut Vec<ModuleItem>,
    context: &ComputedObjectRestContext<'_>,
    collapsed_key_aliases: &mut HashSet<BindingKey>,
) {
    let mut statements = Vec::new();
    let mut rebuilt = Vec::with_capacity(items.len());
    for item in std::mem::take(items) {
        match item {
            ModuleItem::Stmt(stmt) => statements.push(stmt),
            declaration => {
                recover_computed_object_rest_in_stmts(
                    &mut statements,
                    context,
                    collapsed_key_aliases,
                );
                rebuilt.extend(statements.drain(..).map(ModuleItem::Stmt));
                rebuilt.push(declaration);
            }
        }
    }
    recover_computed_object_rest_in_stmts(&mut statements, context, collapsed_key_aliases);
    rebuilt.extend(statements.into_iter().map(ModuleItem::Stmt));
    *items = rebuilt;
}

fn recover_computed_object_rest_in_stmts(
    stmts: &mut Vec<Stmt>,
    context: &ComputedObjectRestContext<'_>,
    collapsed_key_aliases: &mut HashSet<BindingKey>,
) {
    let mut rebuilt = Vec::with_capacity(stmts.len());
    for stmt in std::mem::take(stmts) {
        let Some(extraction) = extract_computed_object_rest(&stmt, context) else {
            rebuilt.push(stmt);
            continue;
        };

        let preceding_access = if extraction.picked_index.is_none() {
            rebuilt
                .last()
                .and_then(|stmt| computed_access_from_single_stmt(stmt, &extraction))
        } else {
            None
        };
        let used_preceding_access = extraction.picked.is_none() && preceding_access.is_some();
        let picked = extraction.picked.clone().or(preceding_access);
        let Some(picked) = picked else {
            rebuilt.push(stmt);
            continue;
        };
        if used_preceding_access {
            rebuilt.pop();
        }
        let preceding_alias = if used_preceding_access {
            rebuilt
                .last()
                .and_then(|stmt| resolve_computed_key_alias_in_stmt(stmt, &extraction.key))
        } else {
            None
        };
        let pattern_key = if binding_key(&extraction.pattern_key) == binding_key(&extraction.key) {
            preceding_alias.unwrap_or_else(|| extraction.key.clone())
        } else {
            extraction.pattern_key.clone()
        };
        if binding_key(&pattern_key) != binding_key(&extraction.key) {
            collapsed_key_aliases.insert(binding_key(&extraction.key));
        }

        if !extraction.before.is_empty() {
            rebuilt.push(var_decl_stmt(
                extraction.span,
                extraction.kind,
                extraction.before,
            ));
        }
        rebuilt.push(build_computed_rest_destructuring(
            extraction.span,
            extraction.kind,
            &extraction.rest,
            &extraction.source,
            &pattern_key,
            &picked,
        ));
        if !extraction.after.is_empty() {
            rebuilt.push(var_decl_stmt(
                extraction.span,
                extraction.kind,
                extraction.after,
            ));
        }
    }
    *stmts = rebuilt;
}

fn remove_unused_computed_key_aliases(module: &mut Module, aliases: &HashSet<BindingKey>) {
    if aliases.is_empty() {
        return;
    }
    let remaining = remaining_refs_outside_declarations(module, aliases, aliases);
    let removable: HashSet<_> = aliases.difference(&remaining).cloned().collect();
    if removable.is_empty() {
        return;
    }

    let mut remover = UnusedComputedKeyAliasRemover {
        removable: &removable,
    };
    module.visit_mut_children_with(&mut remover);
    remove_var_declarators_by_binding(&mut module.body, &removable);
}

struct UnusedComputedKeyAliasRemover<'a> {
    removable: &'a HashSet<BindingKey>,
}

impl VisitMut for UnusedComputedKeyAliasRemover<'_> {
    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        for stmt in stmts.iter_mut() {
            let Stmt::Decl(Decl::Var(var)) = stmt else {
                continue;
            };
            var.decls.retain(|decl| {
                var_declarator_binding_key(decl).is_none_or(|key| !self.removable.contains(&key))
            });
        }
        stmts.retain(|stmt| !matches!(stmt, Stmt::Decl(Decl::Var(var)) if var.decls.is_empty()));
    }
}

fn resolve_computed_key_alias_in_stmt(stmt: &Stmt, key: &Ident) -> Option<Ident> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    resolve_computed_key_alias_from_decl(var.decls.last()?, key)
}

struct ComputedObjectRestExtraction {
    span: Span,
    kind: VarDeclKind,
    rest: BindingIdent,
    source: Box<Expr>,
    key: Ident,
    pattern_key: Ident,
    picked: Option<BindingIdent>,
    picked_index: Option<usize>,
    before: Vec<VarDeclarator>,
    after: Vec<VarDeclarator>,
}

fn extract_computed_object_rest(
    stmt: &Stmt,
    context: &ComputedObjectRestContext<'_>,
) -> Option<ComputedObjectRestExtraction> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    let (rest_index, rest, source, key) =
        var.decls.iter().enumerate().find_map(|(index, decl)| {
            let Pat::Ident(rest) = &decl.name else {
                return None;
            };
            let init = decl.init.as_deref()?;
            let (source, key) = extract_computed_named_owp_args(init, context)?;
            Some((index, rest.clone(), source, key))
        })?;

    // The recovered pattern performs the property read at the rest
    // declarator's position, so only accept a pick immediately before it —
    // any declarator between them would be reordered across that read.
    let picked_match = rest_index.checked_sub(1).and_then(|index| {
        computed_access_from_declarator(&var.decls[index], &source, &key)
            .map(|picked| (index, picked))
    });
    let (picked_index, picked) = picked_match
        .map(|(index, picked)| (Some(index), Some(picked)))
        .unwrap_or((None, None));
    let pattern_key = picked_index
        .and_then(|index| index.checked_sub(1))
        .and_then(|alias_index| resolve_computed_key_alias_from_decl(&var.decls[alias_index], &key))
        .unwrap_or_else(|| key.clone());
    let before = var.decls[..rest_index]
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != picked_index)
        .map(|(_, decl)| decl.clone())
        .collect();
    let after = var.decls[rest_index + 1..].to_vec();

    Some(ComputedObjectRestExtraction {
        span: var.span,
        kind: var.kind,
        rest,
        source,
        key,
        pattern_key,
        picked,
        picked_index,
        before,
        after,
    })
}

fn resolve_computed_key_alias_from_decl(decl: &VarDeclarator, key: &Ident) -> Option<Ident> {
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    if binding_key(&binding.id) != binding_key(key) {
        return None;
    }
    let Expr::Ident(original) = strip_parens(decl.init.as_deref()?) else {
        return None;
    };
    Some(original.clone())
}

fn extract_computed_named_owp_args(
    expr: &Expr,
    context: &ComputedObjectRestContext<'_>,
) -> Option<(Box<Expr>, Ident)> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !is_named_owp_callee(
        callee,
        context.named_helpers,
        context.tslib_namespaces,
        context.swc_numeric_helper_namespaces,
        context.cross_module_namespaces,
    ) || call.args.len() != 2
        || call.args.iter().any(|arg| arg.spread.is_some())
    {
        return None;
    }
    let key = extract_computed_exclusion_key(call.args[1].expr.as_ref(), context)?;
    Some((call.args[0].expr.clone(), key))
}

fn extract_computed_exclusion_key(
    expr: &Expr,
    context: &ComputedObjectRestContext<'_>,
) -> Option<Ident> {
    if let Expr::Array(array) = strip_parens(expr) {
        if array.elems.len() != 1 {
            return None;
        }
        let element = array.elems[0].as_ref()?;
        if element.spread.is_some() {
            return None;
        }
        return extract_property_key_coercion(&element.expr, context);
    }
    extract_property_key_coercion(expr, context)
}

fn extract_property_key_coercion(
    expr: &Expr,
    context: &ComputedObjectRestContext<'_>,
) -> Option<Ident> {
    if let Expr::Cond(_) = strip_parens(expr) {
        let candidate = property_key_cond_candidate(
            expr,
            context.property_key_typeof_helpers,
            context.unresolved_mark,
        )?;
        return Some(Ident::new(candidate.0, DUMMY_SP, candidate.1));
    }

    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };

    if call.args.len() == 1 && call.args[0].spread.is_none() {
        if let Expr::Ident(helper) = strip_parens(callee) {
            if !context.property_key_helpers.contains(&binding_key(helper)) {
                return None;
            }
            let Expr::Ident(key) = strip_parens(call.args[0].expr.as_ref()) else {
                return None;
            };
            return Some(key.clone());
        }
    }

    let Expr::Member(member) = strip_parens(callee) else {
        return None;
    };
    if !member_prop_name(&member.prop, "map")
        || call.args.len() != 1
        || call.args[0].spread.is_some()
    {
        return None;
    }
    let Expr::Ident(helper) = strip_parens(call.args[0].expr.as_ref()) else {
        return None;
    };
    if !context.property_key_helpers.contains(&binding_key(helper)) {
        return None;
    }
    let Expr::Array(keys) = strip_parens(member.obj.as_ref()) else {
        return None;
    };
    if keys.elems.len() != 1 {
        return None;
    }
    let key = keys.elems[0].as_ref()?;
    if key.spread.is_some() {
        return None;
    }
    let Expr::Ident(key) = strip_parens(&key.expr) else {
        return None;
    };
    Some(key.clone())
}

fn computed_access_from_single_stmt(
    stmt: &Stmt,
    extraction: &ComputedObjectRestExtraction,
) -> Option<BindingIdent> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    computed_access_from_declarator(&var.decls[0], &extraction.source, &extraction.key)
}

fn computed_access_from_declarator(
    decl: &VarDeclarator,
    source: &Expr,
    key: &Ident,
) -> Option<BindingIdent> {
    if let Pat::Ident(binding) = &decl.name {
        let Expr::Member(member) = strip_parens(decl.init.as_deref()?) else {
            return None;
        };
        if !same_ident_expr(member.obj.as_ref(), source) {
            return None;
        }
        let MemberProp::Computed(computed) = &member.prop else {
            return None;
        };
        if same_ident_expr(computed.expr.as_ref(), &Expr::Ident(key.clone())) {
            return Some(binding.clone());
        }
        return None;
    }

    let Pat::Object(pattern) = &decl.name else {
        return None;
    };
    if pattern.props.len() != 1 || !same_ident_expr(decl.init.as_deref()?, source) {
        return None;
    }
    let ObjectPatProp::KeyValue(property) = &pattern.props[0] else {
        return None;
    };
    let PropName::Computed(computed) = &property.key else {
        return None;
    };
    if !same_ident_expr(computed.expr.as_ref(), &Expr::Ident(key.clone())) {
        return None;
    }
    let Pat::Ident(binding) = property.value.as_ref() else {
        return None;
    };
    Some(binding.clone())
}

fn same_ident_expr(left: &Expr, right: &Expr) -> bool {
    matches!((strip_parens(left), strip_parens(right)), (Expr::Ident(left), Expr::Ident(right)) if binding_key(left) == binding_key(right))
}

fn var_decl_stmt(span: Span, kind: VarDeclKind, decls: Vec<VarDeclarator>) -> Stmt {
    Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span,
        ctxt: Default::default(),
        kind,
        declare: false,
        decls,
    })))
}

fn build_computed_rest_destructuring(
    span: Span,
    kind: VarDeclKind,
    rest: &BindingIdent,
    source: &Expr,
    key: &Ident,
    picked: &BindingIdent,
) -> Stmt {
    var_decl_stmt(
        span,
        kind,
        vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Object(ObjectPat {
                span: DUMMY_SP,
                props: vec![
                    ObjectPatProp::KeyValue(KeyValuePatProp {
                        key: PropName::Computed(ComputedPropName {
                            span: DUMMY_SP,
                            expr: Box::new(Expr::Ident(key.clone())),
                        }),
                        value: Box::new(Pat::Ident(picked.clone())),
                    }),
                    ObjectPatProp::Rest(RestPat {
                        span: DUMMY_SP,
                        dot3_token: DUMMY_SP,
                        arg: Box::new(Pat::Ident(rest.clone())),
                        type_ann: None,
                    }),
                ],
                optional: false,
                type_ann: None,
            }),
            init: Some(Box::new(source.clone())),
            definite: false,
        }],
    )
}

#[derive(Default)]
struct EsbuildObjectRestBuiltinAliases {
    get_own_property_symbols: HashSet<BindingKey>,
    has_own_property: HashSet<BindingKey>,
    property_is_enumerable: HashSet<BindingKey>,
}

impl EsbuildObjectRestBuiltinAliases {
    fn has_required_signals(&self) -> bool {
        !self.has_own_property.is_empty()
    }

    fn dependency_keys(&self) -> impl Iterator<Item = BindingKey> + '_ {
        self.get_own_property_symbols
            .iter()
            .chain(&self.has_own_property)
            .chain(&self.property_is_enumerable)
            .cloned()
    }
}

fn collect_esbuild_object_rest_builtin_aliases(
    module: &Module,
    unresolved_mark: Mark,
) -> EsbuildObjectRestBuiltinAliases {
    let mut aliases = EsbuildObjectRestBuiltinAliases::default();

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
            match object_rest_builtin_alias_kind(init, unresolved_mark) {
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

fn object_rest_builtin_alias_kind(expr: &Expr, unresolved_mark: Mark) -> Option<&'static str> {
    let Expr::Member(member) = strip_parens(expr) else {
        return None;
    };
    let is_global_object = matches!(member.obj.as_ref(), Expr::Ident(obj) if obj.sym.as_ref() == "Object" && obj.ctxt.outer() == unresolved_mark);
    if is_global_object && static_member_prop_name(&member.prop) == Some("getOwnPropertySymbols") {
        return Some("getOwnPropertySymbols");
    }

    let Expr::Member(proto_member) = member.obj.as_ref() else {
        return None;
    };
    let Expr::Ident(obj) = proto_member.obj.as_ref() else {
        return None;
    };
    if obj.sym.as_ref() != "Object"
        || obj.ctxt.outer() != unresolved_mark
        || !member_prop_name(&proto_member.prop, "prototype")
    {
        return None;
    }
    match static_member_prop_name(&member.prop) {
        Some("hasOwnProperty") => Some("hasOwnProperty"),
        Some("propertyIsEnumerable") => Some("propertyIsEnumerable"),
        _ => None,
    }
}

fn collect_mangled_esbuild_object_rest_helpers(
    module: &Module,
    aliases: &EsbuildObjectRestBuiltinAliases,
) -> HashMap<BindingKey, TranspilerHelperKind> {
    if !aliases.has_required_signals() {
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
            if esbuild_object_rest_helper_matches(init, aliases) {
                helpers.insert(key, TranspilerHelperKind::ObjectWithoutProperties);
            }
        }
    }
    helpers
}

fn esbuild_object_rest_helper_matches(
    expr: &Expr,
    aliases: &EsbuildObjectRestBuiltinAliases,
) -> bool {
    let Some((source, excluded, body)) = object_rest_helper_two_param_block(expr) else {
        return false;
    };
    let Some(target) = find_empty_object_accumulator_ident(&body.stmts) else {
        return false;
    };
    if !block_returns_binding(body, &target) {
        return false;
    }

    let mut marker = EsbuildObjectRestMarker {
        source,
        excluded,
        target: &target,
        aliases,
        saw_for_in_source: false,
        saw_has_own_call: false,
        saw_exclusion_check: false,
        saw_copy: false,
    };
    body.visit_with(&mut marker);
    marker.saw_for_in_source
        && marker.saw_has_own_call
        && marker.saw_exclusion_check
        && marker.saw_copy
}

fn object_rest_helper_two_param_block(
    expr: &Expr,
) -> Option<(&Ident, &Ident, &swc_core::ecma::ast::BlockStmt)> {
    match strip_parens(expr) {
        Expr::Arrow(arrow) => {
            if arrow.params.len() != 2 {
                return None;
            }
            let source = pat_ident(&arrow.params[0])?;
            let excluded = pat_ident(&arrow.params[1])?;
            let BlockStmtOrExpr::BlockStmt(block) = arrow.body.as_ref() else {
                return None;
            };
            Some((source, excluded, block))
        }
        Expr::Fn(fn_expr) => {
            if fn_expr.function.params.len() != 2 {
                return None;
            }
            let source = pat_ident(&fn_expr.function.params[0].pat)?;
            let excluded = pat_ident(&fn_expr.function.params[1].pat)?;
            Some((source, excluded, fn_expr.function.body.as_ref()?))
        }
        _ => None,
    }
}

fn pat_ident(pat: &Pat) -> Option<&Ident> {
    let Pat::Ident(binding) = pat else {
        return None;
    };
    Some(&binding.id)
}

fn find_empty_object_accumulator_ident(stmts: &[Stmt]) -> Option<Ident> {
    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        for decl in &var.decls {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            if matches!(
                decl.init.as_deref(),
                Some(Expr::Object(obj)) if obj.props.is_empty()
            ) {
                return Some(binding.id.clone());
            }
        }
    }
    None
}

fn block_returns_binding(block: &swc_core::ecma::ast::BlockStmt, binding: &Ident) -> bool {
    matches!(
        block.stmts.last(),
        Some(Stmt::Return(ret))
            if ret.arg.as_deref().is_some_and(|arg| is_binding_ref(arg, binding))
    )
}

struct EsbuildObjectRestMarker<'a> {
    source: &'a Ident,
    excluded: &'a Ident,
    target: &'a Ident,
    aliases: &'a EsbuildObjectRestBuiltinAliases,
    saw_for_in_source: bool,
    saw_has_own_call: bool,
    saw_exclusion_check: bool,
    saw_copy: bool,
}

impl Visit for EsbuildObjectRestMarker<'_> {
    fn visit_for_in_stmt(&mut self, for_in: &swc_core::ecma::ast::ForInStmt) {
        if is_binding_ref(&for_in.right, self.source) {
            self.saw_for_in_source = true;
        }
        for_in.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if call.args.len() >= 2
            && call.args[0].spread.is_none()
            && call.args[1].spread.is_none()
            && is_binding_ref(&call.args[0].expr, self.source)
            && callee_is_alias_call_method(&call.callee, &self.aliases.has_own_property)
        {
            self.saw_has_own_call = true;
        }
        if call.args.first().is_some_and(|arg| arg.spread.is_none())
            && callee_is_index_of_on_binding(&call.callee, self.excluded)
        {
            self.saw_exclusion_check = true;
        }
        call.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if assign.op == AssignOp::Assign
            && assign_target_obj_is_binding(&assign.left, self.target)
            && matches!(
                strip_parens(&assign.right),
                Expr::Member(member) if member_obj_is_binding(member, self.source)
            )
        {
            self.saw_copy = true;
        }
        assign.visit_children_with(self);
    }
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

fn callee_is_index_of_on_binding(callee: &Callee, binding: &Ident) -> bool {
    let Callee::Expr(expr) = callee else {
        return false;
    };
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    member_prop_name(&member.prop, "indexOf") && member_obj_is_binding(member, binding)
}

fn assign_target_obj_is_binding(target: &AssignTarget, binding: &Ident) -> bool {
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = target else {
        return false;
    };
    member_obj_is_binding(member, binding)
}

fn member_obj_is_binding(member: &MemberExpr, binding: &Ident) -> bool {
    matches!(member.obj.as_ref(), Expr::Ident(id) if id.sym == binding.sym && id.ctxt == binding.ctxt)
}

fn is_binding_ref(expr: &Expr, binding: &Ident) -> bool {
    matches!(strip_parens(expr), Expr::Ident(id) if id.sym == binding.sym && id.ctxt == binding.ctxt)
}

fn remove_unused_esbuild_object_rest_builtin_aliases(
    module: &mut Module,
    aliases: &EsbuildObjectRestBuiltinAliases,
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

struct ObjectRestProcessor<'a> {
    named_helpers: &'a HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &'a HashSet<BindingKey>,
    swc_numeric_helper_namespaces: &'a HashSet<BindingKey>,
    cross_module_namespaces: &'a HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
    exclusion_arrays: HashMap<BindingKey, Vec<Atom>>,
    unresolved_mark: Mark,
}

impl VisitMut for ObjectRestProcessor<'_> {
    fn visit_mut_fn_decl(&mut self, decl: &mut FnDecl) {
        if self.is_named_helper(&decl.ident) {
            return;
        }
        decl.visit_mut_children_with(self);
    }

    fn visit_mut_var_declarator(&mut self, decl: &mut VarDeclarator) {
        if let Pat::Ident(binding) = &decl.name {
            if self.is_named_helper(&binding.id) {
                return;
            }
        }
        decl.visit_mut_children_with(self);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        reattach_elided_object_rest_in_stmts(
            stmts,
            self.named_helpers,
            self.tslib_namespaces,
            self.cross_module_namespaces,
            self.unresolved_mark,
        );

        let mut new_stmts = Vec::with_capacity(stmts.len());
        let mut exclusion_arrays = self.exclusion_arrays.clone();

        for (index, stmt) in stmts.iter().enumerate() {
            let extraction = try_extract_owp_iife(stmt, &exclusion_arrays).or_else(|| {
                try_extract_owp_named_call(
                    stmt,
                    self.named_helpers,
                    self.tslib_namespaces,
                    self.swc_numeric_helper_namespaces,
                    self.cross_module_namespaces,
                    &exclusion_arrays,
                )
            });

            if let Some((rest_binding, declaration_kind, source, excluded_keys, before, after)) =
                extraction
            {
                let future_jsx_tag_bindings = jsx_tag_bindings_in_stmts(&stmts[index + 1..]);
                if has_jsx_tag_default_pair(
                    &new_stmts,
                    &source,
                    &excluded_keys,
                    &future_jsx_tag_bindings,
                    self.unresolved_mark,
                ) {
                    collect_exclusion_arrays_from_stmt(stmt, &mut exclusion_arrays);
                    new_stmts.push(stmt.clone());
                    continue;
                }
                let mut inline_accesses = declarators_to_accesses(&before, &source, &excluded_keys);
                let preceding_scan = scan_preceding_detailed(
                    &new_stmts,
                    &source,
                    &excluded_keys,
                    self.unresolved_mark,
                );
                for _ in 0..preceding_scan.absorbed {
                    new_stmts.pop();
                }
                if let Some(source_init) = preceding_scan.source_init.clone() {
                    new_stmts.push(build_source_init_stmt(source_init));
                }
                let mut preceding_accesses = preceding_scan.accesses;
                preceding_accesses.append(&mut inline_accesses);
                let scope_names = collect_scope_names(&new_stmts);
                let original_span = stmt.span();
                new_stmts.push(build_rest_destructuring(
                    original_span,
                    declaration_kind,
                    &rest_binding,
                    &source,
                    &excluded_keys,
                    &preceding_accesses,
                    &scope_names,
                ));
                if !after.is_empty() {
                    new_stmts.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
                        span: original_span,
                        ctxt: Default::default(),
                        kind: declaration_kind,
                        declare: false,
                        decls: after,
                    }))));
                }
                continue;
            }

            if let Some((rest_binding, source, excluded_keys)) = try_extract_owp_named_assignment(
                stmt,
                self.named_helpers,
                self.tslib_namespaces,
                self.swc_numeric_helper_namespaces,
                self.cross_module_namespaces,
                &exclusion_arrays,
            ) {
                let original_span = stmt.span();
                let preceding_scan = scan_preceding_detailed(
                    &new_stmts,
                    &source,
                    &excluded_keys,
                    self.unresolved_mark,
                );
                let scope_names = collect_scope_names(&new_stmts);
                if preceding_scan.absorbed > 0 {
                    if let Some(new_stmt) = build_rest_assignment(
                        original_span,
                        &rest_binding,
                        &source,
                        &excluded_keys,
                        &preceding_scan.accesses,
                        &scope_names,
                    ) {
                        for _ in 0..preceding_scan.absorbed {
                            new_stmts.pop();
                        }
                        if let Some(source_init) = preceding_scan.source_init {
                            new_stmts.push(build_source_init_stmt(source_init));
                        }
                        new_stmts.push(new_stmt);
                        continue;
                    }
                }
            }

            collect_exclusion_arrays_from_stmt(stmt, &mut exclusion_arrays);
            new_stmts.push(stmt.clone());
        }

        *stmts = new_stmts;
    }
}

impl ObjectRestProcessor<'_> {
    fn is_named_helper(&self, ident: &Ident) -> bool {
        self.named_helpers
            .contains_key(&(ident.sym.clone(), ident.ctxt))
    }
}

fn reattach_elided_object_rest_in_module_items(
    items: &mut [ModuleItem],
    named_helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
    cross_module_namespaces: &HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
    unresolved_mark: Mark,
) {
    if !module_items_contain_owp_spread_candidate(
        items,
        named_helpers,
        tslib_namespaces,
        cross_module_namespaces,
    ) {
        return;
    }

    for item in items.iter_mut() {
        if let ModuleItem::Stmt(stmt) = item {
            reattach_elided_object_rest_in_stmt(
                stmt,
                named_helpers,
                tslib_namespaces,
                cross_module_namespaces,
                unresolved_mark,
            );
        }
    }

    for rest_idx in 0..items.len() {
        let Some(rest_binding) = module_item_single_undefined_binding(&items[rest_idx]) else {
            continue;
        };
        let preceding: Vec<Stmt> = items[..rest_idx]
            .iter()
            .filter_map(|item| match item {
                ModuleItem::Stmt(stmt) => Some(stmt.clone()),
                ModuleItem::ModuleDecl(_) => None,
            })
            .collect();

        let mut replacement_init = None;
        for item in items.iter_mut().skip(rest_idx + 1) {
            let ModuleItem::Stmt(stmt) = item else {
                continue;
            };
            let mut replacer = ElidedRestSpreadReplacer {
                rest_binding: &rest_binding,
                named_helpers,
                tslib_namespaces,
                cross_module_namespaces,
                preceding: Some(&preceding),
                unresolved_mark,
                replacement_init: None,
            };
            stmt.visit_mut_with(&mut replacer);
            if replacer.replacement_init.is_some() {
                replacement_init = replacer.replacement_init;
                break;
            }
        }

        if let Some(init) = replacement_init {
            set_module_item_single_decl_init(&mut items[rest_idx], init);
        }
    }
}

fn reattach_elided_object_rest_in_stmts(
    stmts: &mut [Stmt],
    named_helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
    cross_module_namespaces: &HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
    unresolved_mark: Mark,
) {
    if !stmts_contain_owp_spread_candidate(
        stmts,
        named_helpers,
        tslib_namespaces,
        cross_module_namespaces,
    ) {
        return;
    }

    for stmt in stmts.iter_mut() {
        reattach_elided_object_rest_in_stmt(
            stmt,
            named_helpers,
            tslib_namespaces,
            cross_module_namespaces,
            unresolved_mark,
        );
    }

    for rest_idx in 0..stmts.len() {
        let Some(rest_binding) = stmt_single_undefined_binding(&stmts[rest_idx]) else {
            continue;
        };
        let preceding = stmts[..rest_idx].to_vec();

        let mut replacement_init = None;
        for stmt in stmts.iter_mut().skip(rest_idx + 1) {
            let mut replacer = ElidedRestSpreadReplacer {
                rest_binding: &rest_binding,
                named_helpers,
                tslib_namespaces,
                cross_module_namespaces,
                preceding: Some(&preceding),
                unresolved_mark,
                replacement_init: None,
            };
            stmt.visit_mut_with(&mut replacer);
            if replacer.replacement_init.is_some() {
                replacement_init = replacer.replacement_init;
                break;
            }
        }

        if let Some(init) = replacement_init {
            set_stmt_single_decl_init(&mut stmts[rest_idx], init);
        }
    }
}

fn module_items_contain_owp_spread_candidate(
    items: &[ModuleItem],
    named_helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
    cross_module_namespaces: &HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
) -> bool {
    let mut visitor = ObjectRestSpreadCandidateVisitor {
        named_helpers,
        tslib_namespaces,
        cross_module_namespaces,
        found: false,
    };
    for item in items {
        item.visit_with(&mut visitor);
        if visitor.found {
            return true;
        }
    }
    false
}

fn stmts_contain_owp_spread_candidate(
    stmts: &[Stmt],
    named_helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
    cross_module_namespaces: &HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
) -> bool {
    let mut visitor = ObjectRestSpreadCandidateVisitor {
        named_helpers,
        tslib_namespaces,
        cross_module_namespaces,
        found: false,
    };
    for stmt in stmts {
        stmt.visit_with(&mut visitor);
        if visitor.found {
            return true;
        }
    }
    false
}

struct ObjectRestSpreadCandidateVisitor<'a> {
    named_helpers: &'a HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &'a HashSet<BindingKey>,
    cross_module_namespaces: &'a HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
    found: bool,
}

impl Visit for ObjectRestSpreadCandidateVisitor<'_> {
    fn visit_prop_or_spread(&mut self, prop: &PropOrSpread) {
        if self.found {
            return;
        }

        let PropOrSpread::Spread(spread) = prop else {
            prop.visit_children_with(self);
            return;
        };

        if extract_named_owp_args(
            &spread.expr,
            self.named_helpers,
            self.tslib_namespaces,
            &HashSet::new(),
            self.cross_module_namespaces,
            &HashMap::new(),
        )
        .or_else(|| try_extract_owp_call(&spread.expr, &HashMap::new()))
        .is_some()
        {
            self.found = true;
            return;
        }

        spread.expr.visit_with(self);
    }
}

fn reattach_elided_object_rest_in_stmt(
    stmt: &mut Stmt,
    named_helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
    cross_module_namespaces: &HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
    unresolved_mark: Mark,
) {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return;
    };

    for rest_idx in 0..var.decls.len() {
        let Some(rest_binding) = undefined_ident_declarator(&var.decls[rest_idx]) else {
            continue;
        };

        let mut replacement_init = None;
        for decl in var.decls.iter_mut().skip(rest_idx + 1) {
            let mut replacer = ElidedRestSpreadReplacer {
                rest_binding: &rest_binding,
                named_helpers,
                tslib_namespaces,
                cross_module_namespaces,
                preceding: None,
                unresolved_mark,
                replacement_init: None,
            };
            decl.visit_mut_with(&mut replacer);
            if replacer.replacement_init.is_some() {
                replacement_init = replacer.replacement_init;
                break;
            }
        }

        if let Some(init) = replacement_init {
            var.decls[rest_idx].init = Some(init);
            break;
        }
    }
}

struct ElidedRestSpreadReplacer<'a> {
    rest_binding: &'a BindingIdent,
    named_helpers: &'a HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &'a HashSet<BindingKey>,
    cross_module_namespaces: &'a HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
    preceding: Option<&'a [Stmt]>,
    unresolved_mark: Mark,
    replacement_init: Option<Box<Expr>>,
}

impl VisitMut for ElidedRestSpreadReplacer<'_> {
    fn visit_mut_prop_or_spread(&mut self, prop: &mut PropOrSpread) {
        if self.replacement_init.is_some() {
            return;
        }

        let PropOrSpread::Spread(spread) = prop else {
            prop.visit_mut_children_with(self);
            return;
        };

        let extraction = extract_named_owp_args(
            &spread.expr,
            self.named_helpers,
            self.tslib_namespaces,
            &HashSet::new(),
            self.cross_module_namespaces,
            &HashMap::new(),
        )
        .or_else(|| try_extract_owp_call(&spread.expr, &HashMap::new()));
        let Some((source, excluded_keys)) = extraction else {
            spread.visit_mut_children_with(self);
            return;
        };

        if let Some(preceding) = self.preceding {
            let (absorbed, _) =
                scan_preceding(preceding, &source, &excluded_keys, self.unresolved_mark);
            if absorbed == 0 {
                spread.visit_mut_children_with(self);
                return;
            }
        }

        self.replacement_init = Some(spread.expr.take());
        *spread.expr = Expr::Ident(self.rest_binding.id.clone());
    }
}

fn module_item_single_undefined_binding(item: &ModuleItem) -> Option<BindingIdent> {
    let ModuleItem::Stmt(stmt) = item else {
        return None;
    };
    stmt_single_undefined_binding(stmt)
}

fn stmt_single_undefined_binding(stmt: &Stmt) -> Option<BindingIdent> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    let [decl] = var.decls.as_slice() else {
        return None;
    };
    undefined_ident_declarator(decl)
}

fn undefined_ident_declarator(decl: &VarDeclarator) -> Option<BindingIdent> {
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    match &decl.init {
        Some(init) if is_undefined_expr(init) => Some(binding.clone()),
        None => Some(binding.clone()),
        _ => None,
    }
}

fn set_module_item_single_decl_init(item: &mut ModuleItem, init: Box<Expr>) {
    if let ModuleItem::Stmt(stmt) = item {
        set_stmt_single_decl_init(stmt, init);
    }
}

fn set_stmt_single_decl_init(stmt: &mut Stmt, init: Box<Expr>) {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return;
    };
    let [decl] = var.decls.as_mut_slice() else {
        return;
    };
    decl.init = Some(init);
}

fn is_undefined_expr(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Ident(id) if id.sym.as_ref() == "undefined")
}

/// Extracted info from a preceding statement that accesses the same source object.
enum PrecedingAccess {
    /// `const { a, b: c } = source` — destructuring with key→binding pairs
    Destructuring(Vec<(Atom, Atom, SyntaxContext, Option<Box<Expr>>)>), // (prop_key, local_binding, binding_ctxt, default)
    /// `const x = source.prop` — single property access
    PropAccess {
        prop: Atom,
        binding: Atom,
        ctxt: SyntaxContext,
    },
    /// Two-statement pair: `const tmp = source.prop; const x = tmp === undefined ? def : tmp`
    PropAccessWithDefault {
        prop: Atom,
        binding: Atom,
        ctxt: SyntaxContext,
        default_value: Box<Expr>,
    },
    /// `source.prop;` — bare access (no binding)
    BareAccess { _prop: Atom },
}

/// Try to extract an `_objectWithoutPropertiesLoose` inline IIFE from a statement.
/// Returns (rest_binding_name, declaration_kind, source_expr, excluded_keys,
/// declarators_before, declarators_after).
/// The before/after declarators are from the same var decl if it had multiple declarators.
fn try_extract_owp_iife(
    stmt: &Stmt,
    exclusion_arrays: &HashMap<BindingKey, Vec<Atom>>,
) -> Option<(
    BindingIdent,
    VarDeclKind,
    Box<Expr>,
    Vec<Atom>,
    Vec<VarDeclarator>,
    Vec<VarDeclarator>,
)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };

    // Find the first declarator whose init is an OWP IIFE
    let owp_idx = var.decls.iter().position(|decl| {
        let Pat::Ident(_) = &decl.name else {
            return false;
        };
        let Some(init) = &decl.init else {
            return false;
        };
        try_extract_owp_call(init, exclusion_arrays).is_some()
    })?;

    let decl = &var.decls[owp_idx];
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let init = decl.init.as_ref()?;
    let (source, excluded_keys) = try_extract_owp_call(init, exclusion_arrays)?;

    let before = var.decls[..owp_idx].to_vec();
    let after = var.decls[owp_idx + 1..].to_vec();
    Some((
        binding.clone(),
        var.kind,
        source,
        excluded_keys,
        before,
        after,
    ))
}

/// Try to extract a named OWP helper call from a statement.
/// Matches: `const rest = helperName(source, ["key1", "key2"])`
fn try_extract_owp_named_call(
    stmt: &Stmt,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
    swc_numeric_helper_namespaces: &HashSet<BindingKey>,
    cross_module_namespaces: &HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
    exclusion_arrays: &HashMap<BindingKey, Vec<Atom>>,
) -> Option<(
    BindingIdent,
    VarDeclKind,
    Box<Expr>,
    Vec<Atom>,
    Vec<VarDeclarator>,
    Vec<VarDeclarator>,
)> {
    if helpers.is_empty()
        && tslib_namespaces.is_empty()
        && swc_numeric_helper_namespaces.is_empty()
        && cross_module_namespaces.is_empty()
    {
        return None;
    }
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };

    let owp_idx = var.decls.iter().position(|decl| {
        let Pat::Ident(_) = &decl.name else {
            return false;
        };
        let Some(init) = &decl.init else {
            return false;
        };
        extract_named_owp_args(
            init,
            helpers,
            tslib_namespaces,
            swc_numeric_helper_namespaces,
            cross_module_namespaces,
            exclusion_arrays,
        )
        .is_some()
    })?;

    let decl = &var.decls[owp_idx];
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let init = decl.init.as_ref()?;
    let (source, excluded_keys) = extract_named_owp_args(
        init,
        helpers,
        tslib_namespaces,
        swc_numeric_helper_namespaces,
        cross_module_namespaces,
        exclusion_arrays,
    )?;

    let before = var.decls[..owp_idx].to_vec();
    let after = var.decls[owp_idx + 1..].to_vec();
    Some((
        binding.clone(),
        var.kind,
        source,
        excluded_keys,
        before,
        after,
    ))
}

fn try_extract_owp_named_assignment(
    stmt: &Stmt,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
    swc_numeric_helper_namespaces: &HashSet<BindingKey>,
    cross_module_namespaces: &HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
    exclusion_arrays: &HashMap<BindingKey, Vec<Atom>>,
) -> Option<(BindingIdent, Box<Expr>, Vec<Atom>)> {
    if helpers.is_empty()
        && tslib_namespaces.is_empty()
        && swc_numeric_helper_namespaces.is_empty()
        && cross_module_namespaces.is_empty()
    {
        return None;
    }

    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(rest_binding)) = &assign.left else {
        return None;
    };
    let (source, excluded_keys) = extract_named_owp_args(
        &assign.right,
        helpers,
        tslib_namespaces,
        swc_numeric_helper_namespaces,
        cross_module_namespaces,
        exclusion_arrays,
    )?;
    Some((rest_binding.clone(), source, excluded_keys))
}

/// Extract (source, excluded_keys) from a call to a known named OWP helper.
fn extract_named_owp_args(
    expr: &Expr,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
    swc_numeric_helper_namespaces: &HashSet<BindingKey>,
    cross_module_namespaces: &HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
    exclusion_arrays: &HashMap<BindingKey, Vec<Atom>>,
) -> Option<(Box<Expr>, Vec<Atom>)> {
    let Expr::Call(CallExpr {
        callee: Callee::Expr(callee),
        args,
        ..
    }) = expr
    else {
        return None;
    };
    if !is_named_owp_callee(
        callee,
        helpers,
        tslib_namespaces,
        swc_numeric_helper_namespaces,
        cross_module_namespaces,
    ) {
        return None;
    }
    if args.len() != 2 || args[0].spread.is_some() || args[1].spread.is_some() {
        return None;
    }
    let keys = extract_exclusion_keys(args[1].expr.as_ref(), exclusion_arrays)?;
    Some((args[0].expr.clone(), keys))
}

fn is_named_owp_callee(
    callee: &Expr,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
    swc_numeric_helper_namespaces: &HashSet<BindingKey>,
    cross_module_namespaces: &HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
) -> bool {
    match callee {
        Expr::Ident(id) => helpers.contains_key(&(id.sym.clone(), id.ctxt)),
        Expr::Member(_) => {
            matches!(
                tslib_member_helper_kind(callee, tslib_namespaces),
                Some(TranspilerHelperKind::ObjectWithoutProperties)
            ) || is_swc_numeric_object_rest_member(callee, swc_numeric_helper_namespaces)
                || cross_module_member_helper_kind(callee, cross_module_namespaces)
                    == Some(TranspilerHelperKind::ObjectWithoutProperties)
        }
        _ => false,
    }
}

fn extract_exclusion_keys(
    expr: &Expr,
    exclusion_arrays: &HashMap<BindingKey, Vec<Atom>>,
) -> Option<Vec<Atom>> {
    match expr {
        Expr::Array(arr) => extract_exclusion_keys_from_array(arr),
        Expr::Ident(id) => exclusion_arrays
            .get(&(id.sym.clone(), id.ctxt))
            .cloned()
            .or_else(|| {
                exclusion_arrays
                    .iter()
                    .find_map(|((sym, _), keys)| (sym == &id.sym).then(|| keys.clone()))
            }),
        _ => None,
    }
}

fn collect_swc_numeric_helper_namespaces(
    module: &Module,
    unresolved_mark: Mark,
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

fn is_numeric_require_call(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return false;
    }
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !matches!(callee.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "require" && id.ctxt.outer() == unresolved_mark)
    {
        return false;
    }
    matches!(call.args[0].expr.as_ref(), Expr::Lit(Lit::Num(_)))
}

fn is_swc_numeric_object_rest_member(expr: &Expr, namespaces: &HashSet<BindingKey>) -> bool {
    let Expr::Member(member) = expr else {
        return false;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };
    namespaces.contains(&(obj.sym.clone(), obj.ctxt))
        && member_prop_atom(&member.prop).is_some_and(|name| name.as_ref() == "_T")
}

fn extract_exclusion_keys_from_array(arr: &swc_core::ecma::ast::ArrayLit) -> Option<Vec<Atom>> {
    let mut keys: Vec<Atom> = Vec::new();
    for elem in &arr.elems {
        let Some(elem) = elem else { return None };
        if elem.spread.is_some() {
            return None;
        }
        let Expr::Lit(Lit::Str(s)) = elem.expr.as_ref() else {
            return None;
        };
        let key_str = s.value.as_str()?;
        keys.push(Atom::from(key_str));
    }
    Some(keys)
}

fn collect_exclusion_arrays_from_module_items(
    items: &[ModuleItem],
) -> HashMap<BindingKey, Vec<Atom>> {
    let mut arrays = HashMap::new();
    for item in items {
        if let ModuleItem::Stmt(stmt) = item {
            collect_exclusion_arrays_from_stmt(stmt, &mut arrays);
        }
    }
    arrays
}

fn collect_exclusion_arrays_from_stmt(stmt: &Stmt, arrays: &mut HashMap<BindingKey, Vec<Atom>>) {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return;
    };
    for decl in &var.decls {
        let Pat::Ident(binding) = &decl.name else {
            continue;
        };
        let Some(init) = &decl.init else {
            continue;
        };
        let Expr::Array(arr) = init.as_ref() else {
            continue;
        };
        let Some(keys) = extract_exclusion_keys_from_array(arr) else {
            continue;
        };
        arrays.insert((binding.id.sym.clone(), binding.id.ctxt), keys);
    }
}

fn remove_unused_exclusion_array_decls(
    body: &mut Vec<ModuleItem>,
    exclusion_arrays: &HashMap<BindingKey, Vec<Atom>>,
) {
    let mut unused = HashSet::new();
    for key in exclusion_arrays.keys() {
        let ident = Ident::new(key.0.clone(), DUMMY_SP, key.1);
        if !ident_used_in_module_items(body, &ident) {
            unused.insert(key.clone());
        }
    }
    if unused.is_empty() {
        return;
    }

    body.retain_mut(|item| {
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

fn remove_unused_numeric_helper_namespace_decls(
    body: &mut Vec<ModuleItem>,
    namespaces: &HashSet<BindingKey>,
) {
    let mut unused = HashSet::new();
    for key in namespaces {
        let ident = Ident::new(key.0.clone(), DUMMY_SP, key.1);
        if !ident_used_in_module_items(body, &ident) {
            unused.insert(key.clone());
        }
    }
    if unused.is_empty() {
        return;
    }

    body.retain_mut(|item| {
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

fn ident_used_in_module_items(body: &[ModuleItem], target: &Ident) -> bool {
    struct Finder<'a> {
        target: &'a Ident,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_binding_ident(&mut self, _: &BindingIdent) {}

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

/// Check if an expression is an OWP IIFE call, returning (source, excluded_keys).
fn try_extract_owp_call(
    expr: &Expr,
    exclusion_arrays: &HashMap<BindingKey, Vec<Atom>>,
) -> Option<(Box<Expr>, Vec<Atom>)> {
    let Expr::Call(CallExpr {
        callee: Callee::Expr(callee),
        args,
        ..
    }) = expr
    else {
        return None;
    };
    if args.len() != 2 || args[0].spread.is_some() || args[1].spread.is_some() {
        return None;
    }
    let keys = extract_exclusion_keys(args[1].expr.as_ref(), exclusion_arrays)?;
    let callee = strip_parens(callee);
    let body_stmts = match callee {
        Expr::Arrow(ArrowExpr { body, params, .. }) if params.len() == 2 => match &**body {
            BlockStmtOrExpr::BlockStmt(block) => &block.stmts,
            _ => return None,
        },
        Expr::Fn(FnExpr { function, .. }) if function.params.len() == 2 => {
            function.body.as_ref()?.stmts.as_slice()
        }
        _ => return None,
    };
    if !is_owp_body(body_stmts) {
        return None;
    }
    Some((args[0].expr.clone(), keys))
}

/// Check if function body matches the objectWithoutPropertiesLoose shape:
/// ```js
/// const/var n = {};
/// for (const/var r in e) {
///     t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
/// }
/// return n;
/// ```
fn is_owp_body(stmts: &[Stmt]) -> bool {
    if is_owp_spec_wrapper_body(stmts) {
        return true;
    }

    // 3 statements: var init, for-in, return
    if stmts.len() != 3 {
        return false;
    }

    // First: var/const n = {}
    let Stmt::Decl(Decl::Var(var)) = &stmts[0] else {
        return false;
    };
    if var.decls.len() != 1 {
        return false;
    }
    let Some(init) = &var.decls[0].init else {
        return false;
    };
    if !matches!(init.as_ref(), Expr::Object(obj) if obj.props.is_empty()) {
        return false;
    }

    // Second: for (... in ...) with indexOf + hasOwnProperty in body
    let Stmt::ForIn(for_in) = &stmts[1] else {
        return false;
    };
    if !for_in_body_has_owp_shape(&for_in.body) {
        return false;
    }

    // Third: return <ident> (the accumulator)
    let Stmt::Return(ret) = &stmts[2] else {
        return false;
    };
    matches!(&ret.arg, Some(arg) if matches!(arg.as_ref(), Expr::Ident(_)))
}

fn is_owp_spec_wrapper_body(stmts: &[Stmt]) -> bool {
    if stmts.len() < 4 {
        return false;
    }

    let has_symbol_copy = stmts.iter().any(|stmt| {
        let Stmt::If(symbols_if) = stmt else {
            return false;
        };
        stmt_has_member_prop(&symbols_if.cons, "getOwnPropertySymbols")
            && stmt_has_member_prop(&symbols_if.cons, "propertyIsEnumerable")
    });
    if !has_symbol_copy {
        return false;
    }

    matches!(stmts.last(), Some(Stmt::Return(ret)) if matches!(&ret.arg, Some(arg) if matches!(arg.as_ref(), Expr::Ident(_))))
}

fn stmt_has_member_prop(stmt: &Stmt, prop: &str) -> bool {
    struct Finder<'a> {
        prop: &'a str,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_member_expr(&mut self, member: &MemberExpr) {
            if self.found {
                return;
            }
            if let MemberProp::Ident(id) = &member.prop {
                if id.sym.as_ref() == self.prop {
                    self.found = true;
                    return;
                }
            }
            member.visit_children_with(self);
        }
    }

    let mut finder = Finder { prop, found: false };
    stmt.visit_with(&mut finder);
    finder.found
}

/// Scan backward from the end of `preceding` for statements that access `source`.
/// Returns (count_absorbed, merged_prop_info).
fn scan_preceding(
    preceding: &[Stmt],
    source: &Expr,
    excluded_keys: &[Atom],
    unresolved_mark: Mark,
) -> (usize, Vec<PrecedingAccess>) {
    let scan = scan_preceding_detailed(preceding, source, excluded_keys, unresolved_mark);
    (scan.absorbed, scan.accesses)
}

struct PrecedingScan {
    absorbed: usize,
    accesses: Vec<PrecedingAccess>,
    source_init: Option<AssignExpr>,
}

fn scan_preceding_detailed(
    preceding: &[Stmt],
    source: &Expr,
    excluded_keys: &[Atom],
    unresolved_mark: Mark,
) -> PrecedingScan {
    let source_name = match source {
        Expr::Ident(id) => &id.sym,
        _ => {
            return PrecedingScan {
                absorbed: 0,
                accesses: vec![],
                source_init: None,
            };
        }
    };

    let mut absorbed = 0;
    let mut accesses = Vec::new();
    let mut source_init = None;
    let mut idx = preceding.len();

    while idx > 0 {
        idx -= 1;
        let stmt = &preceding[idx];

        if let Some((access, init_assign)) =
            try_match_preceding_detailed(stmt, source_name, excluded_keys)
        {
            absorbed += 1;
            if source_init.is_none() {
                source_init = init_assign;
            }
            accesses.push(access);
            continue;
        }

        // Two-statement pair: ternary default (current) + extraction (previous)
        if idx > 0 {
            if let Some(access) = try_match_default_pair(
                &preceding[idx - 1],
                stmt,
                source_name,
                excluded_keys,
                unresolved_mark,
            ) {
                absorbed += 2;
                idx -= 1;
                accesses.push(access);
                continue;
            }
        }

        break;
    }

    accesses.reverse();
    PrecedingScan {
        absorbed,
        accesses,
        source_init,
    }
}

fn has_jsx_tag_default_pair(
    preceding: &[Stmt],
    source: &Expr,
    excluded_keys: &[Atom],
    future_jsx_tag_bindings: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) -> bool {
    if preceding.len() < 2 || future_jsx_tag_bindings.is_empty() {
        return false;
    }
    let source_name = match source {
        Expr::Ident(id) => &id.sym,
        _ => return false,
    };
    let Some(PrecedingAccess::PropAccessWithDefault {
        prop,
        binding,
        ctxt,
        ..
    }) = try_match_default_pair(
        &preceding[preceding.len() - 2],
        &preceding[preceding.len() - 1],
        source_name,
        excluded_keys,
        unresolved_mark,
    )
    else {
        return false;
    };
    excluded_keys.contains(&prop) && future_jsx_tag_bindings.contains(&(binding, ctxt))
}

fn jsx_tag_bindings_in_module_items(items: &[ModuleItem]) -> HashSet<BindingKey> {
    let mut collector = JsxTagBindingCollector::default();
    for item in items {
        item.visit_with(&mut collector);
    }
    collector.bindings
}

fn jsx_tag_bindings_in_stmts(stmts: &[Stmt]) -> HashSet<BindingKey> {
    let mut collector = JsxTagBindingCollector::default();
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
    collector.bindings
}

#[derive(Default)]
struct JsxTagBindingCollector {
    bindings: HashSet<BindingKey>,
}

impl Visit for JsxTagBindingCollector {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if is_jsx_factory_call(call) {
            if let Some(first_arg) = call.args.first() {
                if let Expr::Ident(ident) = first_arg.expr.as_ref() {
                    self.bindings.insert((ident.sym.clone(), ident.ctxt));
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_jsx_element_name(&mut self, name: &JSXElementName) {
        match name {
            JSXElementName::Ident(ident) => {
                self.bindings.insert((ident.sym.clone(), ident.ctxt));
            }
            JSXElementName::JSXMemberExpr(member) => member.visit_children_with(self),
            JSXElementName::JSXNamespacedName(_) => {}
        }
    }
}

fn is_jsx_factory_call(call: &CallExpr) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    match strip_parens(callee.as_ref()) {
        Expr::Ident(ident) => matches!(
            ident.sym.as_ref(),
            "jsx" | "jsxs" | "jsxDEV" | "createElement"
        ),
        Expr::Member(member) => member_prop_atom(&member.prop).is_some_and(|prop| {
            matches!(prop.as_ref(), "jsx" | "jsxs" | "jsxDEV" | "createElement")
        }),
        _ => false,
    }
}

/// Convert preceding declarators from the same var decl to PrecedingAccess entries.
/// Handles `t = e.to` → PropAccess and `e["aria-current"]` → PropAccess with string key.
fn declarators_to_accesses(
    decls: &[VarDeclarator],
    source: &Expr,
    excluded_keys: &[Atom],
) -> Vec<PrecedingAccess> {
    let source_name = match source {
        Expr::Ident(id) => &id.sym,
        _ => return vec![],
    };
    let mut accesses = Vec::new();
    for decl in decls {
        let Pat::Ident(bi) = &decl.name else {
            continue;
        };
        let Some(init) = &decl.init else {
            continue;
        };
        if let Expr::Member(MemberExpr { obj, prop, .. }) = init.as_ref() {
            if let Expr::Ident(obj_id) = obj.as_ref() {
                if obj_id.sym == *source_name {
                    let prop_name = match prop {
                        MemberProp::Ident(id) => Some(id.sym.clone()),
                        MemberProp::Computed(c) => {
                            if let Expr::Lit(Lit::Str(s)) = c.expr.as_ref() {
                                s.value.as_str().map(Atom::from)
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    if let Some(prop_name) = prop_name {
                        if excluded_keys.contains(&prop_name) {
                            accesses.push(PrecedingAccess::PropAccess {
                                prop: prop_name,
                                binding: bi.id.sym.clone(),
                                ctxt: bi.id.ctxt,
                            });
                        }
                    }
                }
            }
        }
    }
    accesses
}

fn try_match_preceding_detailed(
    stmt: &Stmt,
    source_name: &Atom,
    excluded_keys: &[Atom],
) -> Option<(PrecedingAccess, Option<AssignExpr>)> {
    // Case 1: const { a, b } = source
    if let Stmt::Decl(Decl::Var(var)) = stmt {
        if var.decls.len() == 1 {
            let decl = &var.decls[0];
            if let Pat::Object(obj_pat) = &decl.name {
                if let Some(init) = &decl.init {
                    if let Expr::Ident(id) = init.as_ref() {
                        if id.sym == *source_name {
                            let mut pairs = Vec::new();
                            for prop in &obj_pat.props {
                                match prop {
                                    ObjectPatProp::Assign(a) => {
                                        let key = a.key.id.sym.clone();
                                        if excluded_keys.contains(&key) {
                                            pairs.push((
                                                key.clone(),
                                                key,
                                                a.key.id.ctxt,
                                                a.value.clone(),
                                            ));
                                        }
                                    }
                                    ObjectPatProp::KeyValue(kv) => {
                                        let key = prop_name_atom(&kv.key)?;
                                        if excluded_keys.contains(&key) {
                                            if let Pat::Ident(bi) = kv.value.as_ref() {
                                                pairs.push((
                                                    key,
                                                    bi.id.sym.clone(),
                                                    bi.id.ctxt,
                                                    None,
                                                ));
                                            } else if let Pat::Assign(assign) = kv.value.as_ref() {
                                                if let Pat::Ident(bi) = assign.left.as_ref() {
                                                    pairs.push((
                                                        key,
                                                        bi.id.sym.clone(),
                                                        bi.id.ctxt,
                                                        Some(assign.right.clone()),
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            if !pairs.is_empty() {
                                return Some((PrecedingAccess::Destructuring(pairs), None));
                            }
                        }
                    }
                }
            }

            // Case 2: const x = source.prop
            if let Pat::Ident(bi) = &decl.name {
                if let Some(init) = &decl.init {
                    if let Expr::Member(MemberExpr { obj, prop, .. }) = init.as_ref() {
                        if let Some(source_init) = match_source_member_object(obj, source_name) {
                            if let Some(pname) = member_prop_atom(prop) {
                                if excluded_keys.contains(&pname) {
                                    return Some((
                                        PrecedingAccess::PropAccess {
                                            prop: pname,
                                            binding: bi.id.sym.clone(),
                                            ctxt: bi.id.ctxt,
                                        },
                                        source_init,
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Case 3: x = source.prop
    if let Stmt::Expr(ExprStmt { expr, .. }) = stmt {
        if let Expr::Assign(assign) = expr.as_ref() {
            if assign.op == AssignOp::Assign {
                if let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &assign.left {
                    if let Expr::Member(MemberExpr { obj, prop, .. }) = assign.right.as_ref() {
                        if let Some(source_init) = match_source_member_object(obj, source_name) {
                            if let Some(pname) = member_prop_atom(prop) {
                                if excluded_keys.contains(&pname) {
                                    return Some((
                                        PrecedingAccess::PropAccess {
                                            prop: pname,
                                            binding: binding.id.sym.clone(),
                                            ctxt: binding.id.ctxt,
                                        },
                                        source_init,
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Case 4: source.prop; (bare expression statement)
    if let Stmt::Expr(ExprStmt { expr, .. }) = stmt {
        if let Expr::Member(MemberExpr { obj, prop, .. }) = expr.as_ref() {
            if let Some(source_init) = match_source_member_object(obj, source_name) {
                if let Some(pname) = member_prop_atom(prop) {
                    if excluded_keys.contains(&pname) {
                        return Some((PrecedingAccess::BareAccess { _prop: pname }, source_init));
                    }
                }
            }
        }
    }

    None
}

fn match_source_member_object(obj: &Expr, source_name: &Atom) -> Option<Option<AssignExpr>> {
    match strip_parens(obj) {
        Expr::Ident(obj_id) if obj_id.sym == *source_name => Some(None),
        Expr::Assign(assign) if assign.op == AssignOp::Assign => {
            let AssignTarget::Simple(SimpleAssignTarget::Ident(left)) = &assign.left else {
                return None;
            };
            if left.id.sym != *source_name {
                return None;
            }
            Some(Some(assign.clone()))
        }
        _ => None,
    }
}

fn build_source_init_stmt(assign: AssignExpr) -> Stmt {
    let stmt_span = if assign.span.lo.0 != 0 {
        assign.span
    } else {
        DUMMY_SP
    };
    Stmt::Expr(ExprStmt {
        span: stmt_span,
        expr: Box::new(Expr::Assign(assign)),
    })
}

/// Try to match a two-statement pair:
///   extraction: `const tmp = source.prop`
///   default:    one of these forms:
///     - `const x = tmp === undefined ? defaultVal : tmp`  (ternary)
///     - `const x = tmp === undefined || tmp`              (boolean default true)
///     - `const x = tmp !== undefined && tmp`              (boolean default false)
fn try_match_default_pair(
    extraction_stmt: &Stmt,
    default_stmt: &Stmt,
    source_name: &Atom,
    excluded_keys: &[Atom],
    unresolved_mark: Mark,
) -> Option<PrecedingAccess> {
    // 1. Parse the default stmt
    let (final_binding, tmp_name, default_value) =
        extract_default_assignment(default_stmt, unresolved_mark)?;

    // 2. Parse the extraction stmt: const tmp = source.prop
    let Stmt::Decl(Decl::Var(extract_var)) = extraction_stmt else {
        return None;
    };
    if extract_var.decls.len() != 1 {
        return None;
    }
    let extract_decl = &extract_var.decls[0];
    let Pat::Ident(extract_binding) = &extract_decl.name else {
        return None;
    };
    if extract_binding.id.sym != tmp_name {
        return None;
    }
    let Some(extract_init) = &extract_decl.init else {
        return None;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = extract_init.as_ref() else {
        return None;
    };
    let Expr::Ident(obj_id) = obj.as_ref() else {
        return None;
    };
    if obj_id.sym != *source_name {
        return None;
    }
    let prop_name = member_prop_atom(prop)?;
    if !excluded_keys.contains(&prop_name) {
        return None;
    }

    match default_value {
        None => Some(PrecedingAccess::PropAccess {
            prop: prop_name,
            binding: final_binding.id.sym.clone(),
            ctxt: final_binding.id.ctxt,
        }),
        Some(def) => Some(PrecedingAccess::PropAccessWithDefault {
            prop: prop_name,
            binding: final_binding.id.sym.clone(),
            ctxt: final_binding.id.ctxt,
            default_value: def,
        }),
    }
}

/// Extract a default-value assignment from a var declaration.
/// Returns (final_binding, tmp_variable_name, default_expr_or_none).
///
/// Matches three forms:
/// 1. `const x = tmp === undefined ? defaultVal : tmp` → Some(defaultVal)
///    (when defaultVal is `undefined` itself → None, since destructuring
///    naturally produces undefined for missing properties)
/// 2. `const x = tmp === undefined || tmp` → Some(true)
/// 3. `const x = tmp !== undefined && tmp` → Some(false)
fn extract_default_assignment(
    stmt: &Stmt,
    unresolved_mark: Mark,
) -> Option<(BindingIdent, Atom, Option<Box<Expr>>)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(final_binding) = &decl.name else {
        return None;
    };
    let Some(init) = &decl.init else {
        return None;
    };

    match init.as_ref() {
        // Form 1: tmp === undefined ? defaultVal : tmp
        Expr::Cond(CondExpr {
            test, cons, alt, ..
        }) => {
            let (tmp_name, _) = match_undefined_check(test, BinaryOp::EqEqEq, unresolved_mark)?;
            if !matches!(alt.as_ref(), Expr::Ident(id) if id.sym == tmp_name) {
                return None;
            }
            let default_value = if matches!(cons.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "undefined" && (id.ctxt.outer() == unresolved_mark || id.ctxt == SyntaxContext::empty()))
            {
                None
            } else {
                Some(cons.clone())
            };
            Some((final_binding.clone(), tmp_name, default_value))
        }
        // Form 2: tmp === undefined || tmp  →  default true
        Expr::Bin(bin) if bin.op == BinaryOp::LogicalOr => {
            let (tmp_name, _) =
                match_undefined_check(&bin.left, BinaryOp::EqEqEq, unresolved_mark)?;
            if !matches!(bin.right.as_ref(), Expr::Ident(id) if id.sym == tmp_name) {
                return None;
            }
            let default_value = Box::new(Expr::Lit(Lit::Bool(Bool {
                span: DUMMY_SP,
                value: true,
            })));
            Some((final_binding.clone(), tmp_name, Some(default_value)))
        }
        // Form 3: tmp !== undefined && tmp  →  default false
        Expr::Bin(bin) if bin.op == BinaryOp::LogicalAnd => {
            let (tmp_name, _) =
                match_undefined_check(&bin.left, BinaryOp::NotEqEq, unresolved_mark)?;
            if !matches!(bin.right.as_ref(), Expr::Ident(id) if id.sym == tmp_name) {
                return None;
            }
            let default_value = Box::new(Expr::Lit(Lit::Bool(Bool {
                span: DUMMY_SP,
                value: false,
            })));
            Some((final_binding.clone(), tmp_name, Some(default_value)))
        }
        _ => None,
    }
}

/// Match `tmp === undefined` or `tmp !== undefined` (with a specific operator).
/// Returns the tmp variable name and its SyntaxContext.
fn match_undefined_check(
    expr: &Expr,
    expected_op: BinaryOp,
    unresolved_mark: Mark,
) -> Option<(Atom, SyntaxContext)> {
    let Expr::Bin(bin) = expr else { return None };
    if bin.op != expected_op {
        return None;
    }
    let Expr::Ident(tmp) = bin.left.as_ref() else {
        return None;
    };
    // Verify `undefined` is the global, not a shadowed local binding.
    // Accept both resolver-stamped globals (outer == unresolved_mark) and
    // synthesized identifiers from RemoveVoid (SyntaxContext::empty).
    let Expr::Ident(undef_id) = bin.right.as_ref() else {
        return None;
    };
    if undef_id.sym.as_ref() != "undefined" {
        return None;
    }
    let is_global =
        undef_id.ctxt.outer() == unresolved_mark || undef_id.ctxt == SyntaxContext::empty();
    if !is_global {
        return None;
    }
    Some((tmp.sym.clone(), tmp.ctxt))
}

fn build_rest_destructuring(
    original_span: Span,
    kind: VarDeclKind,
    rest_binding: &BindingIdent,
    source: &Expr,
    excluded_keys: &[Atom],
    merged: &[PrecedingAccess],
    scope_names: &std::collections::HashSet<Atom>,
) -> Stmt {
    // Build a map from prop key → (local binding name, SyntaxContext) from preceding accesses.
    // Preserving the original SyntaxContext is critical so that downstream SmartRename
    // can match the destructuring binding to the body references via BindingRenamer.
    let mut key_to_binding: std::collections::HashMap<Atom, (Atom, SyntaxContext)> =
        std::collections::HashMap::new();
    let mut key_to_default: std::collections::HashMap<Atom, Box<Expr>> =
        std::collections::HashMap::new();
    for access in merged {
        match access {
            PrecedingAccess::Destructuring(pairs) => {
                for (key, binding, ctxt, default_value) in pairs {
                    key_to_binding.insert(key.clone(), (binding.clone(), *ctxt));
                    if let Some(default_value) = default_value {
                        key_to_default.insert(key.clone(), default_value.clone());
                    }
                }
            }
            PrecedingAccess::PropAccess {
                prop,
                binding,
                ctxt,
            } => {
                key_to_binding.insert(prop.clone(), (binding.clone(), *ctxt));
            }
            PrecedingAccess::PropAccessWithDefault {
                prop,
                binding,
                ctxt,
                default_value,
            } => {
                key_to_binding.insert(prop.clone(), (binding.clone(), *ctxt));
                key_to_default.insert(prop.clone(), default_value.clone());
            }
            PrecedingAccess::BareAccess { .. } => {
                // No binding — key will be included as shorthand (unused)
            }
        }
    }

    // Track generated aliases to avoid collisions between them
    let mut used_aliases: std::collections::HashSet<Atom> = std::collections::HashSet::new();

    // Build destructuring props for each excluded key
    let mut props: Vec<ObjectPatProp> = Vec::new();
    for key in excluded_keys {
        if let Some((binding, ctxt)) = key_to_binding.get(key) {
            let default_expr = key_to_default.get(key);
            let is_shorthand = *binding == *key && is_valid_ident(key);

            if is_shorthand {
                // Shorthand: { key } or { key = default }
                props.push(ObjectPatProp::Assign(AssignPatProp {
                    span: DUMMY_SP,
                    key: BindingIdent {
                        id: Ident::new(key.clone(), DUMMY_SP, *ctxt),
                        type_ann: None,
                    },
                    value: default_expr.cloned(),
                }));
            } else if let Some(def) = default_expr {
                // Aliased with default: { key: binding = default }
                props.push(ObjectPatProp::KeyValue(KeyValuePatProp {
                    key: make_prop_name(key),
                    value: Box::new(Pat::Assign(AssignPat {
                        span: DUMMY_SP,
                        left: Box::new(Pat::Ident(BindingIdent {
                            id: Ident::new(binding.clone(), DUMMY_SP, *ctxt),
                            type_ann: None,
                        })),
                        right: def.clone(),
                    })),
                }));
            } else {
                // Aliased without default: { key: binding }
                props.push(ObjectPatProp::KeyValue(KeyValuePatProp {
                    key: make_prop_name(key),
                    value: Box::new(Pat::Ident(BindingIdent {
                        id: Ident::new(binding.clone(), DUMMY_SP, *ctxt),
                        type_ann: None,
                    })),
                }));
            }
        } else {
            // Not in preceding — generate a `_key` alias
            let base = format!("_{}", key);
            let alias = find_non_conflicting_alias(&base, scope_names, &used_aliases);
            used_aliases.insert(alias.clone());
            props.push(ObjectPatProp::KeyValue(KeyValuePatProp {
                key: make_prop_name(key),
                value: Box::new(Pat::Ident(BindingIdent {
                    id: Ident::new(alias, DUMMY_SP, Default::default()),
                    type_ann: None,
                })),
            }));
        }
    }

    // Add rest element
    props.push(ObjectPatProp::Rest(RestPat {
        span: DUMMY_SP,
        dot3_token: DUMMY_SP,
        arg: Box::new(Pat::Ident(rest_binding.clone())),
        type_ann: None,
    }));

    let var_span = if original_span.lo.0 != 0 {
        original_span
    } else {
        DUMMY_SP
    };
    Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: var_span,
        ctxt: Default::default(),
        kind,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Object(ObjectPat {
                span: DUMMY_SP,
                props,
                optional: false,
                type_ann: None,
            }),
            init: Some(Box::new((*source).clone())),
            definite: false,
        }],
    })))
}

fn build_rest_assignment(
    original_span: Span,
    rest_binding: &BindingIdent,
    source: &Expr,
    excluded_keys: &[Atom],
    merged: &[PrecedingAccess],
    scope_names: &std::collections::HashSet<Atom>,
) -> Option<Stmt> {
    let mut key_to_binding: std::collections::HashMap<Atom, (Atom, SyntaxContext)> =
        std::collections::HashMap::new();
    let mut key_to_default: std::collections::HashMap<Atom, Box<Expr>> =
        std::collections::HashMap::new();
    for access in merged {
        match access {
            PrecedingAccess::Destructuring(pairs) => {
                for (key, binding, ctxt, default_value) in pairs {
                    key_to_binding.insert(key.clone(), (binding.clone(), *ctxt));
                    if let Some(default_value) = default_value {
                        key_to_default.insert(key.clone(), default_value.clone());
                    }
                }
            }
            PrecedingAccess::PropAccess {
                prop,
                binding,
                ctxt,
            } => {
                key_to_binding.insert(prop.clone(), (binding.clone(), *ctxt));
            }
            PrecedingAccess::PropAccessWithDefault {
                prop,
                binding,
                ctxt,
                default_value,
            } => {
                key_to_binding.insert(prop.clone(), (binding.clone(), *ctxt));
                key_to_default.insert(prop.clone(), default_value.clone());
            }
            PrecedingAccess::BareAccess { .. } => {}
        }
    }

    let mut props: Vec<ObjectPatProp> = Vec::new();
    for key in excluded_keys {
        let (binding, ctxt) = key_to_binding.get(key)?;
        let default_expr = key_to_default.get(key);
        let is_shorthand = *binding == *key && is_valid_ident(key);
        if is_shorthand {
            props.push(ObjectPatProp::Assign(AssignPatProp {
                span: DUMMY_SP,
                key: BindingIdent {
                    id: Ident::new(key.clone(), DUMMY_SP, *ctxt),
                    type_ann: None,
                },
                value: default_expr.cloned(),
            }));
        } else if let Some(def) = default_expr {
            props.push(ObjectPatProp::KeyValue(KeyValuePatProp {
                key: make_prop_name(key),
                value: Box::new(Pat::Assign(AssignPat {
                    span: DUMMY_SP,
                    left: Box::new(Pat::Ident(BindingIdent {
                        id: Ident::new(binding.clone(), DUMMY_SP, *ctxt),
                        type_ann: None,
                    })),
                    right: def.clone(),
                })),
            }));
        } else {
            props.push(ObjectPatProp::KeyValue(KeyValuePatProp {
                key: make_prop_name(key),
                value: Box::new(Pat::Ident(BindingIdent {
                    id: Ident::new(binding.clone(), DUMMY_SP, *ctxt),
                    type_ann: None,
                })),
            }));
        }
    }

    if !scope_names.contains(&rest_binding.id.sym) {
        return None;
    }
    props.push(ObjectPatProp::Rest(RestPat {
        span: DUMMY_SP,
        dot3_token: DUMMY_SP,
        arg: Box::new(Pat::Ident(rest_binding.clone())),
        type_ann: None,
    }));

    let stmt_span = if original_span.lo.0 != 0 {
        original_span
    } else {
        DUMMY_SP
    };
    Some(Stmt::Expr(ExprStmt {
        span: stmt_span,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: stmt_span,
            op: AssignOp::Assign,
            left: AssignTarget::Pat(AssignTargetPat::Object(ObjectPat {
                span: DUMMY_SP,
                props,
                optional: false,
                type_ann: None,
            })),
            right: Box::new((*source).clone()),
        })),
    }))
}

/// Verify the for-in body references `indexOf` and `hasOwnProperty` —
/// the defining features of `_objectWithoutPropertiesLoose`.
fn for_in_body_has_owp_shape(body: &Stmt) -> bool {
    struct MethodFinder {
        has_index_of: bool,
        has_has_own: bool,
    }

    impl Visit for MethodFinder {
        fn visit_member_expr(&mut self, member: &MemberExpr) {
            if let MemberProp::Ident(id) = &member.prop {
                match id.sym.as_ref() {
                    "indexOf" => self.has_index_of = true,
                    "hasOwnProperty" => self.has_has_own = true,
                    _ => {}
                }
            }
            member.obj.visit_with(self);
        }
    }

    let mut finder = MethodFinder {
        has_index_of: false,
        has_has_own: false,
    };
    body.visit_with(&mut finder);
    finder.has_index_of && finder.has_has_own
}

fn member_prop_atom(prop: &MemberProp) -> Option<Atom> {
    match prop {
        MemberProp::Ident(id) => Some(id.sym.clone()),
        MemberProp::Computed(c) => {
            if let Expr::Lit(Lit::Str(s)) = c.expr.as_ref() {
                s.value.as_str().map(Atom::from)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn prop_name_atom(key: &PropName) -> Option<Atom> {
    match key {
        PropName::Ident(id) => Some(id.sym.clone()),
        PropName::Str(s) => s.value.as_str().map(Atom::from),
        _ => None,
    }
}

/// Find an alias name that doesn't collide with scope names or already-used aliases.
fn find_non_conflicting_alias(
    base: &str,
    scope_names: &std::collections::HashSet<Atom>,
    used_aliases: &std::collections::HashSet<Atom>,
) -> Atom {
    let base_atom = Atom::from(base);
    if !scope_names.contains(&base_atom) && !used_aliases.contains(&base_atom) {
        return base_atom;
    }
    for i in 1.. {
        let candidate = Atom::from(format!("{}_{}", base, i));
        if !scope_names.contains(&candidate) && !used_aliases.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// Collect all binding names from a list of statements (top-level idents only).
fn collect_scope_names(stmts: &[Stmt]) -> std::collections::HashSet<Atom> {
    use swc_core::ecma::visit::{Visit, VisitWith};

    struct BindingCollector {
        names: std::collections::HashSet<Atom>,
    }
    impl Visit for BindingCollector {
        fn visit_ident(&mut self, id: &Ident) {
            self.names.insert(id.sym.clone());
        }
    }
    let mut collector = BindingCollector {
        names: std::collections::HashSet::new(),
    };
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
    collector.names
}

fn collect_scope_names_module(items: &[ModuleItem]) -> std::collections::HashSet<Atom> {
    use swc_core::ecma::visit::{Visit, VisitWith};

    struct BindingCollector {
        names: std::collections::HashSet<Atom>,
    }
    impl Visit for BindingCollector {
        fn visit_ident(&mut self, id: &Ident) {
            self.names.insert(id.sym.clone());
        }
    }
    let mut collector = BindingCollector {
        names: std::collections::HashSet::new(),
    };
    for item in items {
        item.visit_with(&mut collector);
    }
    collector.names
}

/// Create a PropName — use Ident for valid JS identifiers, Str for others (e.g. "aria-current").
fn make_prop_name(name: &Atom) -> PropName {
    if is_valid_ident(name) {
        PropName::Ident(swc_core::ecma::ast::IdentName::new(name.clone(), DUMMY_SP))
    } else {
        PropName::Str(swc_core::ecma::ast::Str {
            span: DUMMY_SP,
            value: name.as_str().into(),
            raw: None,
        })
    }
}

/// Check if a string is a valid JS identifier (can be used unquoted as a property name).
fn is_valid_ident(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' && first != '$' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}
