use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap, Spanned, GLOBALS};
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

/// Selects how a completed scope-hoist plan is rendered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ScopeHoistRenderMode {
    /// Merge cyclic components before emitting the recovered ESM graph.
    #[default]
    Executable,
    /// Retain the finer planned clusters for static inspection.
    Inspect,
}

pub fn split_scope_hoisted(source: &str) -> Option<UnpackResult> {
    split_scope_hoisted_with_mode(source, ScopeHoistRenderMode::Executable)
}

pub(crate) fn split_scope_hoisted_with_mode(
    source: &str,
    render_mode: ScopeHoistRenderMode,
) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::parse_es_module(source, "bundle.js", cm.clone()).ok()?;
        split_from_module(&module, cm, render_mode)
    })
}

pub(crate) fn split_scope_hoisted_module_with_mode(
    module: &Module,
    cm: Lrc<SourceMap>,
    render_mode: ScopeHoistRenderMode,
) -> Option<UnpackResult> {
    split_from_module(module, cm, render_mode)
}

fn split_from_module(
    module: &Module,
    cm: Lrc<SourceMap>,
    render_mode: ScopeHoistRenderMode,
) -> Option<UnpackResult> {
    // Unwrap IIFE wrapper if present: `(()=>{ ... })()` or `(function(){ ... })()`
    let iife_body = unwrap_iife(module);
    let body = iife_body.as_deref().unwrap_or(&module.body);

    let plan = analyze_scope_hoist(body)?;
    render_scope_hoist_plan(body, plan, cm, render_mode)
}

struct ScopeHoistPlan {
    items: Vec<TopLevelItem>,
    graph: ReferenceGraph,
    roots: Vec<Cluster>,
    clusters: Vec<Cluster>,
}

fn analyze_scope_hoist(body: &[ModuleItem]) -> Option<ScopeHoistPlan> {
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
    merge_cross_item_writes(&graph, &mut uf);

    // Phase 4: extract the finest useful clusters and identify the entry.
    let roots = extract_root_clusters(&items, &mut uf);
    let clusters = extract_inspection_clusters(&items, &roots);
    (clusters.len() >= 2).then_some(ScopeHoistPlan {
        items,
        graph,
        roots,
        clusters,
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
    } = plan;
    let clusters = match render_mode {
        ScopeHoistRenderMode::Executable => {
            let clusters = if has_pathological_entry_scc(&clusters, &graph) {
                extract_executable_clusters(&items, &graph, roots)
            } else {
                clusters
            };
            merge_cyclic_clusters(clusters, &graph)
        }
        ScopeHoistRenderMode::Inspect => clusters,
    };
    if clusters.len() < 2 {
        return None;
    }

    // Phase 5: emit modules.
    let modules = emit_clusters(body, &items, clusters, cm);
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

fn emit_clusters(
    body: &[ModuleItem],
    items: &[TopLevelItem],
    clusters: Vec<Cluster>,
    cm: Lrc<SourceMap>,
) -> Vec<UnpackedModule> {
    let dynamic_require_helpers = collect_dynamic_require_helpers(body);
    let esbuild_to_esm_helpers = collect_esbuild_to_esm_helpers(body);
    let import_decls = collect_import_decls(body);

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

        // Synthesize imports: for each other cluster that declares names
        // this cluster references, emit `import { ... } from './chunk.js'`.
        for (oi, other_decls) in cluster_declared.iter().enumerate() {
            if oi == ci {
                continue;
            }
            let mut needed: Vec<Atom> = cluster_referenced[ci]
                .iter()
                .filter(|name| !dynamic_require_helpers.contains(*name))
                .filter(|name| !esbuild_to_esm_helpers.contains(*name))
                .filter(|name| other_decls.contains(*name))
                .cloned()
                .collect();
            if needed.is_empty() {
                continue;
            }
            needed.sort();
            module_items.push(make_named_import_stmt(&needed, &filenames[oi]));
        }

        // Collect which names this cluster should export.
        let mut exported: HashSet<Atom> = HashSet::new();
        for (oi, other_refs) in cluster_referenced.iter().enumerate() {
            if oi == ci {
                continue;
            }
            for name in &cluster_declared[ci] {
                if !dynamic_require_helpers.contains(name)
                    && !esbuild_to_esm_helpers.contains(name)
                    && other_refs.contains(name)
                {
                    exported.insert(name.clone());
                }
            }
        }

        // Original body items, with exported declarations promoted to
        // `export function ...` / `export const ...` / `export class ...`.
        let mut leftover_exports: Vec<Atom> = Vec::new();
        let should_rewrite_dynamic_require = !dynamic_require_helpers.is_empty()
            && !cluster_declared[ci]
                .iter()
                .any(|name| dynamic_require_helpers.contains(name));
        let should_rewrite_esbuild_to_esm = !esbuild_to_esm_helpers.is_empty();
        let mut default_interop_bindings = HashSet::new();
        for &i in &cluster.item_indices {
            let mut item = body[i].clone();
            if should_rewrite_dynamic_require {
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
