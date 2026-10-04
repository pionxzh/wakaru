//! CommonJS `export * from "..."` recovery.
//!
//! Three producers lower `export * from "./source.js"` to a key-copy loop:
//!
//! ```text
//! // Babel: an inline loop over the required module
//! var _source = require("./source.js");
//! Object.keys(_source).forEach(function (key) {
//!   if (key === "default" || key === "__esModule") return;
//!   if (Object.prototype.hasOwnProperty.call(_exportNames, key)) return;
//!   if (key in exports && exports[key] === _source[key]) return;
//!   Object.defineProperty(exports, key, { enumerable: true, get: function () { return _source[key]; } });
//! });
//!
//! // TypeScript: `__exportStar(require("./source.js"), exports)`, with
//! // `__exportStar` from tslib or inline:
//! var __exportStar = this && this.__exportStar || function (m, exports) {
//!   for (var p in m) if (p !== "default" && !Object.prototype.hasOwnProperty.call(exports, p)) __createBinding(exports, m, p);
//! };
//!
//! // SWC: `_export_star(require("./source.js"), exports)`, with `_export_star`
//! // from `@swc/helpers/_/_export_star` (called as `._`) or inline:
//! function _export_star(from, to) {
//!   Object.keys(from).forEach(function (k) {
//!     if (k !== "default" && !Object.prototype.hasOwnProperty.call(to, k)) Object.defineProperty(to, k, { enumerable: true, get: function () { return from[k]; } });
//!   });
//!   return from;
//! }
//! ```
//!
//! Minifiers rewrite the guards as `&&` / `||` chains, so every loop body is
//! read as a set of skip conditions plus one copy action ([`CopyBody`]). A
//! loop is a re-export only when it skips `default`, skips the module's own
//! keys (`__esModule`, keys the target already owns, or Babel's
//! `_exportNames`), and its only effect is copying `source[key]` to the
//! target. Inline
//! helpers must have that body; tslib and `@swc/helpers` helpers are trusted
//! by their module path, like the other runtime helpers.

use crate::analysis::binding_uses::BindingUseIndex;
use crate::rules::helper_matcher::{
    binding_key, count_binding_refs, removable_without_remaining_refs,
    remove_fn_decls_from_body_by_binding, remove_var_declarators_by_binding, BindingKey,
};
use crate::rules::transpiler_helper_utils::{
    collect_tslib_namespace_bindings, ts_expr_matches_helper_kind, tslib_require_member_name,
    TsHelperKind,
};

use swc_core::ecma::ast::{BinExpr, KeyValueProp, MethodProp};

use super::*;

const SWC_EXPORT_STAR_PATH: &str = "@swc/helpers/_/_export_star";

pub(super) fn rewrite_commonjs_export_stars(module: &mut Module, unresolved_mark: Mark) {
    let mut recovered = recognize_export_stars(module, unresolved_mark);
    if recovered.is_empty() {
        return;
    }

    let mut cleanup: HashSet<BindingKey> = HashSet::default();
    let mut rewritten = Vec::with_capacity(module.body.len());
    for (index, item) in std::mem::take(&mut module.body).into_iter().enumerate() {
        let Some(recovered) = recovered.remove(&index) else {
            rewritten.push(item);
            continue;
        };
        cleanup.extend(recovered.consumed);
        rewritten.push(make_export_all(&recovered.source, recovered.span));
    }

    module.body = rewritten;
    if cleanup.is_empty() {
        return;
    }
    let removable = removable_without_remaining_refs(&*module, &cleanup);
    if !removable.is_empty() {
        remove_var_declarators_by_binding(&mut module.body, &removable);
        remove_fn_decls_from_body_by_binding(&mut module.body, &removable);
    }
}

/// Module-body statements that [`rewrite_commonjs_export_stars`] replaces
/// with `export * from`. The export-storage analysis skips them: their
/// `exports[key]` and `exports` arguments are the copy the re-export
/// replaces, not accesses of the module's own exports.
pub(super) fn export_star_statement_indices(
    module: &Module,
    unresolved_mark: Mark,
) -> HashSet<usize> {
    recognize_export_stars(module, unresolved_mark)
        .into_keys()
        .collect()
}

fn recognize_export_stars(
    module: &Module,
    unresolved_mark: Mark,
) -> HashMap<usize, RecoveredExportStar> {
    // Match the statement shape before collecting helpers or a use index.
    // Most modules (and later UnEsm passes) have no candidate at all.
    if !module
        .body
        .iter()
        .any(|item| is_export_star_candidate(item, unresolved_mark))
    {
        return HashMap::default();
    }

    let helpers = ExportStarHelpers::collect(module, unresolved_mark);
    let uses = BindingUseIndex::collect(module);
    let local_exports = LocalExports::collect(module, unresolved_mark);
    let requires = collect_require_bindings(module, &uses, unresolved_mark);

    module
        .body
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            helper_call_export_star(item, &helpers, &uses, &requires, unresolved_mark)
                .or_else(|| {
                    loop_export_star(
                        item,
                        index,
                        &uses,
                        &requires,
                        &local_exports,
                        unresolved_mark,
                    )
                })
                .map(|recovered| (index, recovered))
        })
        .collect()
}

fn make_export_all(source: &str, span: Span) -> ModuleItem {
    ModuleItem::ModuleDecl(ModuleDecl::ExportAll(ExportAll {
        span,
        src: Box::new(make_str(source)),
        type_only: false,
        with: None,
    }))
}

/// A top-level `Object.keys(x).forEach(...)` loop or a two-argument call
/// whose second argument is `exports`.
fn is_export_star_candidate(item: &ModuleItem, unresolved_mark: Mark) -> bool {
    let ModuleItem::Stmt(Stmt::Expr(expr_stmt)) = item else {
        return false;
    };
    let Expr::Call(call) = strip_parens(expr_stmt.expr.as_ref()) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let for_each = matches!(strip_parens(callee), Expr::Member(member)
        if matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "forEach"));
    for_each
        || matches!(call.args.as_slice(), [_, target]
            if target.spread.is_none()
                && matches!(strip_parens(&target.expr), Expr::Ident(id)
                    if is_unresolved_ident(id, "exports", unresolved_mark)))
}

// ---------------------------------------------------------------------------
// Helper identities
// ---------------------------------------------------------------------------

struct ExportStarHelpers {
    /// Inline helpers whose body was proven to be an export-star copy, with
    /// the TypeScript `__createBinding` helper the body calls.
    inline: HashMap<BindingKey, Option<BindingKey>>,
    /// `require("tslib")` bindings.
    tslib_namespaces: HashSet<BindingKey>,
    /// `require("@swc/helpers/_/_export_star")` bindings.
    swc_namespaces: HashSet<BindingKey>,
    unresolved_mark: Mark,
}

impl ExportStarHelpers {
    fn collect(module: &Module, unresolved_mark: Mark) -> Self {
        let mut helpers = Self {
            inline: HashMap::default(),
            tslib_namespaces: collect_tslib_namespace_bindings(module, Some(unresolved_mark)),
            swc_namespaces: HashSet::default(),
            unresolved_mark,
        };
        let create_binding_candidates = collect_create_binding_helpers(module);
        for item in &module.body {
            match item {
                ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
                    if let Some(copy) = helper_function_copy(&fn_decl.function, unresolved_mark)
                        .filter(|copy| copy.is_helper_body(&create_binding_candidates))
                    {
                        helpers.inline.insert(
                            binding_key(&fn_decl.ident),
                            copy.create_binding.as_ref().map(binding_key),
                        );
                    }
                }
                ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                    for decl in &var.decls {
                        let (Pat::Ident(binding), Some(init)) = (&decl.name, decl.init.as_deref())
                        else {
                            continue;
                        };
                        if let Expr::Call(call) = strip_parens(init) {
                            if is_require_call(call, unresolved_mark).as_deref()
                                == Some(SWC_EXPORT_STAR_PATH)
                            {
                                helpers.swc_namespaces.insert(binding_key(&binding.id));
                                continue;
                            }
                        }
                        let Some(function) = inline_helper_function(init) else {
                            continue;
                        };
                        if let Some(copy) = helper_function_copy(function, unresolved_mark)
                            .filter(|copy| copy.is_helper_body(&create_binding_candidates))
                        {
                            helpers.inline.insert(
                                binding_key(&binding.id),
                                copy.create_binding.as_ref().map(binding_key),
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        helpers
    }

    /// The bindings a call through `callee` consumes (an inline helper and its
    /// `__createBinding`, or an `@swc/helpers` namespace), or `None` when the
    /// callee is not a proven export-star helper. tslib namespaces stay: they
    /// usually serve other helpers too.
    fn helper_callee_bindings(
        &self,
        callee: &Expr,
        uses: &BindingUseIndex,
    ) -> Option<Vec<BindingKey>> {
        let stable =
            |key: &BindingKey| uses.has_single_declaration(key) && !uses.has_direct_write(key);
        match strip_parens(callee) {
            Expr::Ident(id) => {
                let key = binding_key(id);
                let create_binding = self.inline.get(&key)?;
                stable(&key).then(|| {
                    [Some(key.clone()), create_binding.clone()]
                        .into_iter()
                        .flatten()
                        .collect()
                })
            }
            Expr::Member(member) => {
                let Expr::Ident(object) = strip_parens(&member.obj) else {
                    return (tslib_require_member_name(callee, Some(self.unresolved_mark))
                        == Some("__exportStar"))
                    .then(Vec::new);
                };
                let key = binding_key(object);
                let prop = match &member.prop {
                    MemberProp::Ident(prop) => prop.sym.as_ref(),
                    _ => return None,
                };
                if self.swc_namespaces.contains(&key) {
                    return (prop == "_" && stable(&key)).then(|| vec![key]);
                }
                (self.tslib_namespaces.contains(&key)
                    && prop == "__exportStar"
                    && !uses.has_direct_write(&key))
                .then(Vec::new)
            }
            _ => None,
        }
    }
}

/// The fallback of TypeScript's `this && this.__exportStar || function ...`,
/// or a plain function initializer.
fn inline_helper_function(init: &Expr) -> Option<&Function> {
    match strip_parens(init) {
        Expr::Fn(fn_expr) => Some(&fn_expr.function),
        Expr::Bin(BinExpr {
            op: BinaryOp::LogicalOr,
            left,
            right,
            ..
        }) => {
            let Expr::Bin(BinExpr {
                op: BinaryOp::LogicalAnd,
                left: this,
                right: member,
                ..
            }) = strip_parens(left)
            else {
                return None;
            };
            let Expr::Member(member) = strip_parens(member) else {
                return None;
            };
            if !matches!(strip_parens(this), Expr::This(_))
                || !matches!(member.obj.as_ref(), Expr::This(_))
                || !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "__exportStar")
            {
                return None;
            }
            let Expr::Fn(fn_expr) = strip_parens(right) else {
                return None;
            };
            Some(&fn_expr.function)
        }
        _ => None,
    }
}

/// TypeScript `__createBinding` declarators, proven by the shared tslib
/// helper detection.
fn collect_create_binding_helpers(module: &Module) -> HashSet<BindingKey> {
    let mut helpers = HashSet::default();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            if let (Pat::Ident(binding), Some(init)) = (&decl.name, decl.init.as_deref()) {
                if ts_expr_matches_helper_kind(init, TsHelperKind::CreateBinding) {
                    helpers.insert(binding_key(&binding.id));
                }
            }
        }
    }
    helpers
}

/// `function (source, target) { <copy loop over source into target> }`.
fn helper_function_copy(function: &Function, unresolved_mark: Mark) -> Option<CopyBody> {
    if function.is_async || function.is_generator {
        return None;
    }
    let [source, target] = function.params.as_slice() else {
        return None;
    };
    let (Pat::Ident(source), Pat::Ident(target)) = (&source.pat, &target.pat) else {
        return None;
    };
    let target = CopyTarget::Binding(target.id.clone());
    let stmts = function.body.as_ref()?.stmts.as_slice();
    let (loop_stmt, tail) = match stmts {
        // `return Object.keys(from).forEach(...), from` after minification.
        [Stmt::Return(ReturnStmt { arg: Some(arg), .. })] => {
            let Expr::Seq(seq) = strip_parens(arg) else {
                return None;
            };
            let [loop_expr, returned] = seq.exprs.as_slice() else {
                return None;
            };
            if !matches!(strip_parens(returned), Expr::Ident(id) if same_ident(id, &source.id)) {
                return None;
            }
            return match_keys_for_each(loop_expr, &source.id, target, unresolved_mark);
        }
        [loop_stmt] => (loop_stmt, None),
        [loop_stmt, Stmt::Return(ReturnStmt { arg: Some(arg), .. })] => {
            (loop_stmt, Some(arg.as_ref()))
        }
        _ => return None,
    };
    if let Some(tail) = tail {
        if !matches!(strip_parens(tail), Expr::Ident(id) if same_ident(id, &source.id)) {
            return None;
        }
    }
    match loop_stmt {
        Stmt::Expr(expr_stmt) => {
            match_keys_for_each(&expr_stmt.expr, &source.id, target, unresolved_mark)
        }
        Stmt::ForIn(for_in) => match_for_in(for_in, &source.id, target, unresolved_mark),
        _ => None,
    }
}

/// `HELPER(require("x"), exports)`, or `HELPER(binding, exports)` where the
/// binding is a single-use `var binding = require("x")`.
fn helper_call_export_star(
    item: &ModuleItem,
    helpers: &ExportStarHelpers,
    uses: &BindingUseIndex,
    requires: &HashMap<BindingKey, String>,
    unresolved_mark: Mark,
) -> Option<RecoveredExportStar> {
    let ModuleItem::Stmt(Stmt::Expr(expr_stmt)) = item else {
        return None;
    };
    let Expr::Call(call) = strip_parens(expr_stmt.expr.as_ref()) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let [source, target] = call.args.as_slice() else {
        return None;
    };
    if source.spread.is_some()
        || target.spread.is_some()
        || !matches!(strip_parens(&target.expr), Expr::Ident(id)
            if is_unresolved_ident(id, "exports", unresolved_mark))
    {
        return None;
    }
    let mut consumed = helpers.helper_callee_bindings(callee, uses)?;
    let source = match strip_parens(&source.expr) {
        Expr::Call(require) => is_require_call(require, unresolved_mark)?,
        Expr::Ident(binding) => {
            let key = binding_key(binding);
            let source = requires.get(&key)?;
            if uses.use_count(&key) != 1 {
                return None;
            }
            consumed.push(key);
            source.clone()
        }
        _ => return None,
    };
    Some(RecoveredExportStar {
        source,
        span: expr_stmt.span,
        consumed,
    })
}

/// A top-level Babel loop `Object.keys(binding).forEach(...)` over a
/// `require()` binding that has no other use.
fn loop_export_star(
    item: &ModuleItem,
    index: usize,
    uses: &BindingUseIndex,
    requires: &HashMap<BindingKey, String>,
    local_exports: &LocalExports,
    unresolved_mark: Mark,
) -> Option<RecoveredExportStar> {
    let ModuleItem::Stmt(Stmt::Expr(expr_stmt)) = item else {
        return None;
    };
    let binding = keys_for_each_source(&expr_stmt.expr)?;
    let key = binding_key(binding);
    let source = requires.get(&key)?;
    // Every use of the require binding must be inside the loop: an escaped
    // module object cannot become a re-export.
    if uses.use_count(&key) != count_binding_refs(item, &key) {
        return None;
    }
    let copy = match_keys_for_each(
        &expr_stmt.expr,
        binding,
        CopyTarget::Exports(unresolved_mark),
        unresolved_mark,
    )?;
    if !copy.is_reexport(index, uses, local_exports) {
        return None;
    }
    // `_exportNames` is proven only at its initializer; a member write
    // elsewhere (`_exportNames.y = true`) would add a skipped key.
    if let Some(export_names) = &copy.export_names {
        let names_key = binding_key(export_names);
        if uses.use_count(&names_key) != count_binding_refs(item, &names_key) {
            return None;
        }
    }
    let mut consumed = vec![key];
    consumed.extend(copy.export_names.as_ref().map(binding_key));
    Some(RecoveredExportStar {
        source: source.clone(),
        span: expr_stmt.span,
        consumed,
    })
}

struct RecoveredExportStar {
    source: String,
    span: Span,
    /// Bindings whose declarations may go once nothing references them.
    consumed: Vec<BindingKey>,
}

/// `binding` in `Object.keys(binding).forEach(...)`.
fn keys_for_each_source(expr: &Expr) -> Option<&Ident> {
    let Expr::Call(for_each) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &for_each.callee else {
        return None;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return None;
    };
    let Expr::Call(keys) = strip_parens(&member.obj) else {
        return None;
    };
    let [arg] = keys.args.as_slice() else {
        return None;
    };
    match strip_parens(&arg.expr) {
        Expr::Ident(binding) => Some(binding),
        _ => None,
    }
}

/// Top-level `var binding = require("x")` declarators with a single
/// declaration and no writes. Terser merges adjacent declarations, so the
/// binding need not sit next to the loop that consumes it.
fn collect_require_bindings(
    module: &Module,
    uses: &BindingUseIndex,
    unresolved_mark: Mark,
) -> HashMap<BindingKey, String> {
    let mut requires = HashMap::default();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let (Pat::Ident(binding), Some(init)) = (&decl.name, decl.init.as_deref()) else {
                continue;
            };
            let Expr::Call(call) = strip_parens(init) else {
                continue;
            };
            let Some(source) = is_require_call(call, unresolved_mark) else {
                continue;
            };
            let key = binding_key(&binding.id);
            if uses.has_single_declaration(&key) && !uses.has_direct_write(&key) {
                requires.insert(key, source);
            }
        }
    }
    requires
}

// ---------------------------------------------------------------------------
// Copy loops
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum CopyTarget {
    /// The module's free `exports`.
    Exports(Mark),
    /// A helper parameter.
    Binding(Ident),
}

impl CopyTarget {
    fn matches(&self, expr: &Expr) -> bool {
        let Expr::Ident(id) = strip_parens(expr) else {
            return false;
        };
        match self {
            CopyTarget::Exports(unresolved_mark) => {
                is_unresolved_ident(id, "exports", *unresolved_mark)
            }
            CopyTarget::Binding(binding) => same_ident(id, binding),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CopyAction {
    /// `target[key] = source[key]`
    Assign,
    /// `Object.defineProperty(target, key, { enumerable: true, get() { return source[key]; } })`
    DefineGetter,
    /// TypeScript `__createBinding(target, source, key)`
    CreateBinding,
}

/// A key-copy loop body: what it skips and how it copies.
struct CopyBody {
    skips_default: bool,
    skips_es_module: bool,
    skips_target_own: bool,
    /// Babel's `Object.prototype.hasOwnProperty.call(_exportNames, key)`.
    export_names: Option<Ident>,
    action: Option<CopyAction>,
    create_binding: Option<Ident>,
}

impl CopyBody {
    /// The copy skips the keys a compiler reserves for the module itself:
    /// `__esModule`, keys the target already owns, or Babel's `_exportNames`.
    fn skips_module_keys(&self) -> bool {
        self.skips_es_module || self.skips_target_own || self.export_names.is_some()
    }

    /// A helper cannot see the module's own exports at its call sites, so it
    /// must skip every key the target already owns (TypeScript
    /// `__exportStar`, SWC `_export_star`). An `__esModule` skip alone would
    /// let the copy overwrite a local export written before the call.
    fn is_helper_body(&self, create_bindings: &HashSet<BindingKey>) -> bool {
        self.skips_default
            && self.skips_target_own
            && self.export_names.is_none()
            && match self.action {
                Some(CopyAction::CreateBinding) => self
                    .create_binding
                    .as_ref()
                    .is_some_and(|helper| create_bindings.contains(&binding_key(helper))),
                Some(_) => true,
                None => false,
            }
    }

    /// A top-level loop over a required module that `export *` replaces.
    /// `index` is the loop's position in the module body.
    fn is_reexport(
        &self,
        index: usize,
        uses: &BindingUseIndex,
        local_exports: &LocalExports,
    ) -> bool {
        if !self.skips_default
            || !self.skips_module_keys()
            || !matches!(
                self.action,
                Some(CopyAction::Assign | CopyAction::DefineGetter)
            )
        {
            return false;
        }
        let Some(export_names) = &self.export_names else {
            // An `__esModule` skip alone does not protect the module's own
            // exports: the copy overwrites a local export written before the
            // loop, while in ESM the local export shadows the star export. A
            // top-level write after the loop overwrites the copy instead,
            // which matches ESM.
            return self.skips_target_own || local_exports.all_written_after(index);
        };
        // `_exportNames` lists the module's own exports, which shadow star
        // exports in ESM. A key it lists that the module never exports would
        // be skipped by the loop but re-exported by `export *`.
        let key = binding_key(export_names);
        uses.has_single_declaration(&key)
            && !uses.has_direct_write(&key)
            && local_exports.names_object_is_local(&key)
    }
}

/// `Object.keys(source).forEach(function (key) { ... })` copying into `target`.
fn match_keys_for_each(
    expr: &Expr,
    source: &Ident,
    target: CopyTarget,
    unresolved_mark: Mark,
) -> Option<CopyBody> {
    let Expr::Call(for_each) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(for_each_callee) = &for_each.callee else {
        return None;
    };
    let Expr::Member(for_each_member) = strip_parens(for_each_callee) else {
        return None;
    };
    if !matches!(&for_each_member.prop, MemberProp::Ident(prop) if prop.sym == "forEach") {
        return None;
    }
    let [callback] = for_each.args.as_slice() else {
        return None;
    };
    let Expr::Call(keys) = strip_parens(&for_each_member.obj) else {
        return None;
    };
    let Callee::Expr(keys_callee) = &keys.callee else {
        return None;
    };
    if callback.spread.is_some()
        || !is_unresolved_member_expr(keys_callee, "Object", "keys", unresolved_mark)
        || !matches!(keys.args.as_slice(), [arg]
            if arg.spread.is_none()
                && matches!(strip_parens(&arg.expr), Expr::Ident(id) if same_ident(id, source)))
    {
        return None;
    }
    let mut matcher = CopyMatcher::new(source, target, unresolved_mark);
    match strip_parens(&callback.expr) {
        Expr::Fn(function) => {
            let function = &function.function;
            if function.is_async || function.is_generator {
                return None;
            }
            let [param] = function.params.as_slice() else {
                return None;
            };
            let Pat::Ident(key) = &param.pat else {
                return None;
            };
            matcher.key = Some(key.id.clone());
            matcher.stmts(&function.body.as_ref()?.stmts)?;
        }
        Expr::Arrow(arrow) => {
            if arrow.is_async || arrow.is_generator {
                return None;
            }
            let [Pat::Ident(key)] = arrow.params.as_slice() else {
                return None;
            };
            matcher.key = Some(key.id.clone());
            match arrow.body.as_ref() {
                ArrowFunctionBody::FunctionBody(body) => matcher.stmts(&body.stmts)?,
                ArrowFunctionBody::Expr(expr) => matcher.expr(expr)?,
            }
        }
        _ => return None,
    }
    Some(matcher.body)
}

/// `for (var key in source) ...` copying into `target`.
fn match_for_in(
    for_in: &ForInStmt,
    source: &Ident,
    target: CopyTarget,
    unresolved_mark: Mark,
) -> Option<CopyBody> {
    if !matches!(strip_parens(&for_in.right), Expr::Ident(id) if same_ident(id, source)) {
        return None;
    }
    let ForHead::VarDecl(var) = &for_in.left else {
        return None;
    };
    let [VarDeclarator {
        name: Pat::Ident(key),
        init: None,
        ..
    }] = var.decls.as_slice()
    else {
        return None;
    };
    let mut matcher = CopyMatcher::new(source, target, unresolved_mark);
    matcher.key = Some(key.id.clone());
    matcher.stmts(std::slice::from_ref(for_in.body.as_ref()))?;
    Some(matcher.body)
}

struct CopyMatcher<'a> {
    source: &'a Ident,
    target: CopyTarget,
    key: Option<Ident>,
    unresolved_mark: Mark,
    body: CopyBody,
}

impl<'a> CopyMatcher<'a> {
    fn new(source: &'a Ident, target: CopyTarget, unresolved_mark: Mark) -> Self {
        Self {
            source,
            target,
            key: None,
            unresolved_mark,
            body: CopyBody {
                skips_default: false,
                skips_es_module: false,
                skips_target_own: false,
                export_names: None,
                action: None,
                create_binding: None,
            },
        }
    }

    fn key(&self) -> &Ident {
        self.key
            .as_ref()
            .expect("copy matcher key is set before matching")
    }

    fn is_key(&self, expr: &Expr) -> bool {
        matches!(strip_parens(expr), Expr::Ident(id) if same_ident(id, self.key()))
    }

    fn is_source(&self, expr: &Expr) -> bool {
        matches!(strip_parens(expr), Expr::Ident(id) if same_ident(id, self.source))
    }

    /// `object[key]`
    fn is_keyed_member(&self, expr: &Expr, object: impl Fn(&Expr) -> bool) -> bool {
        let Expr::Member(member) = strip_parens(expr) else {
            return false;
        };
        object(&member.obj)
            && matches!(&member.prop, MemberProp::Computed(computed) if self.is_key(&computed.expr))
    }

    /// Guard statements (`if (skip) return;`) followed by the guarded copy.
    fn stmts(&mut self, stmts: &[Stmt]) -> Option<()> {
        let (last, guards) = stmts.split_last()?;
        for guard in guards {
            let Stmt::If(if_stmt) = guard else {
                return None;
            };
            if if_stmt.alt.is_some() || !is_bare_return(&if_stmt.cons) {
                return None;
            }
            self.skip_when(&if_stmt.test, true)?;
        }
        match last {
            Stmt::If(if_stmt) if if_stmt.alt.is_none() => {
                self.skip_when(&if_stmt.test, false)?;
                self.stmts(std::slice::from_ref(if_stmt.cons.as_ref()))
            }
            Stmt::Block(block) => self.stmts(&block.stmts),
            Stmt::Expr(expr_stmt) => self.expr(&expr_stmt.expr),
            _ => None,
        }
    }

    /// `a && b && copy` / `a || b || copy`: every operand before the copy is a
    /// guard.
    fn expr(&mut self, expr: &Expr) -> Option<()> {
        match strip_parens(expr) {
            Expr::Bin(BinExpr {
                op: BinaryOp::LogicalAnd,
                left,
                right,
                ..
            }) => {
                self.skip_when(left, false)?;
                self.expr(right)
            }
            Expr::Bin(BinExpr {
                op: BinaryOp::LogicalOr,
                left,
                right,
                ..
            }) => {
                self.skip_when(left, true)?;
                self.expr(right)
            }
            expr => self.action(expr),
        }
    }

    /// Record that the loop skips the key when `expr` evaluates to `truthy`.
    fn skip_when(&mut self, expr: &Expr, truthy: bool) -> Option<()> {
        match strip_parens(expr) {
            Expr::Unary(UnaryExpr {
                op: UnaryOp::Bang,
                arg,
                ..
            }) => self.skip_when(arg, !truthy),
            // skip when `a || b` is truthy: skip when either is.
            Expr::Bin(BinExpr {
                op: BinaryOp::LogicalOr,
                left,
                right,
                ..
            }) if truthy => {
                self.skip_when(left, true)?;
                self.skip_when(right, true)
            }
            // skip when `a && b` is falsy: skip when either is.
            Expr::Bin(BinExpr {
                op: BinaryOp::LogicalAnd,
                left,
                right,
                ..
            }) if !truthy && !self.is_same_value_guard(expr) => {
                self.skip_when(left, false)?;
                self.skip_when(right, false)
            }
            expr => self.skip_atom(expr, truthy),
        }
    }

    fn skip_atom(&mut self, expr: &Expr, truthy: bool) -> Option<()> {
        if let Expr::Bin(binary) = expr {
            let skips_on_equal = match binary.op {
                BinaryOp::EqEqEq | BinaryOp::EqEq => truthy,
                BinaryOp::NotEqEq | BinaryOp::NotEq => !truthy,
                _ => return self.skip_positive_atom(expr, truthy),
            };
            if !skips_on_equal {
                return None;
            }
            let name = self.key_compared_string(binary)?;
            match name {
                "default" => self.body.skips_default = true,
                "__esModule" => self.body.skips_es_module = true,
                _ => return None,
            }
            return Some(());
        }
        self.skip_positive_atom(expr, truthy)
    }

    fn skip_positive_atom(&mut self, expr: &Expr, truthy: bool) -> Option<()> {
        if !truthy {
            return None;
        }
        // Babel's `key in target && target[key] === source[key]` only avoids a
        // redundant write; it is accepted but proves nothing.
        if self.is_same_value_guard(expr) {
            return Some(());
        }
        let object = self.has_own_property_object(expr)?;
        if self.target.matches(object) {
            self.body.skips_target_own = true;
            return Some(());
        }
        let Expr::Ident(names) = strip_parens(object) else {
            return None;
        };
        if self.is_source(object)
            || self
                .body
                .export_names
                .as_ref()
                .is_some_and(|existing| !same_ident(existing, names))
        {
            return None;
        }
        self.body.export_names = Some(names.clone());
        Some(())
    }

    /// `key === "name"` or `"name" === key`.
    fn key_compared_string<'e>(&self, binary: &'e BinExpr) -> Option<&'e str> {
        let string = |expr: &'e Expr| match strip_parens(expr) {
            Expr::Lit(Lit::Str(value)) => value.value.as_str(),
            _ => None,
        };
        if self.is_key(&binary.left) {
            string(&binary.right)
        } else if self.is_key(&binary.right) {
            string(&binary.left)
        } else {
            None
        }
    }

    /// `key in target && target[key] === source[key]`
    fn is_same_value_guard(&self, expr: &Expr) -> bool {
        let Expr::Bin(BinExpr {
            op: BinaryOp::LogicalAnd,
            left,
            right,
            ..
        }) = strip_parens(expr)
        else {
            return false;
        };
        let Expr::Bin(in_expr) = strip_parens(left) else {
            return false;
        };
        let Expr::Bin(equal) = strip_parens(right) else {
            return false;
        };
        in_expr.op == BinaryOp::In
            && self.is_key(&in_expr.left)
            && self.target.matches(&in_expr.right)
            && equal.op == BinaryOp::EqEqEq
            && self.is_keyed_member(&equal.left, |object| self.target.matches(object))
            && self.is_keyed_member(&equal.right, |object| self.is_source(object))
    }

    /// `Object.prototype.hasOwnProperty.call(object, key)` or
    /// `object.hasOwnProperty(key)`; returns `object`.
    fn has_own_property_object<'e>(&self, expr: &'e Expr) -> Option<&'e Expr> {
        let Expr::Call(call) = strip_parens(expr) else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Member(member) = strip_parens(callee) else {
            return None;
        };
        let MemberProp::Ident(prop) = &member.prop else {
            return None;
        };
        if call.args.iter().any(|arg| arg.spread.is_some()) {
            return None;
        }
        match (prop.sym.as_ref(), call.args.as_slice()) {
            ("call", [object, key]) if self.is_key(&key.expr) => {
                let Expr::Member(has_own) = strip_parens(&member.obj) else {
                    return None;
                };
                let is_has_own = matches!(&has_own.prop, MemberProp::Ident(prop) if prop.sym == "hasOwnProperty")
                    && is_unresolved_member_expr(
                        &has_own.obj,
                        "Object",
                        "prototype",
                        self.unresolved_mark,
                    );
                is_has_own.then_some(object.expr.as_ref())
            }
            ("hasOwnProperty", [key]) if self.is_key(&key.expr) => Some(member.obj.as_ref()),
            _ => None,
        }
    }

    fn action(&mut self, expr: &Expr) -> Option<()> {
        let action = match expr {
            Expr::Assign(assign) if assign.op == AssignOp::Assign => {
                let AssignTarget::Simple(SimpleAssignTarget::Member(left)) = &assign.left else {
                    return None;
                };
                let left_matches = self.target.matches(&left.obj)
                    && matches!(&left.prop, MemberProp::Computed(computed) if self.is_key(&computed.expr));
                if !left_matches
                    || !self.is_keyed_member(&assign.right, |object| self.is_source(object))
                {
                    return None;
                }
                CopyAction::Assign
            }
            Expr::Call(call) => self.call_action(call)?,
            _ => return None,
        };
        if self.body.action.replace(action).is_some() {
            return None;
        }
        Some(())
    }

    fn call_action(&mut self, call: &CallExpr) -> Option<CopyAction> {
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        if call.args.iter().any(|arg| arg.spread.is_some()) {
            return None;
        }
        if is_unresolved_member_expr(callee, "Object", "defineProperty", self.unresolved_mark) {
            let [target, key, descriptor] = call.args.as_slice() else {
                return None;
            };
            return (self.target.matches(&target.expr)
                && self.is_key(&key.expr)
                && self.is_source_getter_descriptor(&descriptor.expr))
            .then_some(CopyAction::DefineGetter);
        }
        // TypeScript's `__createBinding(target, source, key)`; the caller
        // checks that the callee is a proven `__createBinding` helper.
        let Expr::Ident(helper) = strip_parens(callee) else {
            return None;
        };
        let [target, source, key] = call.args.as_slice() else {
            return None;
        };
        if !self.target.matches(&target.expr)
            || !self.is_source(&source.expr)
            || !self.is_key(&key.expr)
        {
            return None;
        }
        self.body.create_binding = Some(helper.clone());
        Some(CopyAction::CreateBinding)
    }

    /// `{ enumerable: true, get: function () { return source[key]; } }`
    fn is_source_getter_descriptor(&self, expr: &Expr) -> bool {
        let Expr::Object(object) = strip_parens(expr) else {
            return false;
        };
        let mut enumerable = false;
        let mut getter = false;
        for prop in &object.props {
            let PropOrSpread::Prop(prop) = prop else {
                return false;
            };
            match prop.as_ref() {
                Prop::KeyValue(KeyValueProp { key, value }) if prop_name_is(key, "enumerable") => {
                    if !is_true_literal(value) {
                        return false;
                    }
                    enumerable = true;
                }
                Prop::KeyValue(KeyValueProp { key, value }) if prop_name_is(key, "get") => {
                    if !self.returns_source_key(value) {
                        return false;
                    }
                    getter = true;
                }
                Prop::Method(MethodProp { key, function }) if prop_name_is(key, "get") => {
                    if function.is_async
                        || function.is_generator
                        || !function.params.is_empty()
                        || !function
                            .body
                            .as_ref()
                            .is_some_and(|body| self.body_returns_source_key(&body.stmts))
                    {
                        return false;
                    }
                    getter = true;
                }
                _ => return false,
            }
        }
        enumerable && getter
    }

    fn returns_source_key(&self, expr: &Expr) -> bool {
        match strip_parens(expr) {
            Expr::Fn(function) => {
                let function = &function.function;
                !function.is_async
                    && !function.is_generator
                    && function.params.is_empty()
                    && function
                        .body
                        .as_ref()
                        .is_some_and(|body| self.body_returns_source_key(&body.stmts))
            }
            Expr::Arrow(arrow) => {
                !arrow.is_async
                    && !arrow.is_generator
                    && arrow.params.is_empty()
                    && match arrow.body.as_ref() {
                        ArrowFunctionBody::FunctionBody(body) => {
                            self.body_returns_source_key(&body.stmts)
                        }
                        ArrowFunctionBody::Expr(expr) => {
                            self.is_keyed_member(expr, |object| self.is_source(object))
                        }
                    }
            }
            _ => false,
        }
    }

    fn body_returns_source_key(&self, stmts: &[Stmt]) -> bool {
        matches!(stmts, [Stmt::Return(ReturnStmt { arg: Some(arg), .. })]
            if self.is_keyed_member(arg, |object| self.is_source(object)))
    }
}

fn is_bare_return(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(ReturnStmt { arg: None, .. }) => true,
        Stmt::Block(block) => matches!(
            block.stmts.as_slice(),
            [Stmt::Return(ReturnStmt { arg: None, .. })]
        ),
        _ => false,
    }
}

fn is_true_literal(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Lit(Lit::Bool(value)) => value.value,
        // `!0` before UnminifyBooleans has run.
        Expr::Unary(UnaryExpr {
            op: UnaryOp::Bang,
            arg,
            ..
        }) => matches!(strip_parens(arg), Expr::Lit(Lit::Num(num)) if num.value == 0.0),
        _ => false,
    }
}

fn prop_name_is(name: &PropName, expected: &str) -> bool {
    match name {
        PropName::Ident(ident) => ident.sym == expected,
        PropName::Str(value) => value.value.as_str() == Some(expected),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Babel `_exportNames`
// ---------------------------------------------------------------------------

/// What the module exports itself, for checking Babel's `_exportNames`.
struct LocalExports {
    /// `exports.name = ...` and `Object.defineProperty(exports, "name", ...)`
    /// anywhere in the module.
    names: HashSet<Atom>,
    /// Each such write with the module-body index of its statement, or
    /// `None` inside a function or class body, which may run at any time.
    writes: Vec<(Atom, Option<usize>)>,
    /// Top-level `var x = { a: true, ... }` objects, keyed by binding.
    names_objects: HashMap<BindingKey, Vec<Atom>>,
}

impl LocalExports {
    fn collect(module: &Module, unresolved_mark: Mark) -> Self {
        let writes = collect_local_export_writes(module, unresolved_mark);
        Self {
            names: writes.iter().map(|(name, _)| name.clone()).collect(),
            writes,
            names_objects: collect_export_names_objects(module),
        }
    }

    /// The module writes an export other than `default` and `__esModule`,
    /// which every accepted loop skips.
    /// Every export other than `default` and `__esModule` (which every
    /// accepted loop skips) is written by a module-body statement after
    /// `index`.
    fn all_written_after(&self, index: usize) -> bool {
        self.writes.iter().all(|(name, position)| {
            matches!(name.as_ref(), "default" | "__esModule")
                || position.is_some_and(|position| position > index)
        })
    }

    /// `var _exportNames = { a: true, ... }` lists only names this module
    /// exports itself.
    fn names_object_is_local(&self, key: &BindingKey) -> bool {
        self.names_objects
            .get(key)
            .is_some_and(|keys| keys.iter().all(|name| self.names.contains(name)))
    }
}

fn collect_local_export_writes(
    module: &Module,
    unresolved_mark: Mark,
) -> Vec<(Atom, Option<usize>)> {
    struct Collector {
        unresolved_mark: Mark,
        writes: Vec<(Atom, Option<usize>)>,
        index: usize,
        function_depth: usize,
    }
    impl Collector {
        fn record(&mut self, name: Atom) {
            let position = (self.function_depth == 0).then_some(self.index);
            self.writes.push((name, position));
        }
    }
    impl Visit for Collector {
        fn visit_module_items(&mut self, items: &[ModuleItem]) {
            for (index, item) in items.iter().enumerate() {
                self.index = index;
                item.visit_with(self);
            }
        }

        fn visit_function(&mut self, function: &Function) {
            self.function_depth += 1;
            function.visit_children_with(self);
            self.function_depth -= 1;
        }

        fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
            self.function_depth += 1;
            arrow.visit_children_with(self);
            self.function_depth -= 1;
        }

        fn visit_class(&mut self, class: &swc_core::ecma::ast::Class) {
            self.function_depth += 1;
            class.visit_children_with(self);
            self.function_depth -= 1;
        }

        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left {
                if matches!(member.obj.as_ref(), Expr::Ident(id)
                    if is_unresolved_ident(id, "exports", self.unresolved_mark))
                {
                    if let Some(name) = is_ident_prop(&member.prop) {
                        self.record(name);
                    }
                }
            }
            assign.visit_children_with(self);
        }

        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if is_unresolved_member_expr(
                    callee,
                    "Object",
                    "defineProperty",
                    self.unresolved_mark,
                ) {
                    if let [target, name, ..] = call.args.as_slice() {
                        if matches!(strip_parens(&target.expr), Expr::Ident(id)
                            if is_unresolved_ident(id, "exports", self.unresolved_mark))
                        {
                            if let Expr::Lit(Lit::Str(name)) = strip_parens(&name.expr) {
                                if let Some(name) = name.value.as_str() {
                                    self.record(Atom::from(name));
                                }
                            }
                        }
                    }
                }
            }
            call.visit_children_with(self);
        }
    }
    let mut collector = Collector {
        unresolved_mark,
        writes: Vec::new(),
        index: 0,
        function_depth: 0,
    };
    module.visit_with(&mut collector);
    collector.writes
}

/// Top-level `var x = { a: true, ... }` objects, keyed by binding.
fn collect_export_names_objects(module: &Module) -> HashMap<BindingKey, Vec<Atom>> {
    let mut objects = HashMap::default();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let (Pat::Ident(binding), Some(init)) = (&decl.name, decl.init.as_deref()) else {
                continue;
            };
            let Expr::Object(object) = strip_parens(init) else {
                continue;
            };
            let mut keys = Vec::with_capacity(object.props.len());
            let all_true = object.props.iter().all(|prop| {
                let PropOrSpread::Prop(prop) = prop else {
                    return false;
                };
                let Prop::KeyValue(KeyValueProp { key, value }) = prop.as_ref() else {
                    return false;
                };
                let name = match key {
                    PropName::Ident(ident) => ident.sym.clone(),
                    PropName::Str(value) => match value.value.as_str() {
                        Some(name) => Atom::from(name),
                        None => return false,
                    },
                    _ => return false,
                };
                keys.push(name);
                is_true_literal(value)
            });
            if all_true {
                objects.insert(binding_key(&binding.id), keys);
            }
        }
    }
    objects
}
