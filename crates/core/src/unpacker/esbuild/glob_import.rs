//! esbuild glob imports whose every match is a lazily loaded ES module.
//!
//! esbuild bundles `import(`./pages/${p}.js`)` as a `__glob` lookup map from
//! each matching path, written relative to the importer, to a lowered loader,
//! and calls the map with the specifier (producer esbuild@0.28.0 `--bundle`,
//! formats iife and cjs, minified or not):
//!
//! ```js
//! var __glob = (map) => (path) => {
//!   var fn = map[path];
//!   if (fn) return fn();
//!   throw new Error("Module not found in bundle: " + path);
//! };
//! var globImport_pages_js = __glob({
//!   "./pages/home.js": () => Promise.resolve().then(() => (init_home(), home_exports)),
//! });
//! const page = await globImport_pages_js(`./pages/${p}.js`);
//! ```
//!
//! When the importer is itself a lazy module, the map is the state of a
//! virtual lazy module the importer's initializer runs first:
//!
//! ```js
//! var globImport_pages_js;
//! var init_ = __esm({
//!   'import("./pages/**/*.js") in src/i18n.js'() {
//!     globImport_pages_js = __glob({ ... });
//!   }
//! });
//! var init_i18n = __esm({ "src/i18n.js"() { init_(); } });
//! ```
//!
//! Once every loader of a map is planned to become `import()` (see
//! `lazy_import`), [`plan_glob_collapses`] collapses the map: each call
//! becomes `import()` of its own specifier again, the specifier's leading
//! static text pointing at the output directory of the matched modules, and
//! the map, its virtual module, and the calls of the virtual module's
//! initializer leave the bundle.
//!
//! A minified build drops the initializers' path keys, so the matched
//! modules are named after their bindings and the keys name no output file.
//! The keys are the matched modules' source paths, relative to the importer:
//! such modules take their key as their name, below the output root
//! (`../pages/home.js` names `pages/home.js`).

use swc_core::atoms::Atom;
use swc_core::common::Spanned;
use swc_core::ecma::ast::{
    ArrowFunctionBody, AssignOp, BinExpr, BinaryOp, CallExpr, Callee, Decl, EmptyStmt, Expr,
    ExprOrSpread, ExprStmt, Import, Lit, Module, ModuleItem, Pat, Prop, PropName, PropOrSpread,
    Stmt, Str, Tpl, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::collections::{HashMap, HashSet};
use crate::module_path::relative_import_specifier;
use crate::unpacker::{sanitize_relative_path, BindingId};
use crate::utils::paren::strip_parens;

use super::factories::Factory;
use super::lazy_import::{binding_of, factory_body, BindingOccurrences, FactoryBody};

/// Prefix of the leading static text a collapsed call's specifier carries
/// until emission knows the calling module's filename. NUL never appears in
/// a bundler's module path.
const PREFIX_PLACEHOLDER: &str = "\0wakaru-esbuild-glob-prefix:";

pub(super) type SpanKey = (u32, u32);

fn span_key(span: swc_core::common::Span) -> SpanKey {
    (span.lo.0, span.hi.0)
}

#[derive(Default)]
pub(super) struct GlobCollapsePlan {
    /// Call of a collapsed map → output path prefix its specifier's leading
    /// static text resolves to.
    call_prefixes: HashMap<SpanKey, String>,
    /// Calls of a removed virtual module's initializer.
    removed_init_calls: HashSet<SpanKey>,
    /// `(item, declarator)` removed: maps, virtual modules, and `__glob`
    /// helpers no other code calls.
    pub(super) removed_declarators: HashSet<(usize, usize)>,
    /// Top-level statements that only call a removed initializer.
    pub(super) removed_items: HashSet<usize>,
    /// Lazy helper of each removed virtual module.
    pub(super) removed_factory_helpers: Vec<Atom>,
    /// Matched modules renamed after their key: old filename → new.
    pub(super) renamed_targets: HashMap<String, String>,
}

/// Plan the collapse of every `__glob` map in `analysis_module` whose
/// loaders all appear in `loader_targets` (loader call span → output
/// filename of the module it will import). `kept_items` are items the
/// collapse must not remove declarators from. Targets in
/// `named_after_binding` may be renamed after their key; their new names are
/// reserved in `seen_filenames`.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_glob_collapses(
    analysis_module: &Module,
    factories: &[Factory],
    commonjs_helper_syms: &HashSet<Atom>,
    loader_targets: &HashMap<SpanKey, String>,
    named_after_binding: &HashSet<String>,
    seen_filenames: &mut HashSet<String>,
    occurrences: &BindingOccurrences,
    kept_items: &HashSet<usize>,
) -> GlobCollapsePlan {
    let body = &analysis_module.body;
    let mut plan = GlobCollapsePlan::default();
    let helpers = glob_helpers(body);
    if helpers.is_empty() {
        return plan;
    }
    let uninitialized: HashMap<BindingId, (usize, usize)> = top_level_declarators(body)
        .filter(|(_, _, decl)| decl.init.is_none())
        .filter_map(|(item, index, decl)| {
            let Pat::Ident(name) = &decl.name else {
                return None;
            };
            Some((binding_of(&name.id), (item, index)))
        })
        .collect();

    let mut maps = Vec::new();
    for (item, index, decl) in top_level_declarators(body) {
        if let Some((map, helper, entries)) = top_level_map(decl, &helpers) {
            maps.push(GlobMap {
                declarators: vec![(item, index)],
                map,
                helper,
                entries,
                assigned_in_factory: false,
                factory: None,
            });
        }
    }
    for factory in factories {
        if commonjs_helper_syms.contains(&factory.helper_sym) {
            continue;
        }
        let (item, index) = factory.analysis_item_and_declarator();
        let Some((map, helper, entries)) = factory_body(&body[item], index)
            .and_then(|factory_body| virtual_module_map(&factory_body, &helpers))
        else {
            continue;
        };
        let Some(&state) = uninitialized.get(&map) else {
            continue;
        };
        maps.push(GlobMap {
            declarators: vec![state, (item, index)],
            map,
            helper,
            entries,
            assigned_in_factory: true,
            factory: Some((factory.binding.clone(), factory.helper_sym.clone())),
        });
    }
    if maps.is_empty() {
        return plan;
    }

    let mut calls = MapCalls {
        maps: maps.iter().map(|map| map.map.clone()).collect(),
        calls: HashMap::default(),
    };
    analysis_module.visit_with(&mut calls);
    let mut init_calls = StatementCalls {
        callees: maps
            .iter()
            .filter_map(|map| map.factory.as_ref().map(|(binding, _)| binding.clone()))
            .collect(),
        calls: HashMap::default(),
        top_level_items: HashMap::default(),
        item_index: None,
    };
    for (item_index, item) in body.iter().enumerate() {
        init_calls.item_index = Some(item_index);
        item.visit_with(&mut init_calls);
    }

    let mut collapsed_by_helper: HashMap<BindingId, usize> = HashMap::default();
    for map in &maps {
        if map
            .declarators
            .iter()
            .any(|(item, _)| kept_items.contains(item))
        {
            continue;
        }
        let Some(mut targets) = map
            .entries
            .iter()
            .map(|(key, loader)| {
                let target = loader_targets.get(loader)?;
                let target = plan.renamed_targets.get(target).unwrap_or(target);
                Some((key, target.clone()))
            })
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let map_calls = calls.calls.get(&map.map).map_or(&[][..], Vec::as_slice);
        let own_occurrences = 1 + usize::from(map.assigned_in_factory);
        if map_calls.is_empty() || occurrences.count(&map.map) != own_occurrences + map_calls.len()
        {
            continue;
        }
        let (removed_calls, removed_items) = match &map.factory {
            Some((factory, _)) => {
                let calls = init_calls.calls.get(factory).map_or(&[][..], Vec::as_slice);
                if occurrences.count(factory) != 1 + calls.len() {
                    continue;
                }
                let items = init_calls
                    .top_level_items
                    .get(factory)
                    .map_or(&[][..], Vec::as_slice);
                (calls, items)
            }
            None => (&[][..], &[][..]),
        };
        let call_prefixes = |targets: &[(&String, String)]| {
            map_calls
                .iter()
                .map(|call| Some((call.span, output_prefix(&call.static_prefix, targets)?)))
                .collect::<Option<Vec<_>>>()
        };
        let mut renames = Vec::new();
        let prefixes = match call_prefixes(&targets) {
            Some(prefixes) => prefixes,
            None => {
                let Some(names) = names_from_keys(
                    &map_calls[0].static_prefix,
                    &targets,
                    named_after_binding,
                    seen_filenames,
                ) else {
                    continue;
                };
                for ((_, target), name) in targets.iter_mut().zip(names) {
                    renames.push((std::mem::replace(target, name.clone()), name));
                }
                let Some(prefixes) = call_prefixes(&targets) else {
                    continue;
                };
                prefixes
            }
        };
        for (old, new) in renames {
            seen_filenames.insert(new.to_ascii_lowercase());
            plan.renamed_targets.insert(old, new);
        }
        plan.removed_init_calls
            .extend(removed_calls.iter().copied());
        plan.removed_items.extend(removed_items.iter().copied());
        plan.call_prefixes.extend(prefixes);
        plan.removed_declarators
            .extend(map.declarators.iter().copied());
        plan.removed_factory_helpers
            .extend(map.factory.as_ref().map(|(_, helper)| helper.clone()));
        *collapsed_by_helper.entry(map.helper.clone()).or_default() += 1;
    }
    for (helper, location) in helpers {
        let collapsed = collapsed_by_helper.get(&helper).copied().unwrap_or(0);
        if collapsed > 0
            && occurrences.count(&helper) == 1 + collapsed
            && !kept_items.contains(&location.0)
        {
            plan.removed_declarators.insert(location);
        }
    }
    plan
}

impl GlobCollapsePlan {
    /// Rewrite the collapsed calls of `module` and drop the removed
    /// initializer calls nested in it. Removed declarators and top-level
    /// statements are left to the caller, which walks items by index.
    pub(super) fn rewrite_calls(&self, module: &mut Module) {
        module.visit_mut_with(&mut CallRewriter { plan: self });
    }
}

struct GlobMap {
    /// `(item, declarator)` holding the map, then its virtual module's
    /// initializer when it has one.
    declarators: Vec<(usize, usize)>,
    map: BindingId,
    helper: BindingId,
    /// `(key, loader call span)`.
    entries: Vec<(String, SpanKey)>,
    /// The map is assigned inside a virtual module rather than declared with
    /// its value.
    assigned_in_factory: bool,
    /// The virtual module's initializer and lazy helper.
    factory: Option<(BindingId, Atom)>,
}

fn top_level_declarators(
    body: &[ModuleItem],
) -> impl Iterator<Item = (usize, usize, &VarDeclarator)> {
    body.iter().enumerate().flat_map(|(item, module_item)| {
        let decls = match module_item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => var.decls.as_slice(),
            _ => &[],
        };
        decls
            .iter()
            .enumerate()
            .map(move |(index, decl)| (item, index, decl))
    })
}

/// `var __glob = (map) => (path) => { var fn = map[path]; if (fn) return
/// fn(); throw new Error("Module not found in bundle: " + path); }`, by
/// shape and its error message, which minifiers keep.
fn glob_helpers(body: &[ModuleItem]) -> HashMap<BindingId, (usize, usize)> {
    top_level_declarators(body)
        .filter_map(|(item, index, decl)| {
            let Pat::Ident(name) = &decl.name else {
                return None;
            };
            is_glob_helper(decl.init.as_deref()?).then(|| (binding_of(&name.id), (item, index)))
        })
        .collect()
}

fn is_glob_helper(expr: &Expr) -> bool {
    let Expr::Arrow(outer) = strip_parens(expr) else {
        return false;
    };
    let ArrowFunctionBody::Expr(inner) = &*outer.body else {
        return false;
    };
    let Expr::Arrow(inner) = strip_parens(inner) else {
        return false;
    };
    if outer.params.len() != 1 || inner.params.len() != 1 {
        return false;
    }
    let ArrowFunctionBody::FunctionBody(body) = &*inner.body else {
        return false;
    };
    let Some(Stmt::Throw(throw)) = body.stmts.last() else {
        return false;
    };
    let mut message = MessageFinder { found: false };
    throw.arg.visit_with(&mut message);
    body.stmts.len() == 3 && message.found
}

struct MessageFinder {
    found: bool,
}

impl Visit for MessageFinder {
    fn visit_str(&mut self, value: &Str) {
        if value.value.as_str() == Some("Module not found in bundle: ") {
            self.found = true;
        }
    }
}

/// `var G = __glob({...})`: the map binding, its helper, and its entries.
fn top_level_map(
    decl: &VarDeclarator,
    helpers: &HashMap<BindingId, (usize, usize)>,
) -> Option<(BindingId, BindingId, Vec<(String, SpanKey)>)> {
    let Pat::Ident(name) = &decl.name else {
        return None;
    };
    let (helper, entries) = map_call(decl.init.as_deref()?, helpers)?;
    Some((binding_of(&name.id), helper, entries))
}

/// A virtual module whose body is only `G = __glob({...})`.
fn virtual_module_map(
    body: &FactoryBody<'_>,
    helpers: &HashMap<BindingId, (usize, usize)>,
) -> Option<(BindingId, BindingId, Vec<(String, SpanKey)>)> {
    let expr = match body {
        FactoryBody::Stmts([Stmt::Expr(ExprStmt { expr, .. })]) => &**expr,
        FactoryBody::Expr(expr) => *expr,
        FactoryBody::Stmts(_) => return None,
    };
    let Expr::Assign(assign) = strip_parens(expr) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let map = assign.left.as_ident()?;
    let (helper, entries) = map_call(&assign.right, helpers)?;
    Some((binding_of(&map.id), helper, entries))
}

fn map_call(
    expr: &Expr,
    helpers: &HashMap<BindingId, (usize, usize)>,
) -> Option<(BindingId, Vec<(String, SpanKey)>)> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(helper) = strip_parens(callee) else {
        return None;
    };
    let helper = binding_of(helper);
    if !helpers.contains_key(&helper) {
        return None;
    }
    let [ExprOrSpread { spread: None, expr }] = call.args.as_slice() else {
        return None;
    };
    let Expr::Object(object) = strip_parens(expr) else {
        return None;
    };
    let entries = object
        .props
        .iter()
        .map(|prop| {
            let PropOrSpread::Prop(prop) = prop else {
                return None;
            };
            let Prop::KeyValue(entry) = &**prop else {
                return None;
            };
            let PropName::Str(key) = &entry.key else {
                return None;
            };
            let Expr::Arrow(loader) = strip_parens(&entry.value) else {
                return None;
            };
            if !loader.params.is_empty() || loader.is_async || loader.is_generator {
                return None;
            }
            let ArrowFunctionBody::Expr(loader_call) = &*loader.body else {
                return None;
            };
            let Expr::Call(loader_call) = strip_parens(loader_call) else {
                return None;
            };
            Some((key.value.as_str()?.to_string(), span_key(loader_call.span)))
        })
        .collect::<Option<Vec<_>>>()?;
    (!entries.is_empty()).then_some((helper, entries))
}

/// The output path prefix `D` such that each target is `D` followed by what
/// its key has after `static_prefix`. A specifier `static_prefix + rest`
/// that names a key then names its target as `D + rest`.
fn output_prefix(static_prefix: &str, targets: &[(&String, String)]) -> Option<String> {
    let mut prefix: Option<&str> = None;
    for (key, target) in targets {
        let rest = key.strip_prefix(static_prefix)?;
        let target_prefix = target.strip_suffix(rest)?;
        if prefix.is_some_and(|prefix| prefix != target_prefix) {
            return None;
        }
        prefix = Some(target_prefix);
    }
    prefix.map(str::to_string)
}

struct MapCall {
    span: SpanKey,
    static_prefix: String,
}

/// Calls of each map, keyed by map binding. A call whose specifier has no
/// leading static text is skipped, so the map's occurrence count fails.
struct MapCalls {
    maps: HashSet<BindingId>,
    calls: HashMap<BindingId, Vec<MapCall>>,
}

impl Visit for MapCalls {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        call.visit_children_with(self);
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Ident(map) = strip_parens(callee) else {
            return;
        };
        let map = binding_of(map);
        if !self.maps.contains(&map) {
            return;
        }
        let [ExprOrSpread { spread: None, expr }] = call.args.as_slice() else {
            return;
        };
        let Some(static_prefix) = specifier_static_prefix(expr) else {
            return;
        };
        self.calls.entry(map).or_default().push(MapCall {
            span: span_key(call.span),
            static_prefix,
        });
    }
}

/// Names for targets named after their bindings: each key with the call's
/// leading static text replaced by that text's path below the output root,
/// with `./` and `../` dropped (`../pages/` → `pages/`). None when a target
/// has a name of its own or a new name is taken, unsafe, or repeated.
fn names_from_keys(
    static_prefix: &str,
    targets: &[(&String, String)],
    named_after_binding: &HashSet<String>,
    seen_filenames: &HashSet<String>,
) -> Option<Vec<String>> {
    let mut below = static_prefix;
    while let Some(rest) = below
        .strip_prefix("./")
        .or_else(|| below.strip_prefix("../"))
    {
        below = rest;
    }
    let mut taken = HashSet::default();
    targets
        .iter()
        .map(|(key, target)| {
            if !named_after_binding.contains(target) {
                return None;
            }
            let name = format!("{below}{}", key.strip_prefix(static_prefix)?);
            let lower = name.to_ascii_lowercase();
            (sanitize_relative_path(&name, "") == name
                && !seen_filenames.contains(&lower)
                && taken.insert(lower))
            .then_some(name)
        })
        .collect()
}

/// The leading static text of a template literal, or of the leftmost string
/// of a `+` chain.
fn specifier_static_prefix(expr: &Expr) -> Option<String> {
    match strip_parens(expr) {
        Expr::Tpl(tpl) => Some(tpl.quasis.first()?.cooked.as_ref()?.as_str()?.to_string()),
        Expr::Bin(bin) if bin.op == BinaryOp::Add => match leftmost_operand(bin) {
            Expr::Lit(Lit::Str(value)) => Some(value.value.as_str()?.to_string()),
            _ => None,
        },
        _ => None,
    }
}

/// The leftmost operand of a `+` chain. Both versions descend through the
/// same nodes, so a prefix read by one is the prefix written by the other.
fn leftmost_operand(bin: &BinExpr) -> &Expr {
    match &*bin.left {
        Expr::Bin(left) if left.op == BinaryOp::Add => leftmost_operand(left),
        left => strip_parens(left),
    }
}

fn leftmost_operand_mut(bin: &mut BinExpr) -> &mut Expr {
    if matches!(&*bin.left, Expr::Bin(left) if left.op == BinaryOp::Add) {
        let Expr::Bin(left) = &mut *bin.left else {
            unreachable!("checked above");
        };
        return leftmost_operand_mut(left);
    }
    strip_parens_mut(&mut bin.left)
}

/// Calls `f()` of the given bindings in statement position: a whole
/// expression statement, or an element of a sequence that is one.
struct StatementCalls {
    callees: HashSet<BindingId>,
    calls: HashMap<BindingId, Vec<SpanKey>>,
    /// Top-level items that are such a call as a whole.
    top_level_items: HashMap<BindingId, Vec<usize>>,
    item_index: Option<usize>,
}

impl StatementCalls {
    fn statement_call(&self, expr: &Expr) -> Option<(BindingId, SpanKey)> {
        let Expr::Call(call) = strip_parens(expr) else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Ident(callee) = strip_parens(callee) else {
            return None;
        };
        let callee = binding_of(callee);
        (call.args.is_empty() && self.callees.contains(&callee))
            .then(|| (callee, span_key(call.span)))
    }
}

impl Visit for StatementCalls {
    fn visit_module_item(&mut self, item: &ModuleItem) {
        let top_level = self.item_index.take();
        if let (Some(index), ModuleItem::Stmt(Stmt::Expr(stmt))) = (top_level, item) {
            let calls: Option<Vec<_>> = statement_elements(&stmt.expr)
                .into_iter()
                .map(|element| self.statement_call(element))
                .collect();
            if let Some(calls) = calls {
                for (callee, span) in calls {
                    self.calls.entry(callee.clone()).or_default().push(span);
                    let items = self.top_level_items.entry(callee).or_default();
                    if !items.contains(&index) {
                        items.push(index);
                    }
                }
                return;
            }
        }
        item.visit_children_with(self);
    }

    fn visit_expr_stmt(&mut self, stmt: &ExprStmt) {
        for element in statement_elements(&stmt.expr) {
            match self.statement_call(element) {
                Some((callee, span)) => self.calls.entry(callee).or_default().push(span),
                None => element.visit_with(self),
            }
        }
    }
}

/// The expressions an expression statement evaluates for effect: the
/// elements of a sequence, or the expression itself.
fn statement_elements(expr: &Expr) -> Vec<&Expr> {
    match strip_parens(expr) {
        Expr::Seq(seq) => seq.exprs.iter().map(|expr| &**expr).collect(),
        expr => vec![expr],
    }
}

struct CallRewriter<'a> {
    plan: &'a GlobCollapsePlan,
}

impl CallRewriter<'_> {
    fn is_removed_call(&self, expr: &Expr) -> bool {
        matches!(strip_parens(expr), Expr::Call(call)
            if self.plan.removed_init_calls.contains(&span_key(call.span)))
    }

    fn is_removed_stmt(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Expr(ExprStmt { expr, .. })
            if statement_elements(expr).into_iter().all(|element| self.is_removed_call(element)))
    }
}

impl VisitMut for CallRewriter<'_> {
    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.retain(|stmt| !self.is_removed_stmt(stmt));
        stmts.visit_mut_children_with(self);
    }

    /// A removed call in a single-statement position (`if (x) init_();`).
    fn visit_mut_stmt(&mut self, stmt: &mut Stmt) {
        if self.is_removed_stmt(stmt) {
            *stmt = Stmt::Empty(EmptyStmt { span: stmt.span() });
            return;
        }
        stmt.visit_mut_children_with(self);
    }

    fn visit_mut_expr_stmt(&mut self, stmt: &mut ExprStmt) {
        if let Expr::Seq(seq) = &mut *stmt.expr {
            seq.exprs.retain(|expr| !self.is_removed_call(expr));
            if seq.exprs.len() == 1 {
                stmt.expr = seq.exprs.pop().expect("one element");
            }
        }
        stmt.visit_mut_children_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        let Expr::Call(call) = expr else {
            return;
        };
        let Some(prefix) = self.plan.call_prefixes.get(&span_key(call.span)) else {
            return;
        };
        let Some(ExprOrSpread {
            expr: specifier, ..
        }) = call.args.pop()
        else {
            return;
        };
        let mut specifier = *specifier;
        set_static_prefix(&mut specifier, &format!("{PREFIX_PLACEHOLDER}{prefix}"));
        *expr = Expr::Call(CallExpr {
            span: call.span,
            callee: Callee::Import(Import {
                span: call.span,
                phase: Default::default(),
            }),
            args: vec![ExprOrSpread {
                spread: None,
                expr: Box::new(specifier),
            }],
            ..Default::default()
        });
    }
}

fn set_static_prefix(specifier: &mut Expr, prefix: &str) {
    match specifier {
        Expr::Paren(paren) => set_static_prefix(&mut paren.expr, prefix),
        Expr::Tpl(Tpl { quasis, .. }) => {
            if let Some(first) = quasis.first_mut() {
                first.cooked = Some(prefix.into());
                first.raw = escape_template_raw(prefix).into();
            }
        }
        Expr::Bin(bin) => {
            if let Expr::Lit(Lit::Str(value)) = leftmost_operand_mut(bin) {
                value.value = prefix.into();
                value.raw = None;
            }
        }
        _ => {}
    }
}

fn strip_parens_mut(expr: &mut Expr) -> &mut Expr {
    match expr {
        Expr::Paren(paren) => strip_parens_mut(&mut paren.expr),
        expr => expr,
    }
}

fn escape_template_raw(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
}

/// Resolve a collapsed call's placeholder prefix in `call`, an `import()`
/// written in `from_filename`, into a relative specifier.
pub(super) fn resolve_prefix_placeholder(call: &mut CallExpr, from_filename: &str) {
    let Some(ExprOrSpread { expr, .. }) = call.args.first_mut() else {
        return;
    };
    let Some(prefix) = specifier_static_prefix(expr)
        .and_then(|text| text.strip_prefix(PREFIX_PLACEHOLDER).map(str::to_string))
    else {
        return;
    };
    set_static_prefix(expr, &relative_prefix(from_filename, &prefix));
}

/// The relative specifier prefix from `from_filename` to the output path
/// prefix `prefix`, which may end inside a file name (`pages/page-`).
fn relative_prefix(from_filename: &str, prefix: &str) -> String {
    let (dir, stem) = prefix.rsplit_once('/').unwrap_or(("", prefix));
    // Resolve a file in the prefix's directory, then drop its name.
    let probe = if dir.is_empty() {
        "x".to_string()
    } else {
        format!("{dir}/x")
    };
    let spec = relative_import_specifier(from_filename, &probe);
    format!("{}{stem}", spec.strip_suffix('x').unwrap_or(&spec))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_prefix_points_at_the_prefix_directory() {
        assert_eq!(relative_prefix("entry.js", "pages/"), "./pages/");
        assert_eq!(relative_prefix("src/i18n.js", "pages/"), "../pages/");
        assert_eq!(relative_prefix("src/i18n.js", "src/page-"), "./page-");
        assert_eq!(relative_prefix("entry.js", "page-"), "./page-");
        assert_eq!(relative_prefix("a/b/c.js", ""), "../../");
    }

    #[test]
    fn output_prefix_needs_one_prefix_for_every_key() {
        let home_key = "../pages/home.js".to_string();
        let about_key = "../pages/about.js".to_string();
        let home = (&home_key, "pages/home.js".to_string());
        let about = (&about_key, "pages/about.js".to_string());
        assert_eq!(
            output_prefix("../pages/", &[home.clone(), about]).as_deref(),
            Some("pages/")
        );
        // A deduplicated output name breaks the correspondence.
        let renamed = (&about_key, "pages/about_2.js".to_string());
        assert_eq!(output_prefix("../pages/", &[home.clone(), renamed]), None);
        assert_eq!(output_prefix("./other/", &[home]), None);
    }

    #[test]
    fn names_from_keys_name_only_modules_named_after_bindings() {
        let home_key = "../pages/home.js".to_string();
        let about_key = "../pages/about.js".to_string();
        let targets = [
            (&home_key, "c.js".to_string()),
            (&about_key, "r.js".to_string()),
        ];
        let set = |names: &[&str]| -> HashSet<String> {
            names.iter().map(|name| name.to_string()).collect()
        };
        assert_eq!(
            names_from_keys(
                "../pages/",
                &targets,
                &set(&["c.js", "r.js"]),
                &set(&["entry.js"])
            ),
            Some(vec![
                "pages/home.js".to_string(),
                "pages/about.js".to_string()
            ])
        );
        // A taken name, compared lowercased as filename deduplication does.
        assert_eq!(
            names_from_keys(
                "../pages/",
                &targets,
                &set(&["c.js", "r.js"]),
                &set(&["pages/home.js"])
            ),
            None
        );
        // A module with a path key of its own keeps it.
        assert_eq!(
            names_from_keys("../pages/", &targets, &set(&["c.js"]), &set(&["entry.js"])),
            None
        );
    }
}
