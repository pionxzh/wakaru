use std::collections::{HashMap, HashSet};

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    Callee, Decl, Expr, ExprStmt, Ident, ImportDecl, ImportStarAsSpecifier, Lit, Module,
    ModuleDecl, ModuleItem, Pat, Stmt, VarDecl,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::helper_matcher::{
    binding_key, import_specifier_binding_key, var_declarator_binding_key,
};
use super::transpiler_helper_utils::{
    helpers_with_remaining_refs, remove_helper_declarations, BindingKey, LocalHelperContext,
    TranspilerHelperKind, TsHelperKind,
};

/// Detects and unwraps `interopRequireWildcard` helper calls.
///
/// Transforms:
///   `var _a = _interopRequireWildcard(require("a"))`
///   → `import * as _a from "a"`
///
/// Also handles the 2-arg form: `_irw(require("a"), true)` → `import * as _a from "a"`
///
/// For non-require arguments, just unwraps: `_irw(expr)` → `expr`
pub struct UnInteropRequireWildcard;

impl UnInteropRequireWildcard {
    pub(crate) fn run_with_helpers(module: &mut Module, local_helpers: &LocalHelperContext) {
        run_un_interop_require_wildcard(module, local_helpers);
    }
}

impl VisitMut for UnInteropRequireWildcard {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_interop_require_wildcard(module, &local_helpers);
    }
}

fn run_un_interop_require_wildcard(module: &mut Module, local_helpers: &LocalHelperContext) {
    let helpers = local_helpers.helpers_of_kind(TranspilerHelperKind::InteropRequireWildcard);
    let tslib_namespaces = local_helpers.tslib_namespaces();
    let has_direct_tslib_calls =
        local_helpers.has_tslib_require_member_call(TranspilerHelperKind::InteropRequireWildcard);
    local_helpers.remove_unused_inline_ts_helpers(
        module,
        &[TsHelperKind::CreateBinding, TsHelperKind::SetModuleDefault],
    );
    if helpers.is_empty() && tslib_namespaces.is_empty() && !has_direct_tslib_calls {
        return;
    }

    // Phase 1: Convert `var _a = _irw(require("path"))` → `import * as _a from "path"`
    // and unwrap non-require calls in expressions.
    let mut new_body = Vec::with_capacity(module.body.len());
    for item in module.body.drain(..) {
        if let Some(imports) = try_convert_to_namespace_import(&item, local_helpers) {
            new_body.extend(imports);
        } else {
            new_body.push(item);
        }
    }
    module.body = new_body;

    // Phase 2: Unwrap remaining call sites in expressions (non-var-decl contexts)
    let mut unwrapper = WildcardCallUnwrapper { local_helpers };
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

        if let Some(source) = extract_wildcard_require(init, local_helpers) {
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
}

impl VisitMut for WildcardCallUnwrapper<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
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

        // Only unwrap when the first arg is require("...")
        if is_require_call(&call.args[0].expr, self.local_helpers) {
            *expr = *call.args[0].expr.clone();
        }
    }
}

fn is_require_call(expr: &Expr, local_helpers: &LocalHelperContext) -> bool {
    let Expr::Call(call) = expr else { return false };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Ident(id) = callee.as_ref() else {
        return false;
    };
    local_helpers.is_unresolved_or_unguarded_ident(id, "require") && call.args.len() == 1
}

fn collect_import_dependencies(
    module: &Module,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
) -> HashSet<BindingKey> {
    let import_bindings = collect_import_bindings(module);
    if import_bindings.is_empty() {
        return HashSet::new();
    }

    let mut collector = ImportDependencyCollector {
        helpers,
        import_bindings: &import_bindings,
        dependencies: HashSet::new(),
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
        return HashSet::new();
    }

    let mut collector = VarRequireDependencyCollector {
        helpers,
        var_require_bindings: &var_require_bindings,
        dependencies: HashSet::new(),
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
    let mut bindings = HashSet::new();
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
    let mut bindings = HashSet::new();
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
