//! Cross-file `[[Call]]` pins.
//!
//! Phase 1 records which imports are still callees. After every module's facts
//! exist, [`CallRequiredPlan`] resolves those edges to the defining export.
//! Phase 2 reads only that plan and seeds [`CallabilityIndex`] before alias
//! propagation, so the provider stays a function. Calls that Phase 2 will
//! rewrite to `super` are not pins unless the subclass export is itself pinned.

use crate::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::Mark;
use swc_core::ecma::ast::{
    BindingIdent, CallExpr, Callee, Expr, ExprOrSpread, ImportSpecifier, MemberExpr, MemberProp,
    Module, ModuleDecl, ModuleExportName, ModuleItem, Pat,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::helper_matcher::{binding_key, expr_binding_key, member_prop_name, BindingKey};
use super::un_es6_class::super_params_consumed_by_class_recovery;
use crate::facts::{ImportCallEdge, ImportKind, ModuleFacts, ModuleFactsMap};
use crate::module_path::resolve_relative_specifier;
use crate::utils::paren::strip_parens;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CallRequiredPlan {
    /// Canonical module key → exported names that must stay callable.
    by_module: HashMap<String, HashSet<Atom>>,
    /// Definitions left unpinned only because every call to them was predicted
    /// to become `super()`. A call to one of these that survives Phase 2 means
    /// the prediction was wrong.
    predicted_consumed: HashSet<(String, Atom)>,
}

impl CallRequiredPlan {
    pub(crate) fn build(facts: &ModuleFactsMap) -> Self {
        let mut hard: HashSet<(String, Atom)> = HashSet::default();
        let mut soft: Vec<SoftEdge> = Vec::new();
        if facts
            .iter()
            .all(|(_, module)| module.import_call_edges.is_empty())
        {
            return Self::default();
        }
        let modules = ModuleIndex::new(facts);

        for (consumer_key, module) in facts.iter() {
            for edge in &module.import_call_edges {
                let Some(provider) =
                    resolve_imported_module(&modules, consumer_key, edge.source.as_ref())
                else {
                    continue;
                };
                let Some((def_module, def_export)) = resolve_definition(
                    &modules,
                    &provider,
                    edge.imported.clone(),
                    &mut HashSet::default(),
                ) else {
                    continue;
                };
                match &edge.consumed_by_exports {
                    None => {
                        hard.insert((def_module, def_export));
                    }
                    Some(exports) if exports.is_empty() => {}
                    Some(exports) => {
                        // Resolve subclass exports the same way as callees, so a
                        // re-export is compared against the definition that gets pinned.
                        let consumed_by_defs = exports
                            .iter()
                            .filter_map(|name| {
                                resolve_definition(
                                    &modules,
                                    consumer_key,
                                    name.clone(),
                                    &mut HashSet::default(),
                                )
                            })
                            .collect::<Vec<_>>();
                        if consumed_by_defs.is_empty() {
                            continue;
                        }
                        soft.push(SoftEdge {
                            consumed_by_defs,
                            provider: (def_module, def_export),
                        });
                    }
                }
            }
        }

        // A soft edge becomes a pin once its subclass export is pinned, which
        // can pin the next superclass. Bound by the number of edges.
        let mut pinned = hard;
        loop {
            let mut changed = false;
            for edge in &soft {
                let subclass_pinned = edge
                    .consumed_by_defs
                    .iter()
                    .any(|definition| pinned.contains(definition));
                if subclass_pinned && pinned.insert(edge.provider.clone()) {
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let predicted_consumed = soft
            .iter()
            .map(|edge| edge.provider.clone())
            .filter(|provider| !pinned.contains(provider))
            .collect();
        let mut by_module: HashMap<String, HashSet<Atom>> = HashMap::default();
        for (module, exported) in pinned {
            by_module.entry(module).or_default().insert(exported);
        }
        Self {
            by_module,
            predicted_consumed,
        }
    }

    /// `(source, imported)` of calls in the finished Phase 2 `module` that
    /// resolve to a definition this plan left unpinned on a `super()`
    /// prediction. Each one may be a class invoked without `new`.
    pub(crate) fn mispredicted_calls(
        &self,
        facts: &ModuleFactsMap,
        filename: &str,
        module: &Module,
    ) -> Vec<(Atom, Atom)> {
        if self.predicted_consumed.is_empty() {
            return Vec::new();
        }
        let imports = import_index(module);
        if imports.is_empty() {
            return Vec::new();
        }
        let remaining = call_edges(module, &imports, &HashMap::default());
        if remaining.is_empty() {
            return Vec::new();
        }
        let modules = ModuleIndex::new(facts);
        let consumer = filename.strip_prefix("./").unwrap_or(filename);
        let mut found = remaining
            .into_iter()
            .filter(|edge| {
                resolve_imported_module(&modules, consumer, edge.source.as_ref())
                    .and_then(|provider| {
                        resolve_definition(
                            &modules,
                            &provider,
                            edge.imported.clone(),
                            &mut HashSet::default(),
                        )
                    })
                    .is_some_and(|definition| self.predicted_consumed.contains(&definition))
            })
            .map(|edge| (edge.source, edge.imported))
            .collect::<Vec<_>>();
        found.sort();
        found
    }

    pub(crate) fn pinned_exports(&self, filename: &str) -> Option<&HashSet<Atom>> {
        let canonical = filename.strip_prefix("./").unwrap_or(filename);
        self.by_module.get(canonical)
    }
}

struct SoftEdge {
    consumed_by_defs: Vec<(String, Atom)>,
    provider: (String, Atom),
}

pub(crate) fn pinned_export_names(
    plan: Option<&CallRequiredPlan>,
    filename: Option<&str>,
) -> HashSet<Atom> {
    let Some(plan) = plan else {
        return HashSet::default();
    };
    let Some(filename) = filename else {
        return HashSet::default();
    };
    plan.pinned_exports(filename).cloned().unwrap_or_default()
}

/// Import `[[Call]]` edges of one module, read from the facts AST.
///
/// The resolved pin set is not written back onto [`ModuleFactsMap`]; callers
/// build a [`CallRequiredPlan`] once the map is complete.
pub(crate) fn collect_import_call_edges(
    module: &Module,
    unresolved_mark: Mark,
    level: super::RewriteLevel,
) -> Vec<ImportCallEdge> {
    let imports = import_index(module);
    if imports.is_empty() {
        return Vec::new();
    }
    // Without the class-recovery probe every call counts as remaining, so this
    // walk finds a superset of the imports the real walk can report. The probe
    // clones the module and reruns class matching; skip it when no import is a
    // callee at all, which is most modules.
    let no_consumed = HashMap::default();
    let every_call_remains = call_edges(module, &imports, &no_consumed);
    // Minimal does not predict class recovery: every cross-file call pins.
    if every_call_remains.is_empty() || level < super::RewriteLevel::Standard {
        return every_call_remains;
    }
    let mut consumed_by_param: HashMap<BindingKey, Vec<Atom>> = HashMap::default();
    for consumed in super_params_consumed_by_class_recovery(&module.body, unresolved_mark, level) {
        consumed_by_param.insert(consumed.param, consumed.blocked_by);
    }
    call_edges(module, &imports, &consumed_by_param)
}

fn call_edges(
    module: &Module,
    imports: &HashMap<BindingKey, ImportBinding>,
    consumed_by_param: &HashMap<BindingKey, Vec<Atom>>,
) -> Vec<ImportCallEdge> {
    let mut collector = CallEdgeCollector {
        imports,
        consumed_by_param,
        binding_demand: HashMap::default(),
        namespace_demand: HashMap::default(),
        aliases: Vec::new(),
        namespace_aliases: Vec::new(),
    };
    module.visit_with(&mut collector);
    collector.propagate();
    collector.edges()
}

struct ImportBinding {
    source: Atom,
    imported: Atom,
    namespace: bool,
}

fn import_index(module: &Module) -> HashMap<BindingKey, ImportBinding> {
    let mut index = HashMap::default();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        if import.type_only {
            continue;
        }
        let source = Atom::from(import.src.value.as_str().unwrap_or(""));
        for specifier in &import.specifiers {
            let (local, imported, namespace) = match specifier {
                ImportSpecifier::Default(spec) => {
                    (binding_key(&spec.local), Atom::from("default"), false)
                }
                ImportSpecifier::Namespace(spec) => {
                    (binding_key(&spec.local), Atom::from("*"), true)
                }
                ImportSpecifier::Named(spec) if spec.is_type_only => continue,
                ImportSpecifier::Named(spec) => {
                    let imported = spec
                        .imported
                        .as_ref()
                        .map(export_atom)
                        .unwrap_or_else(|| spec.local.sym.clone());
                    (binding_key(&spec.local), imported, false)
                }
            };
            index.insert(
                local,
                ImportBinding {
                    source: source.clone(),
                    imported,
                    namespace,
                },
            );
        }
    }
    index
}

fn export_atom(name: &ModuleExportName) -> Atom {
    match name {
        ModuleExportName::Ident(ident) => ident.sym.clone(),
        ModuleExportName::Str(value) => Atom::from(value.value.as_str().unwrap_or("")),
    }
}

#[derive(Clone)]
struct Demand {
    /// The call is still present after class recovery.
    remains: bool,
    /// Subclass exports whose recovery would rewrite this call to `super`.
    consumed_by: HashSet<Atom>,
}

struct CallEdgeCollector<'a> {
    imports: &'a HashMap<BindingKey, ImportBinding>,
    /// Empty vec: a local IIFE consumes the call, so it never pins.
    consumed_by_param: &'a HashMap<BindingKey, Vec<Atom>>,
    binding_demand: HashMap<BindingKey, Demand>,
    namespace_demand: HashMap<(BindingKey, Atom), Demand>,
    aliases: Vec<(BindingKey, BindingKey)>,
    namespace_aliases: Vec<(BindingKey, BindingKey, Atom)>,
}

impl CallEdgeCollector<'_> {
    fn record_demand(&mut self, binding: BindingKey, demand: Demand) {
        merge_demand(
            self.binding_demand.entry(binding).or_insert(Demand {
                remains: false,
                consumed_by: HashSet::default(),
            }),
            &demand,
        );
    }

    fn demand_for_binding(&self, binding: &BindingKey) -> Option<Demand> {
        if let Some(exports) = self.consumed_by_param.get(binding) {
            if exports.is_empty() {
                // Class recovery consumes the call and the owner is not an
                // export, so nothing can cascade a pin onto the superclass.
                return None;
            }
            return Some(Demand {
                remains: false,
                consumed_by: exports.iter().cloned().collect(),
            });
        }
        Some(Demand {
            remains: true,
            consumed_by: HashSet::default(),
        })
    }

    fn note_expr(&mut self, expr: &Expr) {
        if let Some(binding) = expr_binding_key(expr) {
            if let Some(demand) = self.demand_for_binding(&binding) {
                self.record_demand(binding, demand);
            }
            return;
        }
        if let Some((namespace, member)) = namespace_member(expr, self.imports) {
            self.namespace_demand
                .entry((namespace, member))
                .or_insert(Demand {
                    remains: false,
                    consumed_by: HashSet::default(),
                })
                .remains = true;
        }
    }

    fn note_call(&mut self, call: &CallExpr) {
        // `F.call.apply(G, …)` still has this shape on the facts AST.
        // `[[Call]]` belongs to the first argument, which Phase 2's spread
        // pass has not rewritten yet.
        if let Some(target) = call_apply_target(call) {
            self.note_expr(target);
            return;
        }
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Member(member) = strip_parens(callee) else {
            return;
        };
        if !member_prop_name(&member.prop, "call") && !member_prop_name(&member.prop, "apply") {
            return;
        }
        self.note_expr(strip_parens(&member.obj));
    }

    fn note_iife_aliases(&mut self, call: &CallExpr) {
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let params: Vec<&Pat> = match strip_parens(callee) {
            Expr::Fn(function) => function
                .function
                .params
                .iter()
                .map(|param| &param.pat)
                .collect(),
            Expr::Arrow(arrow) => arrow.params.iter().collect(),
            _ => return,
        };
        let mut spreads_seen = 0usize;
        for (argument_index, argument) in call.args.iter().enumerate() {
            if argument.spread.is_some() {
                spreads_seen += 1;
                continue;
            }
            let argument_expr = strip_parens(&argument.expr);
            if spreads_seen == 0 {
                if let Some(parameter) = params.get(argument_index).and_then(|pat| pat_key(pat)) {
                    self.alias_argument(parameter, argument_expr);
                }
                continue;
            }
            // A spread contributes an unknown number of arguments. Later fixed
            // arguments can land on any parameter from this index onward.
            let minimum = argument_index - spreads_seen;
            for parameter in params.iter().skip(minimum) {
                if let Some(parameter) = pat_key(parameter) {
                    self.alias_argument(parameter, argument_expr);
                }
            }
        }
    }

    fn alias_argument(&mut self, parameter: BindingKey, argument: &Expr) {
        if let Some(argument_key) = expr_binding_key(argument) {
            self.aliases.push((parameter, argument_key));
            return;
        }
        if let Some((namespace, member)) = namespace_member(argument, self.imports) {
            self.namespace_aliases.push((parameter, namespace, member));
        }
    }

    fn propagate(&mut self) {
        loop {
            let mut changed = false;
            let aliases = self.aliases.clone();
            for (parameter, argument) in aliases {
                let Some(demand) = self.binding_demand.get(&parameter).cloned() else {
                    continue;
                };
                let entry = self.binding_demand.entry(argument).or_insert(Demand {
                    remains: false,
                    consumed_by: HashSet::default(),
                });
                changed |= merge_demand(entry, &demand);
            }
            let namespace_aliases = self.namespace_aliases.clone();
            for (parameter, namespace, member) in namespace_aliases {
                let Some(demand) = self.binding_demand.get(&parameter).cloned() else {
                    continue;
                };
                let entry = self
                    .namespace_demand
                    .entry((namespace, member))
                    .or_insert(Demand {
                        remains: false,
                        consumed_by: HashSet::default(),
                    });
                changed |= merge_demand(entry, &demand);
            }
            if !changed {
                break;
            }
        }
    }

    fn edges(&self) -> Vec<ImportCallEdge> {
        let mut merged: HashMap<(Atom, Atom), Option<Vec<Atom>>> = HashMap::default();
        for (binding, demand) in &self.binding_demand {
            let Some(import) = self.imports.get(binding) else {
                continue;
            };
            if import.namespace {
                continue;
            }
            merge_edge(
                &mut merged,
                import.source.clone(),
                import.imported.clone(),
                demand,
            );
        }
        for ((namespace, member), demand) in &self.namespace_demand {
            let Some(import) = self.imports.get(namespace) else {
                continue;
            };
            if !(import.namespace || import.imported.as_ref() == "default") {
                continue;
            }
            merge_edge(&mut merged, import.source.clone(), member.clone(), demand);
        }
        merged
            .into_iter()
            .map(|((source, imported), consumed_by_exports)| ImportCallEdge {
                source,
                imported,
                consumed_by_exports,
            })
            .collect()
    }
}

fn merge_edge(
    merged: &mut HashMap<(Atom, Atom), Option<Vec<Atom>>>,
    source: Atom,
    imported: Atom,
    demand: &Demand,
) {
    let entry = merged
        .entry((source, imported))
        .or_insert_with(|| Some(Vec::new()));
    if demand.remains {
        *entry = None;
        return;
    }
    if let Some(names) = entry {
        for name in &demand.consumed_by {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
    }
}

fn merge_demand(into: &mut Demand, from: &Demand) -> bool {
    let mut changed = false;
    if from.remains && !into.remains {
        into.remains = true;
        changed = true;
    }
    for name in &from.consumed_by {
        if into.consumed_by.insert(name.clone()) {
            changed = true;
        }
    }
    changed
}

/// `F.call.apply(G, …)` — `G` is what still needs `[[Call]]`.
fn call_apply_target(call: &CallExpr) -> Option<&Expr> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(outer) = strip_parens(callee) else {
        return None;
    };
    if !member_prop_name(&outer.prop, "apply") {
        return None;
    }
    let Expr::Member(inner) = strip_parens(&outer.obj) else {
        return None;
    };
    if !member_prop_name(&inner.prop, "call") {
        return None;
    }
    let ExprOrSpread { spread: None, expr } = call.args.first()? else {
        return None;
    };
    Some(strip_parens(expr))
}

/// `ns.Foo` or `mod.Foo` where `ns` is a namespace import and `mod` is a
/// default import (webpack harmony). Computed `ns["Foo"]` is ignored.
fn namespace_member(
    expr: &Expr,
    imports: &HashMap<BindingKey, ImportBinding>,
) -> Option<(BindingKey, Atom)> {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return None;
    };
    let Expr::Ident(ident) = strip_parens(obj) else {
        return None;
    };
    let namespace = binding_key(ident);
    if !imports
        .get(&namespace)
        .is_some_and(|import| import.namespace || import.imported.as_ref() == "default")
    {
        return None;
    }
    let MemberProp::Ident(name) = prop else {
        return None;
    };
    Some((namespace, name.sym.clone()))
}

fn pat_key(pat: &Pat) -> Option<BindingKey> {
    let Pat::Ident(BindingIdent { id, .. }) = pat else {
        return None;
    };
    Some(binding_key(id))
}

impl Visit for CallEdgeCollector<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        self.note_call(call);
        self.note_iife_aliases(call);
        call.visit_children_with(self);
    }
}

/// Exact and extension-stem lookups over the canonical module keys, built once
/// per plan so each edge hop is a hash lookup.
struct ModuleIndex<'a> {
    by_key: HashMap<&'a str, &'a ModuleFacts>,
    by_stem: HashMap<String, Vec<&'a str>>,
}

impl<'a> ModuleIndex<'a> {
    fn new(facts: &'a ModuleFactsMap) -> Self {
        let mut by_key = HashMap::default();
        let mut by_stem: HashMap<String, Vec<&'a str>> = HashMap::default();
        for (key, module) in facts.iter() {
            by_key.insert(key, module);
            by_stem.entry(script_stem(key)).or_default().push(key);
        }
        Self { by_key, by_stem }
    }

    /// Lookup that does not try specifier variants or a root-level fallback.
    fn get_exact(&self, key: &str) -> Option<&'a ModuleFacts> {
        self.by_key.get(key).copied()
    }
}

fn resolve_imported_module(
    modules: &ModuleIndex<'_>,
    from_key: &str,
    specifier: &str,
) -> Option<String> {
    // Relative specifiers resolve only from the importing file. A failed lookup
    // must not fall back to a root-level module with the same basename.
    let resolved = if specifier.starts_with("./") || specifier.starts_with("../") {
        resolve_relative_specifier(from_key, specifier)?
    } else if specifier.starts_with('.') {
        return None;
    } else {
        specifier.to_string()
    };
    unique_module_key(modules, &resolved)
}

fn unique_module_key(modules: &ModuleIndex<'_>, specifier: &str) -> Option<String> {
    let canonical = specifier.strip_prefix("./").unwrap_or(specifier);
    if modules.by_key.contains_key(canonical) {
        return Some(canonical.to_string());
    }
    // Keys are unique map entries, so one stem entry is one module.
    match modules.by_stem.get(&script_stem(canonical))?.as_slice() {
        [key] => Some((*key).to_string()),
        _ => None,
    }
}

fn script_stem(path: &str) -> String {
    for extension in [".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx"] {
        if let Some(stripped) = path.strip_suffix(extension) {
            return format!("{stripped}.*");
        }
    }
    // A bare specifier such as `dep` still names `dep.js` / `dep.ts` when that
    // file is the only match. This is not a root-basename fallback: the stem
    // keeps the directory.
    format!("{path}.*")
}

fn resolve_definition(
    modules: &ModuleIndex<'_>,
    module_key: &str,
    exported: Atom,
    visited: &mut HashSet<(String, Atom)>,
) -> Option<(String, Atom)> {
    if !visited.insert((module_key.to_string(), exported.clone())) {
        return None;
    }
    let module = modules.get_exact(module_key)?;
    if let Some(reexport) = module
        .reexports
        .iter()
        .find(|reexport| reexport.exported == exported)
    {
        let next = resolve_imported_module(modules, module_key, reexport.source.as_ref())?;
        return resolve_definition(modules, &next, reexport.imported.clone(), visited);
    }
    if let Some(export) = module
        .exports
        .iter()
        .find(|export| export.exported == exported)
    {
        if let Some(local) = &export.local {
            if let Some(import) = module.imports.iter().find(|import| &import.local == local) {
                let imported = match &import.kind {
                    ImportKind::Named(name) => name.clone(),
                    ImportKind::Default => Atom::from("default"),
                    ImportKind::Namespace => return None,
                };
                let next = resolve_imported_module(modules, module_key, import.source.as_ref())?;
                return resolve_definition(modules, &next, imported, visited);
            }
            return Some((module_key.to_string(), exported));
        }
        // `export default (function () { return t })()` has no local. The
        // value is still defined in this module. Re-exports were handled above.
        return Some((module_key.to_string(), exported));
    }
    // `export default { Foo: local }` and `module.exports = { Foo: local }`.
    // The member name is not a named export, but `mod.Foo.call` still needs it.
    if module
        .default_object_ident_properties
        .iter()
        .any(|name| name == &exported)
        || module
            .commonjs_default_object
            .as_ref()
            .is_some_and(|object| {
                object
                    .declared_properties
                    .iter()
                    .any(|name| name == &exported)
            })
    {
        return Some((module_key.to_string(), exported));
    }
    // One `export *` can be followed. Several conflict, so the name is not pinned.
    match module.export_star_sources.as_slice() {
        [source] => {
            let next = resolve_imported_module(modules, module_key, source.as_ref())?;
            resolve_definition(modules, &next, exported, visited)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{ExportFact, ExportKind};
    use swc_core::common::{sync::Lrc, FileName, SourceMap, GLOBALS};
    use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::VisitMutWith;

    fn resolved(source: &str) -> Module {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            FileName::Custom("child.js".into()).into(),
            source.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax::default()),
            Default::default(),
            StringInput::from(&*fm),
            None,
        );
        let mut module = Parser::new_from(lexer)
            .parse_module()
            .expect("source should parse");
        module.visit_mut_with(&mut resolver(Mark::new(), Mark::new(), false));
        module
    }

    fn named_export(name: &str) -> ExportFact {
        ExportFact {
            exported: Atom::from(name),
            local: Some(Atom::from(name)),
            kind: ExportKind::Named,
        }
    }

    /// `base.js` exports `Foo`; `child.js` exports `Child` and calls `Foo`
    /// from an `extends` IIFE that Phase 1 predicted becomes `super()`.
    fn predicted_facts(extra_child_edge: Option<ImportCallEdge>) -> ModuleFactsMap {
        let mut facts = ModuleFactsMap::new();
        facts.insert(
            "base.js",
            ModuleFacts {
                exports: vec![named_export("Foo")],
                ..Default::default()
            },
        );
        let mut import_call_edges = vec![ImportCallEdge {
            source: Atom::from("./base.js"),
            imported: Atom::from("Foo"),
            consumed_by_exports: Some(vec![Atom::from("Child")]),
        }];
        import_call_edges.extend(extra_child_edge);
        facts.insert(
            "child.js",
            ModuleFacts {
                exports: vec![named_export("Child")],
                import_call_edges,
                ..Default::default()
            },
        );
        facts
    }

    const SURVIVING_CALL: &str = r#"
import { Foo } from "./base.js";
export function Child() { Foo.call(this); }
"#;

    #[test]
    fn surviving_call_to_a_predicted_super_is_reported() {
        GLOBALS.set(&Default::default(), || {
            let facts = predicted_facts(None);
            let plan = CallRequiredPlan::build(&facts);
            assert!(plan.pinned_exports("base.js").is_none());
            let module = resolved(SURVIVING_CALL);
            assert_eq!(
                plan.mispredicted_calls(&facts, "child.js", &module),
                vec![(Atom::from("./base.js"), Atom::from("Foo"))]
            );
        });
    }

    #[test]
    fn consumed_call_is_not_reported() {
        GLOBALS.set(&Default::default(), || {
            let facts = predicted_facts(None);
            let plan = CallRequiredPlan::build(&facts);
            let module = resolved(
                r#"
import { Foo } from "./base.js";
export class Child extends Foo { constructor() { super(); } }
"#,
            );
            assert!(plan
                .mispredicted_calls(&facts, "child.js", &module)
                .is_empty());
        });
    }

    #[test]
    fn surviving_call_to_a_pinned_provider_is_not_reported() {
        GLOBALS.set(&Default::default(), || {
            // A second, remaining call pins `Foo`, so the call is safe.
            let facts = predicted_facts(Some(ImportCallEdge {
                source: Atom::from("./base.js"),
                imported: Atom::from("Foo"),
                consumed_by_exports: None,
            }));
            let plan = CallRequiredPlan::build(&facts);
            assert!(plan
                .pinned_exports("base.js")
                .is_some_and(|names| names.contains(&Atom::from("Foo"))));
            let module = resolved(SURVIVING_CALL);
            assert!(plan
                .mispredicted_calls(&facts, "child.js", &module)
                .is_empty());
        });
    }
}
