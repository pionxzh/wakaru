//! ES modules that an esbuild bundle reaches only through a lowered `import()`.
//!
//! Without code splitting, esbuild bundles an ES module that is loaded with
//! `import("./lazy.js")` as a lazy initializer and a namespace object, and
//! lowers the `import()` to a call that runs the initializer and returns the
//! namespace (producer esbuild@0.28.0 `--bundle`, formats iife and cjs,
//! minified or not):
//!
//! ```js
//! var lazy_exports = {};
//! __export(lazy_exports, { default: () => lazy_default, named: () => named });
//! function named(x) { return "named " + x; }
//! var lazy_default;
//! var init_lazy = __esm({ "lazy.js"() { lazy_default = "lazy-esm-provider"; } });
//!
//! const mod = await Promise.resolve().then(() => (init_lazy(), lazy_exports));
//! ```
//!
//! A template `import()` builds a `__glob` map whose values are the same
//! loaders. When the initializer and the namespace object are referenced only
//! by such loaders and the module's own declarations only by the module, the
//! module runs exactly when the first loader runs, as an ES module loaded by
//! `import()` does. [`plan_lazy_imports`] finds those modules and
//! [`LazyImportPlan::apply`] rewrites the bundle so the rest of the unpacker
//! sees an ordinary scope-hoisted module: the initializer body moves to top
//! level where the initializer was declared, and each loader becomes
//! `import()` of a placeholder naming the module's output file.
//! [`resolve_lazy_import_placeholders`] turns the placeholder into a relative
//! specifier once the importing module's filename is known. Where ESM
//! semantics differ from the bundle's is listed under
//! `esbuild_lazy_import_source_semantics` in `docs/rewrite-assumptions.md`.

use swc_core::atoms::Atom;
use swc_core::common::{Mark, Span, Spanned};
use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, CallExpr, Callee, Decl, Expr, ExprOrSpread, ExprStmt, Function,
    Ident, Import, Lit, MemberProp, Module, ModuleItem, Pat, Prop, PropOrSpread, Stmt, Str,
    VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::collections::{HashMap, HashSet};
use crate::js_names::is_valid_identifier_name;
use crate::unpacker::{
    stmts_have_function_level_return, stmts_have_function_level_special_bindings, BindingId,
};
use crate::utils::paren::strip_parens;

use super::factories::Factory;
use super::scope_boundaries::{collect_scope_hoisted_boundaries, detect_export_helper};
use super::synthesis::{dedup_filename, relative_import_path};

/// Prefix of the `import()` specifier a rewritten loader carries until
/// emission knows the importing module's filename. NUL never appears in a
/// bundler's module path.
const PLACEHOLDER_PREFIX: &str = "\0wakaru-esbuild-lazy-import:";

/// A lazily loaded module that unpack emits as a plain ES module.
pub(super) struct LazyModule {
    /// The namespace object esbuild built for the module.
    pub(super) namespace: BindingId,
    /// Output filename, already reserved in [`LazyImportPlan::seen_filenames`].
    pub(super) filename: String,
    /// `(public name, local binding)` from the `__export` call.
    pub(super) exports: Vec<(Atom, BindingId)>,
    /// Item index of the initializer declaration, before [`LazyImportPlan::apply`].
    factory_item: usize,
    factory_declarator: usize,
    /// Span of the module's last item once the plan is applied: the inlined
    /// body's last statement, or the declarators that shared the factory's
    /// `var`. Spans survive the entry's item filtering, indices do not.
    pub(super) last_item_span: Option<(u32, u32)>,
}

pub(super) struct LazyImportPlan {
    pub(super) modules: Vec<LazyModule>,
    /// Lazy helpers whose every factory was inlined, so their declarations
    /// still leave the entry.
    pub(super) inlined_helper_syms: HashSet<Atom>,
    /// Filenames taken so far, including `entry.js` and every factory's;
    /// scope module filenames are deduplicated against it.
    pub(super) seen_filenames: HashSet<String>,
    /// Every factory's filename, deduplicated in bundle order as if nothing
    /// were inlined, so inlining one module renames no other.
    pub(super) factory_filenames: HashMap<BindingId, String>,
    loader_spans: HashSet<(u32, u32)>,
}

/// Find the lazily loaded ES modules of a bundle whose resolved AST is
/// `analysis_module`. `factories` are its collected lazy factories.
/// `collect_factories` runs only once the bundle has both namespace objects
/// and loaders.
pub(super) fn plan_lazy_imports(
    analysis_module: &Module,
    collect_factories: impl FnOnce() -> Vec<Factory>,
    commonjs_helper_syms: &HashSet<Atom>,
    unresolved_mark: Mark,
) -> Option<LazyImportPlan> {
    let body = &analysis_module.body;
    let (_, export_helper) = detect_export_helper(body)?;
    let boundaries = collect_scope_hoisted_boundaries(body, &export_helper);
    if boundaries.is_empty() {
        return None;
    }

    let mut loaders = LoaderCollector {
        unresolved_mark,
        loaders: Vec::new(),
    };
    analysis_module.visit_with(&mut loaders);
    if loaders.loaders.is_empty() {
        return None;
    }
    let factories = collect_factories();

    // Each initializer and each namespace object must pair with exactly one
    // partner across every loader.
    let mut namespace_by_factory: HashMap<BindingId, BindingId> = HashMap::default();
    let mut factory_by_namespace: HashMap<BindingId, BindingId> = HashMap::default();
    let mut loader_count: HashMap<BindingId, usize> = HashMap::default();
    let mut conflicting: HashSet<BindingId> = HashSet::default();
    for loader in &loaders.loaders {
        let paired = namespace_by_factory
            .entry(loader.factory.clone())
            .or_insert_with(|| loader.namespace.clone());
        if *paired != loader.namespace {
            conflicting.insert(loader.factory.clone());
        }
        let paired = factory_by_namespace
            .entry(loader.namespace.clone())
            .or_insert_with(|| loader.factory.clone());
        if *paired != loader.factory {
            conflicting.insert(loader.factory.clone());
        }
        *loader_count.entry(loader.factory.clone()).or_default() += 1;
    }

    let mut occurrences = BindingOccurrences::default();
    analysis_module.visit_with(&mut occurrences);

    let mut seen_filenames: HashSet<String> = HashSet::default();
    seen_filenames.insert("entry.js".to_string());
    let factory_filenames: HashMap<BindingId, String> = factories
        .iter()
        .map(|factory| {
            (
                factory.binding.clone(),
                dedup_filename(&factory.filename, &mut seen_filenames),
            )
        })
        .collect();
    let factory_locations: HashSet<(usize, usize)> = factories
        .iter()
        .map(Factory::analysis_item_and_declarator)
        .collect();
    let mut modules = Vec::new();
    let mut inlined_factory_helpers: HashMap<Atom, usize> = HashMap::default();
    for factory in &factories {
        if commonjs_helper_syms.contains(&factory.helper_sym)
            || conflicting.contains(&factory.binding)
        {
            continue;
        }
        let Some(namespace) = namespace_by_factory.get(&factory.binding) else {
            continue;
        };
        let Some(boundary_index) = boundaries
            .iter()
            .position(|boundary| boundary.ns_binding == *namespace)
        else {
            continue;
        };
        let boundary = &boundaries[boundary_index];
        let (factory_item, factory_declarator) = factory.analysis_item_and_declarator();
        // esbuild writes a lazy module's namespace, then its declarations,
        // then its initializer. Scope extraction gives a namespace every item
        // up to the next namespace, so code after the initializer (a module
        // without a namespace object) would join this one; the initializer
        // must end the region, or be followed only by another lazy module's
        // state and initializer.
        if factory_item <= boundary.export_call_index
            || boundaries.get(boundary_index + 1).is_some_and(|next| {
                next.ns_decl_index <= factory_item
                    || !gap_holds_only_factory_state(
                        body,
                        factory_item + 1..next.ns_decl_index,
                        &factory_locations,
                    )
            })
        {
            continue;
        }
        if !boundary
            .export_entries
            .iter()
            .all(|(public, _)| public == "default" || is_valid_identifier_name(public))
        {
            continue;
        }
        let loaders = loader_count[&factory.binding];
        // The initializer: its declaration and one call per loader. The
        // namespace: its declaration, the `__export` call, and one read per
        // loader.
        if occurrences.count(&factory.binding) != 1 + loaders
            || occurrences.count(namespace) != 2 + loaders
        {
            continue;
        }
        let Some(factory_body) = factory_body(&body[factory_item], factory_declarator) else {
            continue;
        };
        if !factory_body_can_move_to_top_level(&factory_body) {
            continue;
        }
        let region = boundary.ns_decl_index..=factory_item;
        if !region_items_only_declare(body, factory_item, factory_declarator, boundary) {
            continue;
        }
        if module_bindings_escape(body, region, &factory.binding, namespace) {
            continue;
        }

        *inlined_factory_helpers
            .entry(factory.helper_sym.clone())
            .or_default() += 1;
        modules.push(LazyModule {
            namespace: namespace.clone(),
            filename: factory_filenames[&factory.binding].clone(),
            exports: boundary.export_entries.clone(),
            factory_item,
            factory_declarator,
            last_item_span: None,
        });
    }
    if modules.is_empty() {
        return None;
    }

    let inlined_namespaces: HashSet<&BindingId> =
        modules.iter().map(|module| &module.namespace).collect();
    let loader_spans = loaders
        .loaders
        .iter()
        .filter(|loader| inlined_namespaces.contains(&loader.namespace))
        .map(|loader| (loader.span.lo.0, loader.span.hi.0))
        .collect();
    let helper_factory_counts =
        factories
            .iter()
            .fold(HashMap::<Atom, usize>::default(), |mut counts, factory| {
                *counts.entry(factory.helper_sym.clone()).or_default() += 1;
                counts
            });
    let inlined_helper_syms = inlined_factory_helpers
        .into_iter()
        .filter(|(helper, inlined)| helper_factory_counts.get(helper) == Some(inlined))
        .map(|(helper, _)| helper)
        .collect();
    Some(LazyImportPlan {
        modules,
        inlined_helper_syms,
        seen_filenames,
        factory_filenames,
        loader_spans,
    })
}

impl LazyImportPlan {
    /// Rewrite `module`, the bundle or its resolved clone (the two share item
    /// layout and spans), and record where each module's code ends.
    pub(super) fn apply(&mut self, module: &mut Module) {
        let mut rewriter = LoaderRewriter {
            loader_spans: &self.loader_spans,
            target_by_namespace: self
                .modules
                .iter()
                .map(|lazy| (lazy.namespace.0.clone(), lazy.filename.clone()))
                .collect(),
        };
        module.visit_mut_with(&mut rewriter);

        let factory_items: HashMap<usize, usize> = self
            .modules
            .iter()
            .enumerate()
            .map(|(index, lazy)| (lazy.factory_item, index))
            .collect();
        let mut body = Vec::with_capacity(module.body.len());
        for (item_index, item) in std::mem::take(&mut module.body).into_iter().enumerate() {
            let Some(&lazy_index) = factory_items.get(&item_index) else {
                body.push(item);
                continue;
            };
            let lazy = &mut self.modules[lazy_index];
            let inlined = inline_factory(item, lazy.factory_declarator);
            // An empty body leaves the module's declarations as its end.
            lazy.last_item_span = inlined.last().or(body.last()).map(|item| {
                let span = item.span();
                (span.lo.0, span.hi.0)
            });
            body.extend(inlined);
        }
        module.body = body;
    }
}

/// Replace each placeholder `import()` specifier in `items` with the path
/// from `from_filename` to the module it names.
pub(super) fn resolve_lazy_import_placeholders(items: &mut [ModuleItem], from_filename: &str) {
    let mut resolver = PlaceholderResolver { from_filename };
    for item in items {
        item.visit_mut_with(&mut resolver);
    }
}

struct Loader {
    span: Span,
    factory: BindingId,
    namespace: BindingId,
}

/// Collects `Promise.resolve().then(() => (init_x(), x_exports))`.
struct LoaderCollector {
    unresolved_mark: Mark,
    loaders: Vec<Loader>,
}

impl Visit for LoaderCollector {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some((factory, namespace)) = lowered_import_loader(call, self.unresolved_mark) {
            self.loaders.push(Loader {
                span: call.span,
                factory: binding_of(factory),
                namespace: binding_of(namespace),
            });
            return;
        }
        call.visit_children_with(self);
    }
}

fn lowered_import_loader(call: &CallExpr, unresolved_mark: Mark) -> Option<(&Ident, &Ident)> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(then) = strip_parens(callee) else {
        return None;
    };
    if !matches!(&then.prop, MemberProp::Ident(prop) if prop.sym == "then") {
        return None;
    }
    let Expr::Call(resolve) = strip_parens(&then.obj) else {
        return None;
    };
    if !resolve.args.is_empty() {
        return None;
    }
    let Callee::Expr(resolve_callee) = &resolve.callee else {
        return None;
    };
    let Expr::Member(resolve_member) = strip_parens(resolve_callee) else {
        return None;
    };
    if !matches!(&resolve_member.prop, MemberProp::Ident(prop) if prop.sym == "resolve") {
        return None;
    }
    let Expr::Ident(promise) = strip_parens(&resolve_member.obj) else {
        return None;
    };
    if promise.sym != "Promise" || promise.ctxt.outer() != unresolved_mark {
        return None;
    }
    let [ExprOrSpread { spread: None, expr }] = call.args.as_slice() else {
        return None;
    };
    let Expr::Arrow(ArrowExpr {
        params,
        body,
        is_async: false,
        is_generator: false,
        ..
    }) = strip_parens(expr)
    else {
        return None;
    };
    let ArrowFunctionBody::Expr(body) = &**body else {
        return None;
    };
    if !params.is_empty() {
        return None;
    }
    let Expr::Seq(sequence) = strip_parens(body) else {
        return None;
    };
    let [init, namespace] = sequence.exprs.as_slice() else {
        return None;
    };
    let Expr::Call(init) = strip_parens(init) else {
        return None;
    };
    let Callee::Expr(init_callee) = &init.callee else {
        return None;
    };
    let (Expr::Ident(factory), Expr::Ident(namespace)) =
        (strip_parens(init_callee), strip_parens(namespace))
    else {
        return None;
    };
    init.args.is_empty().then_some((factory, namespace))
}

fn binding_of(ident: &Ident) -> BindingId {
    (ident.sym.clone(), ident.ctxt)
}

/// Every identifier occurrence by resolved binding, declarations included.
#[derive(Default)]
struct BindingOccurrences {
    counts: HashMap<BindingId, usize>,
}

impl BindingOccurrences {
    fn count(&self, binding: &BindingId) -> usize {
        self.counts.get(binding).copied().unwrap_or(0)
    }
}

impl Visit for BindingOccurrences {
    fn visit_ident(&mut self, ident: &Ident) {
        *self.counts.entry(binding_of(ident)).or_default() += 1;
    }
}

enum FactoryBody<'a> {
    Stmts(&'a [Stmt]),
    Expr(&'a Expr),
}

fn factory_init(item: &ModuleItem, declarator: usize) -> Option<&CallExpr> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };
    let Expr::Call(call) = var.decls.get(declarator)?.init.as_deref()? else {
        return None;
    };
    Some(call)
}

fn factory_body(item: &ModuleItem, declarator: usize) -> Option<FactoryBody<'_>> {
    let call = factory_init(item, declarator)?;
    let [ExprOrSpread { spread: None, expr }] = call.args.as_slice() else {
        return None;
    };
    match &**expr {
        Expr::Object(object) => {
            let [PropOrSpread::Prop(prop)] = object.props.as_slice() else {
                return None;
            };
            let Prop::Method(method) = &**prop else {
                return None;
            };
            plain_function_body(&method.function)
        }
        Expr::Fn(function) => plain_function_body(&function.function),
        Expr::Arrow(ArrowExpr {
            body,
            params,
            is_async: false,
            is_generator: false,
            ..
        }) if params.is_empty() => Some(match &**body {
            ArrowFunctionBody::FunctionBody(block) => FactoryBody::Stmts(&block.stmts),
            ArrowFunctionBody::Expr(expr) => FactoryBody::Expr(expr),
        }),
        _ => None,
    }
}

fn plain_function_body(function: &Function) -> Option<FactoryBody<'_>> {
    if function.is_async || function.is_generator || !function.params.is_empty() {
        return None;
    }
    Some(FactoryBody::Stmts(&function.body.as_ref()?.stmts))
}

/// The body runs as top-level code after the move: it must not return, read
/// the function's `this` or `arguments`, or declare a name whose scope would
/// widen to the module.
fn factory_body_can_move_to_top_level(body: &FactoryBody<'_>) -> bool {
    match body {
        FactoryBody::Stmts(stmts) => {
            !stmts_have_function_level_return(stmts)
                && !stmts_have_function_level_special_bindings(stmts)
                && stmts.iter().all(|stmt| !matches!(stmt, Stmt::Decl(_)))
                && !declares_function_scoped_var(stmts)
        }
        FactoryBody::Expr(expr) => {
            let stmt = Stmt::Expr(ExprStmt {
                span: expr.span(),
                expr: Box::new((*expr).clone()),
            });
            !stmts_have_function_level_special_bindings(std::slice::from_ref(&stmt))
        }
    }
}

fn declares_function_scoped_var(stmts: &[Stmt]) -> bool {
    struct Finder {
        found: bool,
    }
    impl Visit for Finder {
        fn visit_var_decl(&mut self, var: &VarDecl) {
            if var.kind == VarDeclKind::Var {
                self.found = true;
            }
            var.visit_children_with(self);
        }
        fn visit_function(&mut self, _: &Function) {}
        fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    }
    let mut finder = Finder { found: false };
    for stmt in stmts {
        stmt.visit_with(&mut finder);
    }
    finder.found
}

/// Between an initializer and the next namespace, esbuild may write a lazy
/// module without a namespace object whose only top-level code is its state
/// and its initializer, such as a glob import's virtual module
/// (`var G; var init_ = __esm({ ... G = __glob({...}) })`). Those leave the
/// entry with their factory, so they add no code to this module's region.
fn gap_holds_only_factory_state(
    body: &[ModuleItem],
    gap: std::ops::Range<usize>,
    factory_locations: &HashSet<(usize, usize)>,
) -> bool {
    let mut state = HashSet::default();
    let mut written = DeclaredBindings::default();
    for index in gap {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = &body[index] else {
            return false;
        };
        for (declarator, decl) in var.decls.iter().enumerate() {
            if factory_locations.contains(&(index, declarator)) {
                decl.visit_with(&mut written);
                continue;
            }
            let (Pat::Ident(name), None) = (&decl.name, &decl.init) else {
                return false;
            };
            state.insert(binding_of(&name.id));
        }
    }
    state.is_subset(&written.bindings)
}

/// Between the namespace and the initializer, esbuild writes only the
/// module's hoisted declarations; anything else could be another module's
/// code, which must not move into a module that runs later.
fn region_items_only_declare(
    body: &[ModuleItem],
    factory_item: usize,
    factory_declarator: usize,
    boundary: &super::scope_boundaries::ScopeHoistedBoundary,
) -> bool {
    let declares_without_effects = |decl: &VarDeclarator| {
        decl.init
            .as_deref()
            .is_none_or(|init| matches!(strip_parens(init), Expr::Fn(_) | Expr::Arrow(_)))
    };
    let region_declares = (boundary.ns_decl_index..factory_item)
        .filter(|&index| index != boundary.ns_decl_index && index != boundary.export_call_index)
        .all(|index| match &body[index] {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(_))) => true,
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                var.decls.iter().all(declares_without_effects)
            }
            _ => false,
        });
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(factory_var))) = &body[factory_item] else {
        return false;
    };
    region_declares
        && factory_var
            .decls
            .iter()
            .enumerate()
            .all(|(index, decl)| index == factory_declarator || declares_without_effects(decl))
}

/// Whether code outside `region` references a binding the module declares
/// or its initializer writes. Such a reader could run before the first
/// loader, so the module must keep its lazy initializer.
/// The initializer and the namespace object are checked separately: the
/// loaders outside the module reference them by design.
fn module_bindings_escape(
    body: &[ModuleItem],
    region: std::ops::RangeInclusive<usize>,
    factory: &BindingId,
    namespace: &BindingId,
) -> bool {
    let mut declared = DeclaredBindings::default();
    for index in region.clone() {
        body[index].visit_with(&mut declared);
    }
    declared.bindings.remove(factory);
    declared.bindings.remove(namespace);
    let mut reader = ReferencesAny {
        bindings: &declared.bindings,
        found: false,
    };
    for (index, item) in body.iter().enumerate() {
        if !region.contains(&index) {
            item.visit_with(&mut reader);
        }
    }
    reader.found
}

/// Bindings declared at any depth, plus every identifier assigned to, so an
/// initializer's writes to module state count as the module's.
#[derive(Default)]
struct DeclaredBindings {
    bindings: HashSet<BindingId>,
}

impl Visit for DeclaredBindings {
    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        for ident in swc_core::ecma::utils::find_pat_ids::<_, Ident>(&decl.name) {
            self.bindings.insert(binding_of(&ident));
        }
        decl.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, decl: &swc_core::ecma::ast::FnDecl) {
        self.bindings.insert(binding_of(&decl.ident));
        decl.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
        if let Some(ident) = assign.left.as_ident() {
            self.bindings.insert(binding_of(&ident.id));
        }
        assign.visit_children_with(self);
    }
}

struct ReferencesAny<'a> {
    bindings: &'a HashSet<BindingId>,
    found: bool,
}

impl Visit for ReferencesAny<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if self.bindings.contains(&binding_of(ident)) {
            self.found = true;
        }
    }
}

/// The items replacing a factory declaration: the declarators around the
/// factory, then the factory body as top-level statements.
fn inline_factory(item: ModuleItem, declarator: usize) -> Vec<ModuleItem> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(mut var))) = item else {
        unreachable!("planned factory item is a var declaration");
    };
    let after: Vec<VarDeclarator> = var.decls.split_off(declarator + 1);
    let factory = var.decls.pop().expect("planned factory declarator");
    let mut items = Vec::new();
    let around: Vec<VarDeclarator> = var.decls.into_iter().chain(after).collect();
    if !around.is_empty() {
        items.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
            decls: around,
            ..*var
        })))));
    }
    let Some(init) = factory.init else {
        unreachable!("planned factory declarator has an initializer");
    };
    let Expr::Call(mut call) = *init else {
        unreachable!("planned factory initializer is a helper call");
    };
    let body_expr = *call.args.pop().expect("planned factory argument").expr;
    let stmts = match body_expr {
        Expr::Object(mut object) => {
            let Some(PropOrSpread::Prop(prop)) = object.props.pop() else {
                unreachable!("planned factory object has one method");
            };
            let Prop::Method(method) = *prop else {
                unreachable!("planned factory property is a method");
            };
            method
                .function
                .body
                .map(|body| body.stmts)
                .unwrap_or_default()
        }
        Expr::Fn(function) => function
            .function
            .body
            .map(|body| body.stmts)
            .unwrap_or_default(),
        Expr::Arrow(arrow) => match *arrow.body {
            ArrowFunctionBody::FunctionBody(block) => block.stmts,
            ArrowFunctionBody::Expr(expr) => vec![Stmt::Expr(ExprStmt {
                span: expr.span(),
                expr,
            })],
        },
        _ => unreachable!("planned factory argument shape"),
    };
    items.extend(stmts.into_iter().map(ModuleItem::Stmt));
    items
}

struct LoaderRewriter<'a> {
    loader_spans: &'a HashSet<(u32, u32)>,
    /// Keyed by namespace atom: the bundle and its resolved clone share
    /// atoms, while only the clone carries resolved contexts.
    target_by_namespace: HashMap<Atom, String>,
}

impl VisitMut for LoaderRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Call(call) = expr {
            if self
                .loader_spans
                .contains(&(call.span.lo.0, call.span.hi.0))
            {
                if let Some(target) = loader_namespace_atom(call)
                    .and_then(|namespace| self.target_by_namespace.get(namespace))
                {
                    *expr = Expr::Call(CallExpr {
                        span: call.span,
                        callee: Callee::Import(Import {
                            span: call.span,
                            phase: Default::default(),
                        }),
                        args: vec![ExprOrSpread {
                            spread: None,
                            expr: Box::new(Expr::Lit(Lit::Str(Str {
                                span: call.span,
                                value: format!("{PLACEHOLDER_PREFIX}{target}").into(),
                                raw: None,
                            }))),
                        }],
                        ..Default::default()
                    });
                    return;
                }
            }
        }
        expr.visit_mut_children_with(self);
    }
}

/// The namespace atom a loader returns, read structurally so the same
/// lookup works on the unresolved bundle.
fn loader_namespace_atom(call: &CallExpr) -> Option<&Atom> {
    let [ExprOrSpread { expr, .. }] = call.args.as_slice() else {
        return None;
    };
    let Expr::Arrow(arrow) = strip_parens(expr) else {
        return None;
    };
    let ArrowFunctionBody::Expr(body) = &*arrow.body else {
        return None;
    };
    let Expr::Seq(sequence) = strip_parens(body) else {
        return None;
    };
    match sequence.exprs.last().map(|expr| strip_parens(expr)) {
        Some(Expr::Ident(namespace)) => Some(&namespace.sym),
        _ => None,
    }
}

struct PlaceholderResolver<'a> {
    from_filename: &'a str,
}

impl VisitMut for PlaceholderResolver<'_> {
    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        call.visit_mut_children_with(self);
        if !matches!(call.callee, Callee::Import(_)) {
            return;
        }
        let Some(ExprOrSpread { expr, .. }) = call.args.first_mut() else {
            return;
        };
        let Expr::Lit(Lit::Str(specifier)) = &mut **expr else {
            return;
        };
        let Some(target) = specifier
            .value
            .as_str()
            .and_then(|value| value.strip_prefix(PLACEHOLDER_PREFIX))
        else {
            return;
        };
        specifier.value = relative_import_path(self.from_filename, target).into();
        specifier.raw = None;
    }
}
