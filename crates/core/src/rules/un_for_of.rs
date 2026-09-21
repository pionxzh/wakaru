use crate::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Mark, Span, Spanned, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayPat, AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, BindingIdent, BlockStmt,
    CallExpr, Callee, Decl, Expr, ExprOrSpread, ExprStmt, ForHead, ForOfStmt, Ident,
    ImportSpecifier, Lit, MemberExpr, MemberProp, Module, ModuleDecl, ModuleItem, ObjectPatProp,
    Pat, SimpleAssignTarget, Stmt, TryStmt, UnaryExpr, UnaryOp, UpdateExpr, UpdateOp, VarDecl,
    VarDeclKind, VarDeclOrExpr, VarDeclarator,
};
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_uses::BindingUseIndex;
use crate::facts::{ModuleFactsMap, TypeScriptHelperKind};

use super::eval_utils::{js_source_mentions_binding, module_has_with_stmt, DirectEvalAnalyzer};
use super::helper_matcher::{binding_key, static_member_prop_name, BindingKey};
use super::rename_utils::{rename_bindings, BindingRename};
use super::transpiler_helper_utils::{
    tslib_member_ts_helper_kind, tslib_require_ts_helper_kind, LocalHelperContext, TsHelperKind,
};
use super::un_for_await;
use super::RewriteLevel;

use crate::utils::paren::strip_parens;

/// Convert TypeScript/Babel array-index downlevel `for` loops back to `for...of`:
///
/// ```js
/// for (let i = 0, arr = expr; i < arr.length; i++) {
///     const elem = arr[i];
///     // body...
/// }
/// // →
/// for (const elem of expr) {
///     // body...
/// }
/// ```
pub struct UnForOf<'a> {
    level: RewriteLevel,
    unresolved_mark: Option<Mark>,
    module_facts: Option<&'a ModuleFactsMap>,
    current_filename: Option<&'a str>,
    helper_context: ForOfHelperContext,
}

impl UnForOf<'_> {
    pub fn new(level: RewriteLevel) -> Self {
        Self {
            level,
            unresolved_mark: None,
            module_facts: None,
            current_filename: None,
            helper_context: ForOfHelperContext::default(),
        }
    }

    pub fn new_with_mark(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            level,
            unresolved_mark: Some(unresolved_mark),
            module_facts: None,
            current_filename: None,
            helper_context: ForOfHelperContext::default(),
        }
    }
}

impl<'a> UnForOf<'a> {
    pub fn new_with_mark_and_facts(
        unresolved_mark: Mark,
        level: RewriteLevel,
        module_facts: &'a ModuleFactsMap,
    ) -> Self {
        Self {
            level,
            unresolved_mark: Some(unresolved_mark),
            module_facts: Some(module_facts),
            current_filename: None,
            helper_context: ForOfHelperContext::default(),
        }
    }

    pub(crate) fn set_current_filename(&mut self, current_filename: Option<&'a str>) {
        self.current_filename = current_filename;
    }

    pub(crate) fn run_with_helpers(
        &mut self,
        module: &mut Module,
        local_helpers: &LocalHelperContext,
    ) {
        if !Self::should_run_with_level(self.level, module) {
            return;
        }
        let helper_context = ForOfHelperContext::from_local_helpers(
            module,
            local_helpers,
            self.unresolved_mark,
            self.module_facts,
            self.current_filename,
        );
        let previous = std::mem::replace(&mut self.helper_context, helper_context);
        module.visit_mut_children_with(self);
        let helper_context = std::mem::replace(&mut self.helper_context, previous);
        // Dependency-aware cleanup first: it walks the top-level reference
        // graph from the adapter helper, so the helper must still be declared.
        if helper_context.rewrote_async_iterator_loop.get() {
            un_for_await::remove_consumed_async_iterator_helpers(
                module,
                local_helpers,
                &helper_context,
            );
        }
        local_helpers.remove_unused_inline_ts_helpers(
            module,
            &[TsHelperKind::Values, TsHelperKind::AsyncValues],
        );
    }
}

#[derive(Clone, Default)]
pub(super) struct ForOfHelperContext {
    values_helpers: HashSet<BindingKey>,
    /// tslib `__asyncValues` bindings (inline, imported, or required).
    pub(super) async_values_helpers: HashSet<BindingKey>,
    /// Babel `_asyncIterator` / SWC `_async_iterator` callee identities live in
    /// the shared helper context; kept whole so member callee forms resolve.
    pub(super) local_helpers: LocalHelperContext,
    /// esbuild `__forAwait` adapters and the `__knownSymbol` lookup they call,
    /// matched by body shape (the names are mangled in minified output).
    pub(super) esbuild_for_await_helpers: HashSet<BindingKey>,
    pub(super) esbuild_known_symbol_helpers: HashSet<BindingKey>,
    /// Set when an async iterator protocol was folded into `for await`, so the
    /// consumed helper declarations are removed once the walk is done.
    pub(super) rewrote_async_iterator_loop: std::cell::Cell<bool>,
    tslib_namespaces: HashSet<BindingKey>,
    cross_module_values_namespaces: HashMap<BindingKey, HashSet<String>>,
    closure_jscomp_namespaces: HashSet<BindingKey>,
    pub(super) binding_uses: BindingUseIndex,
    unresolved_mark: Option<Mark>,
    /// Module-wide `with` / unknown direct `eval`, plus known eval sources.
    /// Used when dropping a function-scoped (`var`) for-of left.
    module_has_with: bool,
    unknown_direct_eval: bool,
    known_direct_eval_sources: Vec<String>,
}

impl ForOfHelperContext {
    fn from_local_helpers(
        module: &Module,
        local_helpers: &LocalHelperContext,
        unresolved_mark: Option<Mark>,
        module_facts: Option<&ModuleFactsMap>,
        current_filename: Option<&str>,
    ) -> Self {
        let cross_module_values = module_facts
            .map(|facts| collect_cross_module_values_refs(module, facts, current_filename))
            .unwrap_or_default();
        let mut values_helpers = local_helpers.ts_helpers_of_kind(TsHelperKind::Values);
        values_helpers.extend(cross_module_values.direct);
        let mut eval = DirectEvalAnalyzer::default();
        module.visit_with(&mut eval);
        let (esbuild_for_await_helpers, esbuild_known_symbol_helpers) =
            un_for_await::collect_esbuild_for_await_helpers(module);
        Self {
            values_helpers,
            async_values_helpers: local_helpers.ts_helpers_of_kind(TsHelperKind::AsyncValues),
            local_helpers: local_helpers.clone(),
            esbuild_for_await_helpers,
            esbuild_known_symbol_helpers,
            rewrote_async_iterator_loop: std::cell::Cell::new(false),
            tslib_namespaces: local_helpers.tslib_namespaces().clone(),
            cross_module_values_namespaces: cross_module_values.namespaces,
            closure_jscomp_namespaces: collect_closure_jscomp_namespaces(module),
            binding_uses: BindingUseIndex::collect(module),
            unresolved_mark,
            module_has_with: module_has_with_stmt(module),
            unknown_direct_eval: eval.unknown_direct_eval,
            known_direct_eval_sources: eval.known_direct_eval_sources,
        }
    }

    /// A `var` for-of left is visible to `with` and direct `eval` anywhere
    /// in the module. Known eval sources only block when they mention `name`.
    fn dynamic_scope_can_observe_name(&self, name: &Atom) -> bool {
        self.module_has_with
            || self.unknown_direct_eval
            || self
                .known_direct_eval_sources
                .iter()
                .any(|source| js_source_mentions_binding(source, name))
    }

    fn is_ts_values_callee(&self, callee: &Callee) -> bool {
        let Callee::Expr(callee_expr) = callee else {
            return false;
        };
        self.is_ts_values_callee_expr(callee_expr)
    }

    fn is_ts_values_callee_expr(&self, expr: &Expr) -> bool {
        match strip_parens(expr) {
            Expr::Ident(id) => self.values_helpers.contains(&binding_key(id)),
            Expr::Member(_) => {
                tslib_member_ts_helper_kind(expr, &self.tslib_namespaces)
                    == Some(TsHelperKind::Values)
                    || tslib_require_ts_helper_kind(expr, self.unresolved_mark)
                        == Some(TsHelperKind::Values)
                    || is_cross_module_values_member(expr, &self.cross_module_values_namespaces)
            }
            _ => false,
        }
    }

    /// tslib `__asyncValues(iterable)` callee: inline declaration, tslib
    /// import specifier, or a `tslib.__asyncValues` member on a proven
    /// namespace. Cross-module facts are not consulted for the async helper.
    pub(super) fn is_ts_async_values_callee_expr(&self, expr: &Expr) -> bool {
        match strip_parens(expr) {
            Expr::Ident(id) => self.async_values_helpers.contains(&binding_key(id)),
            Expr::Member(_) => {
                tslib_member_ts_helper_kind(expr, &self.tslib_namespaces)
                    == Some(TsHelperKind::AsyncValues)
                    || tslib_require_ts_helper_kind(expr, self.unresolved_mark)
                        == Some(TsHelperKind::AsyncValues)
            }
            _ => false,
        }
    }

    fn is_closure_make_iterator_callee(&self, callee: &Callee) -> bool {
        let Callee::Expr(callee) = callee else {
            return false;
        };
        let callee = strip_closure_indirect_call(callee);
        let Expr::Member(member) = callee else {
            return false;
        };
        if static_member_prop_name(&member.prop) != Some("makeIterator") {
            return false;
        }
        let Expr::Ident(namespace) = strip_parens(&member.obj) else {
            return false;
        };
        if namespace.sym.as_ref() != "$jscomp" {
            return false;
        }

        self.closure_jscomp_namespaces
            .contains(&binding_key(namespace))
            || self
                .unresolved_mark
                .is_some_and(|mark| namespace.ctxt.outer() == mark)
    }

    fn binding_is_used_outside(&self, stmts: &[Stmt], ident: &Ident) -> bool {
        let binding = binding_key(ident);
        BindingUseIndex::collect_stmts(stmts).use_count(&binding)
            != self.binding_uses.use_count(&binding)
    }
}

fn strip_closure_indirect_call(expr: &Expr) -> &Expr {
    let expr = strip_parens(expr);
    let Expr::Seq(sequence) = expr else {
        return expr;
    };
    let [first, callee] = sequence.exprs.as_slice() else {
        return expr;
    };
    if matches!(strip_parens(first), Expr::Lit(Lit::Num(number)) if number.value == 0.0) {
        strip_parens(callee)
    } else {
        expr
    }
}

fn collect_closure_jscomp_namespaces(module: &Module) -> HashSet<BindingKey> {
    struct Collector {
        namespaces: HashSet<BindingKey>,
    }

    impl Visit for Collector {
        fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
            let Pat::Ident(binding) = &decl.name else {
                decl.visit_children_with(self);
                return;
            };
            if binding.id.sym.as_ref() != "$jscomp" {
                decl.visit_children_with(self);
                return;
            }
            let Some(Expr::Bin(bootstrap)) = decl.init.as_deref().map(strip_parens) else {
                decl.visit_children_with(self);
                return;
            };
            if bootstrap.op != BinaryOp::LogicalOr
                || !is_ident_key(strip_parens(&bootstrap.left), &binding.id)
                || !matches!(strip_parens(&bootstrap.right), Expr::Object(object) if object.props.is_empty())
            {
                decl.visit_children_with(self);
                return;
            }
            self.namespaces.insert(binding_key(&binding.id));
            decl.visit_children_with(self);
        }
    }

    let mut collector = Collector {
        namespaces: HashSet::default(),
    };
    module.visit_with(&mut collector);
    collector.namespaces
}

#[derive(Default)]
struct CrossModuleValuesRefs {
    direct: HashSet<BindingKey>,
    namespaces: HashMap<BindingKey, HashSet<String>>,
}

fn collect_cross_module_values_refs(
    module: &Module,
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
) -> CrossModuleValuesRefs {
    let mut refs = CrossModuleValuesRefs::default();
    let mut namespace_factories: HashMap<BindingKey, HashSet<String>> = HashMap::default();

    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        let source = Atom::from(import.src.value.as_str().unwrap_or(""));
        let exported_values = ts_helper_export_names(
            module_facts,
            current_filename,
            &source,
            TypeScriptHelperKind::Values,
        );
        if exported_values.is_empty() {
            continue;
        }

        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Default(default) => {
                    let local = binding_key(&default.local);
                    if module_exports_ts_helper(
                        module_facts,
                        current_filename,
                        &source,
                        "default",
                        TypeScriptHelperKind::Values,
                    ) {
                        refs.direct.insert(local);
                    } else {
                        namespace_factories.insert(local, exported_values.clone());
                    }
                }
                ImportSpecifier::Named(named) => {
                    let imported = named
                        .imported
                        .as_ref()
                        .map(export_name_to_atom)
                        .unwrap_or_else(|| named.local.sym.clone());
                    let local = binding_key(&named.local);
                    if module_exports_ts_helper(
                        module_facts,
                        current_filename,
                        &source,
                        imported.as_ref(),
                        TypeScriptHelperKind::Values,
                    ) {
                        refs.direct.insert(local);
                    } else {
                        namespace_factories.insert(local, exported_values.clone());
                    }
                }
                ImportSpecifier::Namespace(namespace) => {
                    refs.namespaces
                        .insert(binding_key(&namespace.local), exported_values.clone());
                }
            }
        }
    }

    if namespace_factories.is_empty() {
        return refs;
    }

    struct NamespaceFactoryUseCollector<'a, 'b> {
        namespace_factories: &'a HashMap<BindingKey, HashSet<String>>,
        refs: &'b mut CrossModuleValuesRefs,
    }

    impl Visit for NamespaceFactoryUseCollector<'_, '_> {
        fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
            let Pat::Ident(binding) = &decl.name else {
                decl.visit_children_with(self);
                return;
            };
            let Some(init) = decl.init.as_deref() else {
                decl.visit_children_with(self);
                return;
            };
            let Some(factory) = zero_arg_call_ident(init) else {
                decl.visit_children_with(self);
                return;
            };
            if let Some(exports) = self.namespace_factories.get(&binding_key(factory)) {
                self.refs
                    .namespaces
                    .insert(binding_key(&binding.id), exports.clone());
            }
            decl.visit_children_with(self);
        }
    }

    let mut collector = NamespaceFactoryUseCollector {
        namespace_factories: &namespace_factories,
        refs: &mut refs,
    };
    module.visit_with(&mut collector);

    refs
}

fn module_exports_ts_helper(
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
    source: &Atom,
    exported: &str,
    kind: TypeScriptHelperKind,
) -> bool {
    module_facts
        .get_from(current_filename, source.as_ref())
        .is_some_and(|facts| {
            facts
                .ts_helper_exports
                .iter()
                .any(|helper| helper.exported.as_ref() == exported && helper.kind == kind)
        })
}

fn ts_helper_export_names(
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
    source: &Atom,
    kind: TypeScriptHelperKind,
) -> HashSet<String> {
    module_facts
        .get_from(current_filename, source.as_ref())
        .map(|facts| {
            facts
                .ts_helper_exports
                .iter()
                .filter(|helper| helper.kind == kind)
                .map(|helper| helper.exported.to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn zero_arg_call_ident(expr: &Expr) -> Option<&Ident> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    if !call.args.is_empty() {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(id) = strip_parens(callee) else {
        return None;
    };
    Some(id)
}

fn is_cross_module_values_member(
    expr: &Expr,
    namespaces: &HashMap<BindingKey, HashSet<String>>,
) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    let Expr::Ident(obj) = strip_parens(&member.obj) else {
        return false;
    };
    let Some(exported) = static_member_prop_name(&member.prop) else {
        return false;
    };
    namespaces
        .get(&binding_key(obj))
        .is_some_and(|exports| exports.contains(exported))
}

fn export_name_to_atom(name: &swc_core::ecma::ast::ModuleExportName) -> Atom {
    match name {
        swc_core::ecma::ast::ModuleExportName::Ident(id) => id.sym.clone(),
        swc_core::ecma::ast::ModuleExportName::Str(s) => Atom::from(s.value.as_str().unwrap_or("")),
    }
}

impl Default for UnForOf<'_> {
    fn default() -> Self {
        Self::new(RewriteLevel::Standard)
    }
}

impl UnForOf<'_> {
    pub(crate) fn should_run_with_level(level: RewriteLevel, module: &Module) -> bool {
        level >= RewriteLevel::Standard && Self::should_run(module)
    }

    fn should_run(module: &swc_core::ecma::ast::Module) -> bool {
        struct Scan {
            found: bool,
        }
        impl Visit for Scan {
            fn visit_stmt(&mut self, stmt: &Stmt) {
                if self.found {
                    return;
                }
                if matches!(stmt, Stmt::For(_) | Stmt::ForOf(_) | Stmt::Try(_)) {
                    self.found = true;
                    return;
                }
                stmt.visit_children_with(self);
            }
        }
        let mut scan = Scan { found: false };
        module.visit_with(&mut scan);
        scan.found
    }
}

impl VisitMut for UnForOf<'_> {
    fn visit_mut_module(&mut self, module: &mut Module) {
        if !Self::should_run_with_level(self.level, module) {
            return;
        }
        let local_helpers = self.unresolved_mark.map_or_else(
            || LocalHelperContext::collect(module),
            |mark| LocalHelperContext::collect_with_mark(module, mark),
        );
        self.run_with_helpers(module, &local_helpers);
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        let old = std::mem::take(items);
        let mut stmt_run = Vec::new();

        for item in old {
            match item {
                ModuleItem::Stmt(stmt) => stmt_run.push(stmt),
                item => {
                    flush_stmt_run(items, &mut stmt_run, &self.helper_context);
                    items.push(item);
                }
            }
        }
        flush_stmt_run(items, &mut stmt_run, &self.helper_context);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        process_stmt_vec(stmts, &self.helper_context);
    }

    fn visit_mut_stmt(&mut self, stmt: &mut Stmt) {
        stmt.visit_mut_children_with(self);

        if let Some(for_of) = try_convert_for_of(stmt, &self.helper_context) {
            *stmt = Stmt::ForOf(for_of);
        }
        if let Stmt::ForOf(for_of) = stmt {
            try_fold_for_of_entry_slots(for_of, &self.helper_context);
        }
    }
}

fn flush_stmt_run(
    items: &mut Vec<ModuleItem>,
    stmts: &mut Vec<Stmt>,
    helper_context: &ForOfHelperContext,
) {
    if stmts.is_empty() {
        return;
    }
    process_stmt_vec(stmts, helper_context);
    items.extend(std::mem::take(stmts).into_iter().map(ModuleItem::Stmt));
}

fn has_for_of_sequence_candidates(stmts: &[Stmt]) -> bool {
    stmts
        .iter()
        .any(|stmt| matches!(stmt, Stmt::For(_) | Stmt::Try(_)))
}

fn process_stmt_vec(stmts: &mut Vec<Stmt>, helper_context: &ForOfHelperContext) {
    if !has_for_of_sequence_candidates(stmts) {
        return;
    }
    un_for_await::rewrite_async_iterator_loops(stmts, helper_context);
    let old = std::mem::take(stmts);
    let mut i = 0;
    while i < old.len() {
        if let Some(rewrite) = try_convert_closure_iterator_for_stmt(&old[i..], helper_context) {
            stmts.push(Stmt::ForOf(rewrite.for_of));
            i += rewrite.consumed_stmts;
            continue;
        }

        if let Some(rewrite) = try_convert_closure_iterator_sequence(&old[i..], helper_context) {
            stmts.push(Stmt::ForOf(rewrite.for_of));
            i += rewrite.consumed_stmts;
            continue;
        }

        if let Some(rewrite) = try_convert_ts_values_sequence(&old[i..], helper_context) {
            stmts.push(Stmt::ForOf(rewrite.for_of));
            i += rewrite.consumed_stmts;
            continue;
        }

        if let Some(rewrite) = try_convert_swc_iterator_sequence(&old[i..], helper_context) {
            stmts.push(Stmt::ForOf(rewrite.for_of));
            i += rewrite.consumed_stmts;
            continue;
        }

        if let Some(rewrite) = try_convert_iterator_helper_sequence(&old[i..], helper_context) {
            stmts.extend(rewrite.preserved_stmts);
            stmts.push(Stmt::ForOf(rewrite.for_of));
            i += rewrite.consumed_stmts;
            continue;
        }

        if let Some(rewrite) = try_convert_loose_iterator_sequence(&old[i..], helper_context) {
            stmts.push(Stmt::ForOf(rewrite.for_of));
            i += rewrite.consumed_stmts;
            continue;
        }

        let stmt = old[i].clone();
        if let Some(for_of) = try_convert_for_of(&stmt, helper_context) {
            stmts.push(Stmt::ForOf(for_of));
        } else {
            stmts.push(stmt);
        }
        i += 1;
    }
}

struct SequenceRewrite {
    consumed_stmts: usize,
    preserved_stmts: Vec<Stmt>,
    for_of: ForOfStmt,
}

struct ClosureIteratorInit {
    iterator_ident: Ident,
    iterable: Box<Expr>,
}

fn try_convert_closure_iterator_sequence(
    stmts: &[Stmt],
    helper_context: &ForOfHelperContext,
) -> Option<SequenceRewrite> {
    let init = extract_closure_iterator_init(stmts.first()?, helper_context)?;
    let Stmt::For(for_stmt) = stmts.get(1)? else {
        return None;
    };
    let Some(VarDeclOrExpr::VarDecl(loop_init)) = &for_stmt.init else {
        return None;
    };
    let [result_decl] = loop_init.decls.as_slice() else {
        return None;
    };
    let result_ident = pat_as_ident(&result_decl.name)?.id.clone();
    let consumed_stmts = &stmts[..2];
    if helper_context.binding_is_used_outside(consumed_stmts, &init.iterator_ident)
        || helper_context.binding_is_used_outside(consumed_stmts, &result_ident)
    {
        return None;
    }
    let for_of = build_closure_iterator_for_of(
        for_stmt,
        &init.iterator_ident,
        &result_ident,
        result_decl.init.as_deref()?,
        init.iterable,
        stmts[0].span(),
        helper_context,
    )?;
    Some(SequenceRewrite {
        consumed_stmts: 2,
        preserved_stmts: Vec::new(),
        for_of,
    })
}

fn try_convert_closure_iterator_for_stmt(
    stmts: &[Stmt],
    helper_context: &ForOfHelperContext,
) -> Option<SequenceRewrite> {
    let Stmt::For(for_stmt) = stmts.first()? else {
        return None;
    };
    let Some(VarDeclOrExpr::VarDecl(loop_init)) = &for_stmt.init else {
        return None;
    };
    let [iterator_decl, result_decl] = loop_init.decls.as_slice() else {
        return None;
    };
    let iterator_ident = pat_as_ident(&iterator_decl.name)?.id.clone();
    let result_ident = pat_as_ident(&result_decl.name)?.id.clone();
    let iterable =
        extract_closure_make_iterator_arg(iterator_decl.init.as_deref()?, helper_context)?;
    let consumed_stmts = &stmts[..1];
    if helper_context.binding_is_used_outside(consumed_stmts, &iterator_ident)
        || helper_context.binding_is_used_outside(consumed_stmts, &result_ident)
    {
        return None;
    }
    let for_of = build_closure_iterator_for_of(
        for_stmt,
        &iterator_ident,
        &result_ident,
        result_decl.init.as_deref()?,
        iterable,
        stmts[0].span(),
        helper_context,
    )?;
    Some(SequenceRewrite {
        consumed_stmts: 1,
        preserved_stmts: Vec::new(),
        for_of,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_closure_iterator_for_of(
    for_stmt: &swc_core::ecma::ast::ForStmt,
    iterator_ident: &Ident,
    result_ident: &Ident,
    result_init: &Expr,
    iterable: Box<Expr>,
    span: Span,
    helper_context: &ForOfHelperContext,
) -> Option<ForOfStmt> {
    if !is_iterator_next_call(result_init, iterator_ident)
        || !is_not_done_test(for_stmt.test.as_deref()?, result_ident)
        || !for_stmt
            .update
            .as_deref()
            .is_some_and(|update| is_iterator_next_update(update, result_ident, iterator_ident))
    {
        return None;
    }

    // Removing the iterator temporary is only safe when neither the iterator
    // nor its result object is observable outside the canonical loop machinery.
    if stmt_uses_ident_key(&for_stmt.body, iterator_ident) {
        return None;
    }

    let loop_body = match &*for_stmt.body {
        Stmt::Block(block) => block.clone(),
        body => BlockStmt {
            span: body.span(),
            ctxt: Default::default(),
            stmts: vec![body.clone()],
        },
    };
    build_helper_for_of(
        loop_body,
        iterable,
        result_ident.clone(),
        span,
        helper_context,
        false,
    )
}

fn extract_closure_iterator_init(
    stmt: &Stmt,
    helper_context: &ForOfHelperContext,
) -> Option<ClosureIteratorInit> {
    let (iterator_ident, init) = match stmt {
        Stmt::Expr(ExprStmt { expr, .. }) => {
            let Expr::Assign(assign) = expr.as_ref() else {
                return None;
            };
            if assign.op != AssignOp::Assign {
                return None;
            }
            let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &assign.left else {
                return None;
            };
            (binding.id.clone(), assign.right.as_ref())
        }
        Stmt::Decl(Decl::Var(var_decl)) => {
            let [declarator] = var_decl.decls.as_slice() else {
                return None;
            };
            (
                pat_as_ident(&declarator.name)?.id.clone(),
                declarator.init.as_deref()?,
            )
        }
        _ => return None,
    };

    let iterable = extract_closure_make_iterator_arg(init, helper_context)?;
    Some(ClosureIteratorInit {
        iterator_ident,
        iterable,
    })
}

fn extract_closure_make_iterator_arg(
    init: &Expr,
    helper_context: &ForOfHelperContext,
) -> Option<Box<Expr>> {
    let Expr::Call(call) = strip_parens(init) else {
        return None;
    };
    if !helper_context.is_closure_make_iterator_callee(&call.callee) {
        return None;
    }
    let [arg] = call.args.as_slice() else {
        return None;
    };
    if arg.spread.is_some() {
        return None;
    }
    Some(arg.expr.clone())
}

fn try_convert_iterator_helper_sequence(
    stmts: &[Stmt],
    helper_context: &ForOfHelperContext,
) -> Option<SequenceRewrite> {
    if let Some(rewrite) = try_convert_iterator_helper_decl_first_sequence(stmts, helper_context) {
        return Some(rewrite);
    }

    let item_ident = empty_single_var_ident(stmts.first()?)?;

    let mut helper_index = None;
    let mut preserved_stmts = Vec::new();
    for (idx, stmt) in stmts.iter().enumerate().skip(1) {
        if let Some(decl) = stmt_as_single_var_decl(stmt) {
            if decl.decls[0].init.is_some() && pat_as_ident(&decl.decls[0].name).is_some() {
                helper_index = Some(idx);
                break;
            }
        }

        if empty_single_var_ident(stmt).is_some() {
            preserved_stmts.push(stmt.clone());
            continue;
        }

        return None;
    }

    let helper_index = helper_index?;
    let helper_decl = stmt_as_single_var_decl(&stmts[helper_index])?;
    let helper_ident = pat_as_ident(&helper_decl.decls[0].name)?.id.clone();
    let iterable = extract_single_call_arg(helper_decl.decls[0].init.as_ref()?)?;
    let try_stmt = stmt_as_try(stmts.get(helper_index + 1)?)?;
    let helper_loop = extract_iterator_helper_loop(try_stmt, &helper_ident, &item_ident)?;

    let consumed_stmts = helper_index + 2;
    if stmts[consumed_stmts..].iter().any(|stmt| {
        stmt_uses_ident_key(stmt, &item_ident) || stmt_uses_ident_key(stmt, &helper_ident)
    }) {
        return None;
    }

    let for_of = build_helper_for_of(
        helper_loop,
        iterable,
        item_ident,
        stmts[0].span(),
        helper_context,
        false,
    )?;
    Some(SequenceRewrite {
        consumed_stmts,
        preserved_stmts,
        for_of,
    })
}

fn try_convert_iterator_helper_decl_first_sequence(
    stmts: &[Stmt],
    helper_context: &ForOfHelperContext,
) -> Option<SequenceRewrite> {
    let helper_decl = stmt_as_single_var_decl(stmts.first()?)?;
    let helper_ident = pat_as_ident(&helper_decl.decls[0].name)?.id.clone();
    let iterable = extract_single_call_arg(helper_decl.decls[0].init.as_ref()?)?;
    let item_ident = empty_single_var_ident(stmts.get(1)?)?;
    let try_stmt = stmt_as_try(stmts.get(2)?)?;
    let helper_loop = extract_iterator_helper_loop(try_stmt, &helper_ident, &item_ident)?;

    if stmts[3..].iter().any(|stmt| {
        stmt_uses_ident_key(stmt, &item_ident) || stmt_uses_ident_key(stmt, &helper_ident)
    }) {
        return None;
    }

    let for_of = build_helper_for_of(
        helper_loop,
        iterable,
        item_ident,
        stmts[0].span(),
        helper_context,
        false,
    )?;
    Some(SequenceRewrite {
        consumed_stmts: 3,
        preserved_stmts: Vec::new(),
        for_of,
    })
}

fn try_convert_loose_iterator_sequence(
    stmts: &[Stmt],
    helper_context: &ForOfHelperContext,
) -> Option<SequenceRewrite> {
    let item_ident = empty_single_var_ident(stmts.first()?)?;
    let Stmt::For(for_stmt) = stmts.get(1)? else {
        return None;
    };
    let Some(VarDeclOrExpr::VarDecl(init_decl)) = &for_stmt.init else {
        return None;
    };
    let [helper_decl] = init_decl.decls.as_slice() else {
        return None;
    };
    let helper_ident = pat_as_ident(&helper_decl.name)?.id.clone();
    let iterable = extract_single_call_arg(helper_decl.init.as_ref()?)?;
    if for_stmt.update.is_some() {
        return None;
    }
    if !is_loose_iterator_test(for_stmt.test.as_deref()?, &helper_ident, &item_ident) {
        return None;
    }
    if stmts[2..].iter().any(|stmt| {
        stmt_uses_ident_key(stmt, &item_ident) || stmt_uses_ident_key(stmt, &helper_ident)
    }) {
        return None;
    }

    let Stmt::Block(body) = &*for_stmt.body else {
        return None;
    };
    let for_of = build_helper_for_of(
        body.clone(),
        iterable,
        item_ident,
        stmts[0].span(),
        helper_context,
        false,
    )?;
    Some(SequenceRewrite {
        consumed_stmts: 2,
        preserved_stmts: Vec::new(),
        for_of,
    })
}

fn try_convert_ts_values_sequence(
    stmts: &[Stmt],
    helper_context: &ForOfHelperContext,
) -> Option<SequenceRewrite> {
    let error_ident = empty_single_var_ident(stmts.first()?)?;
    let return_ident = empty_single_var_ident(stmts.get(1)?)?;
    let try_stmt = stmt_as_try(stmts.get(2)?)?;
    let helper_loop =
        extract_ts_values_loop(try_stmt, &error_ident, &return_ident, helper_context)?;
    if stmts[3..].iter().any(|stmt| {
        stmt_uses_ident_key(stmt, &error_ident) || stmt_uses_ident_key(stmt, &return_ident)
    }) {
        return None;
    }

    let for_of = build_helper_for_of(
        helper_loop.loop_body,
        helper_loop.iterable,
        helper_loop.result_ident,
        stmts[0].span(),
        helper_context,
        false,
    )?;
    Some(SequenceRewrite {
        consumed_stmts: 3,
        preserved_stmts: Vec::new(),
        for_of,
    })
}

fn try_convert_swc_iterator_sequence(
    stmts: &[Stmt],
    helper_context: &ForOfHelperContext,
) -> Option<SequenceRewrite> {
    let normal_ident = single_var_ident_with_bool(stmts.first()?, true)?;
    let did_error_ident = single_var_ident_with_bool(stmts.get(1)?, false)?;
    let error_ident = empty_single_var_ident(stmts.get(2)?)?;
    let try_stmt = stmt_as_try(stmts.get(3)?)?;
    let helper_loop = extract_swc_iterator_loop(try_stmt, &normal_ident)?;

    if !swc_catch_matches(try_stmt, &did_error_ident, &error_ident) {
        return None;
    }
    if !try_stmt.finalizer.as_ref().is_some_and(|finalizer| {
        let stmt = Stmt::Block(finalizer.clone());
        stmt_uses_ident_key(&stmt, &normal_ident)
            && stmt_uses_ident_key(&stmt, &did_error_ident)
            && stmt_uses_ident_key(&stmt, &error_ident)
            && stmt_uses_ident_key(&stmt, &helper_loop.iterator_ident)
    }) {
        return None;
    }
    if stmts[4..].iter().any(|stmt| {
        stmt_uses_ident_key(stmt, &normal_ident)
            || stmt_uses_ident_key(stmt, &did_error_ident)
            || stmt_uses_ident_key(stmt, &error_ident)
    }) {
        return None;
    }

    let for_of = build_helper_for_of(
        helper_loop.loop_body,
        helper_loop.iterable,
        helper_loop.result_ident,
        stmts[0].span(),
        helper_context,
        false,
    )?;
    Some(SequenceRewrite {
        consumed_stmts: 4,
        preserved_stmts: Vec::new(),
        for_of,
    })
}

struct TsValuesLoop {
    iterable: Box<Expr>,
    result_ident: Ident,
    loop_body: BlockStmt,
}

struct SwcIteratorLoop {
    iterable: Box<Expr>,
    iterator_ident: Ident,
    result_ident: Ident,
    loop_body: BlockStmt,
}

fn extract_iterator_helper_loop(
    try_stmt: &TryStmt,
    helper_ident: &Ident,
    item_ident: &Ident,
) -> Option<BlockStmt> {
    let for_stmt = single_for_stmt(&try_stmt.block)?;

    let Some(VarDeclOrExpr::Expr(init)) = &for_stmt.init else {
        return None;
    };
    if !is_helper_method_call(init, helper_ident, "s") {
        return None;
    }
    if for_stmt.update.is_some() {
        return None;
    }
    let test = for_stmt.test.as_deref()?;
    if !is_iterator_helper_test(test, helper_ident, item_ident) {
        return None;
    }
    if !catch_calls_helper_error(try_stmt, helper_ident) {
        return None;
    }
    if !finally_calls_helper_method(try_stmt.finalizer.as_ref()?, helper_ident, "f") {
        return None;
    }

    let Stmt::Block(body) = &*for_stmt.body else {
        return None;
    };
    Some(body.clone())
}

fn extract_ts_values_loop(
    try_stmt: &TryStmt,
    error_ident: &Ident,
    return_ident: &Ident,
    helper_context: &ForOfHelperContext,
) -> Option<TsValuesLoop> {
    let for_stmt = single_for_stmt(&try_stmt.block)?;
    let Some(VarDeclOrExpr::VarDecl(init_decl)) = &for_stmt.init else {
        return None;
    };
    let [iterator_decl, result_decl] = init_decl.decls.as_slice() else {
        return None;
    };
    let iterator_ident = pat_as_ident(&iterator_decl.name)?.id.clone();
    let result_ident = pat_as_ident(&result_decl.name)?.id.clone();
    let iterable = extract_ts_values_arg(iterator_decl.init.as_ref()?, helper_context)?;
    if !is_iterator_next_call(result_decl.init.as_ref()?, &iterator_ident) {
        return None;
    }
    if !is_not_done_test(for_stmt.test.as_deref()?, &result_ident) {
        return None;
    }
    if !for_stmt
        .update
        .as_deref()
        .is_some_and(|update| is_iterator_next_update(update, &result_ident, &iterator_ident))
    {
        return None;
    }
    if !ts_values_catch_matches(try_stmt, error_ident) {
        return None;
    }
    if !try_stmt.finalizer.as_ref().is_some_and(|finalizer| {
        stmt_uses_ident_key(&Stmt::Block(finalizer.clone()), return_ident)
            && stmt_uses_ident_key(&Stmt::Block(finalizer.clone()), &iterator_ident)
    }) {
        return None;
    }

    let Stmt::Block(body) = &*for_stmt.body else {
        return None;
    };
    Some(TsValuesLoop {
        iterable,
        result_ident,
        loop_body: body.clone(),
    })
}

fn extract_swc_iterator_loop(try_stmt: &TryStmt, normal_ident: &Ident) -> Option<SwcIteratorLoop> {
    let [step_decl_stmt, Stmt::For(for_stmt)] = try_stmt.block.stmts.as_slice() else {
        return None;
    };
    let result_ident = empty_single_var_ident(step_decl_stmt)?;
    let Some(VarDeclOrExpr::VarDecl(init_decl)) = &for_stmt.init else {
        return None;
    };
    let [iterator_decl] = init_decl.decls.as_slice() else {
        return None;
    };
    let iterator_ident = pat_as_ident(&iterator_decl.name)?.id.clone();
    let iterable = extract_symbol_iterator_call_obj(iterator_decl.init.as_ref()?)?;
    if !is_swc_iterator_test(
        for_stmt.test.as_deref()?,
        normal_ident,
        &result_ident,
        &iterator_ident,
    ) {
        return None;
    }
    if !for_stmt
        .update
        .as_deref()
        .is_some_and(|update| is_assign_bool(update, normal_ident, true))
    {
        return None;
    }
    let Stmt::Block(body) = &*for_stmt.body else {
        return None;
    };
    Some(SwcIteratorLoop {
        iterable,
        iterator_ident,
        result_ident,
        loop_body: body.clone(),
    })
}

pub(super) fn build_helper_for_of(
    mut body: BlockStmt,
    iterable: Box<Expr>,
    item_ident: Ident,
    span: Span,
    helper_context: &ForOfHelperContext,
    is_await: bool,
) -> Option<ForOfStmt> {
    let mut element = extract_iterator_value_element(&body.stmts, &item_ident);
    if element.is_none() {
        element = extract_iterator_call_destructuring_element(&body.stmts, &item_ident);
    }
    if element.is_none() {
        element = extract_iterator_destructuring_decl_element(&body.stmts, &item_ident);
    }
    if element.is_none() {
        if body
            .stmts
            .iter()
            .any(|stmt| stmt_uses_ident_key_outside_value_member(stmt, &item_ident))
        {
            return None;
        }
        replace_iterator_value_refs(&mut body, &item_ident);
    }
    let mut element = if let Some(element) = element {
        element
    } else {
        LoopElement {
            pat: Pat::Ident(BindingIdent {
                id: item_ident.clone(),
                type_ann: None,
            }),
            bindings: vec![item_ident.clone()],
            kind: VarDeclKind::Const,
            temp_ident: None,
            consumed_stmts: 0,
            allow_ident_fallback: false,
            temp_kind: VarDeclKind::Const,
        }
    };

    // Remaining-body uses / eval must be checked against the ArrayPat
    // remainder before any ident fallback, so `eval("pair")` still
    // fail-closes the whole helper.
    if let Some(id) = element.temp_ident.clone() {
        let remaining_after_pat = &body.stmts[element.consumed_stmts..];
        if remaining_after_pat
            .iter()
            .any(|stmt| stmt_uses_ident(stmt, &id))
            || dynamic_scope_observes_binding(remaining_after_pat, &id)
        {
            return None;
        }
        let var_temp = body
            .stmts
            .first()
            .and_then(stmt_as_single_var_decl)
            .is_some_and(|decl| decl.kind == VarDeclKind::Var);
        if array_pat_left_is_unsound(
            helper_context,
            &iterable,
            &body.stmts,
            &id,
            &element.pat,
            &element.bindings,
            var_temp,
        ) && (!element.fallback_to_temp_ident()
            || ident_fallback_tdz_in_iterable(&iterable, &element))
        {
            return None;
        }
    } else if matches!(element.pat, Pat::Array(_))
        && expr_observes_lifted_names(&iterable, &element.bindings)
    {
        return None;
    }

    let mut remaining_body = body.stmts[element.consumed_stmts..].to_vec();
    if element.consumed_stmts > 0
        && remaining_body
            .iter()
            .any(|stmt| stmt_uses_ident_key(stmt, &item_ident))
    {
        return None;
    }

    let body_uses = BindingUseIndex::collect_stmts(&remaining_body);
    if writes_consumed_const(&body.stmts[..element.consumed_stmts], &body_uses) {
        return None;
    }
    let is_reassigned = element
        .bindings
        .iter()
        .any(|id| body_uses.has_direct_write(&id.to_id()));
    let kind = if element.kind == VarDeclKind::Var {
        VarDeclKind::Var
    } else if is_reassigned {
        VarDeclKind::Let
    } else {
        VarDeclKind::Const
    };
    rename_lifted_bindings_shadowing_iterable(&iterable, kind, &mut element, &mut remaining_body)?;

    let for_span = if span.lo.0 != 0 { span } else { DUMMY_SP };
    Some(ForOfStmt {
        span: for_span,
        is_await,
        left: ForHead::VarDecl(Box::new(VarDecl {
            span: DUMMY_SP,
            ctxt: Default::default(),
            kind,
            declare: false,
            decls: vec![VarDeclarator {
                span: DUMMY_SP,
                name: element.pat,
                init: None,
                definite: false,
            }],
        })),
        right: iterable,
        body: Box::new(Stmt::Block(BlockStmt {
            span: body.span,
            ctxt: body.ctxt,
            stmts: std::mem::take(&mut remaining_body),
        })),
    })
}

fn extract_iterator_call_destructuring_element(
    stmts: &[Stmt],
    item_ident: &Ident,
) -> Option<LoopElement> {
    let first_decl = stmt_as_single_var_decl(stmts.first()?)?;
    let first = &first_decl.decls[0];
    let Pat::Ident(temp_binding) = &first.name else {
        return None;
    };
    if !is_destructuring_helper_call(first.init.as_ref()?, item_ident) {
        return None;
    }

    let temp_ident = &temp_binding.id;
    let slots = consume_index_slots(&stmts[1..], temp_ident);
    let (pat, bindings, kind, slot_consumed) = array_pat_from_index_slots(slots)?;

    Some(LoopElement {
        pat,
        bindings,
        kind,
        temp_ident: Some(temp_ident.clone()),
        consumed_stmts: 1 + slot_consumed,
        allow_ident_fallback: false,
        temp_kind: first_decl.kind,
    })
}

fn extract_iterator_destructuring_decl_element(
    stmts: &[Stmt],
    item_ident: &Ident,
) -> Option<LoopElement> {
    let first_decl = stmt_as_single_var_decl(stmts.first()?)?;
    let first = &first_decl.decls[0];
    if matches!(first.name, Pat::Ident(_)) {
        return None;
    }
    let init = first.init.as_ref()?;
    if !is_value_member(init, item_ident) {
        return None;
    }
    if pat_uses_ident_key(&first.name, item_ident) {
        return None;
    }

    let mut bindings = Vec::new();
    collect_pat_bindings(&first.name, &mut bindings)?;
    if bindings.is_empty() {
        return None;
    }

    Some(LoopElement {
        pat: first.name.clone(),
        bindings,
        kind: first_decl.kind,
        temp_ident: None,
        consumed_stmts: 1,
        allow_ident_fallback: false,
        temp_kind: first_decl.kind,
    })
}

fn extract_iterator_value_element(stmts: &[Stmt], item_ident: &Ident) -> Option<LoopElement> {
    let first_decl = stmt_as_single_var_decl(stmts.first()?)?;
    let first = &first_decl.decls[0];
    let Pat::Ident(binding) = &first.name else {
        return None;
    };
    if !is_value_member(first.init.as_ref()?, item_ident) {
        return None;
    }

    let temp_ident = &binding.id;
    let slots = consume_index_slots(&stmts[1..], temp_ident);
    if let Some((pat, bindings, kind, slot_consumed)) = array_pat_from_index_slots(slots) {
        return Some(LoopElement {
            pat,
            bindings,
            kind,
            temp_ident: Some(temp_ident.clone()),
            consumed_stmts: 1 + slot_consumed,
            allow_ident_fallback: true,
            temp_kind: first_decl.kind,
        });
    }

    Some(LoopElement {
        pat: Pat::Ident(binding.clone()),
        bindings: vec![binding.id.clone()],
        kind: first_decl.kind,
        temp_ident: None,
        consumed_stmts: 1,
        allow_ident_fallback: true,
        temp_kind: first_decl.kind,
    })
}

fn collect_pat_bindings(pat: &Pat, bindings: &mut Vec<Ident>) -> Option<()> {
    match pat {
        Pat::Ident(binding) => {
            bindings.push(binding.id.clone());
            Some(())
        }
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_pat_bindings(elem, bindings)?;
            }
            Some(())
        }
        Pat::Rest(rest) => collect_pat_bindings(&rest.arg, bindings),
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(key_value) => {
                        collect_pat_bindings(&key_value.value, bindings)?;
                    }
                    ObjectPatProp::Assign(assign) => {
                        bindings.push(assign.key.id.clone());
                    }
                    ObjectPatProp::Rest(rest) => {
                        collect_pat_bindings(&rest.arg, bindings)?;
                    }
                }
            }
            Some(())
        }
        Pat::Assign(assign) => collect_pat_bindings(&assign.left, bindings),
        Pat::Expr(_) | Pat::Invalid(_) => None,
    }
}

fn single_for_stmt(block: &BlockStmt) -> Option<&swc_core::ecma::ast::ForStmt> {
    let [Stmt::For(for_stmt)] = block.stmts.as_slice() else {
        return None;
    };
    Some(for_stmt)
}

pub(super) fn empty_single_var_ident(stmt: &Stmt) -> Option<Ident> {
    let decl = stmt_as_single_var_decl(stmt)?;
    let declarator = &decl.decls[0];
    if declarator.init.is_some() {
        return None;
    }
    Some(pat_as_ident(&declarator.name)?.id.clone())
}

pub(super) fn single_var_ident_with_bool(stmt: &Stmt, value: bool) -> Option<Ident> {
    let decl = stmt_as_single_var_decl(stmt)?;
    let declarator = &decl.decls[0];
    if !declarator.init.as_deref().is_some_and(
        |init| matches!(init, Expr::Lit(Lit::Bool(bool_lit)) if bool_lit.value == value),
    ) {
        return None;
    }
    Some(pat_as_ident(&declarator.name)?.id.clone())
}

pub(super) fn pat_as_ident(pat: &Pat) -> Option<&BindingIdent> {
    let Pat::Ident(ident) = pat else {
        return None;
    };
    Some(ident)
}

pub(super) fn stmt_as_try(stmt: &Stmt) -> Option<&TryStmt> {
    let Stmt::Try(try_stmt) = stmt else {
        return None;
    };
    Some(try_stmt)
}

fn extract_single_call_arg(expr: &Expr) -> Option<Box<Expr>> {
    let Expr::Call(CallExpr { args, .. }) = expr else {
        return None;
    };
    let [ExprOrSpread { spread: None, expr }] = args.as_slice() else {
        return None;
    };
    Some(expr.clone())
}

fn extract_ts_values_arg(expr: &Expr, helper_context: &ForOfHelperContext) -> Option<Box<Expr>> {
    let Expr::Call(CallExpr { callee, args, .. }) = expr else {
        return None;
    };
    if !helper_context.is_ts_values_callee(callee) {
        return None;
    }
    let [ExprOrSpread { spread: None, expr }] = args.as_slice() else {
        return None;
    };
    Some(expr.clone())
}

fn is_loose_iterator_test(expr: &Expr, helper_ident: &Ident, item_ident: &Ident) -> bool {
    let Expr::Unary(UnaryExpr {
        op: UnaryOp::Bang,
        arg,
        ..
    }) = expr
    else {
        return false;
    };
    let Some(done_obj) = extract_done_obj(arg) else {
        return false;
    };
    let Expr::Assign(assign) = done_obj else {
        return false;
    };
    is_assign_ident(assign, item_ident) && is_helper_call(&assign.right, helper_ident)
}

fn is_swc_iterator_test(
    expr: &Expr,
    normal_ident: &Ident,
    result_ident: &Ident,
    iterator_ident: &Ident,
) -> bool {
    let Expr::Unary(UnaryExpr {
        op: UnaryOp::Bang,
        arg,
        ..
    }) = expr
    else {
        return false;
    };
    let Expr::Assign(normal_assign) = strip_parens(arg) else {
        return false;
    };
    if !is_assign_ident(normal_assign, normal_ident) {
        return false;
    }
    let Some(done_obj) = extract_done_obj(&normal_assign.right) else {
        return false;
    };
    let Expr::Assign(next_assign) = done_obj else {
        return false;
    };
    is_assign_ident(next_assign, result_ident)
        && is_iterator_next_call(&next_assign.right, iterator_ident)
}

fn is_iterator_helper_test(expr: &Expr, helper_ident: &Ident, item_ident: &Ident) -> bool {
    let Expr::Unary(UnaryExpr {
        op: UnaryOp::Bang,
        arg,
        ..
    }) = expr
    else {
        return false;
    };
    let Some(done_obj) = extract_done_obj(arg) else {
        return false;
    };
    let Expr::Assign(assign) = done_obj else {
        return false;
    };
    is_assign_ident(assign, item_ident) && is_helper_method_call(&assign.right, helper_ident, "n")
}

fn is_not_done_test(expr: &Expr, result_ident: &Ident) -> bool {
    let Expr::Unary(UnaryExpr {
        op: UnaryOp::Bang,
        arg,
        ..
    }) = expr
    else {
        return false;
    };
    let Some(done_obj) = extract_done_obj(arg) else {
        return false;
    };
    is_ident_key(done_obj, result_ident)
}

pub(super) fn extract_done_obj(expr: &Expr) -> Option<&Expr> {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return None;
    };
    let MemberProp::Ident(prop) = prop else {
        return None;
    };
    if prop.sym.as_ref() != "done" {
        return None;
    }
    Some(strip_parens(obj))
}

pub(super) fn is_assign_ident(assign: &AssignExpr, ident: &Ident) -> bool {
    if assign.op != AssignOp::Assign {
        return false;
    }
    matches!(
        &assign.left,
        AssignTarget::Simple(SimpleAssignTarget::Ident(left)) if left.id.sym == ident.sym && left.id.ctxt == ident.ctxt
    )
}

pub(super) fn is_iterator_next_call(expr: &Expr, iterator_ident: &Ident) -> bool {
    is_helper_method_call(expr, iterator_ident, "next")
}

fn is_iterator_next_update(expr: &Expr, result_ident: &Ident, iterator_ident: &Ident) -> bool {
    let Expr::Assign(assign) = expr else {
        return false;
    };
    is_assign_ident(assign, result_ident) && is_iterator_next_call(&assign.right, iterator_ident)
}

pub(super) fn is_assign_bool(expr: &Expr, ident: &Ident, value: bool) -> bool {
    let Expr::Assign(assign) = expr else {
        return false;
    };
    is_assign_ident(assign, ident)
        && matches!(&*assign.right, Expr::Lit(Lit::Bool(bool_lit)) if bool_lit.value == value)
}

pub(super) fn is_helper_method_call(expr: &Expr, helper_ident: &Ident, method: &str) -> bool {
    let Expr::Call(CallExpr { callee, args, .. }) = expr else {
        return false;
    };
    if !args.is_empty() {
        return false;
    }
    let Callee::Expr(callee_expr) = callee else {
        return false;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
        return false;
    };
    if !is_ident_key(obj, helper_ident) {
        return false;
    }
    matches!(prop, MemberProp::Ident(prop) if prop.sym.as_ref() == method)
}

fn is_helper_call(expr: &Expr, helper_ident: &Ident) -> bool {
    let Expr::Call(CallExpr { callee, args, .. }) = expr else {
        return false;
    };
    if !args.is_empty() {
        return false;
    }
    let Callee::Expr(callee_expr) = callee else {
        return false;
    };
    is_ident_key(callee_expr, helper_ident)
}

fn catch_calls_helper_error(try_stmt: &TryStmt, helper_ident: &Ident) -> bool {
    let Some(catch) = &try_stmt.handler else {
        return false;
    };
    let [Stmt::Expr(expr_stmt)] = catch.body.stmts.as_slice() else {
        return false;
    };
    let Expr::Call(CallExpr { callee, args, .. }) = &*expr_stmt.expr else {
        return false;
    };
    if args.len() != 1 {
        return false;
    }
    let Callee::Expr(callee_expr) = callee else {
        return false;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
        return false;
    };
    is_ident_key(obj, helper_ident)
        && matches!(prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "e")
}

fn finally_calls_helper_method(block: &BlockStmt, helper_ident: &Ident, method: &str) -> bool {
    let [Stmt::Expr(expr_stmt)] = block.stmts.as_slice() else {
        return false;
    };
    is_helper_method_call(&expr_stmt.expr, helper_ident, method)
}

fn ts_values_catch_matches(try_stmt: &TryStmt, error_ident: &Ident) -> bool {
    let Some(catch) = &try_stmt.handler else {
        return false;
    };
    let [Stmt::Expr(expr_stmt)] = catch.body.stmts.as_slice() else {
        return false;
    };
    let Expr::Assign(assign) = &*expr_stmt.expr else {
        return false;
    };
    is_assign_ident(assign, error_ident)
}

fn swc_catch_matches(try_stmt: &TryStmt, did_error_ident: &Ident, error_ident: &Ident) -> bool {
    let Some(catch) = &try_stmt.handler else {
        return false;
    };
    let [Stmt::Expr(first), Stmt::Expr(second)] = catch.body.stmts.as_slice() else {
        return false;
    };
    if !is_assign_bool(&first.expr, did_error_ident, true) {
        return false;
    }
    let Some(param) = catch.param.as_ref().and_then(pat_as_ident) else {
        return false;
    };
    let Expr::Assign(assign) = &*second.expr else {
        return false;
    };
    is_assign_ident(assign, error_ident) && is_ident_key(&assign.right, &param.id)
}

pub(super) fn is_value_member(expr: &Expr, item_ident: &Ident) -> bool {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return false;
    };
    is_ident_key(obj, item_ident)
        && matches!(prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "value")
}

fn pat_uses_ident_key(pat: &Pat, ident: &Ident) -> bool {
    use swc_core::ecma::visit::Visit;

    struct IdentFinder {
        ident: Ident,
        found: bool,
    }

    impl Visit for IdentFinder {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym == self.ident.sym && ident.ctxt == self.ident.ctxt {
                self.found = true;
            }
        }
    }

    let mut finder = IdentFinder {
        ident: ident.clone(),
        found: false,
    };
    finder.visit_pat(pat);
    finder.found
}

fn is_destructuring_helper_call(expr: &Expr, item_ident: &Ident) -> bool {
    let Expr::Call(CallExpr { args, .. }) = expr else {
        return false;
    };
    let Some(ExprOrSpread { spread: None, expr }) = args.first() else {
        return false;
    };
    is_value_member(expr, item_ident)
}

fn extract_symbol_iterator_call_obj(expr: &Expr) -> Option<Box<Expr>> {
    let Expr::Call(CallExpr { callee, args, .. }) = expr else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let Callee::Expr(callee_expr) = callee else {
        return None;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
        return None;
    };
    let MemberProp::Computed(computed) = prop else {
        return None;
    };
    if !is_symbol_iterator_expr(&computed.expr) {
        return None;
    }
    Some(obj.clone())
}

fn is_symbol_iterator_expr(expr: &Expr) -> bool {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return false;
    };
    is_ident(obj, &Atom::from("Symbol"))
        && matches!(prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "iterator")
}

fn replace_iterator_value_refs(block: &mut BlockStmt, item_ident: &Ident) {
    struct Replacer {
        ident: Ident,
    }

    impl VisitMut for Replacer {
        fn visit_mut_expr(&mut self, expr: &mut Expr) {
            expr.visit_mut_children_with(self);
            if is_value_member(expr, &self.ident) {
                *expr = Expr::Ident(self.ident.clone());
            }
        }
    }

    block.visit_mut_with(&mut Replacer {
        ident: item_ident.clone(),
    });
}

fn try_convert_for_of(stmt: &Stmt, helper_context: &ForOfHelperContext) -> Option<ForOfStmt> {
    let Stmt::For(for_stmt) = stmt else {
        return None;
    };

    // --- Init: `let i = 0, arr = <iterable>` ---
    let Some(swc_core::ecma::ast::VarDeclOrExpr::VarDecl(init_decl)) = &for_stmt.init else {
        return None;
    };
    if init_decl.decls.is_empty() || init_decl.decls.len() > 2 {
        return None;
    }
    let idx_decl = &init_decl.decls[0];

    // Index must be initialized to 0
    let Pat::Ident(idx_binding) = &idx_decl.name else {
        return None;
    };
    let idx_ident = &idx_binding.id;
    let Some(idx_init) = &idx_decl.init else {
        return None;
    };
    if !is_zero(idx_init) {
        return None;
    }

    // --- Test: `i < arr.length` ---
    let Some(test) = &for_stmt.test else {
        return None;
    };
    let Expr::Bin(BinExpr {
        op: BinaryOp::Lt,
        left,
        right,
        ..
    }) = &**test
    else {
        return None;
    };
    if !is_ident(left, &idx_ident.sym) {
        return None;
    }

    let IndexedIterable {
        access_obj,
        iterable,
        temp_ident,
    } = extract_indexed_iterable(init_decl, right)?;

    // The index and iterable temporary disappear from the recovered loop.
    // `var` makes either binding function-scoped, so a later loop or statement
    // can still observe it even when the candidate body does not. Retaining an
    // empty declaration would not preserve the index's final value; reject the
    // conversion whenever either removed binding has uses outside this loop.
    let candidate = std::slice::from_ref(stmt);
    if helper_context.binding_is_used_outside(candidate, idx_ident)
        || temp_ident
            .as_ref()
            .is_some_and(|ident| helper_context.binding_is_used_outside(candidate, ident))
    {
        return None;
    }

    // --- Update: `i++` ---
    let Some(update) = &for_stmt.update else {
        return None;
    };
    let Expr::Update(UpdateExpr {
        op: UpdateOp::PlusPlus,
        arg,
        ..
    }) = &**update
    else {
        return None;
    };
    if !is_ident(arg, &idx_ident.sym) {
        return None;
    }

    // --- Body: first statement must declare the element from `arr[i]` ---
    let Stmt::Block(block) = &*for_stmt.body else {
        return None;
    };
    if block.stmts.is_empty() {
        return None;
    }
    let mut element = extract_loop_element(&block.stmts, &access_obj, &idx_ident.sym)?;

    // --- Safety: generated index/temp bindings must not be used in remaining body statements ---
    let remaining_after_pat = &block.stmts[element.consumed_stmts..];
    for body_stmt in remaining_after_pat {
        if stmt_uses_ident(body_stmt, idx_ident) {
            return None;
        }
        if temp_ident
            .as_ref()
            .is_some_and(|id| stmt_uses_ident(body_stmt, id))
        {
            return None;
        }
        if element
            .temp_ident
            .as_ref()
            .is_some_and(|id| stmt_uses_ident(body_stmt, id))
        {
            return None;
        }
    }
    if temp_ident
        .as_ref()
        .is_some_and(|id| dynamic_scope_observes_binding(remaining_after_pat, id))
        || element
            .temp_ident
            .as_ref()
            .is_some_and(|id| dynamic_scope_observes_binding(remaining_after_pat, id))
    {
        return None;
    }
    if let Some(id) = element.temp_ident.clone() {
        let var_temp = block
            .stmts
            .first()
            .and_then(stmt_as_single_var_decl)
            .is_some_and(|decl| decl.kind == VarDeclKind::Var);
        // Body-only liveness: a read in the for-init iterable is live for a
        // function-scoped element temp, not an "inside" use of this loop.
        if array_pat_left_is_unsound(
            helper_context,
            &iterable,
            &block.stmts,
            &id,
            &element.pat,
            &element.bindings,
            var_temp,
        ) && (!element.fallback_to_temp_ident()
            || ident_fallback_tdz_in_iterable(&iterable, &element))
        {
            return None;
        }
    } else if matches!(element.pat, Pat::Array(_))
        && expr_observes_lifted_names(&iterable, &element.bindings)
    {
        return None;
    }
    let remaining_body = &block.stmts[element.consumed_stmts..];

    // Analyze the remaining body after consuming the element declaration.
    // Shared write analysis includes nested targets and distinguishes shadowed bindings.
    let body_uses = BindingUseIndex::collect_stmts(remaining_body);
    if writes_consumed_const(&block.stmts[..element.consumed_stmts], &body_uses) {
        return None;
    }
    let elem_is_reassigned = element
        .bindings
        .iter()
        .any(|id| body_uses.has_direct_write(&id.to_id()));
    let elem_kind = if element.kind == VarDeclKind::Var {
        VarDeclKind::Var
    } else if elem_is_reassigned {
        VarDeclKind::Let
    } else {
        VarDeclKind::Const
    };
    let mut remaining_body = remaining_body.to_vec();
    rename_lifted_bindings_shadowing_iterable(
        &iterable,
        elem_kind,
        &mut element,
        &mut remaining_body,
    )?;

    // --- Build for...of ---
    let for_of_left = ForHead::VarDecl(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: elem_kind,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: element.pat,
            init: None,
            definite: false,
        }],
    }));

    let new_body = Stmt::Block(swc_core::ecma::ast::BlockStmt {
        span: DUMMY_SP,
        ctxt: Default::default(),
        stmts: remaining_body,
    });

    Some(ForOfStmt {
        span: for_stmt.span,
        is_await: false,
        left: for_of_left,
        right: iterable,
        body: Box::new(new_body),
    })
}

struct IndexedIterable {
    access_obj: Box<Expr>,
    iterable: Box<Expr>,
    temp_ident: Option<Ident>,
}

struct LoopElement {
    pat: Pat,
    bindings: Vec<Ident>,
    kind: VarDeclKind,
    temp_ident: Option<Ident>,
    consumed_stmts: usize,
    /// Ident fallback is only valid when `temp` *is* the iterator value
    /// (`step.value` / `arr[i]`). A helper-call temp (`_slicedToArray`) is a
    /// converted array; dropping the call would change the observable value.
    allow_ident_fallback: bool,
    /// Kind of the original temp declaration (`const pair`), not the slot
    /// join (`var value`). Ident fallback must restore this kind.
    temp_kind: VarDeclKind,
}

impl LoopElement {
    fn fallback_to_temp_ident(&mut self) -> bool {
        if !self.allow_ident_fallback {
            return false;
        }
        let Some(id) = self.temp_ident.take() else {
            return false;
        };
        self.pat = Pat::Ident(BindingIdent {
            id: id.clone(),
            type_ann: None,
        });
        self.bindings = vec![id];
        self.kind = self.temp_kind;
        self.consumed_stmts = 1;
        true
    }
}

/// Recovery must not turn an existing const-write error into a valid write.
/// Check individual declarations before their kinds are joined: a mixed
/// const/let destructuring group cannot preserve both kinds in one loop head.
fn writes_consumed_const(stmts: &[Stmt], body_uses: &BindingUseIndex) -> bool {
    stmts.iter().any(|stmt| {
        let Stmt::Decl(Decl::Var(decl)) = stmt else {
            return false;
        };
        decl.kind == VarDeclKind::Const
            && decl.decls.iter().any(|declarator| {
                let bindings: Vec<BindingKey> = find_pat_ids(&declarator.name);
                bindings.iter().any(|id| body_uses.has_direct_write(id))
            })
    })
}

/// Join declaration kinds for the bindings that survive in a recovered loop
/// pattern. The tuple temporary is deliberately excluded because it is
/// removed; a surviving function-scoped binding must keep the whole emitted
/// declaration function-scoped.
fn join_recovered_binding_kind(current: VarDeclKind, next: VarDeclKind) -> VarDeclKind {
    if current == VarDeclKind::Var || next == VarDeclKind::Var {
        VarDeclKind::Var
    } else if current == VarDeclKind::Let || next == VarDeclKind::Let {
        VarDeclKind::Let
    } else {
        VarDeclKind::Const
    }
}

fn extract_indexed_iterable(init_decl: &VarDecl, length_expr: &Expr) -> Option<IndexedIterable> {
    let length_obj = extract_length_obj(length_expr)?;

    match init_decl.decls.as_slice() {
        // TypeScript: `let i = 0, arr = iterable; i < arr.length; i++`
        [_, arr_decl] => {
            let Pat::Ident(arr_binding) = &arr_decl.name else {
                return None;
            };
            if !is_ident(&length_obj, &arr_binding.id.sym) {
                return None;
            }
            let iterable = arr_decl.init.clone()?;
            Some(IndexedIterable {
                access_obj: Box::new(length_obj),
                iterable,
                temp_ident: Some(arr_binding.id.clone()),
            })
        }
        // Babel `iterableIsArray`: `let i = 0; i < items.length; i++`
        [idx_decl] => {
            // The direct-array form only has the index declaration in `init`.
            idx_decl.init.as_ref()?;
            Some(IndexedIterable {
                access_obj: Box::new(length_obj.clone()),
                iterable: Box::new(length_obj),
                temp_ident: None,
            })
        }
        _ => None,
    }
}

fn extract_length_obj(expr: &Expr) -> Option<Expr> {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return None;
    };
    let MemberProp::Ident(length_prop) = prop else {
        return None;
    };
    if length_prop.sym.as_ref() != "length" {
        return None;
    }
    Some(*obj.clone())
}

fn extract_loop_element(stmts: &[Stmt], access_obj: &Expr, idx_sym: &Atom) -> Option<LoopElement> {
    let first_decl = stmt_as_single_var_decl(stmts.first()?)?;
    let first = &first_decl.decls[0];
    let Pat::Ident(temp_binding) = &first.name else {
        return None;
    };
    let temp_ident = &temp_binding.id;
    let first_init = first.init.as_ref()?;
    if !is_index_access(first_init, access_obj, idx_sym) {
        return None;
    }

    let slots = consume_index_slots(&stmts[1..], temp_ident);
    if let Some((pat, bindings, kind, slot_consumed)) = array_pat_from_index_slots(slots) {
        return Some(LoopElement {
            pat,
            bindings,
            kind,
            temp_ident: Some(temp_ident.clone()),
            consumed_stmts: 1 + slot_consumed,
            allow_ident_fallback: true,
            temp_kind: first_decl.kind,
        });
    }

    Some(LoopElement {
        pat: Pat::Ident(temp_binding.clone()),
        bindings: vec![temp_binding.id.clone()],
        kind: first_decl.kind,
        temp_ident: None,
        consumed_stmts: 1,
        allow_ident_fallback: true,
        temp_kind: first_decl.kind,
    })
}

enum IndexSlot {
    Binding { ident: Ident, kind: VarDeclKind },
    Hole,
}

struct ConsumedIndexSlots {
    elems: Vec<Option<Pat>>,
    bindings: Vec<Ident>,
    kind: VarDeclKind,
    consumed: usize,
}

/// Consecutive `temp[0]`, `temp[1]`, … on one binding identity.
/// `const x = temp[i]` becomes a pattern slot; a bare `temp[i];` is a hole.
/// Stop at the first non-slot. Holes do not invent a name.
fn consume_index_slots(stmts: &[Stmt], temp: &Ident) -> ConsumedIndexSlots {
    let mut elems = Vec::new();
    let mut bindings = Vec::new();
    let mut kind = VarDeclKind::Const;
    let mut consumed = 0;

    for stmt in stmts {
        let expected = elems.len() as f64;
        let Some(slot) = take_index_slot(stmt, temp, expected) else {
            break;
        };
        match slot {
            IndexSlot::Binding {
                ident,
                kind: slot_kind,
            } => {
                elems.push(Some(Pat::Ident(BindingIdent {
                    id: ident.clone(),
                    // A declaration-level TypeScript annotation is not valid
                    // on an individual ArrayPat element.
                    type_ann: None,
                })));
                bindings.push(ident);
                kind = join_recovered_binding_kind(kind, slot_kind);
            }
            IndexSlot::Hole => elems.push(None),
        }
        consumed += 1;
    }

    ConsumedIndexSlots {
        elems,
        bindings,
        kind,
        consumed,
    }
}

fn array_pat_from_index_slots(
    slots: ConsumedIndexSlots,
) -> Option<(Pat, Vec<Ident>, VarDeclKind, usize)> {
    if slots.elems.is_empty() || slots.bindings.is_empty() {
        return None;
    }
    Some((
        Pat::Array(ArrayPat {
            span: DUMMY_SP,
            elems: slots.elems,
            optional: false,
            type_ann: None,
        }),
        slots.bindings,
        slots.kind,
        slots.consumed,
    ))
}

fn take_index_slot(stmt: &Stmt, temp: &Ident, expected: f64) -> Option<IndexSlot> {
    if let Some(decl) = stmt_as_single_var_decl(stmt) {
        let declarator = &decl.decls[0];
        let Pat::Ident(binding) = &declarator.name else {
            return None;
        };
        let init = declarator.init.as_ref()?;
        if !is_numeric_index_access_key(init, temp, expected) {
            return None;
        }
        return Some(IndexSlot::Binding {
            ident: binding.id.clone(),
            kind: decl.kind,
        });
    }

    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    if is_numeric_index_access_key(expr, temp, expected) {
        return Some(IndexSlot::Hole);
    }
    None
}

/// Direct `eval` / `with` in `stmts` can observe `ident` by printed name.
/// Unknown eval sources and any `with` fail closed; a known string blocks
/// only when it mentions the binding (same contract as `dead_decls`).
/// A lexical element lifted into the loop head is in TDZ while the iterable
/// is evaluated, so `for (const e of e)` throws where the lowered loop read
/// the enclosing `e`. Rename the lifted bindings that share a printed name
/// with the iterable to a fresh name inside the loop. `var` is hoisted and
/// already resolved to the same function-scoped binding, so it is left alone.
/// Fails closed when direct `eval` or `with` in the body could observe the
/// original name.
fn rename_lifted_bindings_shadowing_iterable(
    iterable: &Expr,
    kind: VarDeclKind,
    element: &mut LoopElement,
    body: &mut [Stmt],
) -> Option<()> {
    if kind == VarDeclKind::Var {
        return Some(());
    }
    let colliding: Vec<Ident> = element
        .bindings
        .iter()
        .filter(|id| {
            let names: HashSet<Atom> = [id.sym.clone()].into_iter().collect();
            expr_observes_printed_names(iterable, &names)
        })
        .cloned()
        .collect();
    if colliding.is_empty() {
        return Some(());
    }
    if colliding
        .iter()
        .any(|id| dynamic_scope_observes_binding(body, id))
    {
        return None;
    }

    let mut used = HashSet::default();
    collect_printed_names(iterable, &mut used);
    for stmt in body.iter() {
        collect_printed_names(stmt, &mut used);
    }
    collect_printed_names(&element.pat, &mut used);
    let renames: Vec<BindingRename> = colliding
        .iter()
        .map(|id| BindingRename {
            old: (id.sym.clone(), id.ctxt),
            new: fresh_printed_name(&id.sym, &mut used),
        })
        .collect();
    rename_bindings(&mut element.pat, &renames);
    for stmt in body.iter_mut() {
        rename_bindings(stmt, &renames);
    }
    for binding in element.bindings.iter_mut() {
        if let Some(rename) = renames
            .iter()
            .find(|rename| rename.old.0 == binding.sym && rename.old.1 == binding.ctxt)
        {
            binding.sym = rename.new.clone();
        }
    }
    Some(())
}

fn collect_printed_names<N>(node: &N, names: &mut HashSet<Atom>)
where
    N: for<'a> VisitWith<PrintedNameCollector<'a>>,
{
    let mut collector = PrintedNameCollector { names };
    node.visit_with(&mut collector);
}

struct PrintedNameCollector<'a> {
    names: &'a mut HashSet<Atom>,
}

impl Visit for PrintedNameCollector<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        self.names.insert(ident.sym.clone());
    }
}

fn fresh_printed_name(base: &Atom, used: &mut HashSet<Atom>) -> Atom {
    for suffix in 1usize.. {
        let candidate: Atom = format!("{base}_{suffix}").into();
        if !used.contains(&candidate) {
            used.insert(candidate.clone());
            return candidate;
        }
    }
    unreachable!("an unused suffix always exists")
}

fn dynamic_scope_observes_binding(stmts: &[Stmt], ident: &Ident) -> bool {
    struct WithFinder {
        found: bool,
    }
    impl Visit for WithFinder {
        fn visit_with_stmt(&mut self, _: &swc_core::ecma::ast::WithStmt) {
            self.found = true;
        }
        fn visit_stmt(&mut self, stmt: &Stmt) {
            if !self.found {
                stmt.visit_children_with(self);
            }
        }
    }

    let mut withs = WithFinder { found: false };
    for stmt in stmts {
        stmt.visit_with(&mut withs);
        if withs.found {
            return true;
        }
    }

    let mut eval = DirectEvalAnalyzer::default();
    for stmt in stmts {
        stmt.visit_with(&mut eval);
    }
    eval.unknown_direct_eval
        || eval
            .known_direct_eval_sources
            .iter()
            .any(|source| js_source_mentions_binding(source, &ident.sym))
}

/// ArrayPat left is unsound when the original temp still escapes, a `var`
/// temp is visible to module-level eval/`with`, or lifting the recovered
/// bindings would put those names in TDZ while the iterable runs.
fn array_pat_left_is_unsound(
    helper_context: &ForOfHelperContext,
    iterable: &Expr,
    inside: &[Stmt],
    temp: &Ident,
    pat: &Pat,
    lifted: &[Ident],
    var_temp: bool,
) -> bool {
    helper_context.binding_is_used_outside(inside, temp)
        || (var_temp && helper_context.dynamic_scope_can_observe_name(&temp.sym))
        || (matches!(pat, Pat::Array(_)) && expr_observes_lifted_names(iterable, lifted))
}

fn expr_observes_lifted_names(expr: &Expr, lifted: &[Ident]) -> bool {
    let names: HashSet<Atom> = lifted.iter().map(|id| id.sym.clone()).collect();
    expr_observes_printed_names(expr, &names)
}

/// Ident fallback of a lexical temp puts that name in TDZ on the iterable.
/// Unknown eval is treated the same. `var` is hoisted, so it is not TDZ.
fn ident_fallback_tdz_in_iterable(iterable: &Expr, element: &LoopElement) -> bool {
    if element.kind == VarDeclKind::Var {
        return false;
    }
    let Pat::Ident(binding) = &element.pat else {
        return false;
    };
    let names: HashSet<Atom> = [binding.id.sym.clone()].into_iter().collect();
    expr_observes_printed_names(iterable, &names)
}

/// Lifting body bindings into the for-of head puts those names in TDZ while
/// the iterable runs. Fail closed if the RHS already mentions a dropped or
/// lifted printed name, or a direct eval/`with` that could.
fn for_of_right_observes_fold_names(right: &Expr, dropped: &Ident, lifted: &[Ident]) -> bool {
    let mut names: HashSet<Atom> = lifted.iter().map(|id| id.sym.clone()).collect();
    names.insert(dropped.sym.clone());
    expr_observes_printed_names(right, &names)
}

fn expr_observes_printed_names(expr: &Expr, names: &HashSet<Atom>) -> bool {
    if names.is_empty() {
        return false;
    }

    struct Finder<'a> {
        names: &'a HashSet<Atom>,
        found: bool,
    }
    impl Visit for Finder<'_> {
        fn visit_ident(&mut self, ident: &Ident) {
            if !self.found && self.names.contains(&ident.sym) {
                self.found = true;
            }
        }
        fn visit_with_stmt(&mut self, _: &swc_core::ecma::ast::WithStmt) {
            self.found = true;
        }
        fn visit_stmt(&mut self, stmt: &Stmt) {
            if !self.found {
                stmt.visit_children_with(self);
            }
        }
    }

    let mut finder = Finder {
        names,
        found: false,
    };
    expr.visit_with(&mut finder);
    if finder.found {
        return true;
    }

    let mut eval = DirectEvalAnalyzer::default();
    expr.visit_with(&mut eval);
    eval.unknown_direct_eval
        || eval.known_direct_eval_sources.iter().any(|source| {
            names
                .iter()
                .any(|name| js_source_mentions_binding(source, name))
        })
}

fn try_fold_for_of_entry_slots(for_of: &mut ForOfStmt, helper_context: &ForOfHelperContext) {
    let ForHead::VarDecl(decl) = &for_of.left else {
        return;
    };
    if decl.decls.len() != 1 {
        return;
    }
    let declarator = &decl.decls[0];
    if declarator.init.is_some() {
        return;
    }
    let Pat::Ident(binding) = &declarator.name else {
        return;
    };
    let left_kind = decl.kind;
    let temp = binding.id.clone();

    let Stmt::Block(body) = &*for_of.body else {
        return;
    };
    // Count only body uses as "inside". The iterable / after-loop reads of a
    // function-scoped left binding are live and must keep the ident.
    if helper_context.binding_is_used_outside(&body.stmts, &temp) {
        return;
    }
    // `var` leaks past the loop. `with` / unknown direct eval anywhere in
    // the module, or a known eval source that mentions the name, can still
    // observe it after we drop the left ident.
    if left_kind == VarDeclKind::Var && helper_context.dynamic_scope_can_observe_name(&temp.sym) {
        return;
    }
    let slots = consume_index_slots(&body.stmts, &temp);
    let Some((pat, bindings, slot_kind, consumed)) = array_pat_from_index_slots(slots) else {
        return;
    };
    let remaining = body.stmts[consumed..].to_vec();
    if remaining.iter().any(|stmt| stmt_uses_ident(stmt, &temp))
        || dynamic_scope_observes_binding(&remaining, &temp)
        || for_of_right_observes_fold_names(&for_of.right, &temp, &bindings)
    {
        return;
    }

    let body_uses = BindingUseIndex::collect_stmts(&remaining);
    if writes_consumed_const(&body.stmts[..consumed], &body_uses) {
        return;
    }
    let is_reassigned = bindings
        .iter()
        .any(|id| body_uses.has_direct_write(&id.to_id()));
    let kind = if slot_kind == VarDeclKind::Var {
        VarDeclKind::Var
    } else if is_reassigned {
        VarDeclKind::Let
    } else {
        slot_kind
    };

    let body_span = body.span;
    let body_ctxt = body.ctxt;
    for_of.left = ForHead::VarDecl(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: pat,
            init: None,
            definite: false,
        }],
    }));
    *for_of.body = Stmt::Block(BlockStmt {
        span: body_span,
        ctxt: body_ctxt,
        stmts: remaining,
    });
}

pub(super) fn stmt_as_single_var_decl(stmt: &Stmt) -> Option<&VarDecl> {
    let Stmt::Decl(Decl::Var(decl)) = stmt else {
        return None;
    };
    (decl.decls.len() == 1).then_some(decl)
}

fn is_index_access(expr: &Expr, obj_expr: &Expr, idx_sym: &Atom) -> bool {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return false;
    };
    if !same_ident_expr(obj, obj_expr) {
        return false;
    }
    let MemberProp::Computed(computed) = prop else {
        return false;
    };
    is_ident(&computed.expr, idx_sym)
}

fn is_numeric_index_access_key(expr: &Expr, obj: &Ident, index: f64) -> bool {
    let Expr::Member(MemberExpr {
        obj: member_obj,
        prop,
        ..
    }) = strip_parens(expr)
    else {
        return false;
    };
    if !is_ident_key(member_obj, obj) {
        return false;
    }
    let MemberProp::Computed(computed) = prop else {
        return false;
    };
    matches!(&*computed.expr, Expr::Lit(Lit::Num(num)) if num.value == index)
}

fn is_zero(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(swc_core::ecma::ast::Lit::Num(n)) if n.value == 0.0)
}

fn is_ident(expr: &Expr, sym: &Atom) -> bool {
    matches!(expr, Expr::Ident(id) if &id.sym == sym)
}

pub(super) fn is_ident_key(expr: &Expr, ident: &Ident) -> bool {
    matches!(expr, Expr::Ident(id) if id.sym == ident.sym && id.ctxt == ident.ctxt)
}

fn same_ident_expr(left: &Expr, right: &Expr) -> bool {
    match (left, right) {
        (Expr::Ident(left), Expr::Ident(right)) => left.sym == right.sym && left.ctxt == right.ctxt,
        _ => false,
    }
}

/// Check if a statement references a specific binding (by sym + ctxt).
pub(super) fn stmt_uses_ident(stmt: &Stmt, target: &Ident) -> bool {
    use swc_core::ecma::visit::Visit;

    struct IdentFinder {
        sym: Atom,
        ctxt: SyntaxContext,
        found: bool,
    }

    impl Visit for IdentFinder {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym == self.sym && ident.ctxt == self.ctxt {
                self.found = true;
            }
        }
    }

    let mut finder = IdentFinder {
        sym: target.sym.clone(),
        ctxt: target.ctxt,
        found: false,
    };
    finder.visit_stmt(stmt);
    finder.found
}

/// Check if a statement references the exact identifier binding.
pub(super) fn stmt_uses_ident_key(stmt: &Stmt, ident: &Ident) -> bool {
    use swc_core::ecma::visit::Visit;

    struct IdentFinder {
        sym: Atom,
        ctxt: SyntaxContext,
        found: bool,
    }

    impl Visit for IdentFinder {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym == self.sym && ident.ctxt == self.ctxt {
                self.found = true;
            }
        }
    }

    let mut finder = IdentFinder {
        sym: ident.sym.clone(),
        ctxt: ident.ctxt,
        found: false,
    };
    finder.visit_stmt(stmt);
    finder.found
}

/// Check if a statement references the iterator result binding anywhere except
/// as the object in `result.value`.
fn stmt_uses_ident_key_outside_value_member(stmt: &Stmt, ident: &Ident) -> bool {
    use swc_core::ecma::visit::{Visit, VisitWith};

    struct IdentFinder {
        sym: Atom,
        ctxt: SyntaxContext,
        found: bool,
    }

    impl Visit for IdentFinder {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym == self.sym && ident.ctxt == self.ctxt {
                self.found = true;
            }
        }

        fn visit_member_expr(&mut self, member: &MemberExpr) {
            if let Expr::Ident(obj) = &*member.obj {
                if obj.sym == self.sym
                    && obj.ctxt == self.ctxt
                    && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "value")
                {
                    return;
                }
            }
            member.visit_children_with(self);
        }
    }

    let mut finder = IdentFinder {
        sym: ident.sym.clone(),
        ctxt: ident.ctxt,
        found: false,
    };
    finder.visit_stmt(stmt);
    finder.found
}
