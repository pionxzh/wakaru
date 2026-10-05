//! Export getter helpers lowered to per-name getter definitions.
//!
//! Some producers define every export getter through a helper call instead of
//! one `Object.defineProperty(exports, ...)` per name:
//!
//! ```text
//! // swc: one getter per export, from an object of getters or functions
//! function _export(target, all) {
//!   for (var name in all) Object.defineProperty(target, name, {
//!     enumerable: true,
//!     get: Object.getOwnPropertyDescriptor(all, name).get, // swc 1.16
//!     // get: all[name],                                     // older swc
//!   });
//! }
//! _export(exports, { get count() { return count; } });
//!
//! // esbuild: getters on a namespace object that becomes `module.exports`
//! var __export = (target, all) => {
//!   for (var name in all) __defProp(target, name, { get: all[name], enumerable: true });
//! };
//! var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);
//! var mod_exports = {};
//! __export(mod_exports, { count: () => count });
//! module.exports = __toCommonJS(mod_exports);
//!
//! // sucrase: one helper call per re-exported name
//! function _createNamedExportFrom(obj, localName, importedName) {
//!   Object.defineProperty(exports, localName, { enumerable: true, configurable: true, get: () => obj[importedName] });
//! }
//! _createNamedExportFrom(_dep, "count", "count");
//! ```
//!
//! A minifier inlines the swc helper into the module, as a loop over the
//! getter object or as a call of the helper's function in place:
//!
//! ```text
//! var all = { get count() { return count; } };
//! for (var name in all) Object.defineProperty(exports, name, { enumerable: !0, get: GETTER });
//! !function (target, all) { for (var name in all) ... }(exports, { ... });
//! ```
//!
//! Each call becomes the getter definitions it performs, written as
//! `Object.defineProperty(exports, "name", { enumerable: true, get })`. The
//! export-storage analysis classifies those names as getters. The storage
//! rewrite exports a getter of a local binding live; the statement path
//! re-exports a getter of an imported member. The result is still the same
//! CommonJS module, so a module that later keeps its CommonJS boundary loses
//! nothing.
//!
//! A helper is recognized by its body, not by its name. Its binding must have
//! one declaration and no write.
//!
//! webpack's runtime `require.d` has no body in an unpacked module; its
//! semantics come from the runtime. [`lower_webpack_export_definitions`]
//! lowers every form a top-level call takes:
//!
//! ```text
//! require.d(exports, { x: () => x });            // object of getters
//! require.d(exports, "x", function () { return x; }); // webpack 4, one name
//! require.d(exports, ["c", 0, c, "x", () => x]); // webpack 5.108+: a `0` slot
//!                                                // is followed by a value
//! require.d(exports, { x: () => x }, { c });     // rspack: getters, values
//! ((target, getters) => {                        // the getter loop inlined
//!   for (const key in getters)
//!     Object.defineProperty(target, key, { enumerable: true, get: getters[key] });
//! })(exports, { x: () => x });
//! ```
//!
//! A value is a data property read when the call runs, so it becomes
//! `exports.c = c` at the call's position. `UnEsm` restores the calls as
//! written when the module stays CommonJS.

use crate::analysis::binding_uses::BindingUseIndex;
use crate::rules::helper_matcher::{
    binding_key, count_binding_refs, removable_without_remaining_refs,
    remove_fn_decls_from_body_by_binding, remove_var_declarators_by_binding, BindingKey,
};

use swc_core::ecma::ast::{
    BinExpr, ComputedPropName, ExprOrSpread, FnExpr, ForOfStmt, KeyValueProp, MethodProp, ObjectLit,
};

use super::*;

/// Lower every recognized export getter helper call to per-name getter
/// definitions, and remove the helpers once nothing else refers to them.
/// Returns whether an esbuild `__toCommonJS` namespace was lowered, which
/// only a module compiled from ESM has.
pub(crate) fn lower_export_getter_helpers(module: &mut Module, unresolved_mark: Mark) -> bool {
    if !module
        .body
        .iter()
        .any(|item| is_helper_call_candidate(item, unresolved_mark))
    {
        return false;
    }
    let uses = BindingUseIndex::collect(module);
    let helpers = GetterHelpers::collect(module, &uses, unresolved_mark);

    let mut lowered: HashMap<usize, Vec<ModuleItem>> = HashMap::default();
    let mut removed: HashSet<usize> = HashSet::default();
    let mut consumed: HashSet<BindingKey> = HashSet::default();
    for (index, item) in module.body.iter().enumerate() {
        if let Some(definitions) = lower_inline_getter_map_call(item, unresolved_mark) {
            lowered.insert(index, definitions);
            continue;
        }
        if let Some(definitions) = index.checked_sub(1).and_then(|previous| {
            lower_getter_map_loop(&module.body[previous], item, &uses, unresolved_mark)
        }) {
            removed.insert(index - 1);
            lowered.insert(index, definitions);
            continue;
        }
        if helpers.is_empty() {
            continue;
        }
        if let Some((callee, definitions)) = helpers.lower_exports_call(item, &uses) {
            consumed.insert(callee);
            lowered.insert(index, definitions);
        } else if let Some((callee, import)) = helpers.lower_to_esm(item, &uses) {
            consumed.insert(callee);
            lowered.insert(index, vec![import]);
        }
    }
    let namespace = helpers.lower_namespace_exports(module, &uses);
    let lowered_namespace = namespace.is_some();
    if let Some(namespace) = namespace {
        consumed.extend(namespace.consumed);
        removed.extend(namespace.removed);
        lowered.insert(namespace.module_exports_index, namespace.definitions);
        for (index, item) in namespace.rewritten {
            lowered.insert(index, vec![item]);
        }
    }
    if lowered.is_empty() {
        return false;
    }

    let body = std::mem::take(&mut module.body);
    for (index, item) in body.into_iter().enumerate() {
        if removed.contains(&index) {
            continue;
        }
        match lowered.remove(&index) {
            Some(definitions) => module.body.extend(definitions),
            None => module.body.push(item),
        }
    }
    // The helpers a consumed helper calls go with it once unused.
    consumed.extend(helpers.dependencies.iter().cloned());
    let removable = removable_without_remaining_refs(&*module, &consumed);
    if !removable.is_empty() {
        remove_var_declarators_by_binding(&mut module.body, &removable);
        remove_fn_decls_from_body_by_binding(&mut module.body, &removable);
    }
    lowered_namespace
}

/// A top-level statement shaped like a helper call: `f(exports, { ... })`,
/// `f(binding, "a", "b")`, or `module.exports = f(binding)`. Most modules
/// have none, and then no binding analysis runs.
fn is_helper_call_candidate(item: &ModuleItem, unresolved_mark: Mark) -> bool {
    if to_esm_declarator(item, unresolved_mark).is_some() {
        return true;
    }
    let statement = match item {
        ModuleItem::Stmt(Stmt::Expr(statement)) => statement,
        ModuleItem::Stmt(Stmt::ForIn(_)) => return true,
        _ => return false,
    };
    let ident_callee = |call: &CallExpr| matches!(&call.callee, Callee::Expr(callee) if matches!(strip_parens(callee), Expr::Ident(_)));
    if discarded_call(&statement.expr).is_some_and(|call| inline_helper_function(call).is_some()) {
        return true;
    }
    match strip_parens(&statement.expr) {
        Expr::Call(call) if ident_callee(call) => {
            if let Some([target, entries]) = plain_args(&call.args) {
                return matches!(strip_parens(target), Expr::Ident(_))
                    && matches!(strip_parens(entries), Expr::Object(_));
            }
            if let Some([source, local, imported]) = plain_args(&call.args) {
                return matches!(strip_parens(source), Expr::Ident(_))
                    && string_value(local).is_some()
                    && string_value(imported).is_some();
            }
            false
        }
        Expr::Assign(assign) => {
            matches!(&assign.left, AssignTarget::Simple(SimpleAssignTarget::Member(member))
                if is_module_exports_expr(&Expr::Member(member.clone()), unresolved_mark))
                && matches!(strip_parens(&assign.right), Expr::Call(call)
                    if ident_callee(call)
                        && matches!(plain_args(&call.args), Some([namespace]) if matches!(strip_parens(namespace), Expr::Ident(_))))
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Helper identities
// ---------------------------------------------------------------------------

/// How a getter-map helper reads the getter of each entry.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GetterMap {
    /// `get: all[name]`: each entry's value is the getter function.
    Values,
    /// `get: Object.getOwnPropertyDescriptor(all, name).get`: each entry is
    /// itself a getter.
    Accessors,
}

struct GetterHelpers {
    unresolved_mark: Mark,
    /// `(target, all)` helpers that define one getter on `target` per entry
    /// of `all` (swc `_export`, esbuild `__export`).
    getter_maps: HashMap<BindingKey, GetterMap>,
    /// esbuild `__toCommonJS`: a fresh object with an `__esModule` marker and
    /// a getter for each own property of its argument.
    to_common_js: HashSet<BindingKey>,
    /// sucrase `_createNamedExportFrom(obj, localName, importedName)`.
    named_export_from: HashSet<BindingKey>,
    /// esbuild `__reExport(target, mod, secondTarget)`: copies every key of
    /// `mod` except `default` that the targets do not own yet.
    pub(super) re_export: HashSet<BindingKey>,
    /// esbuild `__toESM(mod)`: a namespace object for a required module,
    /// with `default` set to the module unless it is marked `__esModule`.
    to_esm: HashSet<BindingKey>,
    /// Bindings that only the helpers above use: esbuild `__copyProps` and
    /// `__hasOwnProp`.
    dependencies: HashSet<BindingKey>,
}

impl GetterHelpers {
    fn collect(module: &Module, uses: &BindingUseIndex, unresolved_mark: Mark) -> Self {
        let mut helpers = Self {
            unresolved_mark,
            getter_maps: HashMap::default(),
            to_common_js: HashSet::default(),
            named_export_from: HashSet::default(),
            re_export: HashSet::default(),
            to_esm: HashSet::default(),
            dependencies: HashSet::default(),
        };
        let mut functions: Vec<(&Ident, HelperFunction)> = Vec::new();
        let mut has_own_aliases: HashSet<BindingKey> = HashSet::default();
        for item in &module.body {
            match item {
                ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
                    functions.push((&fn_decl.ident, HelperFunction::Function(&fn_decl.function)));
                }
                ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                    for decl in &var.decls {
                        let (Pat::Ident(binding), Some(init)) = (&decl.name, decl.init.as_deref())
                        else {
                            continue;
                        };
                        match strip_parens(init) {
                            Expr::Fn(FnExpr { function, .. }) => {
                                functions.push((&binding.id, HelperFunction::Function(function)));
                            }
                            Expr::Arrow(arrow) => {
                                functions.push((&binding.id, HelperFunction::Arrow(arrow)));
                            }
                            init if is_has_own_property(init, unresolved_mark) => {
                                has_own_aliases.insert(binding_key(&binding.id));
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        let stable =
            |key: &BindingKey| uses.has_single_declaration(key) && !uses.has_direct_write(key);
        let has_own_aliases: HashSet<BindingKey> = has_own_aliases
            .into_iter()
            .filter(|key| stable(key))
            .collect();

        let mut copy_props: HashSet<BindingKey> = HashSet::default();
        for (ident, function) in &functions {
            let key = binding_key(ident);
            if !stable(&key) {
                continue;
            }
            if let Some(map) = getter_map_helper(function, unresolved_mark) {
                helpers.getter_maps.insert(key.clone(), map);
            }
            if is_named_export_from_helper(function, unresolved_mark) {
                helpers.named_export_from.insert(key.clone());
            }
            if let Some(has_own) = copy_props_helper(function, &has_own_aliases, unresolved_mark) {
                copy_props.insert(key.clone());
                helpers.dependencies.insert(key);
                helpers.dependencies.extend(has_own);
            }
        }
        for (ident, function) in &functions {
            let key = binding_key(ident);
            if !stable(&key) {
                continue;
            }
            if is_to_common_js_helper(function, &copy_props, unresolved_mark) {
                helpers.to_common_js.insert(key);
            } else if is_re_export_helper(function, &copy_props) {
                helpers.re_export.insert(key);
            } else if is_to_esm_helper(function, &copy_props, unresolved_mark) {
                helpers.to_esm.insert(key);
            }
        }
        helpers
    }

    fn is_empty(&self) -> bool {
        self.getter_maps.is_empty() && self.named_export_from.is_empty() && self.to_esm.is_empty()
    }

    /// `var ns = __toESM(require("x"));` becomes `import * as ns from "x"`,
    /// as Babel's `_interopRequireWildcard(require("x"))` does. The
    /// two-argument form (`isNodeMode`) always sets `default` to the module,
    /// which a namespace import of a module marked `__esModule` would not.
    fn lower_to_esm(
        &self,
        item: &ModuleItem,
        uses: &BindingUseIndex,
    ) -> Option<(BindingKey, ModuleItem)> {
        let (binding, callee, source) = to_esm_declarator(item, self.unresolved_mark)?;
        let key = binding_key(binding);
        if !self.to_esm.contains(&callee)
            || !uses.has_single_declaration(&key)
            || uses.has_direct_write(&key)
        {
            return None;
        }
        let import = ImportDecl {
            span: module_item_span(item),
            specifiers: vec![ImportSpecifier::Namespace(ImportStarAsSpecifier {
                span: DUMMY_SP,
                local: binding.clone(),
            })],
            src: Box::new(make_str(&source)),
            type_only: false,
            with: None,
            phase: Default::default(),
        };
        Some((callee, ModuleItem::ModuleDecl(ModuleDecl::Import(import))))
    }

    /// `_export(exports, { ... })` or `_createNamedExportFrom(dep, "a", "b")`
    /// as a top-level statement: the helper and the definitions it performs.
    fn lower_exports_call(
        &self,
        item: &ModuleItem,
        uses: &BindingUseIndex,
    ) -> Option<(BindingKey, Vec<ModuleItem>)> {
        let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
            return None;
        };
        let Expr::Call(call) = strip_parens(&statement.expr) else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Ident(callee) = strip_parens(callee) else {
            return None;
        };
        let callee = binding_key(callee);
        let unresolved_mark = self.unresolved_mark;
        if let Some(map) = self.getter_maps.get(&callee) {
            let [target, entries] = plain_args(&call.args)?;
            if !matches!(strip_parens(target), Expr::Ident(id)
                if is_unresolved_ident(id, "exports", unresolved_mark))
            {
                return None;
            }
            let Expr::Object(entries) = strip_parens(entries) else {
                return None;
            };
            let getters = getter_map_entries(entries, *map)?;
            let definitions = getters
                .into_iter()
                .map(|(name, getter)| {
                    getter_definition(statement.span, &name, getter, false, unresolved_mark)
                })
                .collect();
            return Some((callee, definitions));
        }
        if self.named_export_from.contains(&callee) {
            let [source, local, imported] = plain_args(&call.args)?;
            // The helper reads `obj[importedName]` from the value it received;
            // a getter reading the binding sees the same object only while
            // the binding is never written.
            let Expr::Ident(source) = strip_parens(source) else {
                return None;
            };
            if uses.has_direct_write(&binding_key(source))
                || !uses.has_declaration(&binding_key(source))
            {
                return None;
            }
            let (Some(local), Some(imported)) = (string_value(local), string_value(imported))
            else {
                return None;
            };
            if is_prototype_mutating_member_name(local.as_ref()) {
                return None;
            }
            let getter = Box::new(Expr::Arrow(ArrowExpr {
                span: DUMMY_SP,
                ctxt: Default::default(),
                params: Vec::new(),
                body: Box::new(ArrowFunctionBody::Expr(Box::new(member_read(
                    source.clone(),
                    &imported,
                )))),
                is_async: false,
                is_generator: false,
                type_params: None,
                return_type: None,
            }));
            let definition =
                getter_definition(statement.span, &local, getter, true, unresolved_mark);
            return Some((callee, vec![definition]));
        }
        None
    }

    /// esbuild's namespace object:
    ///
    /// ```text
    /// var mod_exports = {};
    /// __export(mod_exports, { ... });
    /// module.exports = __toCommonJS(mod_exports);
    /// ```
    ///
    /// `module.exports` becomes a fresh object with an `__esModule` marker and
    /// one getter per entry, each reading the namespace's getter. When the
    /// module refers to neither `module` nor `exports` anywhere else, nothing
    /// can observe which object holds the getters, so they are defined on
    /// `exports` directly.
    fn lower_namespace_exports(
        &self,
        module: &Module,
        uses: &BindingUseIndex,
    ) -> Option<LoweredNamespace> {
        let unresolved_mark = self.unresolved_mark;
        if self.to_common_js.is_empty() {
            return None;
        }
        let (module_exports_index, to_common_js, namespace) =
            module.body.iter().enumerate().find_map(|(index, item)| {
                let (callee, namespace) =
                    self.module_exports_to_common_js(item, unresolved_mark)?;
                Some((index, callee, namespace))
            })?;
        let namespace_key = binding_key(&namespace);
        let exports_key = binding_key(&make_unresolved_ident("exports".into(), unresolved_mark));
        let module_key = binding_key(&make_unresolved_ident("module".into(), unresolved_mark));
        // `__reExport(namespace, require("x"), module.exports)` after the
        // assignment copies into both objects; `export *` from the namespace
        // is only observable through `module.exports`.
        let re_exports: Vec<(usize, &CallExpr)> = module.body[module_exports_index + 1..]
            .iter()
            .enumerate()
            .filter_map(|(offset, item)| {
                let call = self.namespace_re_export(item, &namespace)?;
                Some((module_exports_index + 1 + offset, call))
            })
            .collect();
        if count_binding_refs(module, &exports_key) != 0
            || count_binding_refs(module, &module_key) != 1 + re_exports.len()
            || !uses.has_single_declaration(&namespace_key)
            || uses.has_direct_write(&namespace_key)
            || uses.use_count(&namespace_key) != 2 + re_exports.len()
        {
            return None;
        }

        // `var mod_exports = {};` and one getter-map call on it, both before
        // the `module.exports` assignment.
        let mut declaration_index = None;
        let mut definition = None;
        for (index, item) in module.body[..module_exports_index].iter().enumerate() {
            if is_empty_object_declaration(item, &namespace) {
                declaration_index = Some(index);
                continue;
            }
            if let Some((callee, getters)) = self.namespace_getter_call(item, &namespace) {
                if definition.is_some() {
                    return None;
                }
                definition = Some((index, callee, getters));
            }
        }
        let declaration_index = declaration_index?;
        let (definition_index, getter_map, getters) = definition?;
        if definition_index < declaration_index {
            return None;
        }

        let span = module_item_span(&module.body[module_exports_index]);
        let mut definitions = vec![esmodule_marker(span, unresolved_mark)];
        definitions.extend(
            getters.into_iter().map(|(name, getter)| {
                getter_definition(span, &name, getter, false, unresolved_mark)
            }),
        );
        // The copy into the namespace is unobservable once it is gone, and
        // `module.exports` is `exports` again: `__reExport(exports, mod)`.
        let rewritten = re_exports
            .into_iter()
            .map(|(index, call)| {
                let mut call = call.clone();
                call.args.truncate(2);
                call.args[0] = exports_ident(unresolved_mark).as_arg();
                let item = ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                    span: module_item_span(&module.body[index]),
                    expr: Box::new(Expr::Call(call)),
                }));
                (index, item)
            })
            .collect();
        Some(LoweredNamespace {
            module_exports_index,
            definitions,
            removed: vec![declaration_index, definition_index],
            consumed: vec![getter_map, to_common_js],
            rewritten,
        })
    }

    /// `__reExport(namespace, require("x"), module.exports);`
    fn namespace_re_export<'a>(
        &self,
        item: &'a ModuleItem,
        namespace: &Ident,
    ) -> Option<&'a CallExpr> {
        let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
            return None;
        };
        let Expr::Call(call) = strip_parens(&statement.expr) else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Ident(callee) = strip_parens(callee) else {
            return None;
        };
        if !self.re_export.contains(&binding_key(callee)) {
            return None;
        }
        let [target, source, second] = plain_args(&call.args)?;
        (is_ident_expr(target, namespace)
            && matches!(strip_parens(source), Expr::Call(require)
                if is_require_call(require, self.unresolved_mark).is_some())
            && is_module_exports_expr(strip_parens(second), self.unresolved_mark))
        .then_some(call)
    }

    /// `module.exports = __toCommonJS(namespace);`
    fn module_exports_to_common_js(
        &self,
        item: &ModuleItem,
        unresolved_mark: Mark,
    ) -> Option<(BindingKey, Ident)> {
        let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
            return None;
        };
        let Expr::Assign(assign) = strip_parens(&statement.expr) else {
            return None;
        };
        if assign.op != AssignOp::Assign {
            return None;
        }
        let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assign.left else {
            return None;
        };
        if !is_module_exports_expr(&Expr::Member(target.clone()), unresolved_mark) {
            return None;
        }
        let Expr::Call(call) = strip_parens(&assign.right) else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Ident(callee) = strip_parens(callee) else {
            return None;
        };
        let callee = binding_key(callee);
        if !self.to_common_js.contains(&callee) {
            return None;
        }
        let [namespace] = plain_args(&call.args)?;
        let Expr::Ident(namespace) = strip_parens(namespace) else {
            return None;
        };
        Some((callee, namespace.clone()))
    }

    /// `__export(namespace, { ... });`
    fn namespace_getter_call(
        &self,
        item: &ModuleItem,
        namespace: &Ident,
    ) -> Option<(BindingKey, Vec<(Atom, Box<Expr>)>)> {
        let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
            return None;
        };
        let Expr::Call(call) = strip_parens(&statement.expr) else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Ident(callee) = strip_parens(callee) else {
            return None;
        };
        let callee = binding_key(callee);
        let map = self.getter_maps.get(&callee)?;
        let [target, entries] = plain_args(&call.args)?;
        if !matches!(strip_parens(target), Expr::Ident(id) if same_ident(id, namespace)) {
            return None;
        }
        let Expr::Object(entries) = strip_parens(entries) else {
            return None;
        };
        Some((callee, getter_map_entries(entries, *map)?))
    }
}

struct LoweredNamespace {
    module_exports_index: usize,
    definitions: Vec<ModuleItem>,
    removed: Vec<usize>,
    consumed: Vec<BindingKey>,
    /// Re-export calls rewritten to copy into `exports`, by body index.
    rewritten: Vec<(usize, ModuleItem)>,
}

/// The esbuild `__reExport` helpers of a module, for the export-star
/// recovery: `__reExport(exports, require("x"))` is `export * from "x"`.
pub(super) fn esbuild_re_export_helpers(
    module: &Module,
    unresolved_mark: Mark,
) -> HashSet<BindingKey> {
    let uses = BindingUseIndex::collect(module);
    GetterHelpers::collect(module, &uses, unresolved_mark).re_export
}

/// `var binding = CALLEE(require("x"));` as the only declarator: the
/// binding, the callee, and the source.
fn to_esm_declarator(
    item: &ModuleItem,
    unresolved_mark: Mark,
) -> Option<(&Ident, BindingKey, String)> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };
    let [VarDeclarator {
        name: Pat::Ident(binding),
        init: Some(init),
        ..
    }] = var.decls.as_slice()
    else {
        return None;
    };
    let Expr::Call(call) = strip_parens(init) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(callee) = strip_parens(callee) else {
        return None;
    };
    let [module] = plain_args(&call.args)?;
    let Expr::Call(require) = strip_parens(module) else {
        return None;
    };
    let source = is_require_call(require, unresolved_mark)?;
    Some((&binding.id, binding_key(callee), source))
}

#[derive(Clone, Copy)]
enum HelperFunction<'a> {
    Function(&'a Function),
    Arrow(&'a ArrowExpr),
}

impl<'a> HelperFunction<'a> {
    fn params(self) -> Option<Vec<&'a Ident>> {
        match self {
            Self::Function(function) => {
                if function.is_async || function.is_generator {
                    return None;
                }
                function
                    .params
                    .iter()
                    .map(|param| match &param.pat {
                        Pat::Ident(binding) => Some(&binding.id),
                        _ => None,
                    })
                    .collect()
            }
            Self::Arrow(arrow) => {
                if arrow.is_async || arrow.is_generator {
                    return None;
                }
                arrow
                    .params
                    .iter()
                    .map(|param| match param {
                        Pat::Ident(binding) => Some(&binding.id),
                        _ => None,
                    })
                    .collect()
            }
        }
    }

    /// The body statements, or `None` for an expression-bodied arrow.
    fn stmts(self) -> Option<&'a [Stmt]> {
        match self {
            Self::Function(function) => Some(function.body.as_ref()?.stmts.as_slice()),
            Self::Arrow(arrow) => match arrow.body.as_ref() {
                ArrowFunctionBody::FunctionBody(body) => Some(body.stmts.as_slice()),
                ArrowFunctionBody::Expr(_) => None,
            },
        }
    }

    /// The body as the expressions it evaluates, the last one returned:
    /// `(a, b)` as an arrow body, or `{ a; return b; }` once
    /// `SimplifySequence` has split it.
    fn effects(self) -> Option<Vec<&'a Expr>> {
        let mut effects = Vec::new();
        let returned = match self {
            Self::Arrow(ArrowExpr { body, .. })
                if matches!(body.as_ref(), ArrowFunctionBody::Expr(_)) =>
            {
                self.returned_expr()?
            }
            _ => {
                let (last, init) = self.stmts()?.split_last()?;
                for stmt in init {
                    let Stmt::Expr(statement) = stmt else {
                        return None;
                    };
                    effects.push(statement.expr.as_ref());
                }
                let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = last else {
                    return None;
                };
                arg.as_ref()
            }
        };
        match strip_parens(returned) {
            Expr::Seq(seq) => effects.extend(seq.exprs.iter().map(|expr| expr.as_ref())),
            expr => effects.push(expr),
        }
        Some(effects)
    }

    /// The returned expression of an expression-bodied arrow or a function
    /// whose body is one `return`.
    fn returned_expr(self) -> Option<&'a Expr> {
        match self {
            Self::Arrow(arrow) => match arrow.body.as_ref() {
                ArrowFunctionBody::Expr(expr) => Some(expr),
                ArrowFunctionBody::FunctionBody(body) => single_return(&body.stmts),
            },
            Self::Function(function) => single_return(&function.body.as_ref()?.stmts),
        }
    }
}

fn single_return(stmts: &[Stmt]) -> Option<&Expr> {
    match stmts {
        [Stmt::Return(ReturnStmt { arg: Some(arg), .. })] => Some(arg),
        _ => None,
    }
}

/// The only statement of a block or a bare statement.
fn single_stmt(stmt: &Stmt) -> Option<&Stmt> {
    match stmt {
        Stmt::Block(block) => match block.stmts.as_slice() {
            [stmt] => Some(stmt),
            _ => None,
        },
        stmt => Some(stmt),
    }
}

fn call_stmt(stmt: &Stmt) -> Option<&CallExpr> {
    let Stmt::Expr(statement) = single_stmt(stmt)? else {
        return None;
    };
    match strip_parens(&statement.expr) {
        Expr::Call(call) => Some(call),
        _ => None,
    }
}

fn plain_args<const N: usize>(args: &[ExprOrSpread]) -> Option<[&Expr; N]> {
    if args.len() != N || args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }
    let args: Vec<&Expr> = args.iter().map(|arg| arg.expr.as_ref()).collect();
    args.try_into().ok()
}

fn is_ident_expr(expr: &Expr, ident: &Ident) -> bool {
    matches!(strip_parens(expr), Expr::Ident(id) if same_ident(id, ident))
}

/// `object[key]` with both identifiers given.
fn is_computed_lookup(expr: &Expr, object: &Ident, key: &Ident) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    is_ident_expr(&member.obj, object)
        && matches!(&member.prop, MemberProp::Computed(ComputedPropName { expr, .. })
            if is_ident_expr(expr, key))
}

fn is_bool_lit(expr: &Expr, value: bool) -> bool {
    matches!(strip_parens(expr), Expr::Lit(Lit::Bool(lit)) if lit.value == value)
}

/// The key-value entries of a property descriptor literal, by name. `None`
/// for any other kind of entry, a computed or repeated key.
fn descriptor_entries(expr: &Expr) -> Option<HashMap<String, &Expr>> {
    let Expr::Object(object) = strip_parens(expr) else {
        return None;
    };
    let mut entries = HashMap::default();
    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(KeyValueProp { key, value }) = prop.as_ref() else {
            return None;
        };
        if entries
            .insert(prop_name_as_atom(key)?.to_string(), value.as_ref())
            .is_some()
        {
            return None;
        }
    }
    Some(entries)
}

/// `Object.prototype.hasOwnProperty`.
fn is_has_own_property(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    is_unresolved_member_expr(&member.obj, "Object", "prototype", unresolved_mark)
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "hasOwnProperty")
}

/// `(target, all) => { for (name in all) Object.defineProperty(target, name,
/// { enumerable: true, get: GETTER }); }`, where GETTER is `all[name]` or
/// `Object.getOwnPropertyDescriptor(all, name).get`.
fn getter_map_helper(function: &HelperFunction, unresolved_mark: Mark) -> Option<GetterMap> {
    let [target, all] = function.params()?.try_into().ok()?;
    let [Stmt::ForIn(for_in)] = function.stmts()? else {
        return None;
    };
    getter_map_loop(
        for_in,
        |defined_target| is_ident_expr(defined_target, target),
        all,
        unresolved_mark,
    )
}

/// `for (name in all) Object.defineProperty(TARGET, name, { enumerable: true,
/// get: GETTER })`, where GETTER is `all[name]` or
/// `Object.getOwnPropertyDescriptor(all, name).get`.
fn getter_map_loop(
    for_in: &ForInStmt,
    is_target: impl Fn(&Expr) -> bool,
    all: &Ident,
    unresolved_mark: Mark,
) -> Option<GetterMap> {
    if !is_ident_expr(&for_in.right, all) {
        return None;
    }
    let name = match &for_in.left {
        ForHead::VarDecl(var) => match var.decls.as_slice() {
            [VarDeclarator {
                name: Pat::Ident(binding),
                init: None,
                ..
            }] => &binding.id,
            _ => return None,
        },
        _ => return None,
    };
    let call = call_stmt(&for_in.body)?;
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !is_unresolved_member_expr(callee, "Object", "defineProperty", unresolved_mark) {
        return None;
    }
    let [defined_target, defined_name, descriptor] = plain_args(&call.args)?;
    if !is_target(defined_target) || !is_ident_expr(defined_name, name) {
        return None;
    }
    let entries = descriptor_entries(descriptor)?;
    if entries.len() != 2 || !is_bool_lit(entries.get("enumerable")?, true) {
        return None;
    }
    let getter = *entries.get("get")?;
    if is_computed_lookup(getter, all, name) {
        return Some(GetterMap::Values);
    }
    // `Object.getOwnPropertyDescriptor(all, name).get`
    let Expr::Member(member) = strip_parens(getter) else {
        return None;
    };
    if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "get") {
        return None;
    }
    let Expr::Call(descriptor_call) = strip_parens(&member.obj) else {
        return None;
    };
    let Callee::Expr(descriptor_callee) = &descriptor_call.callee else {
        return None;
    };
    let [object, key] = plain_args(&descriptor_call.args)?;
    (is_unresolved_member_expr(
        descriptor_callee,
        "Object",
        "getOwnPropertyDescriptor",
        unresolved_mark,
    ) && is_ident_expr(object, all)
        && is_ident_expr(key, name))
    .then_some(GetterMap::Accessors)
}

/// `function (obj, localName, importedName) { Object.defineProperty(exports,
/// localName, { enumerable: true, configurable: true, get: () =>
/// obj[importedName] }); }`
fn is_named_export_from_helper(function: &HelperFunction, unresolved_mark: Mark) -> bool {
    let Some(params) = function.params() else {
        return false;
    };
    let Ok([object, local, imported]) = <[&Ident; 3]>::try_from(params) else {
        return false;
    };
    let Some([stmt]) = function.stmts() else {
        return false;
    };
    let Some(call) = call_stmt(stmt) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !is_unresolved_member_expr(callee, "Object", "defineProperty", unresolved_mark) {
        return false;
    }
    let Some([target, name, descriptor]) = plain_args(&call.args) else {
        return false;
    };
    if !matches!(strip_parens(target), Expr::Ident(id) if is_unresolved_ident(id, "exports", unresolved_mark))
        || !is_ident_expr(name, local)
    {
        return false;
    }
    let Some(entries) = descriptor_entries(descriptor) else {
        return false;
    };
    let allowed = entries
        .keys()
        .all(|key| matches!(key.as_str(), "enumerable" | "configurable" | "get"));
    allowed
        && entries
            .get("enumerable")
            .is_some_and(|value| is_bool_lit(value, true))
        && entries
            .get("configurable")
            .is_none_or(|value| matches!(strip_parens(value), Expr::Lit(Lit::Bool(_))))
        && entries.get("get").is_some_and(|getter| {
            let getter = match strip_parens(getter) {
                Expr::Arrow(arrow) if arrow.params.is_empty() => HelperFunction::Arrow(arrow),
                Expr::Fn(FnExpr { function, .. }) if function.params.is_empty() => {
                    HelperFunction::Function(function)
                }
                _ => return false,
            };
            getter
                .returned_expr()
                .is_some_and(|returned| is_computed_lookup(returned, object, imported))
        })
}

/// esbuild's `__copyProps`:
///
/// ```text
/// (to, from, except, desc) => {
///   if (from && typeof from === "object" || typeof from === "function") {
///     for (let key of Object.getOwnPropertyNames(from))
///       if (!__hasOwnProp.call(to, key) && key !== except)
///         Object.defineProperty(to, key, {
///           get: () => from[key],
///           enumerable: !(desc = Object.getOwnPropertyDescriptor(from, key)) || desc.enumerable,
///         });
///   }
///   return to;
/// }
/// ```
///
/// The type guard is not checked: `__toCommonJS` only passes the namespace
/// object literal. Returns the `hasOwnProperty` alias the body uses.
fn copy_props_helper(
    function: &HelperFunction,
    has_own_aliases: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) -> Option<Vec<BindingKey>> {
    let params = function.params()?;
    let (to, from, except, desc) = match params.as_slice() {
        [to, from, except, desc] => (*to, *from, *except, *desc),
        _ => return None,
    };
    let [Stmt::If(guard), Stmt::Return(ReturnStmt {
        arg: Some(returned),
        ..
    })] = function.stmts()?
    else {
        return None;
    };
    if guard.alt.is_some() || !is_ident_expr(returned, to) {
        return None;
    }
    let Stmt::ForOf(ForOfStmt {
        left,
        right,
        body,
        is_await: false,
        ..
    }) = single_stmt(&guard.cons)?
    else {
        return None;
    };
    let key = match left {
        ForHead::VarDecl(var) => match var.decls.as_slice() {
            [VarDeclarator {
                name: Pat::Ident(binding),
                init: None,
                ..
            }] => &binding.id,
            _ => return None,
        },
        _ => return None,
    };
    let Expr::Call(names) = strip_parens(right) else {
        return None;
    };
    let Callee::Expr(names_callee) = &names.callee else {
        return None;
    };
    let [names_of] = plain_args(&names.args)?;
    if !is_unresolved_member_expr(
        names_callee,
        "Object",
        "getOwnPropertyNames",
        unresolved_mark,
    ) || !is_ident_expr(names_of, from)
    {
        return None;
    }
    let Stmt::If(skip) = single_stmt(body)? else {
        return None;
    };
    if skip.alt.is_some() {
        return None;
    }
    let has_own = copy_guard_has_own(
        &skip.test,
        to,
        key,
        except,
        has_own_aliases,
        unresolved_mark,
    )?;
    let define = call_stmt(&skip.cons)?;
    let Callee::Expr(define_callee) = &define.callee else {
        return None;
    };
    if !is_unresolved_member_expr(define_callee, "Object", "defineProperty", unresolved_mark) {
        return None;
    }
    let [target, name, descriptor] = plain_args(&define.args)?;
    if !is_ident_expr(target, to) || !is_ident_expr(name, key) {
        return None;
    }
    let entries = descriptor_entries(descriptor)?;
    if entries.len() != 2 {
        return None;
    }
    let Expr::Arrow(getter) = strip_parens(entries.get("get")?) else {
        return None;
    };
    if !getter.params.is_empty()
        || !HelperFunction::Arrow(getter)
            .returned_expr()
            .is_some_and(|returned| is_computed_lookup(returned, from, key))
    {
        return None;
    }
    is_copied_enumerable(entries.get("enumerable")?, from, key, desc, unresolved_mark)
        .then_some(has_own)
}

/// `!HAS_OWN.call(to, key) && key !== except`, in either order. Returns the
/// `hasOwnProperty` alias, if the guard reads one.
fn copy_guard_has_own(
    test: &Expr,
    to: &Ident,
    key: &Ident,
    except: &Ident,
    has_own_aliases: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) -> Option<Vec<BindingKey>> {
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalAnd,
        left,
        right,
        ..
    }) = strip_parens(test)
    else {
        return None;
    };
    let is_except = |expr: &Expr| {
        matches!(strip_parens(expr), Expr::Bin(BinExpr { op: BinaryOp::NotEqEq, left, right, .. })
            if (is_ident_expr(left, key) && is_ident_expr(right, except))
                || (is_ident_expr(left, except) && is_ident_expr(right, key)))
    };
    let not_own = |expr: &Expr| -> Option<Vec<BindingKey>> {
        let Expr::Unary(UnaryExpr {
            op: UnaryOp::Bang,
            arg,
            ..
        }) = strip_parens(expr)
        else {
            return None;
        };
        let Expr::Call(call) = strip_parens(arg) else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Member(member) = strip_parens(callee) else {
            return None;
        };
        if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "call") {
            return None;
        }
        let [object, name] = plain_args(&call.args)?;
        if !is_ident_expr(object, to) || !is_ident_expr(name, key) {
            return None;
        }
        match strip_parens(&member.obj) {
            Expr::Ident(alias) if has_own_aliases.contains(&binding_key(alias)) => {
                Some(vec![binding_key(alias)])
            }
            method if is_has_own_property(method, unresolved_mark) => Some(Vec::new()),
            _ => None,
        }
    };
    if is_except(right) {
        not_own(left)
    } else if is_except(left) {
        not_own(right)
    } else {
        None
    }
}

/// `!(desc = Object.getOwnPropertyDescriptor(from, key)) || desc.enumerable`
fn is_copied_enumerable(
    expr: &Expr,
    from: &Ident,
    key: &Ident,
    desc: &Ident,
    unresolved_mark: Mark,
) -> bool {
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalOr,
        left,
        right,
        ..
    }) = strip_parens(expr)
    else {
        return false;
    };
    let Expr::Unary(UnaryExpr {
        op: UnaryOp::Bang,
        arg,
        ..
    }) = strip_parens(left)
    else {
        return false;
    };
    let Expr::Assign(assign) = strip_parens(arg) else {
        return false;
    };
    let assigns_desc = assign.op == AssignOp::Assign
        && matches!(&assign.left, AssignTarget::Simple(SimpleAssignTarget::Ident(binding))
            if same_ident(&binding.id, desc));
    let reads_descriptor = matches!(strip_parens(&assign.right), Expr::Call(call)
        if matches!(&call.callee, Callee::Expr(callee)
            if is_unresolved_member_expr(callee, "Object", "getOwnPropertyDescriptor", unresolved_mark))
            && plain_args(&call.args).is_some_and(|[object, name]: [&Expr; 2]|
                is_ident_expr(object, from) && is_ident_expr(name, key)));
    let reads_enumerable = matches!(strip_parens(right), Expr::Member(member)
        if is_ident_expr(&member.obj, desc)
            && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "enumerable"));
    assigns_desc && reads_descriptor && reads_enumerable
}

/// esbuild's `__reExport`:
/// `(target, mod, secondTarget) => (__copyProps(target, mod, "default"),
/// secondTarget && __copyProps(secondTarget, mod, "default"))`.
fn is_re_export_helper(function: &HelperFunction, copy_props: &HashSet<BindingKey>) -> bool {
    let Some(params) = function.params() else {
        return false;
    };
    let [target, module, second] = params.as_slice() else {
        return false;
    };
    let Some(effects) = function.effects() else {
        return false;
    };
    let [first_copy, second_copy] = effects.as_slice() else {
        return false;
    };
    let is_copy = |expr: &Expr, to: &Ident| {
        let Expr::Call(call) = strip_parens(expr) else {
            return false;
        };
        matches!(&call.callee, Callee::Expr(callee)
            if matches!(strip_parens(callee), Expr::Ident(id) if copy_props.contains(&binding_key(id))))
            && plain_args(&call.args).is_some_and(|[copy_to, from, except]: [&Expr; 3]| {
                is_ident_expr(copy_to, to)
                    && is_ident_expr(from, module)
                    && string_value(except).as_deref() == Some("default")
            })
    };
    is_copy(first_copy, target)
        && matches!(strip_parens(second_copy), Expr::Bin(BinExpr { op: BinaryOp::LogicalAnd, left, right, .. })
            if is_ident_expr(left, second) && is_copy(right, second))
}

/// esbuild's `__toESM`:
///
/// ```text
/// (mod, isNodeMode, target) => (
///   target = mod != null ? __create(__getProtoOf(mod)) : {},
///   __copyProps(
///     isNodeMode || !mod || !mod.__esModule
///       ? __defProp(target, "default", { value: mod, enumerable: true })
///       : target,
///     mod))
/// ```
///
/// The prototype of the copy is not checked: a namespace import exposes the
/// same keys either way.
fn is_to_esm_helper(
    function: &HelperFunction,
    copy_props: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) -> bool {
    let Some(params) = function.params() else {
        return false;
    };
    let [module, node_mode, target] = params.as_slice() else {
        return false;
    };
    let Some(effects) = function.effects() else {
        return false;
    };
    let [init, copy] = effects.as_slice() else {
        return false;
    };
    let assigns_target = matches!(strip_parens(init), Expr::Assign(assign)
        if assign.op == AssignOp::Assign
            && matches!(&assign.left, AssignTarget::Simple(SimpleAssignTarget::Ident(binding))
                if same_ident(&binding.id, target)));
    let Expr::Call(copy) = strip_parens(copy) else {
        return false;
    };
    let copies = matches!(&copy.callee, Callee::Expr(callee)
        if matches!(strip_parens(callee), Expr::Ident(id) if copy_props.contains(&binding_key(id))));
    let Some([to, from]) = plain_args(&copy.args) else {
        return false;
    };
    let Expr::Cond(choice) = strip_parens(to) else {
        return false;
    };
    // `isNodeMode || !mod || !mod.__esModule`
    let mut tests = Vec::new();
    collect_or_operands(&choice.test, &mut tests);
    let tests_interop = matches!(tests.as_slice(), [node, missing, not_es_module]
        if is_ident_expr(node, node_mode)
            && matches!(strip_parens(missing), Expr::Unary(UnaryExpr { op: UnaryOp::Bang, arg, .. })
                if is_ident_expr(arg, module))
            && matches!(strip_parens(not_es_module), Expr::Unary(UnaryExpr { op: UnaryOp::Bang, arg, .. })
                if matches!(strip_parens(arg), Expr::Member(member)
                    if is_ident_expr(&member.obj, module)
                        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "__esModule"))));
    // `__defProp(target, "default", { value: mod, enumerable: true })`
    let defines_default = matches!(strip_parens(&choice.cons), Expr::Call(define)
    if matches!(&define.callee, Callee::Expr(callee)
        if is_unresolved_member_expr(callee, "Object", "defineProperty", unresolved_mark))
        && plain_args(&define.args).is_some_and(|[object, name, descriptor]: [&Expr; 3]| {
            is_ident_expr(object, target)
                && string_value(name).as_deref() == Some("default")
                && descriptor_entries(descriptor).is_some_and(|entries| {
                    entries.len() == 2
                        && entries.get("value").is_some_and(|value| is_ident_expr(value, module))
                        && entries.get("enumerable").is_some_and(|value| is_bool_lit(value, true))
                })
        }));
    assigns_target
        && copies
        && tests_interop
        && defines_default
        && is_ident_expr(&choice.alt, target)
        && is_ident_expr(from, module)
}

fn collect_or_operands<'a>(expr: &'a Expr, operands: &mut Vec<&'a Expr>) {
    match strip_parens(expr) {
        Expr::Bin(BinExpr {
            op: BinaryOp::LogicalOr,
            left,
            right,
            ..
        }) => {
            collect_or_operands(left, operands);
            collect_or_operands(right, operands);
        }
        expr => operands.push(expr),
    }
}

/// esbuild's `__toCommonJS`:
/// `(mod) => __copyProps(Object.defineProperty({}, "__esModule", { value: true }), mod)`.
fn is_to_common_js_helper(
    function: &HelperFunction,
    copy_props: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) -> bool {
    let Some(params) = function.params() else {
        return false;
    };
    let [module] = params.as_slice() else {
        return false;
    };
    let Some(Expr::Call(call)) = function.returned_expr().map(strip_parens) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !matches!(strip_parens(callee), Expr::Ident(id) if copy_props.contains(&binding_key(id))) {
        return false;
    }
    let Some([target, source]) = plain_args(&call.args) else {
        return false;
    };
    if !is_ident_expr(source, module) {
        return false;
    }
    let Expr::Call(marker) = strip_parens(target) else {
        return false;
    };
    let Callee::Expr(marker_callee) = &marker.callee else {
        return false;
    };
    let Some([object, name, descriptor]) = plain_args(&marker.args) else {
        return false;
    };
    is_unresolved_member_expr(marker_callee, "Object", "defineProperty", unresolved_mark)
        && matches!(strip_parens(object), Expr::Object(ObjectLit { props, .. }) if props.is_empty())
        && is_esmodule_name_arg(name)
        && is_esmodule_descriptor(descriptor)
}

// ---------------------------------------------------------------------------
// Getter maps
// ---------------------------------------------------------------------------

/// The getters a getter-map helper defines for `entries`, in definition
/// order. `None` when an entry is not what the helper's getter read expects
/// (a value that is not a getter function, or a getter read through
/// `all[name]`), or when the keys are computed, repeated, or `__proto__`.
fn getter_map_entries(entries: &ObjectLit, map: GetterMap) -> Option<Vec<(Atom, Box<Expr>)>> {
    let mut getters = Vec::with_capacity(entries.props.len());
    let mut seen: HashSet<Atom> = HashSet::default();
    for prop in &entries.props {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let (key, getter) = match (prop.as_ref(), map) {
            (Prop::KeyValue(KeyValueProp { key, value }), GetterMap::Values) => {
                match strip_parens(value) {
                    Expr::Arrow(arrow) if arrow.params.is_empty() => {}
                    Expr::Fn(FnExpr { function, .. }) if function.params.is_empty() => {}
                    _ => return None,
                }
                (key, value.clone())
            }
            (Prop::Getter(getter), GetterMap::Accessors) => (
                &getter.key,
                Box::new(Expr::Fn(FnExpr {
                    ident: None,
                    function: getter.function.clone(),
                })),
            ),
            _ => return None,
        };
        let name: Atom = match key {
            PropName::Ident(ident) => ident.sym.clone(),
            PropName::Str(str) => str.value.as_str()?.into(),
            _ => return None,
        };
        if is_prototype_mutating_member_name(name.as_ref()) || !seen.insert(name.clone()) {
            return None;
        }
        getters.push((name, getter));
    }
    Some(getters)
}

/// The function a call invokes in place: `(function (a, b) { ... })(x, y)`
/// or the arrow form.
fn inline_helper_function(call: &CallExpr) -> Option<HelperFunction<'_>> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    match strip_parens(callee) {
        Expr::Fn(FnExpr { function, .. }) => Some(HelperFunction::Function(function)),
        Expr::Arrow(arrow) => Some(HelperFunction::Arrow(arrow)),
        _ => None,
    }
}

/// The call of an expression statement, also behind the `!` or `void` a
/// minifier puts in front of a function it calls in place.
fn discarded_call(expr: &Expr) -> Option<&CallExpr> {
    match strip_parens(expr) {
        Expr::Call(call) => Some(call),
        Expr::Unary(unary) if matches!(unary.op, UnaryOp::Bang | UnaryOp::Void) => {
            match strip_parens(&unary.arg) {
                Expr::Call(call) => Some(call),
                _ => None,
            }
        }
        _ => None,
    }
}

/// A getter-map helper called in place on `exports`, as a minifier inlines
/// it: the definitions it performs.
fn lower_inline_getter_map_call(
    item: &ModuleItem,
    unresolved_mark: Mark,
) -> Option<Vec<ModuleItem>> {
    let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
        return None;
    };
    let call = discarded_call(&statement.expr)?;
    let map = getter_map_helper(&inline_helper_function(call)?, unresolved_mark)?;
    let [target, entries] = plain_args(&call.args)?;
    if !matches!(strip_parens(target), Expr::Ident(id)
        if is_unresolved_ident(id, "exports", unresolved_mark))
    {
        return None;
    }
    let Expr::Object(entries) = strip_parens(entries) else {
        return None;
    };
    let getters = getter_map_entries(entries, map)?;
    Some(
        getters
            .into_iter()
            .map(|(name, getter)| {
                getter_definition(statement.span, &name, getter, false, unresolved_mark)
            })
            .collect(),
    )
}

/// A getter-map loop over the object declared by the statement before it,
/// as a minifier inlines the helper: the definitions it performs. The object
/// must be read nowhere else, so the loop sees the entries it was declared
/// with.
fn lower_getter_map_loop(
    declaration: &ModuleItem,
    item: &ModuleItem,
    uses: &BindingUseIndex,
    unresolved_mark: Mark,
) -> Option<Vec<ModuleItem>> {
    let ModuleItem::Stmt(Stmt::ForIn(for_in)) = item else {
        return None;
    };
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = declaration else {
        return None;
    };
    let [VarDeclarator {
        name: Pat::Ident(binding),
        init: Some(init),
        ..
    }] = var.decls.as_slice()
    else {
        return None;
    };
    let Expr::Object(entries) = strip_parens(init) else {
        return None;
    };
    let map = getter_map_loop(
        for_in,
        |target| {
            matches!(strip_parens(target), Expr::Ident(id)
            if is_unresolved_ident(id, "exports", unresolved_mark))
        },
        &binding.id,
        unresolved_mark,
    )?;
    let key = binding_key(&binding.id);
    let mut loop_reads = 0;
    for_in.visit_with(&mut BindingRefCounter {
        binding: &binding.id,
        count: &mut loop_reads,
    });
    if !uses.has_single_declaration(&key)
        || uses.has_direct_write(&key)
        || uses.use_count(&key) != loop_reads
    {
        return None;
    }
    let getters = getter_map_entries(entries, map)?;
    Some(
        getters
            .into_iter()
            .map(|(name, getter)| {
                getter_definition(for_in.span, &name, getter, false, unresolved_mark)
            })
            .collect(),
    )
}

struct BindingRefCounter<'a> {
    binding: &'a Ident,
    count: &'a mut usize,
}

impl Visit for BindingRefCounter<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == self.binding.sym && ident.ctxt == self.binding.ctxt {
            *self.count += 1;
        }
    }
}

fn string_value(expr: &Expr) -> Option<Atom> {
    match strip_parens(expr) {
        Expr::Lit(Lit::Str(str)) => str.value.as_str().map(Atom::from),
        _ => None,
    }
}

/// `object.name`, or `object["name"]` when the name cannot follow a dot.
/// Reserved words can (`object.default`).
fn member_read(object: Ident, name: &Atom) -> Expr {
    let prop = if is_valid_js_ident(name.as_ref()) {
        MemberProp::Ident(IdentName::new(name.clone(), DUMMY_SP))
    } else {
        MemberProp::Computed(ComputedPropName {
            span: DUMMY_SP,
            expr: Box::new(Expr::Lit(Lit::Str(make_str(name.as_ref())))),
        })
    };
    Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: Box::new(Expr::Ident(object)),
        prop,
    })
}

fn object_define_property(span: Span, args: Vec<Box<Expr>>, unresolved_mark: Mark) -> ModuleItem {
    let callee = Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: Box::new(Expr::Ident(make_unresolved_ident(
            "Object".into(),
            unresolved_mark,
        ))),
        prop: MemberProp::Ident(IdentName::new("defineProperty".into(), DUMMY_SP)),
    });
    ModuleItem::Stmt(Stmt::Expr(ExprStmt {
        span,
        expr: Box::new(Expr::Call(CallExpr {
            span: DUMMY_SP,
            ctxt: Default::default(),
            callee: Callee::Expr(Box::new(callee)),
            args: args.into_iter().map(|expr| expr.as_arg()).collect(),
            type_args: None,
        })),
    }))
}

fn key_value(name: &str, value: Expr) -> PropOrSpread {
    PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
        key: PropName::Ident(IdentName::new(name.into(), DUMMY_SP)),
        value: Box::new(value),
    })))
}

fn bool_lit(value: bool) -> Expr {
    Expr::Lit(Lit::Bool(swc_core::ecma::ast::Bool {
        span: DUMMY_SP,
        value,
    }))
}

fn exports_ident(unresolved_mark: Mark) -> Box<Expr> {
    Box::new(Expr::Ident(make_unresolved_ident(
        "exports".into(),
        unresolved_mark,
    )))
}

/// `Object.defineProperty(exports, "name", { enumerable: true, get })`, with
/// `configurable: true` where the helper set it.
fn getter_definition(
    span: Span,
    name: &Atom,
    getter: Box<Expr>,
    configurable: bool,
    unresolved_mark: Mark,
) -> ModuleItem {
    let mut props = vec![key_value("enumerable", bool_lit(true))];
    if configurable {
        props.push(key_value("configurable", bool_lit(true)));
    }
    let getter = match *getter {
        // `get() { ... }` keeps an accessor's own function shape.
        Expr::Fn(FnExpr {
            ident: None,
            function,
        }) => PropOrSpread::Prop(Box::new(Prop::Method(MethodProp {
            key: PropName::Ident(IdentName::new("get".into(), DUMMY_SP)),
            function,
        }))),
        getter => key_value("get", getter),
    };
    props.push(getter);
    object_define_property(
        span,
        vec![
            exports_ident(unresolved_mark),
            Box::new(Expr::Lit(Lit::Str(make_str(name.as_ref())))),
            Box::new(Expr::Object(ObjectLit {
                span: DUMMY_SP,
                props,
            })),
        ],
        unresolved_mark,
    )
}

/// `Object.defineProperty(exports, "__esModule", { value: true })`
fn esmodule_marker(span: Span, unresolved_mark: Mark) -> ModuleItem {
    object_define_property(
        span,
        vec![
            exports_ident(unresolved_mark),
            Box::new(Expr::Lit(Lit::Str(make_str("__esModule")))),
            Box::new(Expr::Object(ObjectLit {
                span: DUMMY_SP,
                props: vec![key_value("value", bool_lit(true))],
            })),
        ],
        unresolved_mark,
    )
}

/// `var namespace = {};` with no other declarator.
fn is_empty_object_declaration(item: &ModuleItem, namespace: &Ident) -> bool {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return false;
    };
    matches!(var.decls.as_slice(), [VarDeclarator { name: Pat::Ident(binding), init: Some(init), .. }]
        if same_ident(&binding.id, namespace)
            && matches!(strip_parens(init), Expr::Object(ObjectLit { props, .. }) if props.is_empty()))
}

/// One export defined by webpack's `require.d`.
enum WebpackExport {
    /// A getter returning this expression.
    Getter(Box<Expr>),
    /// A data property holding this value, read when the call runs.
    Value(Box<Expr>),
}

/// Lower webpack's runtime `require.d(exports, definition)` statements, and
/// the getter-loop IIFE, to per-name definitions: getters to
/// `Object.defineProperty(exports, "x", { enumerable: true, get })`, and
/// values to `exports.x = value`. Returns whether any statement was lowered.
///
/// A default-object compatibility block after a getter-loop IIFE is removed
/// with it.
pub(crate) fn lower_webpack_export_definitions(module: &mut Module, unresolved_mark: Mark) -> bool {
    if !has_webpack_export_definitions(module, unresolved_mark) {
        return false;
    }
    // The runtime skips a key that `exports` already owns; a second
    // `Object.defineProperty` of the same non-configurable key would throw.
    let mut seen: HashSet<Atom> = HashSet::default();
    let unique = module
        .body
        .iter()
        .filter_map(|item| webpack_export_statement(item, unresolved_mark))
        .flatten()
        .all(|(name, _)| seen.insert(name));
    if !unique {
        return false;
    }
    let mut lowered_getter_loop = false;
    for item in std::mem::take(&mut module.body) {
        let Some(exports) = webpack_export_statement(&item, unresolved_mark) else {
            if !(lowered_getter_loop && is_exports_default_compat_postamble(&item, unresolved_mark))
            {
                module.body.push(item);
            }
            continue;
        };
        lowered_getter_loop |= extract_webpack_export_getter_iife(&item, unresolved_mark).is_some();
        let span = module_item_span(&item);
        for (name, export) in exports {
            module.body.push(match export {
                WebpackExport::Getter(expr) => getter_definition(
                    span,
                    &name,
                    Box::new(arrow_returning(expr)),
                    false,
                    unresolved_mark,
                ),
                WebpackExport::Value(value) => exports_write(span, &name, value, unresolved_mark),
            });
        }
    }
    true
}

pub(crate) fn has_webpack_export_definitions(module: &Module, unresolved_mark: Mark) -> bool {
    module
        .body
        .iter()
        .any(|item| webpack_export_statement(item, unresolved_mark).is_some())
}

fn webpack_export_statement(
    item: &ModuleItem,
    unresolved_mark: Mark,
) -> Option<Vec<(Atom, WebpackExport)>> {
    if let Some(getters) = extract_webpack_export_getter_iife(item, unresolved_mark) {
        return Some(
            getters
                .into_iter()
                .map(|(name, expr)| (name, WebpackExport::Getter(expr)))
                .collect(),
        );
    }
    let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
        return None;
    };
    let Expr::Call(call) = strip_parens(&statement.expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !is_unresolved_member_expr(callee, "require", "d", unresolved_mark) {
        return None;
    }
    if call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }
    let target = call.args.first()?;
    if !matches!(strip_parens(&target.expr), Expr::Ident(id)
        if is_unresolved_ident(id, "exports", unresolved_mark))
    {
        return None;
    }
    let mut exports = Vec::new();
    match &call.args[1..] {
        [definition] => match strip_parens(&definition.expr) {
            Expr::Object(getters) => {
                for (name, expr) in extract_export_getter_map(getters)? {
                    exports.push((name, WebpackExport::Getter(expr)));
                }
            }
            Expr::Array(array) => {
                let mut elements = array.elems.iter();
                while let Some(key) = elements.next() {
                    let name = string_value(&key.as_ref()?.expr)?;
                    let binding = &elements.next()?.as_ref()?.expr;
                    let export = if matches!(strip_parens(binding), Expr::Lit(Lit::Num(num)) if num.value == 0.0)
                    {
                        WebpackExport::Value(elements.next()?.as_ref()?.expr.clone())
                    } else {
                        WebpackExport::Getter(extract_getter_expr_return_expr(binding)?)
                    };
                    exports.push((name, export));
                }
            }
            _ => return None,
        },
        [name, getter] if string_value(&name.expr).is_some() => {
            let name = string_value(&name.expr)?;
            exports.push((
                name,
                WebpackExport::Getter(extract_getter_expr_return_expr(&getter.expr)?),
            ));
        }
        [getters, values] => {
            let (Expr::Object(getters), Expr::Object(values)) =
                (strip_parens(&getters.expr), strip_parens(&values.expr))
            else {
                return None;
            };
            for (name, expr) in extract_export_getter_map(getters)? {
                exports.push((name, WebpackExport::Getter(expr)));
            }
            for prop in &values.props {
                let PropOrSpread::Prop(prop) = prop else {
                    return None;
                };
                let (name, value) = match prop.as_ref() {
                    Prop::KeyValue(entry) => (prop_name_as_atom(&entry.key)?, entry.value.clone()),
                    Prop::Shorthand(ident) => {
                        (ident.sym.clone(), Box::new(Expr::Ident(ident.clone())))
                    }
                    _ => return None,
                };
                exports.push((name, WebpackExport::Value(value)));
            }
        }
        _ => return None,
    }
    let mut seen: HashSet<Atom> = HashSet::default();
    if exports.is_empty()
        || !exports.iter().all(|(name, _)| {
            !is_prototype_mutating_member_name(name.as_ref()) && seen.insert(name.clone())
        })
    {
        return None;
    }
    Some(exports)
}

fn arrow_returning(expr: Box<Expr>) -> Expr {
    Expr::Arrow(ArrowExpr {
        span: DUMMY_SP,
        ctxt: Default::default(),
        params: Vec::new(),
        body: Box::new(ArrowFunctionBody::Expr(expr)),
        is_async: false,
        is_generator: false,
        type_params: None,
        return_type: None,
    })
}

/// `exports.name = value;`, or `exports["name"] = value;` when the name
/// cannot follow a dot.
fn exports_write(span: Span, name: &Atom, value: Box<Expr>, unresolved_mark: Mark) -> ModuleItem {
    let Expr::Member(target) = member_read(
        make_unresolved_ident("exports".into(), unresolved_mark),
        name,
    ) else {
        unreachable!("member_read builds a member expression");
    };
    ModuleItem::Stmt(Stmt::Expr(ExprStmt {
        span,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Member(target)),
            right: value,
        })),
    }))
}

/// Turn each getter that reads a member of `require(<id>)`, where the id is
/// a bundler module id the unpacker could not resolve to a file of this
/// input, into `exports.x = binding.member;` right after that binding's
/// declaration.
///
/// Such a getter has no source to re-export from, so `UnEsm` would leave it
/// in the module, and an ES module throws on its `exports` access. The
/// assignment becomes a snapshot export: the value the getter returned once
/// the `require` ran, without later changes to that property. webpack defines
/// its getters before the `require` declarations, so the write moves down,
/// but only past other export definitions and `require` statements, which a
/// getter definition cannot observe. Getters with anything else in between
/// stay as written.
pub(super) fn snapshot_unresolved_id_require_getters(module: &mut Module, unresolved_mark: Mark) {
    let mut declarations: HashMap<BindingId, usize> = module
        .body
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            unresolved_id_require_declaration(item, unresolved_mark).map(|binding| (binding, index))
        })
        .collect();
    if declarations.is_empty() {
        return;
    }
    let uses = BindingUseIndex::collect(module);
    declarations.retain(|binding, _| {
        uses.has_single_declaration(binding) && !is_written_through(&uses, binding)
    });
    if declarations.is_empty() {
        return;
    }

    // Getter index → index of the statement the snapshot follows.
    let mut targets: HashMap<usize, usize> = HashMap::default();
    for (index, item) in module.body.iter().enumerate() {
        let Some(binding) = unresolved_id_getter_binding(item, unresolved_mark) else {
            continue;
        };
        let Some(&declaration) = declarations.get(&binding) else {
            continue;
        };
        if declaration < index {
            targets.insert(index, index);
        } else if module.body[index + 1..declaration]
            .iter()
            .all(|item| is_getter_transparent_item(item, unresolved_mark))
        {
            targets.insert(index, declaration);
        }
    }
    if targets.is_empty() {
        return;
    }

    let mut writes: HashMap<usize, Vec<ModuleItem>> = HashMap::default();
    let mut order: Vec<usize> = targets.keys().copied().collect();
    order.sort_unstable();
    for index in order {
        let write = unresolved_id_getter_write(&module.body[index], unresolved_mark)
            .expect("getter matched above");
        writes.entry(targets[&index]).or_default().push(write);
    }
    for (index, item) in std::mem::take(&mut module.body).into_iter().enumerate() {
        if !targets.contains_key(&index) {
            module.body.push(item);
        }
        if let Some(writes) = writes.remove(&index) {
            module.body.extend(writes);
        }
    }
}

/// The binding of a single top-level `var r = require(12345);`.
fn unresolved_id_require_declaration(
    item: &ModuleItem,
    unresolved_mark: Mark,
) -> Option<BindingId> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };
    let [declarator] = var.decls.as_slice() else {
        return None;
    };
    let Pat::Ident(binding) = &declarator.name else {
        return None;
    };
    let Expr::Call(call) = strip_parens(declarator.init.as_deref()?) else {
        return None;
    };
    is_unresolved_id_require_call(call, unresolved_mark).then(|| binding_id(&binding.id))
}

fn is_unresolved_id_require_call(call: &CallExpr, unresolved_mark: Mark) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    matches!(callee.as_ref(), Expr::Ident(id) if is_unresolved_ident(id, "require", unresolved_mark))
        && matches!(
            call.args.as_slice(),
            [ExprOrSpread { spread: None, expr }] if matches!(expr.as_ref(), Expr::Lit(Lit::Num(_)))
        )
}

/// `Object.defineProperty(exports, "x", { enumerable: true, get: () => r.y })`
/// as `(name, r, y)`.
fn define_property_member_getter(
    item: &ModuleItem,
    unresolved_mark: Mark,
) -> Option<(Atom, Ident, Atom)> {
    let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
        return None;
    };
    let Expr::Call(call) = strip_parens(&statement.expr) else {
        return None;
    };
    if !is_object_define_property_global_call(call, unresolved_mark)
        || call.args.len() != 3
        || !is_cjs_export_object_expr(&call.args[0].expr, unresolved_mark)
    {
        return None;
    }
    let name = literal_export_name_arg(&call.args[1].expr)?;
    let (base, member) = extract_define_property_getter_member(&call.args[2].expr)?;
    Some((name, base, member))
}

fn unresolved_id_getter_binding(item: &ModuleItem, unresolved_mark: Mark) -> Option<BindingId> {
    define_property_member_getter(item, unresolved_mark).map(|(_, base, _)| binding_id(&base))
}

fn unresolved_id_getter_write(item: &ModuleItem, unresolved_mark: Mark) -> Option<ModuleItem> {
    let (name, base, member) = define_property_member_getter(item, unresolved_mark)?;
    Some(exports_write(
        module_item_span(item),
        &name,
        Box::new(member_read(base, &member)),
        unresolved_mark,
    ))
}

/// A statement a getter definition can move past: another export definition
/// or a `require` that only loads a module.
fn is_getter_transparent_item(item: &ModuleItem, unresolved_mark: Mark) -> bool {
    let ModuleItem::Stmt(stmt) = item else {
        return matches!(item, ModuleItem::ModuleDecl(ModuleDecl::Import(_)));
    };
    let is_require = |expr: &Expr| {
        matches!(strip_parens(expr), Expr::Call(call)
            if matches!(&call.callee, Callee::Expr(callee)
                if matches!(callee.as_ref(), Expr::Ident(id)
                    if is_unresolved_ident(id, "require", unresolved_mark))))
    };
    let is_getter_definition = |expr: &Expr| {
        matches!(strip_parens(expr), Expr::Call(call)
            if is_object_define_property_global_call(call, unresolved_mark)
                && call.args.len() == 3
                && is_cjs_export_object_expr(&call.args[0].expr, unresolved_mark)
                && literal_export_name_arg(&call.args[1].expr).is_some()
                && extract_define_property_getter_expr(&call.args[2].expr).is_some())
    };
    match stmt {
        Stmt::Expr(statement) => {
            is_require(&statement.expr) || is_getter_definition(&statement.expr)
        }
        Stmt::Decl(Decl::Var(var)) => var.decls.iter().all(|declarator| {
            matches!(&declarator.name, Pat::Ident(_))
                && declarator.init.as_deref().is_some_and(is_require)
        }),
        _ => false,
    }
}
