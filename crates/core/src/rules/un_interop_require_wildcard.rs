use crate::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::util::take::Take;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrowFunctionBody, AssignOp, AssignTarget, CallExpr, Callee, Decl, Expr, ExprOrSpread,
    ExprStmt, FnExpr, Ident, Import, ImportDecl, ImportStarAsSpecifier, Lit, MemberProp, Module,
    ModuleDecl, ModuleItem, Pat, ReturnStmt, SimpleAssignTarget, Stmt, UnaryOp, VarDecl,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::arrow_function::has_this_or_arguments;
use super::decl_utils::fresh_binding_ident;
use super::eval_utils::{has_dynamic_scope_construct, module_blocks_global_reference};
use super::helper_matcher::{
    binding_key, import_specifier_binding_key, var_declarator_binding_key,
};
use super::transpiler_helper_utils::{
    helpers_with_remaining_refs, remove_helper_declarations, BindingKey, LocalHelperContext,
    TranspilerHelperKind, TsHelperKind,
};
use super::un_interop_require_default::InteropScope;
use crate::js_names::{is_reserved_binding_name, is_valid_identifier_name};
use crate::utils::paren::strip_parens;

/// Detects and unwraps `interopRequireWildcard` helper calls.
///
/// Transforms:
///   `var _a = _interopRequireWildcard(require("a"))`
///   → `import * as _a from "a"`
///
/// Also handles the 2-arg form: `_irw(require("a"), true)` → `import * as _a from "a"`,
/// and a wrapped binding: `var _a = require("a"); var ns = _irw(_a)` →
/// `import * as ns from "a"` when `_a` is declared once and never written.
///
/// A top-level `target.x = _irw(require("a"))` gets its own binding:
/// `import * as x from "a"; target.x = x;`. Any other `_irw(require("a"))`
/// is unwrapped to the `require` call; other arguments are left wrapped.
///
/// Both produce ESM semantics, so they belong to a module that becomes ESM.
/// The registered rule ([`InteropScope::RuntimeHelpers`]) only drops unused
/// inline TypeScript sub-helpers; `UnEsm` runs the rest
/// ([`InteropScope::All`]) once it commits to converting the module.
pub struct UnInteropRequireWildcard;

impl UnInteropRequireWildcard {
    pub(crate) fn run_with_helpers(
        module: &mut Module,
        local_helpers: &LocalHelperContext,
        scope: InteropScope,
    ) {
        run_un_interop_require_wildcard(module, local_helpers, scope);
    }
}

impl VisitMut for UnInteropRequireWildcard {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_interop_require_wildcard(module, &local_helpers, InteropScope::All);
    }
}

fn run_un_interop_require_wildcard(
    module: &mut Module,
    local_helpers: &LocalHelperContext,
    scope: InteropScope,
) {
    let helpers = local_helpers.helpers_of_kind(TranspilerHelperKind::InteropRequireWildcard);
    let tslib_namespaces = local_helpers.tslib_namespaces();
    let has_direct_tslib_calls =
        local_helpers.has_tslib_require_member_call(TranspilerHelperKind::InteropRequireWildcard);
    local_helpers.remove_unused_inline_ts_helpers(
        module,
        &[TsHelperKind::CreateBinding, TsHelperKind::SetModuleDefault],
    );
    // No helper rule reads a namespace interop of a helper runtime module.
    if scope == InteropScope::RuntimeHelpers {
        return;
    }
    if helpers.is_empty() && tslib_namespaces.is_empty() && !has_direct_tslib_calls {
        return;
    }

    // Phase 1: Convert `var _a = _irw(require("path"))` → `import * as _a from "path"`
    // and unwrap non-require calls in expressions. A binding written anywhere
    // in the module must stay a var: an assignment to an import binding is a
    // runtime error in ESM.
    let written = collect_written_bindings(module);
    // Every identifier name in the module, so a new import binding can
    // neither collide with a binding nor capture a reference in any scope.
    let mut used_names: HashSet<Atom> = if module
        .body
        .iter()
        .any(|item| assigned_wildcard_require(item, local_helpers).is_some())
    {
        let mut names = IdentNames::default();
        module.visit_with(&mut names);
        names.0
    } else {
        HashSet::default()
    };
    let declaration_counts = top_level_declaration_counts(module);
    // `var a = require("x")` bindings declared so far, never written or
    // redeclared: wrapping one is wrapping the same cached module.
    let mut requires: HashMap<BindingKey, swc_core::ecma::ast::Str> = HashMap::default();
    let mut new_body = Vec::with_capacity(module.body.len());
    let body = std::mem::take(&mut module.body);
    for item in body {
        collect_stable_require(
            &item,
            &written,
            &declaration_counts,
            local_helpers,
            &mut requires,
        );
        if let Some(imports) =
            try_convert_to_namespace_import(&item, local_helpers, &written, &requires)
        {
            new_body.extend(imports);
        } else if let Some(source) = assigned_wildcard_require(&item, local_helpers) {
            new_body.extend(hoist_assigned_namespace(item, source, &mut used_names));
        } else {
            new_body.push(item);
        }
    }
    module.body = new_body;

    // Phase 2: Unwrap remaining call sites in expressions (non-var-decl contexts)
    let mut unwrapper = WildcardCallUnwrapper {
        local_helpers,
        restore_dynamic_imports: !module_blocks_global_reference(module, "Promise"),
    };
    module.visit_mut_with(&mut unwrapper);

    if helpers.is_empty() {
        return;
    }

    // Phase 3: Remove helper declarations only if no untransformed calls
    // remain. When the helper is removed, also clean helper-owned local and
    // import dependencies.
    let dependency_roots =
        local_helpers.helper_cleanup_candidates_with_dependencies(module, helpers);
    if dependency_roots.is_empty() {
        return;
    }

    let import_dependencies = collect_import_dependencies(module, &dependency_roots);
    let var_require_dependencies =
        collect_var_require_dependencies(module, &dependency_roots, local_helpers);
    let removable_helpers: HashMap<BindingKey, TranspilerHelperKind> = dependency_roots
        .into_iter()
        .chain(
            import_dependencies
                .iter()
                .map(|key| (key.clone(), TranspilerHelperKind::HelperDependency)),
        )
        .chain(
            var_require_dependencies
                .iter()
                .map(|key| (key.clone(), TranspilerHelperKind::HelperDependency)),
        )
        .collect();
    let remaining = helpers_with_remaining_refs(module, &removable_helpers);
    let safe_declarations: HashMap<BindingKey, TranspilerHelperKind> = removable_helpers
        .iter()
        .filter(|(key, _)| {
            !remaining.contains(*key)
                && !import_dependencies.contains(*key)
                && !var_require_dependencies.contains(*key)
        })
        .map(|(key, kind)| (key.clone(), *kind))
        .collect();
    let safe_imports: HashSet<BindingKey> = import_dependencies
        .into_iter()
        .filter(|key| !remaining.contains(key))
        .collect();
    let safe_var_requires: HashSet<BindingKey> = var_require_dependencies
        .into_iter()
        .filter(|key| !remaining.contains(key))
        .collect();

    remove_helper_declarations(&mut module.body, &safe_declarations);
    remove_var_require_bindings_preserve_side_effects(&mut module.body, &safe_var_requires);
    remove_import_bindings_preserve_side_effects(&mut module.body, &safe_imports);

    let local_helpers = LocalHelperContext::collect(module);
    local_helpers.remove_unused_inline_ts_helpers(
        module,
        &[TsHelperKind::CreateBinding, TsHelperKind::SetModuleDefault],
    );
}

/// Try to convert a `var _x = _irw(require("path"))` into `import * as _x from "path"`.
/// Returns None if the item doesn't match this pattern.
fn try_convert_to_namespace_import(
    item: &ModuleItem,
    local_helpers: &LocalHelperContext,
    written: &HashSet<BindingKey>,
    requires: &HashMap<BindingKey, swc_core::ecma::ast::Str>,
) -> Option<Vec<ModuleItem>> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };

    let mut result = Vec::new();
    let mut remaining_decls = Vec::new();
    let mut had_conversion = false;

    for decl in &var.decls {
        let Pat::Ident(bi) = &decl.name else {
            remaining_decls.push(decl.clone());
            continue;
        };
        let Some(init) = &decl.init else {
            remaining_decls.push(decl.clone());
            continue;
        };

        if written.contains(&binding_key(&bi.id)) {
            remaining_decls.push(decl.clone());
            continue;
        }

        if let Some(source) = extract_wildcard_require(init, local_helpers)
            .or_else(|| extract_wildcard_of_required(init, local_helpers, requires))
        {
            // Convert to: import * as _x from "source"
            let import = ImportDecl {
                span: DUMMY_SP,
                specifiers: vec![swc_core::ecma::ast::ImportSpecifier::Namespace(
                    ImportStarAsSpecifier {
                        span: DUMMY_SP,
                        local: Ident::new(bi.id.sym.clone(), DUMMY_SP, bi.id.ctxt),
                    },
                )],
                src: Box::new(source),
                type_only: false,
                with: None,
                phase: Default::default(),
            };
            result.push(ModuleItem::ModuleDecl(ModuleDecl::Import(import)));
            had_conversion = true;
        } else {
            remaining_decls.push(decl.clone());
        }
    }

    if !had_conversion {
        return None;
    }

    // Keep any remaining declarators in the var statement
    if !remaining_decls.is_empty() {
        let mut var = var.clone();
        var.decls = remaining_decls;
        result.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))));
    }

    Some(result)
}

/// `target.x = _irw(require("path"));` as a top-level statement: the source.
/// TypeScript and Babel lower `export * as x from "path"` to this shape.
fn assigned_wildcard_require(
    item: &ModuleItem,
    local_helpers: &LocalHelperContext,
) -> Option<swc_core::ecma::ast::Str> {
    let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign
        || !matches!(
            &assign.left,
            AssignTarget::Simple(SimpleAssignTarget::Member(_))
        )
    {
        return None;
    }
    extract_wildcard_require(&assign.right, local_helpers)
}

/// Give a namespace that is assigned to a property its own import binding:
/// `import * as x from "path"; target.x = x;`. Unwrapping the call to the
/// raw `require` instead would lose the namespace the helper builds.
fn hoist_assigned_namespace(
    item: ModuleItem,
    source: swc_core::ecma::ast::Str,
    used_names: &mut HashSet<Atom>,
) -> Vec<ModuleItem> {
    let ModuleItem::Stmt(Stmt::Expr(mut statement)) = item else {
        unreachable!("checked by assigned_wildcard_require");
    };
    let Expr::Assign(assign) = statement.expr.as_mut() else {
        unreachable!("checked by assigned_wildcard_require");
    };
    let base = match &assign.left {
        AssignTarget::Simple(SimpleAssignTarget::Member(member)) => match &member.prop {
            MemberProp::Ident(prop)
                if is_valid_identifier_name(prop.sym.as_ref())
                    && !is_reserved_binding_name(prop.sym.as_ref()) =>
            {
                prop.sym.to_string()
            }
            _ => "ns".to_string(),
        },
        _ => "ns".to_string(),
    };
    let mut name = Atom::from(base.as_str());
    let mut index = 1usize;
    while !used_names.insert(name.clone()) {
        name = Atom::from(format!("{base}_{index}"));
        index += 1;
    }
    let local = fresh_binding_ident(name, DUMMY_SP);
    *assign.right = Expr::Ident(local.clone());
    let import = ImportDecl {
        span: DUMMY_SP,
        specifiers: vec![swc_core::ecma::ast::ImportSpecifier::Namespace(
            ImportStarAsSpecifier {
                span: DUMMY_SP,
                local,
            },
        )],
        src: Box::new(source),
        type_only: false,
        with: None,
        phase: Default::default(),
    };
    vec![
        ModuleItem::ModuleDecl(ModuleDecl::Import(import)),
        ModuleItem::Stmt(Stmt::Expr(statement)),
    ]
}

/// How many times each binding is declared directly in the module body.
fn top_level_declaration_counts(module: &Module) -> HashMap<BindingKey, usize> {
    let mut counts: HashMap<BindingKey, usize> = HashMap::default();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    for id in swc_core::ecma::utils::find_pat_ids::<_, swc_core::ecma::ast::Id>(
                        &decl.name,
                    ) {
                        *counts.entry(id).or_default() += 1;
                    }
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function))) => {
                *counts.entry(binding_key(&function.ident)).or_default() += 1;
            }
            _ => {}
        }
    }
    counts
}

/// Record `var a = require("x")` when `a` is declared once and never
/// written.
fn collect_stable_require(
    item: &ModuleItem,
    written: &HashSet<BindingKey>,
    declaration_counts: &HashMap<BindingKey, usize>,
    local_helpers: &LocalHelperContext,
    requires: &mut HashMap<BindingKey, swc_core::ecma::ast::Str>,
) {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return;
    };
    for decl in &var.decls {
        let (Pat::Ident(binding), Some(init)) = (&decl.name, decl.init.as_deref()) else {
            continue;
        };
        let key = binding_key(&binding.id);
        if written.contains(&key) || declaration_counts.get(&key) != Some(&1) {
            continue;
        }
        if let Some(source) = require_source(init, local_helpers) {
            requires.insert(key, source);
        }
    }
}

/// `_irw(a)` where `a` is a recorded `var a = require("x")`: the source.
/// sucrase and rollup keep the module in its own binding and wrap that.
fn extract_wildcard_of_required(
    expr: &Expr,
    local_helpers: &LocalHelperContext,
    requires: &HashMap<BindingKey, swc_core::ecma::ast::Str>,
) -> Option<swc_core::ecma::ast::Str> {
    let Expr::Call(call) = expr else { return None };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !local_helpers.is_helper_callee(callee, TranspilerHelperKind::InteropRequireWildcard) {
        return None;
    }
    let [arg] = call.args.as_slice() else {
        return None;
    };
    if arg.spread.is_some() {
        return None;
    }
    let Expr::Ident(module) = arg.expr.as_ref() else {
        return None;
    };
    requires.get(&binding_key(module)).cloned()
}

/// `require("x")`: the source.
fn require_source(
    expr: &Expr,
    local_helpers: &LocalHelperContext,
) -> Option<swc_core::ecma::ast::Str> {
    let Expr::Call(call) = expr else { return None };
    if !is_require_call(expr, local_helpers) {
        return None;
    }
    match call.args[0].expr.as_ref() {
        Expr::Lit(Lit::Str(source)) => Some(source.clone()),
        _ => None,
    }
}

#[derive(Default)]
struct IdentNames(HashSet<Atom>);

impl Visit for IdentNames {
    fn visit_ident(&mut self, ident: &Ident) {
        self.0.insert(ident.sym.clone());
    }
}

/// Extract the require source from `_irw(require("path"))` or `_irw(require("path"), true)`.
fn extract_wildcard_require(
    expr: &Expr,
    local_helpers: &LocalHelperContext,
) -> Option<swc_core::ecma::ast::Str> {
    let Expr::Call(call) = expr else { return None };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };

    if !local_helpers.is_helper_callee(callee, TranspilerHelperKind::InteropRequireWildcard) {
        return None;
    }

    if call.args.is_empty() || call.args.len() > 2 {
        return None;
    }
    if call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }
    // The second argument is Babel's nodeInterop flag; generated call sites
    // pass a literal (or nothing). Anything else is not a recognized producer
    // shape, and dropping it could delete a side effect.
    if let Some(second) = call.args.get(1) {
        if !is_canonical_interop_flag(second.expr.as_ref()) {
            return None;
        }
    }

    // First arg must be require("path")
    let Expr::Call(require_call) = call.args[0].expr.as_ref() else {
        return None;
    };
    let Callee::Expr(require_callee) = &require_call.callee else {
        return None;
    };
    let Expr::Ident(require_id) = require_callee.as_ref() else {
        return None;
    };
    if !local_helpers.is_unresolved_or_unguarded_ident(require_id, "require")
        || require_call.args.len() != 1
        || require_call.args[0].spread.is_some()
    {
        return None;
    }
    let Expr::Lit(Lit::Str(source)) = require_call.args[0].expr.as_ref() else {
        return None;
    };

    Some(source.clone())
}

/// Unwrap remaining wildcard calls in non-var-decl contexts,
/// but only when the argument is a `require()` call.
/// Non-require arguments are left as-is because the helper synthesizes
/// a namespace object that may differ from the raw expression value.
struct WildcardCallUnwrapper<'a> {
    local_helpers: &'a LocalHelperContext,
    /// Whether `Promise` is provably the global: no `with` statement or
    /// direct `eval` can shadow it.
    restore_dynamic_imports: bool,
}

impl VisitMut for WildcardCallUnwrapper<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if self.restore_dynamic_imports {
            if let Some(import) = lowered_dynamic_import(expr, self.local_helpers) {
                *expr = import;
            }
        }
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        let Callee::Expr(callee) = &call.callee else {
            return;
        };

        if !self
            .local_helpers
            .is_helper_callee(callee, TranspilerHelperKind::InteropRequireWildcard)
        {
            return;
        }

        if call.args.is_empty() || call.args.len() > 2 {
            return;
        }
        if call.args.iter().any(|arg| arg.spread.is_some()) {
            return;
        }
        // Unwrapping drops the second argument, so it must be a canonical
        // side-effect-free interop flag.
        if let Some(second) = call.args.get(1) {
            if !is_canonical_interop_flag(second.expr.as_ref()) {
                return;
            }
        }

        // Only unwrap when the first arg is require("...")
        if is_require_call(&call.args[0].expr, self.local_helpers) {
            *expr = *call.args[0].expr.take();
        }
    }
}

/// The `import(x)` that Babel, TypeScript, swc, sucrase, and rollup lower to
/// CommonJS as a wildcard interop of a `require` deferred to a promise
/// callback:
///
/// ```js
/// Promise.resolve().then(() => _interopRequireWildcard(require("./a")))
/// Promise.resolve(spec).then((s) => _interopRequireWildcard(require(s)))
/// (s => new Promise(r => r(`${s}`)).then(s => _interopRequireWildcard(require(s))))(spec)
/// (function (t) { return Promise.resolve().then(function () { return _interopNamespaceDefault(require(t)); }); })(spec)
/// ```
///
/// A non-literal specifier is passed through the promise or a wrapper call,
/// so it is evaluated before the callback runs, like the specifier of
/// `import()`. TypeScript before 4.5 and sucrase defer it into the callback;
/// `import()` evaluates it at the call again, as the source did. Callbacks
/// may be arrows or `function` expressions; a `function` callback must not
/// give the specifier its own `this`, `arguments`, or `eval` scope. See
/// `lowered_dynamic_import_source_semantics` in
/// `docs/rewrite-assumptions.md`.
fn lowered_dynamic_import(expr: &Expr, local_helpers: &LocalHelperContext) -> Option<Expr> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let specifier = wrapped_import_specifier(call, local_helpers)
        .or_else(|| lowered_import_specifier(call, local_helpers))?;
    Some(Expr::Call(CallExpr {
        span: call.span,
        callee: Callee::Import(Import {
            span: DUMMY_SP,
            phase: Default::default(),
        }),
        args: vec![ExprOrSpread {
            spread: None,
            expr: Box::new(specifier.clone()),
        }],
        ..Default::default()
    }))
}

/// The specifier of `<promise>.then(callback)`, where the callback returns
/// the wildcard interop of `require(specifier)`.
fn lowered_import_specifier<'a>(
    then_call: &'a CallExpr,
    local_helpers: &LocalHelperContext,
) -> Option<&'a Expr> {
    let [callback] = then_call.args.as_slice() else {
        return None;
    };
    if callback.spread.is_some() {
        return None;
    }
    let passed = promise_resolution(
        member_call_object(&then_call.callee, "then")?,
        local_helpers,
    )?;
    let (params, returned, is_function) = callback_return(&callback.expr)?;
    let specifier = wildcard_require_specifier(returned, local_helpers)?;
    match (passed, params.as_slice()) {
        (None, []) => (!(is_function
            && (has_this_or_arguments(specifier) || has_dynamic_scope_construct(specifier))))
        .then_some(specifier),
        (Some(passed), [Pat::Ident(param)]) => {
            reads_binding(specifier, &param.id).then_some(passed)
        }
        _ => None,
    }
}

/// The specifier passed to a one-parameter wrapper whose body is a lowered
/// import of that parameter, or of the parameter converted to a string.
fn wrapped_import_specifier<'a>(
    call: &'a CallExpr,
    local_helpers: &LocalHelperContext,
) -> Option<&'a Expr> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let [specifier] = call.args.as_slice() else {
        return None;
    };
    let (params, Expr::Call(lowered), _) = callback_return(callee)? else {
        return None;
    };
    let [Pat::Ident(param)] = params.as_slice() else {
        return None;
    };
    let inner = lowered_import_specifier(lowered, local_helpers)?;
    let inner = match strip_parens(inner) {
        Expr::Tpl(tpl)
            if tpl.exprs.len() == 1 && tpl.quasis.iter().all(|quasi| quasi.raw.is_empty()) =>
        {
            &tpl.exprs[0]
        }
        inner => inner,
    };
    (specifier.spread.is_none() && reads_binding(inner, &param.id)).then_some(&specifier.expr)
}

/// What `Promise.resolve()`, `Promise.resolve(x)`, or `new Promise(r => r(x))`
/// resolves with: `None` for nothing, `Some(x)` for `x`.
fn promise_resolution<'a>(
    expr: &'a Expr,
    local_helpers: &LocalHelperContext,
) -> Option<Option<&'a Expr>> {
    let is_promise = |expr: &Expr| {
        matches!(strip_parens(expr), Expr::Ident(promise)
            if local_helpers.is_unresolved_or_unguarded_ident(promise, "Promise"))
    };
    match strip_parens(expr) {
        Expr::Call(call) => {
            if !is_promise(member_call_object(&call.callee, "resolve")?) {
                return None;
            }
            match call.args.as_slice() {
                [] => Some(None),
                [value] if value.spread.is_none() => Some(Some(&value.expr)),
                _ => None,
            }
        }
        Expr::New(new) if is_promise(&new.callee) => {
            let [executor] = new.args.as_deref()? else {
                return None;
            };
            let (params, Expr::Call(call), _) = callback_return(&executor.expr)? else {
                return None;
            };
            let [Pat::Ident(resolve)] = params.as_slice() else {
                return None;
            };
            let Callee::Expr(callee) = &call.callee else {
                return None;
            };
            match call.args.as_slice() {
                [value]
                    if executor.spread.is_none()
                        && value.spread.is_none()
                        && reads_binding(callee, &resolve.id) =>
                {
                    Some(Some(&value.expr))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The parameters and returned expression of a non-async, non-generator
/// arrow or function expression whose body only returns, and whether it is
/// a `function`.
fn callback_return(expr: &Expr) -> Option<(Vec<&Pat>, &Expr, bool)> {
    match strip_parens(expr) {
        Expr::Arrow(arrow) if !arrow.is_async && !arrow.is_generator => {
            let returned = match arrow.body.as_ref() {
                ArrowFunctionBody::Expr(expr) => expr.as_ref(),
                ArrowFunctionBody::FunctionBody(body) => single_return(&body.stmts)?,
            };
            Some((arrow.params.iter().collect(), strip_parens(returned), false))
        }
        Expr::Fn(FnExpr { function, .. }) if !function.is_async && !function.is_generator => {
            let returned = single_return(&function.body.as_ref()?.stmts)?;
            let params = function.params.iter().map(|param| &param.pat).collect();
            Some((params, strip_parens(returned), true))
        }
        _ => None,
    }
}

fn reads_binding(expr: &Expr, binding: &Ident) -> bool {
    matches!(strip_parens(expr), Expr::Ident(read) if read.to_id() == binding.to_id())
}

/// `obj` of a call to `obj.name(...)`.
fn member_call_object<'a>(callee: &'a Callee, name: &str) -> Option<&'a Expr> {
    let Callee::Expr(callee) = callee else {
        return None;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return None;
    };
    matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == name).then_some(&member.obj)
}

fn single_return(stmts: &[Stmt]) -> Option<&Expr> {
    match stmts {
        [Stmt::Return(ReturnStmt { arg: Some(arg), .. })] => Some(arg),
        _ => None,
    }
}

/// `x` of `_irw(require(x))`, under the same argument checks as the call
/// unwrapping.
fn wildcard_require_specifier<'a>(
    expr: &'a Expr,
    local_helpers: &LocalHelperContext,
) -> Option<&'a Expr> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !local_helpers.is_helper_callee(callee, TranspilerHelperKind::InteropRequireWildcard)
        || call.args.is_empty()
        || call.args.len() > 2
        || call.args.iter().any(|arg| arg.spread.is_some())
        || call
            .args
            .get(1)
            .is_some_and(|flag| !is_canonical_interop_flag(&flag.expr))
        || !is_require_call(&call.args[0].expr, local_helpers)
    {
        return None;
    }
    let Expr::Call(require) = call.args[0].expr.as_ref() else {
        return None;
    };
    Some(require.args[0].expr.as_ref())
}

/// The canonical shapes of Babel's second `_interopRequireWildcard` argument
/// (the nodeInterop flag) after earlier syntax rules ran: a literal
/// (UnminifyBooleans already rewrote `!0`/`!1`), or `void <literal>`.
fn is_canonical_interop_flag(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(_) => true,
        Expr::Unary(unary) if unary.op == UnaryOp::Void => {
            matches!(unary.arg.as_ref(), Expr::Lit(_))
        }
        _ => false,
    }
}

/// Collects bindings written anywhere in the module. Uses the shared
/// [`BindingUseIndex`] direct-write pass — it already covers parenthesized
/// assignment targets, update operands, destructuring targets, and for-in/of
/// heads, which a bespoke collector here would have to re-enumerate.
fn collect_written_bindings(module: &Module) -> HashSet<BindingKey> {
    crate::analysis::binding_uses::BindingUseIndex::collect_direct_write_bindings(module)
}

fn is_require_call(expr: &Expr, local_helpers: &LocalHelperContext) -> bool {
    let Expr::Call(call) = expr else { return false };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Ident(id) = callee.as_ref() else {
        return false;
    };
    local_helpers.is_unresolved_or_unguarded_ident(id, "require")
        && call.args.len() == 1
        && call.args[0].spread.is_none()
}

fn collect_import_dependencies(
    module: &Module,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
) -> HashSet<BindingKey> {
    let import_bindings = collect_import_bindings(module);
    if import_bindings.is_empty() {
        return HashSet::default();
    }

    let mut collector = ImportDependencyCollector {
        helpers,
        import_bindings: &import_bindings,
        dependencies: HashSet::default(),
    };
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl)))
                if helpers.contains_key(&binding_key(&fn_decl.ident)) =>
            {
                fn_decl.function.visit_with(&mut collector);
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    if var_declarator_binding_key(decl)
                        .as_ref()
                        .is_some_and(|key| helpers.contains_key(key))
                    {
                        if let Some(init) = &decl.init {
                            init.visit_with(&mut collector);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    collector.dependencies
}

fn collect_var_require_dependencies(
    module: &Module,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    local_helpers: &LocalHelperContext,
) -> HashSet<BindingKey> {
    let var_require_bindings = collect_var_require_bindings(module, local_helpers);
    if var_require_bindings.is_empty() {
        return HashSet::default();
    }

    let mut collector = VarRequireDependencyCollector {
        helpers,
        var_require_bindings: &var_require_bindings,
        dependencies: HashSet::default(),
    };
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl)))
                if helpers.contains_key(&binding_key(&fn_decl.ident)) =>
            {
                fn_decl.function.visit_with(&mut collector);
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    if var_declarator_binding_key(decl)
                        .as_ref()
                        .is_some_and(|key| helpers.contains_key(key))
                    {
                        if let Some(init) = &decl.init {
                            init.visit_with(&mut collector);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    collector.dependencies
}

fn collect_var_require_bindings(
    module: &Module,
    local_helpers: &LocalHelperContext,
) -> HashSet<BindingKey> {
    let mut bindings = HashSet::default();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            if decl
                .init
                .as_deref()
                .is_some_and(|init| is_require_call(init, local_helpers))
            {
                if let Some(key) = var_declarator_binding_key(decl) {
                    bindings.insert(key);
                }
            }
        }
    }
    bindings
}

fn collect_import_bindings(module: &Module) -> HashSet<BindingKey> {
    let mut bindings = HashSet::default();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        bindings.extend(import.specifiers.iter().map(import_specifier_binding_key));
    }
    bindings
}

struct ImportDependencyCollector<'a> {
    helpers: &'a HashMap<BindingKey, TranspilerHelperKind>,
    import_bindings: &'a HashSet<BindingKey>,
    dependencies: HashSet<BindingKey>,
}

impl Visit for ImportDependencyCollector<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        let key = binding_key(ident);
        if self.import_bindings.contains(&key) && !self.helpers.contains_key(&key) {
            self.dependencies.insert(key);
        }
    }
}

struct VarRequireDependencyCollector<'a> {
    helpers: &'a HashMap<BindingKey, TranspilerHelperKind>,
    var_require_bindings: &'a HashSet<BindingKey>,
    dependencies: HashSet<BindingKey>,
}

impl Visit for VarRequireDependencyCollector<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        let key = binding_key(ident);
        if self.var_require_bindings.contains(&key) && !self.helpers.contains_key(&key) {
            self.dependencies.insert(key);
        }
    }
}

fn remove_var_require_bindings_preserve_side_effects(
    body: &mut Vec<ModuleItem>,
    removable: &HashSet<BindingKey>,
) {
    if removable.is_empty() {
        return;
    }

    let mut new_body = Vec::with_capacity(body.len());
    for item in body.drain(..) {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            new_body.push(item);
            continue;
        };

        let mut original_var = *var;
        let decls = std::mem::take(&mut original_var.decls);
        let mut pending_decls = Vec::new();
        let mut changed = false;
        for decl in decls {
            if var_declarator_binding_key(&decl).is_some_and(|key| removable.contains(&key)) {
                changed = true;
                push_var_decl(&mut new_body, &original_var, &mut pending_decls);
                if let Some(init) = decl.init {
                    new_body.push(ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                        span: DUMMY_SP,
                        expr: init,
                    })));
                }
            } else {
                pending_decls.push(decl);
            }
        }

        if changed {
            push_var_decl(&mut new_body, &original_var, &mut pending_decls);
        } else {
            original_var.decls = pending_decls;
            new_body.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(
                original_var,
            )))));
        }
    }
    *body = new_body;
}

fn push_var_decl(
    body: &mut Vec<ModuleItem>,
    original: &VarDecl,
    decls: &mut Vec<swc_core::ecma::ast::VarDeclarator>,
) {
    if decls.is_empty() {
        return;
    }
    let mut var = original.clone();
    var.decls = std::mem::take(decls);
    body.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(var)))));
}

fn remove_import_bindings_preserve_side_effects(
    body: &mut [ModuleItem],
    removable: &HashSet<BindingKey>,
) {
    if removable.is_empty() {
        return;
    }

    for item in body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        import
            .specifiers
            .retain(|specifier| !removable.contains(&import_specifier_binding_key(specifier)));
    }
}
