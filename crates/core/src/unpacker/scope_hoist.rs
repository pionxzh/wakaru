#[cfg(test)]
use std::cell::Cell;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap, Span, Spanned, GLOBALS};
use swc_core::ecma::ast::*;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::emit_esm::{
    dedup_filename, emit_items, make_named_export_stmt, make_named_import_stmt,
    try_promote_fn_class_export, FilenameDedupStyle,
};
use super::{
    module_item_declared_names, spans_byte_ranges, BundleFormat, UnpackResult, UnpackedModule,
};

const MIN_DECLARATIONS: usize = 10;
const PATHOLOGICAL_ENTRY_SCC_MIN_CLUSTERS: usize = 64;
const PATHOLOGICAL_ENTRY_SCC_MIN_FRACTION_DENOMINATOR: usize = 4;
const INSPECT_MAX_CROSS_WRITE_COMPONENT_CLUSTERS: usize = 8;

#[cfg(test)]
thread_local! {
    static EMIT_RELATION_SYMBOL_PROBES: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
fn reset_emit_relation_symbol_probe_count() {
    EMIT_RELATION_SYMBOL_PROBES.with(|probes| probes.set(0));
}

#[cfg(test)]
fn emit_relation_symbol_probe_count() -> usize {
    EMIT_RELATION_SYMBOL_PROBES.with(Cell::get)
}

#[cfg(test)]
fn record_emit_relation_symbol_probe() {
    EMIT_RELATION_SYMBOL_PROBES.with(|probes| probes.set(probes.get() + 1));
}

/// Selects how a completed scope-hoist plan is rendered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ScopeHoistRenderMode {
    /// Merge cyclic components before emitting the recovered ESM graph.
    #[default]
    Executable,
    /// Retain the finer planned clusters for static inspection.
    Inspect,
}

/// Where the scope-hoisted source being split came from. Direct assets are
/// whole Rollup/Vite-style chunks, where true modules are almost always one
/// contiguous run of top-level items (measured 99.88% over source-map-
/// verified corpora), so Inspect can rely on the adjacency invariant.
/// Nested modules are bodies extracted from a structural bundle (webpack
/// module concatenation, esbuild closures), which measured only ~91%
/// contiguous — there Inspect keeps the component-cap policy instead.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeHoistSource {
    DirectAsset,
    NestedModule,
}

/// Stable, source-oriented description of one top-level item used by the
/// scope-hoist research trace. This is an internal-core debugging surface, not
/// part of the supported `wakaru` facade API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeHoistTraceItem {
    pub index: usize,
    pub source_range: Option<(u32, u32)>,
    pub declared_names: Vec<String>,
    pub referenced_items: Vec<usize>,
    pub written_items: Vec<usize>,
    /// Canonical cluster id after Signals 1–5 (the lowest member index).
    pub signal_cluster: usize,
    /// Canonical cluster id after the current Inspect cross-write policy.
    pub post_write_cluster: usize,
}

/// One cross-cluster writer/owner edge after Signals 1–5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeHoistCrossWriteTraceEdge {
    pub writer_item: usize,
    pub owner_item: usize,
    pub writer_cluster: usize,
    pub owner_cluster: usize,
    /// Distinct owner clusters written by the writer's Signal 1–5 cluster.
    pub writer_target_cluster_degree: usize,
    /// Signal 1–5 clusters in the undirected write-connected component.
    pub component_cluster_count: usize,
    /// Clusters in the residual component formed only by degree-one writers.
    pub leaf_component_cluster_count: usize,
    pub kept_by_inspect_policy: bool,
}

/// Opt-in analysis data for corpus research on heuristic scope splitting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeHoistTrace {
    pub source_bytes: usize,
    pub minimum_declarations: usize,
    pub declaration_count: usize,
    pub eligible: bool,
    pub would_split: bool,
    pub signal_cluster_count: usize,
    pub post_write_cluster_count: usize,
    /// Modules the component-cap policy would expose after singleton folding.
    pub component_cap_output_cluster_count: usize,
    /// Modules bounded leaf restoration would expose before its backoff.
    pub leaf_candidate_output_cluster_count: usize,
    /// Whether bounded leaf restoration survived the output-count backoff.
    pub bounded_leaf_restoration_accepted: bool,
    pub items: Vec<ScopeHoistTraceItem>,
    pub cross_write_edges: Vec<ScopeHoistCrossWriteTraceEdge>,
}

pub fn split_scope_hoisted(source: &str) -> Option<UnpackResult> {
    split_scope_hoisted_with_mode(
        source,
        ScopeHoistRenderMode::Executable,
        ScopeHoistSource::DirectAsset,
    )
}

pub(crate) fn split_scope_hoisted_with_mode(
    source: &str,
    render_mode: ScopeHoistRenderMode,
    origin: ScopeHoistSource,
) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::parse_es_module(source, "bundle.js", cm.clone()).ok()?;
        split_from_module(&module, cm, render_mode, origin)
    })
}

pub(crate) fn split_scope_hoisted_module_with_mode(
    module: &Module,
    cm: Lrc<SourceMap>,
    render_mode: ScopeHoistRenderMode,
    origin: ScopeHoistSource,
) -> Option<UnpackResult> {
    split_from_module(module, cm, render_mode, origin)
}

/// Analyze the same direct top-level source consumed by the heuristic
/// splitter, retaining byte ranges and cross-write topology for an external
/// source-map oracle. Parse failures are reported as `None`.
pub fn trace_scope_hoisted(source: &str) -> Option<ScopeHoistTrace> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::parse_es_module(source, "bundle.js", cm.clone()).ok()?;
        let iife_body = unwrap_iife(&module);
        let body = iife_body.as_deref().unwrap_or(&module.body);
        Some(trace_scope_hoist_body(source.len(), body, &cm))
    })
}

fn trace_scope_hoist_body(
    source_bytes: usize,
    body: &[ModuleItem],
    cm: &SourceMap,
) -> ScopeHoistTrace {
    let items = collect_top_level_items(body);
    let declaration_count = items
        .iter()
        .filter(|item| !item.declared_names.is_empty())
        .count();
    let eligible = declaration_count >= MIN_DECLARATIONS;
    let graph = build_reference_graph(&items);
    let mut signal_uf = UnionFind::new(items.len());
    if eligible {
        apply_merge_signals(&items, &graph, &mut signal_uf);
    }
    let topology = analyze_cross_item_writes(&graph, &signal_uf);
    let signal_clusters = canonical_cluster_ids(&signal_uf, items.len());

    let mut post_write_uf = signal_uf.clone();
    let inspect_decision = eligible.then(|| {
        merge_bounded_cross_item_writes(
            &items,
            &graph,
            &mut post_write_uf,
            INSPECT_MAX_CROSS_WRITE_COMPONENT_CLUSTERS,
        )
    });
    let post_write_clusters = canonical_cluster_ids(&post_write_uf, items.len());
    let would_split = if eligible {
        let mut final_uf = post_write_uf.clone();
        let roots = extract_root_clusters(&items, &mut final_uf);
        extract_inspection_clusters(&items, &roots).len() >= 2
    } else {
        false
    };

    let mut cross_write_edges = Vec::new();
    if eligible {
        for (writer_item, targets) in graph.writes.iter().enumerate() {
            for &owner_item in targets {
                let writer_root = topology.signal_root_by_item[writer_item];
                let owner_root = topology.signal_root_by_item[owner_item];
                if writer_root == owner_root {
                    continue;
                }
                let component_cluster_count = topology.component_cluster_count_by_root[writer_root];
                let leaf_component_cluster_count =
                    topology.leaf_component_cluster_count_by_root[writer_root];
                cross_write_edges.push(ScopeHoistCrossWriteTraceEdge {
                    writer_item,
                    owner_item,
                    writer_cluster: signal_clusters[writer_item],
                    owner_cluster: signal_clusters[owner_item],
                    writer_target_cluster_degree: topology.writer_target_degrees[writer_root],
                    component_cluster_count,
                    leaf_component_cluster_count,
                    kept_by_inspect_policy: retain_inspect_cross_write_edge(
                        &topology,
                        writer_root,
                        INSPECT_MAX_CROSS_WRITE_COMPONENT_CLUSTERS,
                        inspect_decision.as_ref().is_some_and(|decision| {
                            decision
                                .restored_components
                                .contains(&topology.write_component_by_root[writer_root])
                        }),
                    ),
                });
            }
        }
        cross_write_edges.sort_by_key(|edge| (edge.writer_item, edge.owner_item));
    }

    let traced_items = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let mut declared_names = item
                .declared_names
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            declared_names.sort();
            let mut referenced_items = graph.references[index].iter().copied().collect::<Vec<_>>();
            referenced_items.sort_unstable();
            let mut written_items = graph.writes[index].iter().copied().collect::<Vec<_>>();
            written_items.sort_unstable();
            ScopeHoistTraceItem {
                index,
                source_range: super::span_byte_range(cm, body[index].span()),
                declared_names,
                referenced_items,
                written_items,
                signal_cluster: signal_clusters[index],
                post_write_cluster: post_write_clusters[index],
            }
        })
        .collect();

    ScopeHoistTrace {
        source_bytes,
        minimum_declarations: MIN_DECLARATIONS,
        declaration_count,
        eligible,
        would_split,
        signal_cluster_count: signal_clusters
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .len(),
        post_write_cluster_count: post_write_clusters
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .len(),
        component_cap_output_cluster_count: inspect_decision
            .as_ref()
            .map_or(0, |decision| decision.component_cap_output_clusters),
        leaf_candidate_output_cluster_count: inspect_decision
            .as_ref()
            .map_or(0, |decision| decision.leaf_candidate_output_clusters),
        bounded_leaf_restoration_accepted: inspect_decision
            .is_some_and(|decision| decision.bounded_leaf_restoration_accepted),
        items: traced_items,
        cross_write_edges,
    }
}

fn split_from_module(
    module: &Module,
    cm: Lrc<SourceMap>,
    render_mode: ScopeHoistRenderMode,
    origin: ScopeHoistSource,
) -> Option<UnpackResult> {
    // Unwrap IIFE wrapper if present: `(()=>{ ... })()` or `(function(){ ... })()`
    let iife_body = unwrap_iife(module);
    let body = iife_body.as_deref().unwrap_or(&module.body);

    let plan = analyze_scope_hoist(body, render_mode, origin)?;
    render_scope_hoist_plan(body, plan, cm, render_mode)
}

struct ScopeHoistPlan {
    items: Vec<TopLevelItem>,
    graph: ReferenceGraph,
    roots: Vec<Cluster>,
    clusters: Vec<Cluster>,
    inspection_context_by_item: Option<Vec<usize>>,
}

fn analyze_scope_hoist(
    body: &[ModuleItem],
    render_mode: ScopeHoistRenderMode,
    origin: ScopeHoistSource,
) -> Option<ScopeHoistPlan> {
    // Phase 1: collect top-level items with metadata.
    let items = collect_top_level_items(body);
    let decl_count = items
        .iter()
        .filter(|i| !i.declared_names.is_empty())
        .count();
    if decl_count < MIN_DECLARATIONS {
        return None;
    }

    // Phase 2: build reference graph.
    let graph = build_reference_graph(&items);

    // Phase 3: cluster via union-find.
    let mut uf = UnionFind::new(items.len());
    apply_merge_signals(&items, &graph, &mut uf);
    let inspection_context_by_item = match render_mode {
        ScopeHoistRenderMode::Executable => {
            merge_cross_item_writes(&graph, &mut uf);
            None
        }
        ScopeHoistRenderMode::Inspect => {
            let topology = analyze_cross_item_writes(&graph, &uf);
            let context_by_item = topology
                .signal_root_by_item
                .iter()
                .map(|&root| topology.write_component_by_root[root])
                .collect();
            match origin {
                ScopeHoistSource::DirectAsset => {
                    merge_adjacent_cross_item_writes(&graph, &mut uf);
                }
                ScopeHoistSource::NestedModule => {
                    merge_bounded_cross_item_writes(
                        &items,
                        &graph,
                        &mut uf,
                        INSPECT_MAX_CROSS_WRITE_COMPONENT_CLUSTERS,
                    );
                }
            }
            Some(context_by_item)
        }
    };

    // Phase 4: extract the finest useful clusters and identify the entry.
    let roots = extract_root_clusters(&items, &mut uf);
    let clusters = extract_inspection_clusters(&items, &roots);
    (clusters.len() >= 2).then_some(ScopeHoistPlan {
        items,
        graph,
        roots,
        clusters,
        inspection_context_by_item,
    })
}

fn render_scope_hoist_plan(
    body: &[ModuleItem],
    plan: ScopeHoistPlan,
    cm: Lrc<SourceMap>,
    render_mode: ScopeHoistRenderMode,
) -> Option<UnpackResult> {
    let ScopeHoistPlan {
        items,
        graph,
        roots,
        clusters,
        inspection_context_by_item,
    } = plan;
    let clusters = match render_mode {
        ScopeHoistRenderMode::Executable => {
            let repartitioned = has_pathological_entry_scc(&clusters, &graph);
            let base = if repartitioned {
                extract_executable_clusters(&items, &graph, roots.clone())
            } else {
                clusters
            };
            let folded = merge_cyclic_clusters(
                fold_startup_effects_into_entry(body, &items, &graph, base),
                &graph,
            );
            if folded.len() >= 2 || repartitioned {
                folded
            } else {
                // A singleton-contracted entry can form a false cycle with
                // every chunk once bare statements return to it (the chunks
                // reference the entry's singleton helpers, the statements
                // reference the chunks). Repartitioning assigns optional
                // singletons to nearby module clusters instead of the entry,
                // which breaks the cycle and preserves the split.
                merge_cyclic_clusters(
                    fold_startup_effects_into_entry(
                        body,
                        &items,
                        &graph,
                        extract_executable_clusters(&items, &graph, roots),
                    ),
                    &graph,
                )
            }
        }
        ScopeHoistRenderMode::Inspect => clusters,
    };
    if clusters.len() < 2 {
        return None;
    }

    // Phase 5: emit modules.
    let modules = emit_clusters(
        body,
        &items,
        clusters,
        inspection_context_by_item.as_deref(),
        cm,
    );
    Some(UnpackResult::without_cycle_warnings(
        modules,
        BundleFormat::ScopeHoisted,
    ))
}

/// Detect and unwrap an IIFE wrapper: `(()=>{ ... })()` or `(function(){ ... })()`
/// Returns the inner body statements (plus any trailing top-level items)
/// converted to ModuleItems. Only matches when the first item is an IIFE call.
fn unwrap_iife(module: &Module) -> Option<Vec<ModuleItem>> {
    let first = module.body.first()?;
    let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = first else {
        return None;
    };
    let Expr::Call(call) = &**expr else {
        return None;
    };
    if !call.args.is_empty() {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let inner = match &**callee {
        Expr::Paren(paren) => &*paren.expr,
        other => other,
    };
    let stmts = match inner {
        Expr::Arrow(arrow) if arrow.params.is_empty() => {
            if let BlockStmtOrExpr::BlockStmt(block) = &*arrow.body {
                Some(&block.stmts)
            } else {
                None
            }
        }
        Expr::Fn(fn_expr) => {
            if fn_expr.function.params.is_empty() {
                fn_expr.function.body.as_ref().map(|b| &b.stmts)
            } else {
                None
            }
        }
        _ => None,
    }?;
    let mut items: Vec<ModuleItem> = stmts.iter().cloned().map(ModuleItem::Stmt).collect();
    items.extend(module.body[1..].iter().cloned());
    Some(items)
}

#[cfg(test)]
fn debug_clusters(source: &str) -> Vec<(Vec<String>, bool)> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::parse_es_module(source, "bundle.js", cm)
            .ok()
            .unwrap();
        let items = collect_top_level_items(&module.body);
        let graph = build_reference_graph(&items);
        let mut uf = UnionFind::new(items.len());
        apply_merge_signals(&items, &graph, &mut uf);
        let roots = extract_root_clusters(&items, &mut uf);
        let inspection_clusters = extract_inspection_clusters(&items, &roots);
        let clusters = if has_pathological_entry_scc(&inspection_clusters, &graph) {
            extract_executable_clusters(&items, &graph, roots)
        } else {
            inspection_clusters
        };
        let clusters = merge_cyclic_clusters(clusters, &graph);
        clusters
            .iter()
            .map(|c| {
                let names: Vec<String> = c
                    .item_indices
                    .iter()
                    .flat_map(|&i| {
                        if items[i].declared_names.is_empty() {
                            vec!["<expr>".to_string()]
                        } else {
                            items[i]
                                .declared_names
                                .iter()
                                .map(|n| n.to_string())
                                .collect()
                        }
                    })
                    .collect();
                (names, c.is_entry)
            })
            .collect()
    })
}

// ---------------------------------------------------------------------------
// Phase 1: Collect top-level items
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct TopLevelItem {
    declared_names: Vec<Atom>,
    referenced_names: HashSet<Atom>,
    written_names: HashSet<Atom>,
    is_module_decl: bool,
}

fn collect_top_level_items(body: &[ModuleItem]) -> Vec<TopLevelItem> {
    body.iter()
        .map(|item| {
            let declared_names = module_item_declared_names(item);
            let (referenced_names, written_names) =
                item_referenced_and_written_names(item, &declared_names);
            let is_module_decl = matches!(item, ModuleItem::ModuleDecl(_));
            TopLevelItem {
                declared_names,
                referenced_names,
                written_names,
                is_module_decl,
            }
        })
        .collect()
}

fn item_referenced_and_written_names(
    item: &ModuleItem,
    own_names: &[Atom],
) -> (HashSet<Atom>, HashSet<Atom>) {
    let own: HashSet<&Atom> = own_names.iter().collect();
    let mut collector = RefCollector {
        refs: HashSet::new(),
        writes: HashSet::new(),
        own_names: &own,
        block_bindings: HashSet::new(),
        var_bindings: HashSet::new(),
    };
    item.visit_with(&mut collector);
    (collector.refs, collector.writes)
}

struct RefCollector<'a> {
    refs: HashSet<Atom>,
    writes: HashSet<Atom>,
    own_names: &'a HashSet<&'a Atom>,
    /// Block-scoped bindings (let/const, params, catch). Saved/restored on
    /// block and function boundaries.
    block_bindings: HashSet<Atom>,
    /// Function-scoped `var` bindings. Saved/restored only on function
    /// boundaries so they survive block-level restores.
    var_bindings: HashSet<Atom>,
}

impl RefCollector<'_> {
    fn is_local(&self, sym: &Atom) -> bool {
        self.block_bindings.contains(sym) || self.var_bindings.contains(sym)
    }

    fn record_write(&mut self, ident: &Ident) {
        if !self.own_names.contains(&ident.sym) && !self.is_local(&ident.sym) {
            self.refs.insert(ident.sym.clone());
            self.writes.insert(ident.sym.clone());
        }
    }

    fn visit_assignment_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Ident(ident) => self.record_write(ident),
            Expr::Paren(paren) => self.visit_assignment_expr(&paren.expr),
            Expr::TsAs(ts_as) => self.visit_assignment_expr(&ts_as.expr),
            Expr::TsSatisfies(ts_satisfies) => self.visit_assignment_expr(&ts_satisfies.expr),
            Expr::TsNonNull(ts_non_null) => self.visit_assignment_expr(&ts_non_null.expr),
            Expr::TsTypeAssertion(ts_assertion) => self.visit_assignment_expr(&ts_assertion.expr),
            Expr::TsInstantiation(ts_instantiation) => {
                self.visit_assignment_expr(&ts_instantiation.expr)
            }
            _ => expr.visit_with(self),
        }
    }

    fn visit_assignment_pat(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(binding) => self.record_write(&binding.id),
            Pat::Array(array) => {
                for element in array.elems.iter().flatten() {
                    self.visit_assignment_pat(element);
                }
            }
            Pat::Object(object) => {
                for prop in &object.props {
                    match prop {
                        ObjectPatProp::KeyValue(key_value) => {
                            if let PropName::Computed(computed) = &key_value.key {
                                computed.visit_with(self);
                            }
                            self.visit_assignment_pat(&key_value.value);
                        }
                        ObjectPatProp::Assign(assign) => {
                            self.record_write(&assign.key);
                            assign.value.visit_with(self);
                        }
                        ObjectPatProp::Rest(rest) => self.visit_assignment_pat(&rest.arg),
                    }
                }
            }
            Pat::Assign(assign) => {
                self.visit_assignment_pat(&assign.left);
                assign.right.visit_with(self);
            }
            Pat::Rest(rest) => self.visit_assignment_pat(&rest.arg),
            Pat::Expr(expr) => self.visit_assignment_expr(expr),
            Pat::Invalid(_) => {}
        }
    }

    fn visit_assignment_target(&mut self, target: &AssignTarget) {
        match target {
            AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
                self.record_write(&binding.id)
            }
            AssignTarget::Simple(SimpleAssignTarget::Paren(paren)) => {
                self.visit_assignment_expr(&paren.expr)
            }
            AssignTarget::Simple(simple) => simple.visit_children_with(self),
            AssignTarget::Pat(AssignTargetPat::Array(array)) => {
                self.visit_assignment_pat(&Pat::Array(array.clone()))
            }
            AssignTarget::Pat(AssignTargetPat::Object(object)) => {
                self.visit_assignment_pat(&Pat::Object(object.clone()))
            }
            AssignTarget::Pat(AssignTargetPat::Invalid(_)) => {}
        }
    }
}

impl Visit for RefCollector<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if !self.own_names.contains(&ident.sym) && !self.is_local(&ident.sym) {
            self.refs.insert(ident.sym.clone());
        }
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        self.visit_assignment_target(&assign.left);
        assign.right.visit_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        self.visit_assignment_expr(&update.arg);
    }

    fn visit_var_decl(&mut self, decl: &VarDecl) {
        let target = match decl.kind {
            VarDeclKind::Var => &mut self.var_bindings,
            VarDeclKind::Let | VarDeclKind::Const => &mut self.block_bindings,
        };
        for d in &decl.decls {
            collect_pat_bindings(&d.name, target);
        }
        for d in &decl.decls {
            d.name.visit_with(self);
            d.init.visit_with(self);
        }
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.block_bindings.insert(decl.ident.sym.clone());
        decl.function.visit_with(self);
    }

    fn visit_class_decl(&mut self, decl: &ClassDecl) {
        self.block_bindings.insert(decl.ident.sym.clone());
        decl.class.visit_with(self);
    }

    fn visit_fn_expr(&mut self, expr: &FnExpr) {
        let outer = self.block_bindings.clone();
        if let Some(ident) = &expr.ident {
            self.block_bindings.insert(ident.sym.clone());
        }
        expr.function.visit_with(self);
        self.block_bindings = outer;
    }

    fn visit_class_expr(&mut self, expr: &ClassExpr) {
        let outer = self.block_bindings.clone();
        if let Some(ident) = &expr.ident {
            self.block_bindings.insert(ident.sym.clone());
        }
        expr.class.visit_with(self);
        self.block_bindings = outer;
    }

    fn visit_function(&mut self, f: &Function) {
        let outer_block = self.block_bindings.clone();
        let outer_var = self.var_bindings.clone();
        self.var_bindings.clear();
        for param in &f.params {
            collect_pat_bindings(&param.pat, &mut self.block_bindings);
        }
        for param in &f.params {
            param.visit_with(self);
        }
        f.body.visit_with(self);
        self.block_bindings = outer_block;
        self.var_bindings = outer_var;
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        let outer_block = self.block_bindings.clone();
        let outer_var = self.var_bindings.clone();
        self.var_bindings.clear();
        for param in &arrow.params {
            collect_pat_bindings(param, &mut self.block_bindings);
        }
        for param in &arrow.params {
            param.visit_with(self);
        }
        arrow.body.visit_with(self);
        self.block_bindings = outer_block;
        self.var_bindings = outer_var;
    }

    fn visit_catch_clause(&mut self, clause: &CatchClause) {
        let outer = self.block_bindings.clone();
        if let Some(param) = &clause.param {
            collect_pat_bindings(param, &mut self.block_bindings);
        }
        clause.body.visit_with(self);
        self.block_bindings = outer;
    }

    fn visit_block_stmt(&mut self, block: &BlockStmt) {
        let outer = self.block_bindings.clone();
        for stmt in &block.stmts {
            stmt.visit_with(self);
        }
        self.block_bindings = outer;
    }

    fn visit_for_stmt(&mut self, stmt: &ForStmt) {
        let outer = self.block_bindings.clone();
        stmt.init.visit_with(self);
        stmt.test.visit_with(self);
        stmt.update.visit_with(self);
        stmt.body.visit_with(self);
        self.block_bindings = outer;
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        let outer = self.block_bindings.clone();
        match &stmt.left {
            ForHead::Pat(pat) => self.visit_assignment_pat(pat),
            _ => stmt.left.visit_with(self),
        }
        stmt.right.visit_with(self);
        stmt.body.visit_with(self);
        self.block_bindings = outer;
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        let outer = self.block_bindings.clone();
        match &stmt.left {
            ForHead::Pat(pat) => self.visit_assignment_pat(pat),
            _ => stmt.left.visit_with(self),
        }
        stmt.right.visit_with(self);
        stmt.body.visit_with(self);
        self.block_bindings = outer;
    }

    fn visit_member_prop(&mut self, _prop: &MemberProp) {}

    fn visit_member_expr(&mut self, expr: &MemberExpr) {
        expr.obj.visit_with(self);
        if let MemberProp::Computed(c) = &expr.prop {
            c.visit_with(self);
        }
    }

    fn visit_super_prop(&mut self, prop: &SuperProp) {
        if let SuperProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }

    fn visit_jsx_member_expr(&mut self, expr: &JSXMemberExpr) {
        expr.obj.visit_with(self);
    }

    fn visit_prop(&mut self, prop: &Prop) {
        match prop {
            Prop::Shorthand(ident) => {
                if !self.own_names.contains(&ident.sym) && !self.is_local(&ident.sym) {
                    self.refs.insert(ident.sym.clone());
                }
            }
            Prop::KeyValue(kv) => {
                if let PropName::Computed(c) = &kv.key {
                    c.visit_with(self);
                }
                kv.value.visit_with(self);
            }
            Prop::Method(m) => {
                if let PropName::Computed(c) = &m.key {
                    c.visit_with(self);
                }
                m.function.visit_with(self);
            }
            Prop::Getter(g) => {
                if let PropName::Computed(c) = &g.key {
                    c.visit_with(self);
                }
                g.body.visit_with(self);
            }
            Prop::Setter(s) => {
                if let PropName::Computed(c) = &s.key {
                    c.visit_with(self);
                }
                s.param.visit_with(self);
                s.body.visit_with(self);
            }
            Prop::Assign(a) => {
                a.value.visit_with(self);
            }
        }
    }

    fn visit_key_value_pat_prop(&mut self, prop: &KeyValuePatProp) {
        if let PropName::Computed(c) = &prop.key {
            c.visit_with(self);
        }
        prop.value.visit_with(self);
    }
}

fn collect_pat_bindings(pat: &Pat, bindings: &mut HashSet<Atom>) {
    match pat {
        Pat::Ident(bi) => {
            bindings.insert(bi.id.sym.clone());
        }
        Pat::Array(arr) => {
            for elem in arr.elems.iter().flatten() {
                collect_pat_bindings(elem, bindings);
            }
        }
        Pat::Object(obj) => {
            for prop in &obj.props {
                match prop {
                    ObjectPatProp::Assign(a) => {
                        bindings.insert(a.key.sym.clone());
                    }
                    ObjectPatProp::KeyValue(kv) => {
                        collect_pat_bindings(&kv.value, bindings);
                    }
                    ObjectPatProp::Rest(r) => {
                        collect_pat_bindings(&r.arg, bindings);
                    }
                }
            }
        }
        Pat::Rest(r) => {
            collect_pat_bindings(&r.arg, bindings);
        }
        Pat::Assign(a) => {
            collect_pat_bindings(&a.left, bindings);
        }
        Pat::Expr(_) | Pat::Invalid(_) => {}
    }
}

fn collect_dynamic_require_helpers(body: &[ModuleItem]) -> HashSet<Atom> {
    let mut helpers = HashSet::new();
    for item in body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            let Some(init) = &decl.init else { continue };
            if is_esbuild_dynamic_require_helper(init) {
                helpers.insert(binding.id.sym.clone());
            }
        }
    }
    helpers
}

fn collect_esbuild_to_esm_helpers(body: &[ModuleItem]) -> HashSet<Atom> {
    let mut helpers = HashSet::new();
    for item in body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            let Some(init) = &decl.init else { continue };
            if is_esbuild_to_esm_helper(init) {
                helpers.insert(binding.id.sym.clone());
            }
        }
    }
    helpers
}

fn is_esbuild_dynamic_require_helper(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    if call.args.len() != 1 {
        return false;
    }

    let mut detector = DynamicRequireHelperDetector::default();
    call.visit_with(&mut detector);
    detector.has_typeof_require
        && detector.has_require_apply_arguments
        && detector.has_dynamic_require_message
}

fn is_esbuild_to_esm_helper(expr: &Expr) -> bool {
    let mut detector = EsbuildToEsmHelperDetector::default();
    expr.visit_with(&mut detector);
    detector.has_es_module_check && detector.has_default_define
}

#[derive(Default)]
struct DynamicRequireHelperDetector {
    has_typeof_require: bool,
    has_require_apply_arguments: bool,
    has_dynamic_require_message: bool,
}

impl Visit for DynamicRequireHelperDetector {
    fn visit_unary_expr(&mut self, expr: &UnaryExpr) {
        if expr.op == UnaryOp::TypeOf {
            if let Expr::Ident(ident) = expr.arg.as_ref() {
                if ident.sym.as_ref() == "require" {
                    self.has_typeof_require = true;
                }
            }
        }
        expr.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Member(member) = callee.as_ref() {
                if matches!(member.obj.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "require")
                    && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "apply")
                    && call.args.len() == 2
                    && matches!(call.args[0].expr.as_ref(), Expr::This(_))
                    && matches!(call.args[1].expr.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "arguments")
                {
                    self.has_require_apply_arguments = true;
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_str(&mut self, str: &Str) {
        if str
            .value
            .as_str()
            .unwrap_or("")
            .contains("Dynamic require of")
        {
            self.has_dynamic_require_message = true;
        }
    }
}

#[derive(Default)]
struct EsbuildToEsmHelperDetector {
    has_es_module_check: bool,
    has_default_define: bool,
}

impl Visit for EsbuildToEsmHelperDetector {
    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "__esModule") {
            self.has_es_module_check = true;
        }
        member.visit_children_with(self);
    }

    fn visit_str(&mut self, str: &Str) {
        if str.value.as_str().unwrap_or("") == "default" {
            self.has_default_define = true;
        }
    }
}

struct DynamicRequireHelperRewriter<'a> {
    helpers: &'a HashSet<Atom>,
    shadowed_scopes: Vec<HashSet<Atom>>,
}

impl<'a> DynamicRequireHelperRewriter<'a> {
    fn new(helpers: &'a HashSet<Atom>) -> Self {
        Self {
            helpers,
            shadowed_scopes: Vec::new(),
        }
    }

    fn is_shadowed(&self, sym: &Atom) -> bool {
        self.shadowed_scopes
            .iter()
            .rev()
            .any(|scope| scope.contains(sym))
    }

    fn push_shadowed(&mut self, names: HashSet<Atom>) {
        self.shadowed_scopes.push(names);
    }

    fn pop_shadowed(&mut self) {
        self.shadowed_scopes.pop();
    }
}

impl VisitMut for DynamicRequireHelperRewriter<'_> {
    fn visit_mut_function(&mut self, function: &mut Function) {
        let mut names = collect_local_bindings_from_function(function);
        for param in &function.params {
            collect_pat_bindings(&param.pat, &mut names);
        }
        self.push_shadowed(names);
        function.visit_mut_children_with(self);
        self.pop_shadowed();
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        let mut names = HashSet::new();
        for param in &arrow.params {
            collect_pat_bindings(param, &mut names);
        }
        if let BlockStmtOrExpr::BlockStmt(block) = arrow.body.as_ref() {
            collect_local_bindings_from_stmts(&block.stmts, &mut names);
        }
        self.push_shadowed(names);
        arrow.visit_mut_children_with(self);
        self.pop_shadowed();
    }

    fn visit_mut_block_stmt(&mut self, block: &mut BlockStmt) {
        let mut names = HashSet::new();
        collect_local_bindings_from_stmts(&block.stmts, &mut names);
        self.push_shadowed(names);
        block.visit_mut_children_with(self);
        self.pop_shadowed();
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Ident(ident) = expr {
            if self.helpers.contains(&ident.sym) && !self.is_shadowed(&ident.sym) {
                *ident = Ident::new("require".into(), ident.span, Default::default());
                return;
            }
        }
        expr.visit_mut_children_with(self);
    }
}

fn collect_local_bindings_from_function(function: &Function) -> HashSet<Atom> {
    let mut names = HashSet::new();
    if let Some(body) = &function.body {
        collect_local_bindings_from_stmts(&body.stmts, &mut names);
    }
    names
}

fn collect_local_bindings_from_stmts(stmts: &[Stmt], names: &mut HashSet<Atom>) {
    struct Collector<'a> {
        names: &'a mut HashSet<Atom>,
    }

    impl Visit for Collector<'_> {
        fn visit_binding_ident(&mut self, binding: &BindingIdent) {
            self.names.insert(binding.id.sym.clone());
        }

        fn visit_function(&mut self, _: &Function) {}

        fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

        fn visit_class(&mut self, _: &Class) {}
    }

    let mut collector = Collector { names };
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
}

fn unwrap_esbuild_to_esm_helper_item(
    item: &mut ModuleItem,
    helpers: &HashSet<Atom>,
    default_bindings: &mut HashSet<Atom>,
) {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return;
    };

    for decl in &mut var.decls {
        let Pat::Ident(binding) = &decl.name else {
            continue;
        };
        let Some(init) = &mut decl.init else {
            continue;
        };
        let Some(require_call) = take_wrapped_require_call(init, helpers) else {
            continue;
        };
        *init = require_call;
        default_bindings.insert(binding.id.sym.clone());
    }
}

fn take_wrapped_require_call(expr: &Expr, helpers: &HashSet<Atom>) -> Option<Box<Expr>> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(helper) = callee.as_ref() else {
        return None;
    };
    if !helpers.contains(&helper.sym) {
        return None;
    }

    let first_arg = call.args.first()?;
    if is_require_call(first_arg.expr.as_ref()) {
        Some(first_arg.expr.clone())
    } else {
        None
    }
}

fn is_require_call(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    matches!(callee.as_ref(), Expr::Ident(ident) if ident.sym.as_ref() == "require")
}

struct DefaultInteropMemberRewriter<'a> {
    bindings: &'a HashSet<Atom>,
    shadowed_scopes: Vec<HashSet<Atom>>,
}

impl<'a> DefaultInteropMemberRewriter<'a> {
    fn new(bindings: &'a HashSet<Atom>) -> Self {
        Self {
            bindings,
            shadowed_scopes: Vec::new(),
        }
    }

    fn is_shadowed(&self, sym: &Atom) -> bool {
        self.shadowed_scopes
            .iter()
            .rev()
            .any(|scope| scope.contains(sym))
    }

    fn push_shadowed(&mut self, names: HashSet<Atom>) {
        self.shadowed_scopes.push(names);
    }

    fn pop_shadowed(&mut self) {
        self.shadowed_scopes.pop();
    }
}

impl VisitMut for DefaultInteropMemberRewriter<'_> {
    fn visit_mut_function(&mut self, function: &mut Function) {
        let mut names = collect_local_bindings_from_function(function);
        for param in &function.params {
            collect_pat_bindings(&param.pat, &mut names);
        }
        self.push_shadowed(names);
        function.visit_mut_children_with(self);
        self.pop_shadowed();
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        let mut names = HashSet::new();
        for param in &arrow.params {
            collect_pat_bindings(param, &mut names);
        }
        if let BlockStmtOrExpr::BlockStmt(block) = arrow.body.as_ref() {
            collect_local_bindings_from_stmts(&block.stmts, &mut names);
        }
        self.push_shadowed(names);
        arrow.visit_mut_children_with(self);
        self.pop_shadowed();
    }

    fn visit_mut_block_stmt(&mut self, block: &mut BlockStmt) {
        let mut names = HashSet::new();
        collect_local_bindings_from_stmts(&block.stmts, &mut names);
        self.push_shadowed(names);
        block.visit_mut_children_with(self);
        self.pop_shadowed();
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Member(member) = expr {
            if let Expr::Ident(obj) = member.obj.as_ref() {
                if self.bindings.contains(&obj.sym)
                    && !self.is_shadowed(&obj.sym)
                    && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "default")
                {
                    *expr = Expr::Ident(obj.clone());
                    return;
                }
            }
        }
        expr.visit_mut_children_with(self);
    }
}

// ---------------------------------------------------------------------------
// Phase 2: Reference graph
// ---------------------------------------------------------------------------

struct ReferenceGraph {
    references: Vec<HashSet<usize>>,
    referenced_by: Vec<HashSet<usize>>,
    writes: Vec<HashSet<usize>>,
}

fn build_reference_graph(items: &[TopLevelItem]) -> ReferenceGraph {
    let mut name_to_item: HashMap<Atom, usize> = HashMap::new();
    for (idx, item) in items.iter().enumerate() {
        for name in &item.declared_names {
            name_to_item.insert(name.clone(), idx);
        }
    }

    let n = items.len();
    let mut references = vec![HashSet::new(); n];
    let mut referenced_by = vec![HashSet::new(); n];
    let mut writes = vec![HashSet::new(); n];

    for (idx, item) in items.iter().enumerate() {
        for ref_name in &item.referenced_names {
            if let Some(&target_idx) = name_to_item.get(ref_name) {
                if target_idx != idx {
                    references[idx].insert(target_idx);
                    referenced_by[target_idx].insert(idx);
                }
            }
        }
        for written_name in &item.written_names {
            if let Some(&target_idx) = name_to_item.get(written_name) {
                if target_idx != idx {
                    writes[idx].insert(target_idx);
                }
            }
        }
    }

    ReferenceGraph {
        references,
        referenced_by,
        writes,
    }
}

// ---------------------------------------------------------------------------
// Phase 3: Clustering
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return;
        }
        if self.rank[ra] < self.rank[rb] {
            self.parent[ra] = rb;
        } else if self.rank[ra] > self.rank[rb] {
            self.parent[rb] = ra;
        } else {
            self.parent[rb] = ra;
            self.rank[ra] += 1;
        }
    }
}

/// ESM imports are immutable bindings. If one planned cluster writes a binding
/// declared by another, emitting that edge as an import would produce invalid
/// JavaScript. Keep each writer with the item that owns the mutable binding.
fn merge_cross_item_writes(graph: &ReferenceGraph, uf: &mut UnionFind) {
    for (writer, targets) in graph.writes.iter().enumerate() {
        for &target in targets {
            uf.union(writer, target);
        }
    }
}

/// Direct-asset Inspect policy: the contiguity invariant. Scope-hoisting
/// bundlers emit each original module as one contiguous run of top-level
/// items, so a writer/owner merge is same-module evidence only when the two
/// clusters are neighbors in item order. A merge is accepted when the
/// clusters' item-index hulls overlap or touch; runtime hubs that mutate
/// state across distant modules fail the test and are skipped instead of
/// gluing unrelated modules together. Executable rendering keeps the
/// unconditional merge: splitting a writer from its owner would turn the
/// write into an assignment to an imported binding. Edges are processed in
/// sorted order so the result cannot depend on hash iteration order.
fn merge_adjacent_cross_item_writes(graph: &ReferenceGraph, uf: &mut UnionFind) {
    let item_count = graph.writes.len();
    let mut hulls: Vec<(usize, usize)> = (0..item_count).map(|item| (item, item)).collect();
    for item in 0..item_count {
        let root = uf.find(item);
        let (lo, hi) = hulls[root];
        hulls[root] = (lo.min(item), hi.max(item));
    }

    let mut edges: Vec<(usize, usize)> = graph
        .writes
        .iter()
        .enumerate()
        .flat_map(|(writer, targets)| targets.iter().map(move |&target| (writer, target)))
        .collect();
    edges.sort_unstable();
    edges.dedup();

    for (writer, target) in edges {
        let writer_root = uf.find(writer);
        let target_root = uf.find(target);
        if writer_root == target_root {
            continue;
        }
        let (a_lo, a_hi) = hulls[writer_root];
        let (b_lo, b_hi) = hulls[target_root];
        let adjacent = a_lo.max(b_lo) <= a_hi.min(b_hi) + 1;
        if !adjacent {
            continue;
        }
        uf.union(writer, target);
        let merged_root = uf.find(writer);
        hulls[merged_root] = (a_lo.min(b_lo), a_hi.max(b_hi));
    }
}

struct CrossWriteTopology {
    signal_root_by_item: Vec<usize>,
    writer_target_degrees: Vec<usize>,
    write_component_by_root: Vec<usize>,
    component_cluster_count_by_root: Vec<usize>,
    leaf_component_cluster_count_by_root: Vec<usize>,
}

fn analyze_cross_item_writes(graph: &ReferenceGraph, uf: &UnionFind) -> CrossWriteTopology {
    let item_count = graph.writes.len();
    let mut signal_uf = uf.clone();
    let signal_root_by_item: Vec<_> = (0..item_count).map(|item| signal_uf.find(item)).collect();
    let mut writer_targets = vec![HashSet::new(); item_count];
    let mut write_components = UnionFind::new(item_count);

    for (writer, targets) in graph.writes.iter().enumerate() {
        let writer_root = signal_root_by_item[writer];
        for &target in targets {
            let target_root = signal_root_by_item[target];
            if writer_root != target_root {
                writer_targets[writer_root].insert(target_root);
                write_components.union(writer_root, target_root);
            }
        }
    }

    let signal_roots: HashSet<_> = signal_root_by_item.iter().copied().collect();
    let mut component_counts = HashMap::new();
    let mut component_minimums = HashMap::new();
    for &signal_root in &signal_roots {
        let component = write_components.find(signal_root);
        *component_counts.entry(component).or_insert(0usize) += 1;
        component_minimums
            .entry(component)
            .and_modify(|minimum: &mut usize| *minimum = (*minimum).min(signal_root))
            .or_insert(signal_root);
    }

    let mut write_component_by_root = (0..item_count).collect::<Vec<_>>();
    let mut component_cluster_count_by_root = vec![1; item_count];
    for signal_root in signal_roots {
        let component = write_components.find(signal_root);
        write_component_by_root[signal_root] = component_minimums[&component];
        component_cluster_count_by_root[signal_root] = component_counts[&component];
    }

    let writer_target_degrees = writer_targets.iter().map(HashSet::len).collect::<Vec<_>>();
    let mut leaf_components = UnionFind::new(item_count);
    for (writer, targets) in graph.writes.iter().enumerate() {
        let writer_root = signal_root_by_item[writer];
        if writer_target_degrees[writer_root] != 1 {
            continue;
        }
        for &target in targets {
            let target_root = signal_root_by_item[target];
            if writer_root != target_root {
                leaf_components.union(writer_root, target_root);
            }
        }
    }

    let signal_roots: HashSet<_> = signal_root_by_item.iter().copied().collect();
    let mut leaf_component_counts = HashMap::new();
    for &signal_root in &signal_roots {
        let component = leaf_components.find(signal_root);
        *leaf_component_counts.entry(component).or_insert(0usize) += 1;
    }
    let mut leaf_component_cluster_count_by_root = vec![1; item_count];
    for signal_root in signal_roots {
        let component = leaf_components.find(signal_root);
        leaf_component_cluster_count_by_root[signal_root] = leaf_component_counts[&component];
    }

    CrossWriteTopology {
        signal_root_by_item,
        writer_target_degrees,
        write_component_by_root,
        component_cluster_count_by_root,
        leaf_component_cluster_count_by_root,
    }
}

fn retain_inspect_cross_write_edge(
    topology: &CrossWriteTopology,
    writer_root: usize,
    max_component_clusters: usize,
    restore_bounded_leaves: bool,
) -> bool {
    topology.component_cluster_count_by_root[writer_root] <= max_component_clusters
        || (restore_bounded_leaves
            && topology.writer_target_degrees[writer_root] == 1
            && topology.leaf_component_cluster_count_by_root[writer_root] <= max_component_clusters)
}

fn canonical_cluster_ids(uf: &UnionFind, item_count: usize) -> Vec<usize> {
    let mut uf = uf.clone();
    let roots: Vec<_> = (0..item_count).map(|item| uf.find(item)).collect();
    let mut minimum_by_root = HashMap::new();
    for (item, &root) in roots.iter().enumerate() {
        minimum_by_root
            .entry(root)
            .and_modify(|minimum: &mut usize| *minimum = (*minimum).min(item))
            .or_insert(item);
    }
    roots
        .into_iter()
        .map(|root| minimum_by_root[&root])
        .collect()
}

/// Keep useful writer/owner evidence for Inspect without allowing one
/// write-connected runtime component to glue a large plan together. The cap
/// counts clusters already formed by Signals 1–5, not raw top-level items.
/// In an oversized component, tentatively retain only degree-one writer edges
/// whose leaf-only residual component also fits under the cap. This preserves
/// local mutable-owner evidence while pruning write hubs and long leaf chains.
/// Because changing the emitted module count produced mixed corpus tradeoffs
/// (including singleton promotion and purity regressions), back off to the
/// component cap unless each write component preserves the post-folding
/// inspection cluster count exactly when applied in canonical component order.
/// This prevents opposite count changes in independent components from
/// cancelling out. Decisions are computed before any union so edge discovery
/// is independent of hash order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InspectCrossWriteDecision {
    component_cap_output_clusters: usize,
    leaf_candidate_output_clusters: usize,
    bounded_leaf_restoration_accepted: bool,
    restored_components: HashSet<usize>,
}

fn merge_bounded_cross_item_writes(
    items: &[TopLevelItem],
    graph: &ReferenceGraph,
    uf: &mut UnionFind,
    max_component_clusters: usize,
) -> InspectCrossWriteDecision {
    let topology = analyze_cross_item_writes(graph, uf);
    let mut component_cap_uf = uf.clone();
    let mut leaf_candidate_uf = uf.clone();
    let mut leaf_edges_by_component: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();

    for (writer, targets) in graph.writes.iter().enumerate() {
        let writer_root = topology.signal_root_by_item[writer];
        let retained_by_component_cap =
            retain_inspect_cross_write_edge(&topology, writer_root, max_component_clusters, false);
        let retained_by_leaf_candidate =
            retain_inspect_cross_write_edge(&topology, writer_root, max_component_clusters, true);
        if retained_by_component_cap {
            for &target in targets {
                component_cap_uf.union(writer, target);
            }
        }
        if retained_by_leaf_candidate {
            for &target in targets {
                leaf_candidate_uf.union(writer, target);
                if !retained_by_component_cap && topology.signal_root_by_item[target] != writer_root
                {
                    leaf_edges_by_component
                        .entry(topology.write_component_by_root[writer_root])
                        .or_default()
                        .push((writer, target));
                }
            }
        }
    }

    let component_cap_output_clusters = inspection_output_cluster_count(items, &component_cap_uf);
    let leaf_candidate_output_clusters = if leaf_edges_by_component.is_empty() {
        component_cap_output_clusters
    } else {
        inspection_output_cluster_count(items, &leaf_candidate_uf)
    };

    let mut selected_uf = component_cap_uf;
    let mut restored_components = HashSet::new();
    let mut component_ids = leaf_edges_by_component.keys().copied().collect::<Vec<_>>();
    component_ids.sort_unstable();
    for component_id in component_ids {
        let mut candidate_uf = selected_uf.clone();
        for &(writer, target) in &leaf_edges_by_component[&component_id] {
            candidate_uf.union(writer, target);
        }
        if inspection_output_cluster_count(items, &candidate_uf) == component_cap_output_clusters {
            selected_uf = candidate_uf;
            restored_components.insert(component_id);
        }
    }

    let bounded_leaf_restoration_accepted = !restored_components.is_empty();
    *uf = selected_uf;

    InspectCrossWriteDecision {
        component_cap_output_clusters,
        leaf_candidate_output_clusters,
        bounded_leaf_restoration_accepted,
        restored_components,
    }
}

fn inspection_output_cluster_count(items: &[TopLevelItem], uf: &UnionFind) -> usize {
    let mut uf = uf.clone();
    let roots = extract_root_clusters(items, &mut uf);
    extract_inspection_clusters(items, &roots).len()
}

#[allow(clippy::needless_range_loop)]
fn apply_merge_signals(items: &[TopLevelItem], graph: &ReferenceGraph, uf: &mut UnionFind) {
    let adjacency_window = 3;

    // Signal 1: Mutual references — A references B AND B references A.
    for i in 0..items.len() {
        for &j in &graph.references[i] {
            if graph.references[j].contains(&i) {
                uf.union(i, j);
            }
        }
    }

    // Signal 2: Adjacent dependency chain.
    // Merge (i, i+1) when i+1 references i AND all of i+1's file-local
    // references point to items already in i's cluster. This prevents
    // entry code (which fans out to multiple groups) from chaining into
    // module clusters.
    for i in 0..items.len().saturating_sub(1) {
        let j = i + 1;
        let j_refs_i = items[j]
            .referenced_names
            .iter()
            .any(|name| items[i].declared_names.contains(name));
        if !j_refs_i {
            continue;
        }
        let i_root = uf.find(i);
        let all_in_same_cluster = graph.references[j]
            .iter()
            .all(|&target| uf.find(target) == i_root);
        if all_in_same_cluster {
            uf.union(i, j);
        }
    }

    // Signal 3: Inert helper merge.
    // An "inert" item has no file-local references (e.g. `const _data = new WeakMap()`).
    // If it's exclusively consumed by one item within the adjacency window,
    // it's a private helper — merge unconditionally.
    for b in 0..items.len() {
        if items[b].declared_names.is_empty() || !graph.references[b].is_empty() {
            continue;
        }
        if graph.referenced_by[b].len() != 1 {
            continue;
        }
        let &consumer = graph.referenced_by[b].iter().next().unwrap();
        let dist = consumer.abs_diff(b);
        if dist <= adjacency_window {
            uf.union(b, consumer);
        }
    }

    // Signal 4: Adjacency + shared reference.
    for i in 0..items.len() {
        if items[i].declared_names.is_empty() {
            continue;
        }
        let end = (i + adjacency_window + 1).min(items.len());
        for j in (i + 1)..end {
            if items[j].declared_names.is_empty() {
                continue;
            }
            if uf.find(i) == uf.find(j) {
                continue;
            }
            let has_shared_ref = graph.references[i]
                .iter()
                .any(|target| graph.references[j].contains(target));
            if has_shared_ref {
                uf.union(i, j);
            }
        }
    }

    // Signal 5: Exclusive consumer (conservative).
    // Merge B into A when B is exclusively consumed by A, within adjacency
    // window, AND A's cluster currently references at most 1 other cluster.
    // This prevents entry code (high fan-out across clusters) from absorbing
    // module code.
    for b in 0..items.len() {
        if items[b].declared_names.is_empty() {
            continue;
        }
        if graph.referenced_by[b].len() != 1 {
            continue;
        }
        let &consumer = graph.referenced_by[b].iter().next().unwrap();
        if uf.find(b) == uf.find(consumer) {
            continue;
        }
        let dist = consumer.abs_diff(b);
        if dist > adjacency_window {
            continue;
        }
        let consumer_root = uf.find(consumer);
        let b_root = uf.find(b);
        let cluster_members: Vec<usize> = (0..items.len())
            .filter(|&k| uf.find(k) == consumer_root)
            .collect();
        let mut ref_targets: HashSet<usize> = HashSet::new();
        for k in &cluster_members {
            for &t in &graph.references[*k] {
                let tr = uf.find(t);
                if tr != consumer_root && tr != b_root {
                    ref_targets.insert(tr);
                }
            }
        }
        if ref_targets.len() <= 1 {
            uf.union(b, consumer);
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 4: Extract clusters and identify entry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Cluster {
    item_indices: Vec<usize>,
    is_entry: bool,
}

fn extract_root_clusters(items: &[TopLevelItem], uf: &mut UnionFind) -> Vec<Cluster> {
    let mut root_to_indices: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..items.len() {
        root_to_indices.entry(uf.find(i)).or_default().push(i);
    }

    let mut roots: Vec<Cluster> = root_to_indices
        .into_values()
        .map(|mut item_indices| {
            item_indices.sort_unstable();
            let decl_count = item_indices
                .iter()
                .filter(|&&item_index| !items[item_index].declared_names.is_empty())
                .count();
            let has_module_decl = item_indices
                .iter()
                .any(|&item_index| items[item_index].is_module_decl);
            let has_declarationless_item = item_indices
                .iter()
                .any(|&item_index| items[item_index].declared_names.is_empty());
            Cluster {
                item_indices,
                // Declarationless effects and native module declarations must
                // remain reachable through the executable entry. Singleton
                // declarations are optional entry material and are repartitioned
                // below instead of being marked here.
                is_entry: has_module_decl || (decl_count < 2 && has_declarationless_item),
            }
        })
        .collect();
    roots.sort_by_key(|cluster| cluster.item_indices[0]);
    roots
}

fn extract_inspection_clusters(items: &[TopLevelItem], roots: &[Cluster]) -> Vec<Cluster> {
    let min_cluster_decls = 2;

    // Classify: clusters with enough declarations are "module clusters".
    // Small clusters (< min_cluster_decls declarations) and clusters with
    // ModuleDecl items get folded into the entry.
    let mut entry_indices: Vec<usize> = Vec::new();
    let mut module_clusters: Vec<Cluster> = Vec::new();

    for cluster in roots {
        let decl_count = cluster
            .item_indices
            .iter()
            .filter(|&&i| !items[i].declared_names.is_empty())
            .count();
        let has_module_decl = cluster
            .item_indices
            .iter()
            .any(|&i| items[i].is_module_decl);

        if has_module_decl || decl_count < min_cluster_decls {
            entry_indices.extend(cluster.item_indices.iter().copied());
        } else {
            module_clusters.push(Cluster {
                item_indices: cluster.item_indices.clone(),
                is_entry: false,
            });
        }
    }

    // Build final result: module clusters + entry.
    if !entry_indices.is_empty() {
        entry_indices.sort();
        module_clusters.push(Cluster {
            item_indices: entry_indices,
            is_entry: true,
        });
    }

    // If we ended up with only an entry (no module clusters), return
    // empty so the caller falls back to single-module output.
    if module_clusters.iter().all(|c| c.is_entry) {
        return vec![];
    }

    module_clusters
}

/// Build executable clusters without contracting every singleton declaration
/// into one global entry.
///
/// Root SCCs are merged first. The resulting condensation graph is a DAG, so
/// assigning singleton SCCs to the nearest established module in one stable
/// topological order creates contiguous DAG regions and cannot introduce a
/// cycle between those module regions. Module declarations and declarationless
/// effects remain in the entry; the normal SCC merge below still handles any
/// cycle involving that mandatory entry material.
fn extract_executable_clusters(
    items: &[TopLevelItem],
    graph: &ReferenceGraph,
    roots: Vec<Cluster>,
) -> Vec<Cluster> {
    let root_graph = build_cluster_graph(&roots, graph);
    let mut atoms: Vec<Cluster> = strongly_connected_components(&root_graph)
        .into_iter()
        .map(|component| {
            let mut item_indices = Vec::new();
            let mut is_entry = false;
            for root_index in component {
                item_indices.extend(roots[root_index].item_indices.iter().copied());
                is_entry |= roots[root_index].is_entry;
            }
            item_indices.sort_unstable();
            Cluster {
                item_indices,
                is_entry,
            }
        })
        .collect();
    atoms.sort_by_key(|atom| atom.item_indices[0]);

    let atom_graph = build_cluster_graph(&atoms, graph);
    let Some(topo_order) = stable_topological_order(&atoms, &atom_graph) else {
        // SCC contraction should always produce a DAG. Preserve correctness if
        // that invariant is ever violated by falling back to one source-ordered
        // executable component.
        let mut item_indices = (0..items.len()).collect::<Vec<_>>();
        item_indices.sort_unstable();
        return vec![Cluster {
            item_indices,
            is_entry: true,
        }];
    };

    let mut anchors: Vec<(usize, Cluster)> = Vec::new();
    let mut singleton_atoms: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut entry_indices = Vec::new();
    for (topo_position, atom_index) in topo_order.into_iter().enumerate() {
        let atom = &atoms[atom_index];
        let decl_count = atom
            .item_indices
            .iter()
            .filter(|&&item_index| !items[item_index].declared_names.is_empty())
            .count();
        if atom.is_entry {
            entry_indices.extend(atom.item_indices.iter().copied());
        } else if decl_count == 1 {
            singleton_atoms.push((topo_position, atom.item_indices.clone()));
        } else {
            anchors.push((
                topo_position,
                Cluster {
                    item_indices: atom.item_indices.clone(),
                    is_entry: false,
                },
            ));
        }
    }

    if anchors.is_empty() {
        entry_indices.extend(
            singleton_atoms
                .into_iter()
                .flat_map(|(_, item_indices)| item_indices),
        );
        entry_indices.sort_unstable();
        return vec![Cluster {
            item_indices: entry_indices,
            is_entry: true,
        }];
    }

    // Preserve an executable entry when the old small-root policy would have
    // supplied one but the source has no declarationless or module-declaration
    // root. The latest singleton is the closest available approximation of
    // trailing startup/export state in a flat scope-hoisted file.
    if entry_indices.is_empty() && !singleton_atoms.is_empty() {
        let entry_atom_index = singleton_atoms
            .iter()
            .enumerate()
            .max_by_key(|(_, (_, item_indices))| item_indices.last().copied().unwrap_or(0))
            .map(|(index, _)| index)
            .expect("non-empty singleton atoms checked above");
        let (_, item_indices) = singleton_atoms.swap_remove(entry_atom_index);
        entry_indices.extend(item_indices);
    }

    for (topo_position, item_indices) in singleton_atoms {
        let anchor_index = (0..anchors.len())
            .min_by_key(|&anchor_index| {
                (
                    topo_position.abs_diff(anchors[anchor_index].0),
                    anchor_index,
                )
            })
            .expect("non-empty anchors checked above");
        anchors[anchor_index].1.item_indices.extend(item_indices);
    }

    let mut clusters = anchors
        .into_iter()
        .map(|(_, mut cluster)| {
            cluster.item_indices.sort_unstable();
            cluster
        })
        .collect::<Vec<_>>();
    if !entry_indices.is_empty() {
        entry_indices.sort_unstable();
        clusters.push(Cluster {
            item_indices: entry_indices,
            is_entry: true,
        });
    }
    clusters.sort_by_key(|cluster| cluster.item_indices[0]);
    clusters
}

fn stable_topological_order(clusters: &[Cluster], graph: &[HashSet<usize>]) -> Option<Vec<usize>> {
    let mut indegree = vec![0usize; graph.len()];
    for targets in graph {
        for &target in targets {
            indegree[target] += 1;
        }
    }

    let mut ready = BinaryHeap::new();
    for (cluster_index, &degree) in indegree.iter().enumerate() {
        if degree == 0 {
            ready.push(Reverse((
                clusters[cluster_index].item_indices[0],
                cluster_index,
            )));
        }
    }

    let mut order = Vec::with_capacity(graph.len());
    while let Some(Reverse((_, cluster_index))) = ready.pop() {
        order.push(cluster_index);
        for &target in &graph[cluster_index] {
            indegree[target] -= 1;
            if indegree[target] == 0 {
                ready.push(Reverse((clusters[target].item_indices[0], target)));
            }
        }
    }

    (order.len() == graph.len()).then_some(order)
}

/// Executable outputs must preserve top-level effect order: a declarationless
/// item (a bare effect statement such as `state++;` or a top-level call) runs
/// whenever the module holding it is first imported, so emitting it inside a
/// lazily imported chunk reorders it against the entry statements around it.
/// Move each such item into the entry, which emits items in original source
/// order. A bare writer drags its whole write-group along — every item that
/// writes a binding must stay with the declaration (imported bindings are
/// immutable), while pure readers may stay behind and import the moved
/// binding as a live view. Inspect mode keeps fine-grained clusters and
/// explicitly does not promise initialization order.
fn fold_startup_effects_into_entry(
    body: &[ModuleItem],
    items: &[TopLevelItem],
    graph: &ReferenceGraph,
    clusters: Vec<Cluster>,
) -> Vec<Cluster> {
    let clusters = move_bare_statements_into_entry(items, graph, clusters);
    fold_unreachable_effectful_clusters(body, graph, clusters)
}

fn move_bare_statements_into_entry(
    items: &[TopLevelItem],
    graph: &ReferenceGraph,
    clusters: Vec<Cluster>,
) -> Vec<Cluster> {
    let mut write_groups = UnionFind::new(items.len());
    merge_cross_item_writes(graph, &mut write_groups);
    let mut entry_write_roots = HashSet::new();
    for (item_index, item) in items.iter().enumerate() {
        if item.declared_names.is_empty() && !item.is_module_decl {
            entry_write_roots.insert(write_groups.find(item_index));
        }
    }
    if entry_write_roots.is_empty() {
        return clusters;
    }

    let mut entry_indices = Vec::new();
    let mut kept = Vec::new();
    for cluster in clusters {
        if cluster.is_entry {
            entry_indices.extend(cluster.item_indices);
            continue;
        }
        let (moved, stay): (Vec<_>, Vec<_>) = cluster
            .item_indices
            .into_iter()
            .partition(|&item_index| entry_write_roots.contains(&write_groups.find(item_index)));
        entry_indices.extend(moved);
        if !stay.is_empty() {
            kept.push(Cluster {
                item_indices: stay,
                is_entry: false,
            });
        }
    }
    if entry_indices.is_empty() {
        return kept;
    }
    entry_indices.sort_unstable();
    kept.push(Cluster {
        item_indices: entry_indices,
        is_entry: true,
    });
    kept.sort_by_key(|cluster| cluster.item_indices[0]);
    kept
}

/// A cluster the entry never transitively imports never executes, so a
/// side-effectful initializer parked there (singleton repartitioning attaches
/// singletons by topological distance, without requiring a reference) is
/// silently dropped from the program. Fold every unreachable cluster that
/// carries startup effects into the entry — its items return to their source
/// positions there. Pure unreachable clusters stay split: never running them
/// is unobservable, and they remain readable output.
fn fold_unreachable_effectful_clusters(
    body: &[ModuleItem],
    graph: &ReferenceGraph,
    mut clusters: Vec<Cluster>,
) -> Vec<Cluster> {
    if !clusters.iter().any(|cluster| cluster.is_entry) {
        return clusters;
    }
    let cluster_graph = build_cluster_graph(&clusters, graph);
    let mut fold = vec![false; clusters.len()];
    // Folding a cluster makes everything it references reachable, which can
    // spare other effectful clusters from folding — iterate to a fixpoint
    // (each round only grows the fold set, so it terminates).
    loop {
        let mut reachable = vec![false; clusters.len()];
        let mut queue: Vec<usize> = (0..clusters.len())
            .filter(|&index| clusters[index].is_entry || fold[index])
            .collect();
        for &index in &queue {
            reachable[index] = true;
        }
        while let Some(index) = queue.pop() {
            for &target in &cluster_graph[index] {
                if !reachable[target] {
                    reachable[target] = true;
                    queue.push(target);
                }
            }
        }
        let mut changed = false;
        for (index, cluster) in clusters.iter().enumerate() {
            if reachable[index] || fold[index] {
                continue;
            }
            if cluster
                .item_indices
                .iter()
                .any(|&item_index| item_executes_startup_effects(&body[item_index]))
            {
                fold[index] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    if !fold.iter().any(|&folds| folds) {
        return clusters;
    }

    let mut entry_indices = Vec::new();
    let mut kept = Vec::new();
    for (index, cluster) in clusters.drain(..).enumerate() {
        if cluster.is_entry || fold[index] {
            entry_indices.extend(cluster.item_indices);
        } else {
            kept.push(cluster);
        }
    }
    entry_indices.sort_unstable();
    kept.push(Cluster {
        item_indices: entry_indices,
        is_entry: true,
    });
    kept.sort_by_key(|cluster| cluster.item_indices[0]);
    kept
}

/// True when executing this item is observable: bare statements, and
/// declarations whose initializers run code.
fn item_executes_startup_effects(item: &ModuleItem) -> bool {
    use crate::analysis::purity::{is_pure_decl, is_pure_default_decl, is_pure_init};
    match item {
        ModuleItem::Stmt(Stmt::Decl(decl)) => !is_pure_decl(decl),
        ModuleItem::Stmt(_) => true,
        ModuleItem::ModuleDecl(decl) => match decl {
            ModuleDecl::Import(_) | ModuleDecl::ExportAll(_) | ModuleDecl::ExportNamed(_) => false,
            ModuleDecl::ExportDecl(export) => !is_pure_decl(&export.decl),
            ModuleDecl::ExportDefaultDecl(export) => !is_pure_default_decl(&export.decl),
            ModuleDecl::ExportDefaultExpr(export) => !is_pure_init(&export.expr),
            _ => true,
        },
    }
}

/// Detect when the synthetic entry turns a substantial part of a large plan into
/// one component. Small SCCs keep the established clustering behavior.
fn has_pathological_entry_scc(clusters: &[Cluster], graph: &ReferenceGraph) -> bool {
    if clusters.len() < PATHOLOGICAL_ENTRY_SCC_MIN_CLUSTERS {
        return false;
    }

    let cluster_graph = build_cluster_graph(clusters, graph);
    strongly_connected_components(&cluster_graph)
        .into_iter()
        .any(|component| {
            component.len() >= PATHOLOGICAL_ENTRY_SCC_MIN_CLUSTERS
                && component
                    .len()
                    .saturating_mul(PATHOLOGICAL_ENTRY_SCC_MIN_FRACTION_DENOMINATOR)
                    >= clusters.len()
                && component
                    .iter()
                    .any(|&cluster_index| clusters[cluster_index].is_entry)
        })
}

/// Merge import cycles created by cluster extraction before imports are emitted.
///
/// Small item clusters are folded into one synthetic entry. That contraction can
/// create a cycle even when the original item graph is acyclic. Merging after
/// emission is too late because emitted module order does not retain source order;
/// combining the clusters here lets us sort their original item indices instead.
fn merge_cyclic_clusters(clusters: Vec<Cluster>, graph: &ReferenceGraph) -> Vec<Cluster> {
    if clusters.len() < 2 {
        return clusters;
    }

    let cluster_graph = build_cluster_graph(&clusters, graph);
    let components = strongly_connected_components(&cluster_graph);
    if components.iter().all(|component| component.len() == 1) {
        return clusters;
    }

    let mut merged = Vec::with_capacity(components.len());
    for component in components {
        let mut item_indices = Vec::new();
        let mut is_entry = false;
        for cluster_index in component {
            item_indices.extend(clusters[cluster_index].item_indices.iter().copied());
            is_entry |= clusters[cluster_index].is_entry;
        }
        item_indices.sort_unstable();
        merged.push(Cluster {
            item_indices,
            is_entry,
        });
    }
    merged.sort_by_key(|cluster| cluster.item_indices[0]);
    merged
}

fn build_cluster_graph(clusters: &[Cluster], graph: &ReferenceGraph) -> Vec<HashSet<usize>> {
    let mut item_to_cluster = vec![usize::MAX; graph.references.len()];
    for (cluster_index, cluster) in clusters.iter().enumerate() {
        for &item_index in &cluster.item_indices {
            item_to_cluster[item_index] = cluster_index;
        }
    }

    let mut cluster_graph = vec![HashSet::new(); clusters.len()];
    for (cluster_index, cluster) in clusters.iter().enumerate() {
        for &item_index in &cluster.item_indices {
            for &target_item in &graph.references[item_index] {
                let target_cluster = item_to_cluster[target_item];
                if target_cluster != usize::MAX && target_cluster != cluster_index {
                    cluster_graph[cluster_index].insert(target_cluster);
                }
            }
        }
    }
    cluster_graph
}

/// Iterative Kosaraju traversal, avoiding recursion for large scope-hoisted files.
fn strongly_connected_components(graph: &[HashSet<usize>]) -> Vec<Vec<usize>> {
    let adjacency: Vec<Vec<usize>> = graph
        .iter()
        .map(|neighbors| neighbors.iter().copied().collect())
        .collect();
    let mut visited = vec![false; graph.len()];
    let mut finish_order = Vec::with_capacity(graph.len());

    for start in 0..graph.len() {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        let mut stack = vec![(start, 0usize)];
        while let Some((node, next_neighbor)) = stack.last_mut() {
            if *next_neighbor < adjacency[*node].len() {
                let neighbor = adjacency[*node][*next_neighbor];
                *next_neighbor += 1;
                if !visited[neighbor] {
                    visited[neighbor] = true;
                    stack.push((neighbor, 0));
                }
            } else {
                finish_order.push(*node);
                stack.pop();
            }
        }
    }

    let mut reverse = vec![Vec::new(); graph.len()];
    for (source, targets) in adjacency.iter().enumerate() {
        for &target in targets {
            reverse[target].push(source);
        }
    }

    visited.fill(false);
    let mut components = Vec::new();
    for &start in finish_order.iter().rev() {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        let mut component = Vec::new();
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            component.push(node);
            for &neighbor in &reverse[node] {
                if !visited[neighbor] {
                    visited[neighbor] = true;
                    stack.push(neighbor);
                }
            }
        }
        components.push(component);
    }
    components
}

// ---------------------------------------------------------------------------
// Phase 5: Emit modules
// ---------------------------------------------------------------------------

struct ClusterSymbolLinks {
    imports_by_consumer: Vec<Vec<(usize, Vec<Atom>)>>,
    exports_by_producer: Vec<HashSet<Atom>>,
}

fn build_cluster_symbol_links(
    cluster_declared: &[HashSet<Atom>],
    cluster_referenced: &[HashSet<Atom>],
    dynamic_require_helpers: &HashSet<Atom>,
    esbuild_to_esm_helpers: &HashSet<Atom>,
) -> ClusterSymbolLinks {
    let mut declaring_clusters: HashMap<Atom, Vec<usize>> = HashMap::new();
    for (cluster_index, declared_names) in cluster_declared.iter().enumerate() {
        for name in declared_names {
            declaring_clusters
                .entry(name.clone())
                .or_default()
                .push(cluster_index);
        }
    }

    let mut imports_by_consumer = (0..cluster_referenced.len())
        .map(|_| HashMap::<usize, Vec<Atom>>::new())
        .collect::<Vec<_>>();
    let mut exports_by_producer = vec![HashSet::new(); cluster_declared.len()];

    for (consumer_index, referenced_names) in cluster_referenced.iter().enumerate() {
        for name in referenced_names {
            if dynamic_require_helpers.contains(name) || esbuild_to_esm_helpers.contains(name) {
                continue;
            }

            #[cfg(test)]
            record_emit_relation_symbol_probe();
            let Some(producer_indices) = declaring_clusters.get(name) else {
                continue;
            };
            for &producer_index in producer_indices {
                if producer_index == consumer_index {
                    continue;
                }
                imports_by_consumer[consumer_index]
                    .entry(producer_index)
                    .or_default()
                    .push(name.clone());
                exports_by_producer[producer_index].insert(name.clone());
            }
        }
    }

    let imports_by_consumer = imports_by_consumer
        .into_iter()
        .map(|by_producer| {
            let mut links = by_producer.into_iter().collect::<Vec<_>>();
            for (_, names) in &mut links {
                names.sort();
            }
            links.sort_by_key(|(producer_index, _)| *producer_index);
            links
        })
        .collect();

    ClusterSymbolLinks {
        imports_by_consumer,
        exports_by_producer,
    }
}

fn emit_clusters(
    body: &[ModuleItem],
    items: &[TopLevelItem],
    clusters: Vec<Cluster>,
    inspection_context_by_item: Option<&[usize]>,
    cm: Lrc<SourceMap>,
) -> Vec<UnpackedModule> {
    let dynamic_require_helpers = collect_dynamic_require_helpers(body);
    let esbuild_to_esm_helpers = collect_esbuild_to_esm_helpers(body);
    let import_decls = collect_import_decls(body);

    // A fine cluster may use coarse component evidence only when every item
    // in that emitted module belongs to one write component. The synthetic
    // entry often folds unrelated singleton components together; leaving its
    // context empty avoids turning that garbage-bag boundary into new glue.
    let cluster_contexts = inspection_context_by_item.map(|context_by_item| {
        clusters
            .iter()
            .map(|cluster| {
                let mut contexts = cluster
                    .item_indices
                    .iter()
                    .map(|&item| context_by_item[item]);
                let first = contexts.next()?;
                contexts.all(|context| context == first).then_some(first)
            })
            .collect::<Vec<_>>()
    });
    let mut context_cluster_counts = HashMap::new();
    if let Some(cluster_contexts) = &cluster_contexts {
        for context in cluster_contexts.iter().flatten() {
            *context_cluster_counts.entry(*context).or_insert(0usize) += 1;
        }
    }
    let mut context_spans = HashMap::<usize, Vec<Span>>::new();
    if let Some(context_by_item) = inspection_context_by_item {
        for (item, &context) in context_by_item.iter().enumerate() {
            if context_cluster_counts.get(&context).copied().unwrap_or(0) >= 2 {
                context_spans
                    .entry(context)
                    .or_default()
                    .push(body[item].span());
            }
        }
    }
    let context_ranges = context_spans
        .into_iter()
        .map(|(context, spans)| (context, spans_byte_ranges(&cm, spans.into_iter())))
        .collect::<HashMap<_, _>>();

    // Pre-compute: which names does each cluster declare?
    let cluster_declared: Vec<HashSet<Atom>> = clusters
        .iter()
        .map(|c| {
            c.item_indices
                .iter()
                .flat_map(|&i| items[i].declared_names.iter().cloned())
                .collect()
        })
        .collect();

    // Pre-compute: which names does each cluster reference?
    let cluster_referenced: Vec<HashSet<Atom>> = clusters
        .iter()
        .map(|c| {
            c.item_indices
                .iter()
                .flat_map(|&i| items[i].referenced_names.iter().cloned())
                .collect()
        })
        .collect();

    let mut symbol_links = build_cluster_symbol_links(
        &cluster_declared,
        &cluster_referenced,
        &dynamic_require_helpers,
        &esbuild_to_esm_helpers,
    );

    // Assign final filenames first so synthesized imports point at the same
    // paths the caller will write. Chunk names are derived from minified
    // bindings, so collisions are common in scope-concatenated packages.
    let mut seen_filenames = HashSet::new();
    let filenames: Vec<String> = clusters
        .iter()
        .map(|c| {
            if c.is_entry {
                dedup_cluster_filename("entry.js", &mut seen_filenames)
            } else {
                let name = derive_chunk_name(items, c);
                dedup_cluster_filename(&format!("{name}.js"), &mut seen_filenames)
            }
        })
        .collect();

    let mut modules = Vec::new();

    for (ci, cluster) in clusters.iter().enumerate() {
        let mut module_items: Vec<ModuleItem> = Vec::new();

        if !cluster.is_entry {
            module_items.extend(imports_for_references(
                &cluster_referenced[ci],
                &import_decls,
            ));
        }

        // Synthesize imports from the indexed cross-cluster symbol links.
        for (producer_index, needed) in &symbol_links.imports_by_consumer[ci] {
            module_items.push(make_named_import_stmt(needed, &filenames[*producer_index]));
        }

        // Collect which names this cluster should export.
        let mut exported = std::mem::take(&mut symbol_links.exports_by_producer[ci]);

        // Original body items, with exported declarations promoted to
        // `export function ...` / `export const ...` / `export class ...`.
        let mut leftover_exports: Vec<Atom> = Vec::new();
        let should_rewrite_esbuild_to_esm = !esbuild_to_esm_helpers.is_empty();
        let mut default_interop_bindings = HashSet::new();
        for &i in &cluster.item_indices {
            let mut item = body[i].clone();
            // Restore `r("react")` helper calls to direct requires everywhere
            // except the item declaring the helper itself — the helper can
            // land in any cluster (repartitioning attaches singletons to
            // module anchors), and only its own declaration must stay intact.
            let declares_dynamic_require_helper = items[i]
                .declared_names
                .iter()
                .any(|name| dynamic_require_helpers.contains(name));
            if !dynamic_require_helpers.is_empty() && !declares_dynamic_require_helper {
                item.visit_mut_with(&mut DynamicRequireHelperRewriter::new(
                    &dynamic_require_helpers,
                ));
            }
            if should_rewrite_esbuild_to_esm {
                unwrap_esbuild_to_esm_helper_item(
                    &mut item,
                    &esbuild_to_esm_helpers,
                    &mut default_interop_bindings,
                );
            }
            if exported.is_empty() {
                module_items.push(item);
                continue;
            }
            match try_promote_export(&item, &exported) {
                ExportPromotion::Promoted(new_item, promoted_names) => {
                    module_items.push(new_item);
                    for name in &promoted_names {
                        exported.remove(name);
                    }
                }
                ExportPromotion::Split(split_items, names) => {
                    module_items.extend(split_items);
                    for name in &names {
                        exported.remove(name);
                    }
                }
                ExportPromotion::None => {
                    module_items.push(item);
                }
            }
        }
        // Any names that couldn't be promoted inline get a trailing export.
        leftover_exports.extend(exported.iter().cloned());
        if !leftover_exports.is_empty() {
            leftover_exports.sort();
            module_items.push(make_named_export_stmt(&leftover_exports));
        }

        if !default_interop_bindings.is_empty() {
            for item in &mut module_items {
                item.visit_mut_with(&mut DefaultInteropMemberRewriter::new(
                    &default_interop_bindings,
                ));
            }
        }

        if module_items.is_empty() {
            continue;
        }

        let id = if cluster.is_entry {
            "entry".to_string()
        } else {
            derive_chunk_name(items, cluster)
        };

        let code = emit_items(module_items, filenames[ci].clone(), cm.clone());
        modules.push(UnpackedModule {
            id,
            is_entry: cluster.is_entry,
            code,
            filename: filenames[ci].clone(),
            source_ranges: spans_byte_ranges(
                &cm,
                cluster.item_indices.iter().map(|&i| body[i].span()),
            ),
            inspection_context_ranges: cluster_contexts
                .as_ref()
                .and_then(|contexts| contexts[ci])
                .and_then(|context| context_ranges.get(&context).cloned())
                .unwrap_or_default(),
            source_input: String::new(),
            generated_source_map: Vec::new(),
        });
    }

    modules
}

fn collect_import_decls(body: &[ModuleItem]) -> Vec<ImportDecl> {
    body.iter()
        .filter_map(|item| match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => Some(import.clone()),
            _ => None,
        })
        .collect()
}

fn imports_for_references(
    referenced_names: &HashSet<Atom>,
    import_decls: &[ImportDecl],
) -> Vec<ModuleItem> {
    import_decls
        .iter()
        .filter_map(|import| import_for_references(import, referenced_names))
        .map(|import| ModuleItem::ModuleDecl(ModuleDecl::Import(import)))
        .collect()
}

fn import_for_references(
    import: &ImportDecl,
    referenced_names: &HashSet<Atom>,
) -> Option<ImportDecl> {
    let specifiers: Vec<ImportSpecifier> = import
        .specifiers
        .iter()
        .filter(|specifier| {
            import_specifier_local(specifier).is_some_and(|local| referenced_names.contains(local))
        })
        .cloned()
        .collect();

    if specifiers.is_empty() {
        return None;
    }

    let mut import = import.clone();
    import.specifiers = specifiers;
    Some(import)
}

fn import_specifier_local(specifier: &ImportSpecifier) -> Option<&Atom> {
    match specifier {
        ImportSpecifier::Named(named) => Some(&named.local.sym),
        ImportSpecifier::Default(default) => Some(&default.local.sym),
        ImportSpecifier::Namespace(namespace) => Some(&namespace.local.sym),
    }
}

enum ExportPromotion {
    Promoted(ModuleItem, Vec<Atom>),
    Split(Vec<ModuleItem>, Vec<Atom>),
    None,
}

fn try_promote_export(item: &ModuleItem, exported: &HashSet<Atom>) -> ExportPromotion {
    if let Some((new_item, names)) = try_promote_fn_class_export(item, exported) {
        return ExportPromotion::Promoted(new_item, names);
    }
    match item {
        // `const x = ..., y = ...` — check if all or some declarators are exported.
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
            let decl_names: Vec<Atom> = var_decl
                .decls
                .iter()
                .filter_map(|d| {
                    if let Pat::Ident(bi) = &d.name {
                        Some(bi.id.sym.clone())
                    } else {
                        Option::None
                    }
                })
                .collect();
            let export_names: Vec<Atom> = decl_names
                .iter()
                .filter(|n| exported.contains(*n))
                .cloned()
                .collect();
            if export_names.is_empty() {
                return ExportPromotion::None;
            }
            if export_names.len() == decl_names.len() {
                // All declarators exported → `export const x = ..., y = ...`
                let new_item = ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                    span: Default::default(),
                    decl: Decl::Var(var_decl.clone()),
                }));
                ExportPromotion::Promoted(new_item, export_names)
            } else {
                // Partial — split without reordering initializer evaluation.
                let export_set: HashSet<&Atom> = export_names.iter().collect();
                let mut items = Vec::new();
                for decl in &var_decl.decls {
                    let is_exported =
                        matches!(&decl.name, Pat::Ident(bi) if export_set.contains(&bi.id.sym));
                    let mut split_decl = var_decl.clone();
                    split_decl.decls = vec![decl.clone()];
                    if is_exported {
                        items.push(ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                            span: Default::default(),
                            decl: Decl::Var(split_decl),
                        })));
                    } else {
                        items.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(split_decl))));
                    }
                }
                ExportPromotion::Split(items, export_names)
            }
        }
        _ => ExportPromotion::None,
    }
}

fn derive_chunk_name(items: &[TopLevelItem], cluster: &Cluster) -> String {
    // Use the first declared class name if there is one.
    for &i in &cluster.item_indices {
        if !items[i].declared_names.is_empty() {
            // Prefer classes — they're often the most meaningful name.
            let name = &items[i].declared_names[0];
            if name.len() > 1 {
                return format!("chunk_{name}");
            }
        }
    }
    // Fallback: first declared name.
    for &i in &cluster.item_indices {
        if !items[i].declared_names.is_empty() {
            return format!("chunk_{}", items[i].declared_names[0]);
        }
    }
    format!("chunk_{}", cluster.item_indices[0])
}

fn dedup_cluster_filename(filename: &str, seen: &mut HashSet<String>) -> String {
    dedup_filename(
        filename,
        seen,
        FilenameDedupStyle::PathAware {
            fallback_stem: "chunk",
        },
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "scope_hoist_tests.rs"]
mod tests;
