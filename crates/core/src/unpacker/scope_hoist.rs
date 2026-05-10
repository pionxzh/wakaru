use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, SourceMap, GLOBALS};
use swc_core::ecma::ast::*;
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::{UnpackResult, UnpackedModule};

const MIN_DECLARATIONS: usize = 10;

pub fn split_scope_hoisted(source: &str) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::parse_es_module(source, "bundle.js", cm.clone()).ok()?;
        split_from_module(&module, cm)
    })
}

fn split_from_module(module: &Module, cm: Lrc<SourceMap>) -> Option<UnpackResult> {
    // Unwrap IIFE wrapper if present: `(()=>{ ... })()` or `(function(){ ... })()`
    let iife_body = unwrap_iife(module);
    let body = iife_body.as_deref().unwrap_or(&module.body);

    // Phase 1: collect top-level items with metadata.
    let items = collect_top_level_items(body);

    let decl_count = items.iter().filter(|i| !i.declared_names.is_empty()).count();
    if decl_count < MIN_DECLARATIONS {
        return None;
    }

    // Phase 2: build reference graph.
    let graph = build_reference_graph(&items);

    // Phase 3: cluster via union-find.
    let mut uf = UnionFind::new(items.len());
    apply_merge_signals(&items, &graph, &mut uf);

    // Phase 4: extract clusters and identify entry.
    let clusters = extract_clusters(&items, &mut uf);
    if clusters.len() < 2 {
        return None;
    }

    // Phase 5: emit modules.
    let modules = emit_clusters(body, &items, clusters, cm);
    Some(UnpackResult { modules })
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
        let module = super::parse_es_module(source, "bundle.js", cm).ok().unwrap();
        let items = collect_top_level_items(&module.body);
        let graph = build_reference_graph(&items);
        let mut uf = UnionFind::new(items.len());
        apply_merge_signals(&items, &graph, &mut uf);
        let clusters = extract_clusters(&items, &mut uf);
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
    is_module_decl: bool,
}

fn collect_top_level_items(body: &[ModuleItem]) -> Vec<TopLevelItem> {
    body.iter()
        .map(|item| {
            let declared_names = item_declared_names(item);
            let referenced_names = item_referenced_names(item, &declared_names);
            let is_module_decl = matches!(item, ModuleItem::ModuleDecl(_));
            TopLevelItem {
                declared_names,
                referenced_names,
                is_module_decl,
            }
        })
        .collect()
}

fn item_declared_names(item: &ModuleItem) -> Vec<Atom> {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(f))) => vec![f.ident.sym.clone()],
        ModuleItem::Stmt(Stmt::Decl(Decl::Class(c))) => vec![c.ident.sym.clone()],
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => var
            .decls
            .iter()
            .filter_map(|d| {
                if let Pat::Ident(bi) = &d.name {
                    Some(bi.id.sym.clone())
                } else {
                    None
                }
            })
            .collect(),
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => match &export.decl {
            Decl::Fn(f) => vec![f.ident.sym.clone()],
            Decl::Class(c) => vec![c.ident.sym.clone()],
            Decl::Var(var) => var
                .decls
                .iter()
                .filter_map(|d| {
                    if let Pat::Ident(bi) = &d.name {
                        Some(bi.id.sym.clone())
                    } else {
                        None
                    }
                })
                .collect(),
            _ => vec![],
        },
        _ => vec![],
    }
}

fn item_referenced_names(item: &ModuleItem, own_names: &[Atom]) -> HashSet<Atom> {
    let own: HashSet<&Atom> = own_names.iter().collect();
    let mut collector = RefCollector {
        refs: HashSet::new(),
        own_names: &own,
        block_bindings: HashSet::new(),
        var_bindings: HashSet::new(),
    };
    item.visit_with(&mut collector);
    collector.refs
}

struct RefCollector<'a> {
    refs: HashSet<Atom>,
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
}

impl Visit for RefCollector<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if !self.own_names.contains(&ident.sym) && !self.is_local(&ident.sym) {
            self.refs.insert(ident.sym.clone());
        }
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
        stmt.left.visit_with(self);
        stmt.right.visit_with(self);
        stmt.body.visit_with(self);
        self.block_bindings = outer;
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        let outer = self.block_bindings.clone();
        stmt.left.visit_with(self);
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

    fn visit_prop(&mut self, prop: &Prop) {
        match prop {
            Prop::Shorthand(ident) => {
                self.refs.insert(ident.sym.clone());
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

// ---------------------------------------------------------------------------
// Phase 2: Reference graph
// ---------------------------------------------------------------------------

struct ReferenceGraph {
    references: Vec<HashSet<usize>>,
    referenced_by: Vec<HashSet<usize>>,
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

    for (idx, item) in items.iter().enumerate() {
        for ref_name in &item.referenced_names {
            if let Some(&target_idx) = name_to_item.get(ref_name) {
                if target_idx != idx {
                    references[idx].insert(target_idx);
                    referenced_by[target_idx].insert(idx);
                }
            }
        }
    }

    ReferenceGraph {
        references,
        referenced_by,
    }
}

// ---------------------------------------------------------------------------
// Phase 3: Clustering
// ---------------------------------------------------------------------------

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

#[derive(Debug)]
struct Cluster {
    item_indices: Vec<usize>,
    is_entry: bool,
}

fn extract_clusters(items: &[TopLevelItem], uf: &mut UnionFind) -> Vec<Cluster> {
    let min_cluster_decls = 2;

    // Group items by cluster root.
    let mut root_to_indices: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..items.len() {
        root_to_indices.entry(uf.find(i)).or_default().push(i);
    }

    let mut clusters: Vec<Cluster> = root_to_indices
        .into_values()
        .map(|mut indices| {
            indices.sort();
            Cluster {
                item_indices: indices,
                is_entry: false,
            }
        })
        .collect();
    clusters.sort_by_key(|c| c.item_indices[0]);

    // Classify: clusters with enough declarations are "module clusters".
    // Small clusters (< min_cluster_decls declarations) and clusters with
    // ModuleDecl items get folded into the entry.
    let mut entry_indices: Vec<usize> = Vec::new();
    let mut module_clusters: Vec<Cluster> = Vec::new();

    for cluster in clusters {
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
            entry_indices.extend(cluster.item_indices);
        } else {
            module_clusters.push(cluster);
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

// ---------------------------------------------------------------------------
// Phase 5: Emit modules
// ---------------------------------------------------------------------------

fn emit_clusters(
    body: &[ModuleItem],
    items: &[TopLevelItem],
    clusters: Vec<Cluster>,
    cm: Lrc<SourceMap>,
) -> Vec<UnpackedModule> {
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

    // Assign filenames first so we can reference them in imports.
    let filenames: Vec<String> = clusters
        .iter()
        .map(|c| {
            if c.is_entry {
                "entry.js".to_string()
            } else {
                let name = derive_chunk_name(items, c);
                format!("{name}.js")
            }
        })
        .collect();

    let mut modules = Vec::new();

    for (ci, cluster) in clusters.iter().enumerate() {
        let mut module_items: Vec<ModuleItem> = Vec::new();

        // Synthesize imports: for each other cluster that declares names
        // this cluster references, emit `import { ... } from './chunk.js'`.
        for (oi, other_decls) in cluster_declared.iter().enumerate() {
            if oi == ci {
                continue;
            }
            let mut needed: Vec<&Atom> = cluster_referenced[ci]
                .iter()
                .filter(|name| other_decls.contains(*name))
                .collect();
            if needed.is_empty() {
                continue;
            }
            needed.sort();
            module_items.push(make_import_stmt(&needed, &filenames[oi]));
        }

        // Collect which names this cluster should export.
        let mut exported: HashSet<Atom> = HashSet::new();
        for (oi, other_refs) in cluster_referenced.iter().enumerate() {
            if oi == ci {
                continue;
            }
            for name in &cluster_declared[ci] {
                if other_refs.contains(name) {
                    exported.insert(name.clone());
                }
            }
        }

        // Original body items, with exported declarations promoted to
        // `export function ...` / `export const ...` / `export class ...`.
        let mut leftover_exports: Vec<Atom> = Vec::new();
        for &i in &cluster.item_indices {
            let item = &body[i];
            if exported.is_empty() {
                module_items.push(item.clone());
                continue;
            }
            match try_promote_export(item, &exported) {
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
                    module_items.push(item.clone());
                }
            }
        }
        // Any names that couldn't be promoted inline get a trailing export.
        leftover_exports.extend(exported.iter().cloned());
        if !leftover_exports.is_empty() {
            leftover_exports.sort();
            let refs: Vec<&Atom> = leftover_exports.iter().collect();
            module_items.push(make_export_stmt(&refs));
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
        });
    }

    modules
}

enum ExportPromotion {
    Promoted(ModuleItem, Vec<Atom>),
    Split(Vec<ModuleItem>, Vec<Atom>),
    None,
}

fn try_promote_export(item: &ModuleItem, exported: &HashSet<Atom>) -> ExportPromotion {
    match item {
        // `function foo() {}` → `export function foo() {}`
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) if exported.contains(&fn_decl.ident.sym) => {
            let names = vec![fn_decl.ident.sym.clone()];
            let new_item = ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                span: Default::default(),
                decl: Decl::Fn(fn_decl.clone()),
            }));
            ExportPromotion::Promoted(new_item, names)
        }
        // `class Foo {}` → `export class Foo {}`
        ModuleItem::Stmt(Stmt::Decl(Decl::Class(class_decl)))
            if exported.contains(&class_decl.ident.sym) =>
        {
            let names = vec![class_decl.ident.sym.clone()];
            let new_item = ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                span: Default::default(),
                decl: Decl::Class(class_decl.clone()),
            }));
            ExportPromotion::Promoted(new_item, names)
        }
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

fn make_import_stmt(names: &[&Atom], from: &str) -> ModuleItem {
    use swc_core::ecma::ast::{ImportDecl, ImportNamedSpecifier, ImportSpecifier, Str};
    let specifiers = names
        .iter()
        .map(|name| {
            ImportSpecifier::Named(ImportNamedSpecifier {
                span: Default::default(),
                local: Ident::new((*name).clone(), Default::default(), Default::default()),
                imported: None,
                is_type_only: false,
            })
        })
        .collect();
    ModuleItem::ModuleDecl(ModuleDecl::Import(ImportDecl {
        span: Default::default(),
        specifiers,
        src: Box::new(Str {
            span: Default::default(),
            value: format!("./{from}").into(),
            raw: None,
        }),
        type_only: false,
        with: None,
        phase: Default::default(),
    }))
}

fn make_export_stmt(names: &[&Atom]) -> ModuleItem {
    use swc_core::ecma::ast::{ExportNamedSpecifier, ExportSpecifier, ModuleExportName, NamedExport};
    let specifiers = names
        .iter()
        .map(|name| {
            ExportSpecifier::Named(ExportNamedSpecifier {
                span: Default::default(),
                orig: ModuleExportName::Ident(Ident::new(
                    (*name).clone(),
                    Default::default(),
                    Default::default(),
                )),
                exported: None,
                is_type_only: false,
            })
        })
        .collect();
    ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
        span: Default::default(),
        specifiers,
        src: None,
        type_only: false,
        with: None,
    }))
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

fn emit_items(items: Vec<ModuleItem>, filename: String, cm: Lrc<SourceMap>) -> String {
    let module = Module {
        span: Default::default(),
        body: items,
        shebang: None,
    };
    let _fm = cm.new_source_file(FileName::Custom(filename).into(), String::new());
    emit_module_raw(&module, cm).unwrap_or_default()
}

fn emit_module_raw(module: &Module, cm: Lrc<SourceMap>) -> anyhow::Result<String> {
    let mut output = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: Config::default().with_minify(false),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm.clone(), "\n", &mut output, None),
        };
        emitter.emit_module(module)?;
    }
    String::from_utf8(output).map_err(|e| anyhow::anyhow!("{e}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn split(source: &str) -> Option<Vec<(String, String, bool)>> {
        let result = split_scope_hoisted(source)?;
        Some(
            result
                .modules
                .into_iter()
                .map(|m| (m.filename, m.code, m.is_entry))
                .collect(),
        )
    }

    fn count_modules(source: &str) -> usize {
        split(source).map(|m| m.len()).unwrap_or(0)
    }

    fn two_group_fixture(b1: &str) -> String {
        [
            r#"
            function a1() { return 1; }
            function a2() { return a1() + 1; }
            function a3() { return a2() * 2; }
            function a4() { return a3() + 3; }
            function a5() { return a4() - 1; }
            "#,
            b1,
            r#"
            function b2() { return b1() + 10; }
            function b3() { return b2() * 20; }
            function b4() { return b3() + 30; }
            function b5() { return b4() - 10; }

            const k = a5() + b5();
            console.log(k);
            "#,
        ]
        .join("\n")
    }

    fn assert_splits(source: &str, reason: &str) {
        let n = count_modules(source);
        assert!(n >= 2, "{reason}, got {n} modules");
    }

    fn assert_does_not_split(source: &str, reason: &str) {
        let n = count_modules(source);
        assert!(n < 2, "{reason}, got {n} modules");
    }

    #[test]
    fn too_few_declarations_returns_none() {
        let input = r#"
            function a() { return 1; }
            function b() { return a(); }
            const c = 3;
        "#;
        assert!(split(input).is_none());
    }

    #[test]
    fn splits_independent_groups() {
        // Two clearly independent groups of functions + an entry using both.
        let input = r#"
            function helperA1() { return 1; }
            function helperA2() { return helperA1() + 1; }
            function helperA3() { return helperA2() * 2; }
            function publicA() { return helperA3(); }

            function helperB1() { return 10; }
            function helperB2() { return helperB1() + 10; }
            function helperB3() { return helperB2() * 20; }
            function publicB() { return helperB3(); }

            const x = publicA();
            const y = publicB();
            console.log(x, y);
        "#;
        let n = count_modules(input);
        assert!(n >= 2, "expected at least 2 modules, got {n}");
    }

    #[test]
    fn entry_gets_module_decls() {
        let input = r#"
            function helperA1() { return 1; }
            function helperA2() { return helperA1() + 1; }
            function helperA3() { return helperA2() * 2; }
            function helperA4() { return helperA3() + 5; }
            function publicA() { return helperA4(); }

            function helperB1() { return 10; }
            function helperB2() { return helperB1() + 10; }
            function helperB3() { return helperB2() * 20; }
            function helperB4() { return helperB3() + 50; }
            function publicB() { return helperB4(); }

            const result = publicA() + publicB();
            export { result };
        "#;
        let modules = split(input).expect("should split");
        let entry = modules.iter().find(|(_, _, is_entry)| *is_entry);
        assert!(entry.is_some(), "should have an entry module");
        let (filename, code, _) = entry.unwrap();
        assert_eq!(filename, "entry.js");
        assert!(
            code.contains("export"),
            "entry should contain export statement"
        );
    }

    #[test]
    fn class_with_private_helpers_stays_together() {
        // A class with WeakMap helpers should cluster together.
        let input = r#"
            function utilA() { return 1; }
            function utilB() { return utilA() + 2; }
            function utilC() { return utilB() + 3; }
            function utilD() { return utilC() * 2; }
            function utilE() { return utilD() - 1; }
            function utilF() { return utilE() + 7; }

            const _data = new WeakMap();
            const _listeners = new WeakMap();
            class Store {
                constructor(initial) {
                    _data.set(this, initial);
                    _listeners.set(this, []);
                }
                get(key) { return _data.get(this)[key]; }
                set(key, value) {
                    _data.get(this)[key] = value;
                    for (const fn1 of _listeners.get(this)) fn1(key, value);
                }
            }

            const s = new Store({});
            s.set("x", utilF());
            console.log(s.get("x"));
        "#;
        let modules = split(input).expect("should split");

        // Find the module containing Store.
        let store_module = modules
            .iter()
            .find(|(_, code, _)| code.contains("class Store"));
        assert!(store_module.is_some(), "should have a Store module");
        let (_, code, _) = store_module.unwrap();
        assert!(
            code.contains("_data") && code.contains("_listeners"),
            "WeakMap helpers should be in the same module as Store"
        );
    }

    #[test]
    fn vite_fixture_clusters() {
        let input = include_str!("../../tests/bundles/vite-gen/dist/es/bundle.mjs");
        let clusters = debug_clusters(input);
        let module_count = clusters.iter().filter(|(_, e)| !e).count();
        assert!(
            module_count >= 3,
            "expected at least 3 module clusters from vite fixture, got {module_count}"
        );

        // The algorithm should identify at least these modules:
        // - Logger module (LogLevel + Logger class)
        // - Store module (_data, _subs, CHANGE, RESET, Store)
        // - API module (BASE_URL, request, getUser, getPosts)
        let has_logger = clusters.iter().any(|(names, _)| {
            names.contains(&"LogLevel".to_string()) && names.contains(&"Logger".to_string())
        });
        let has_store = clusters
            .iter()
            .any(|(names, _)| names.contains(&"Store".to_string()));
        let has_api = clusters.iter().any(|(names, _)| {
            names.contains(&"BASE_URL".to_string()) && names.contains(&"request".to_string())
        });

        assert!(has_logger, "should cluster Logger module");
        assert!(has_store, "should cluster Store module");
        assert!(has_api, "should cluster API module");
    }

    #[test]
    fn vite_fixture_import_export() {
        let input = include_str!("../../tests/bundles/vite-gen/dist/es/bundle.mjs");
        let modules = split(input).expect("should split vite fixture");

        // Every non-entry chunk should have an export statement.
        for (filename, code, is_entry) in &modules {
            if *is_entry {
                continue;
            }
            assert!(
                code.contains("export"),
                "{filename} should have export statement"
            );
        }

        // Entry should import from the chunks it references.
        let entry = modules
            .iter()
            .find(|(_, _, is_entry)| *is_entry)
            .expect("should have entry");
        assert!(
            entry.1.contains("import"),
            "entry should have import statements"
        );
        assert!(
            entry
                .1
                .contains("import { getPosts, getUser } from \"./chunk_BASE_URL.js\";"),
            "entry imports from API chunk should be sorted, got:\n{}",
            entry.1
        );
        assert!(
            entry
                .1
                .contains("import { LogLevel, Logger } from \"./chunk_LogLevel.js\";"),
            "entry imports from Logger chunk should be sorted, got:\n{}",
            entry.1
        );
    }

    #[test]
    fn partial_var_export_preserves_declarator_order() {
        let input = r#"
            function a1() { return 1; }
            function a2() { return a1() + 1; }
            function a3() { return a2() + 1; }
            function a4() { return a3() + 1; }
            const exported = mark("exported"), kept = mark("kept");

            function b1() { return 10; }
            function b2() { return b1() + 1; }
            function b3() { return b2() + 1; }
            function b4() { return b3() + 1; }
            function b5() { return b4() + exported; }
            console.log(b5());
        "#;

        let modules = split(input).expect("should split");
        let entry = modules
            .iter()
            .find(|(_, _, is_entry)| *is_entry)
            .expect("should have entry");
        let exported_pos = entry
            .1
            .find("export const exported = mark(\"exported\");")
            .expect("should export the referenced declarator inline");
        let kept_pos = entry
            .1
            .find("const kept = mark(\"kept\");")
            .expect("should keep the unreferenced declarator");
        assert!(
            exported_pos < kept_pos,
            "partial var export should preserve declarator order, got:\n{}",
            entry.1
        );
    }

    #[test]
    fn vite_fixture_minified_clusters() {
        let input = include_str!("../../tests/bundles/vite-gen/dist/es-min/bundle.mjs");
        let clusters = debug_clusters(input);
        let module_count = clusters.iter().filter(|(_, e)| !e).count();
        assert!(
            module_count >= 3,
            "expected at least 3 module clusters from minified vite fixture, got {module_count}"
        );
    }

    #[test]
    fn minified_names_still_split() {
        let input = r#"
            function a() { return 1; }
            function b() { return a() + 1; }
            function c() { return b() * 2; }
            function d() { return c() + 3; }
            function e() { return d() - 1; }

            function f() { return 10; }
            function g() { return f() + 10; }
            function h() { return g() * 20; }
            function i() { return h() + 30; }
            function j() { return i() - 10; }

            const k = d() + j();
            console.log(k);
        "#;
        let n = count_modules(input);
        assert!(n >= 2, "expected at least 2 modules with minified names, got {n}");
    }

    #[test]
    fn local_shadows_do_not_create_false_refs() {
        for (name, b1) in [
            (
                "local const shadow",
                "function b1() { const a5 = 10; return a5; }",
            ),
            (
                "nested function declaration shadow",
                "function b1() { function a5() { return 10; } return a5(); }",
            ),
            (
                "destructuring shadow",
                "function b1(o) { const { a5 } = o; return a5; }",
            ),
        ] {
            let input = two_group_fixture(b1);
            assert_splits(&input, &format!("{name} should not merge groups"));
        }
    }

    #[test]
    fn block_scoped_bindings_do_not_suppress_outer_refs() {
        for (name, b1) in [
            (
                "if-block const",
                "function b1(flag) { if (flag) { const a5 = 10; } return a5(); }",
            ),
            (
                "for-loop let",
                "function b1() { for (let a5 = 0; a5 < 3; a5++) {} return a5(); }",
            ),
        ] {
            let input = two_group_fixture(b1);
            assert_does_not_split(&input, &format!("{name} should leave later a5() as a top-level ref"));
        }
    }

    #[test]
    fn var_in_block_survives_block_restore() {
        let input = two_group_fixture(
            r#"
            function b1(flag) { if (flag) { var a5 = function(){ return 10; }; } return a5(); }
            "#,
        );
        assert_splits(
            &input,
            "var in block should shadow at function scope after block exit",
        );
    }

    #[test]
    fn binding_pattern_defaults_reference_top_level() {
        for (name, b1) in [
            (
                "parameter default",
                "function b1(x = a5()) { return x; }",
            ),
            (
                "destructured parameter default",
                "function b1({x = a5()} = {}) { return x; }",
            ),
            (
                "object binding pattern default",
                "function b1(o) { const {x = a5()} = o; return x; }",
            ),
            (
                "array binding pattern default",
                "function b1(arr) { const [x = a5()] = arr; return x; }",
            ),
        ] {
            let input = two_group_fixture(b1);
            assert_does_not_split(&input, &format!("{name} should detect top-level a5 ref"));
        }
    }

    #[test]
    fn iife_trailing_statements_preserved() {
        // Trailing statements after the IIFE should end up in the output.
        let input = r#"(function() {
            function a1() { return 1; }
            function a2() { return a1() + 1; }
            function a3() { return a2() * 2; }
            function a4() { return a3() + 3; }
            function a5() { return a4() - 1; }

            function b1() { return 10; }
            function b2() { return b1() + 10; }
            function b3() { return b2() * 20; }
            function b4() { return b3() + 30; }
            function b5() { return b4() - 10; }

            var result = a5() + b5();
        })();
        console.log("after");
        "#;
        let modules = split(input).expect("should split IIFE bundle");
        let all_code: String = modules.iter().map(|(_, code, _)| code.as_str()).collect();
        assert!(
            all_code.contains("after"),
            "trailing statement after IIFE should be preserved"
        );
    }
}
