use std::collections::{HashMap, HashSet, VecDeque};

use swc_core::ecma::ast::{
    ArrowExpr, BlockStmt, BlockStmtOrExpr, Class, Decl, Expr, Function, Ident, ImportDecl, Lit,
    MemberProp, Module, ModuleItem, Pat, PropName, Stmt, VarDecl, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::binding_facts::collect_binding_facts;
use super::decl_utils::{binding_id, BindingId};
use super::eval_utils::{direct_eval_call_source, js_source_mentions_binding, EvalCallSource};
use super::helper_matcher::BindingKey;
use crate::utils::paren::strip_parens;

pub struct DeadDecls;

impl VisitMut for DeadDecls {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let candidates = collect_removable_bindings(module);
        if candidates.is_empty() {
            return;
        }

        let alive = compute_alive(module, &candidates);

        let dead: HashSet<BindingKey> = candidates
            .into_iter()
            .filter(|key| !alive.contains(key))
            .collect();

        if dead.is_empty() {
            return;
        }

        remove_dead(module, &dead);
    }
}

pub struct DeadUninitializedDecls;

impl VisitMut for DeadUninitializedDecls {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let facts = collect_binding_facts(module);
        let eval_protected = collect_eval_protected_uninitialized(module);
        let mut candidates = facts.uninitialized;
        candidates.extend(collect_local_undefined_initialized(module));
        let unused_uninitialized = candidates
            .into_iter()
            .filter(|binding| !eval_protected.contains(binding))
            .filter(|binding| facts.references.get(binding).copied().unwrap_or(0) <= 1)
            .collect::<HashSet<_>>();
        if unused_uninitialized.is_empty() {
            return;
        }

        module.visit_mut_with(&mut UninitializedDeclStripper {
            dead: &unused_uninitialized,
        });
    }
}

fn collect_local_undefined_initialized(module: &Module) -> HashSet<BindingId> {
    let mut collector = LocalUndefinedInitCollector::default();
    module.visit_with(&mut collector);
    collector.bindings
}

#[derive(Default)]
struct LocalUndefinedInitCollector {
    bindings: HashSet<BindingId>,
    function_depth: usize,
}

impl Visit for LocalUndefinedInitCollector {
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

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if self.function_depth > 0
            && declarator
                .init
                .as_deref()
                .is_some_and(is_undefined_initializer)
        {
            if let Pat::Ident(binding) = &declarator.name {
                self.bindings.insert(binding_id(&binding.id));
            }
        }
        declarator.visit_children_with(self);
    }
}

fn collect_eval_protected_uninitialized(module: &Module) -> HashSet<BindingId> {
    let mut collector = EvalProtectedUninitializedCollector::default();
    collector.visit_module_scope(module);
    collector
        .protected
        .extend(collect_global_enumerated_uninitialized(module));
    collector.protected
}

fn collect_global_enumerated_uninitialized(module: &Module) -> HashSet<BindingId> {
    let uninitialized = collect_current_scope_uninitialized_from_module(module);
    if uninitialized.is_empty() {
        return HashSet::new();
    }

    let mut observer = GlobalForInObserver::default();
    module.visit_with(&mut observer);
    if observer.found {
        uninitialized
    } else {
        HashSet::new()
    }
}

#[derive(Default)]
struct GlobalForInObserver {
    found: bool,
    function_depth: usize,
}

impl GlobalForInObserver {
    fn is_global_object_expr(&self, expr: &Expr) -> bool {
        match strip_parens(expr) {
            Expr::Ident(id) if matches!(id.sym.as_ref(), "globalThis" | "window" | "self") => true,
            Expr::This(_) if self.function_depth == 0 => true,
            _ => false,
        }
    }
}

impl Visit for GlobalForInObserver {
    fn visit_for_in_stmt(&mut self, stmt: &swc_core::ecma::ast::ForInStmt) {
        stmt.left.visit_with(self);
        stmt.right.visit_with(self);
        if self.is_global_object_expr(&stmt.right) {
            self.found = true;
        }
        stmt.body.visit_with(self);
    }

    fn visit_function(&mut self, function: &Function) {
        self.function_depth += 1;
        function.visit_children_with(self);
        self.function_depth -= 1;
    }
}

#[derive(Default)]
struct EvalProtectedUninitializedCollector {
    scope_stack: Vec<HashSet<BindingId>>,
    protected: HashSet<BindingId>,
}

impl EvalProtectedUninitializedCollector {
    fn visit_module_scope(&mut self, module: &Module) {
        self.with_scope(
            collect_current_scope_uninitialized_from_module(module),
            |this| {
                module.visit_children_with(this);
            },
        );
    }

    fn with_scope(&mut self, uninitialized: HashSet<BindingId>, visit: impl FnOnce(&mut Self)) {
        self.scope_stack.push(uninitialized);
        visit(self);
        self.scope_stack.pop();
    }

    fn protect_unknown_eval(&mut self) {
        for scope in &self.scope_stack {
            self.protected.extend(scope.iter().cloned());
        }
    }

    fn protect_known_eval_source(&mut self, source: &str) {
        for scope in &self.scope_stack {
            self.protected.extend(
                scope
                    .iter()
                    .filter(|binding| js_source_mentions_binding(source, &binding.0))
                    .cloned(),
            );
        }
    }
}

impl Visit for EvalProtectedUninitializedCollector {
    fn visit_function(&mut self, function: &Function) {
        self.with_scope(
            collect_current_scope_uninitialized_from_function(function),
            |this| {
                function.visit_children_with(this);
            },
        );
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.with_scope(
            collect_current_scope_uninitialized_from_arrow(arrow),
            |this| {
                arrow.visit_children_with(this);
            },
        );
    }

    fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
        if let Some(source) = direct_eval_call_source(call) {
            match source {
                EvalCallSource::NoSource => {}
                EvalCallSource::Known(source) => self.protect_known_eval_source(&source),
                EvalCallSource::Unknown => self.protect_unknown_eval(),
            }
            for arg in &call.args {
                arg.expr.visit_with(self);
            }
            return;
        }

        call.visit_children_with(self);
    }
}

#[derive(Default)]
struct CurrentScopeUninitializedCollector {
    uninitialized: HashSet<BindingId>,
    include_undefined_init: bool,
}

impl Visit for CurrentScopeUninitializedCollector {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if declarator.init.is_none()
            || (self.include_undefined_init
                && declarator
                    .init
                    .as_deref()
                    .is_some_and(is_undefined_initializer))
        {
            if let Pat::Ident(binding) = &declarator.name {
                self.uninitialized.insert(binding_id(&binding.id));
            }
        }
        declarator.visit_children_with(self);
    }

    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

    fn visit_class(&mut self, _: &Class) {}
}

fn collect_current_scope_uninitialized_from_module(module: &Module) -> HashSet<BindingId> {
    let mut collector = CurrentScopeUninitializedCollector::default();
    for item in &module.body {
        item.visit_with(&mut collector);
    }
    collector.uninitialized
}

fn collect_current_scope_uninitialized_from_function(function: &Function) -> HashSet<BindingId> {
    let mut collector = CurrentScopeUninitializedCollector {
        include_undefined_init: true,
        ..Default::default()
    };
    if let Some(body) = &function.body {
        collect_current_scope_uninitialized_from_block(body, &mut collector);
    }
    collector.uninitialized
}

fn collect_current_scope_uninitialized_from_arrow(arrow: &ArrowExpr) -> HashSet<BindingId> {
    let mut collector = CurrentScopeUninitializedCollector {
        include_undefined_init: true,
        ..Default::default()
    };
    match arrow.body.as_ref() {
        BlockStmtOrExpr::BlockStmt(body) => {
            collect_current_scope_uninitialized_from_block(body, &mut collector);
        }
        BlockStmtOrExpr::Expr(expr) => {
            expr.visit_with(&mut collector);
        }
    }
    collector.uninitialized
}

fn collect_current_scope_uninitialized_from_block(
    block: &BlockStmt,
    collector: &mut CurrentScopeUninitializedCollector,
) {
    for stmt in &block.stmts {
        stmt.visit_with(collector);
    }
}

fn compute_alive(module: &Module, candidates: &HashSet<BindingKey>) -> HashSet<BindingKey> {
    let mut edges: HashMap<BindingKey, HashSet<BindingKey>> = HashMap::new();
    let mut roots: HashSet<BindingKey> = HashSet::new();

    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
                let key = (fn_decl.ident.sym.clone(), fn_decl.ident.ctxt);
                if candidates.contains(&key) {
                    let refs = collect_refs_in_node(fn_decl, candidates, &key);
                    edges.entry(key).or_default().extend(refs);
                    continue;
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
                let mut all_candidates = true;
                for decl in &var_decl.decls {
                    if let Pat::Ident(ident) = &decl.name {
                        let key = (ident.sym.clone(), ident.ctxt);
                        if candidates.contains(&key) {
                            let refs = collect_refs_in_node(decl, candidates, &key);
                            edges.entry(key).or_default().extend(refs);
                            continue;
                        }
                    }
                    all_candidates = false;
                }
                if all_candidates {
                    continue;
                }
            }
            _ => {}
        }

        let mut collector = RootRefCollector {
            candidates,
            found: HashSet::new(),
        };
        item.visit_with(&mut collector);
        roots.extend(collector.found);
    }

    let mut alive: HashSet<BindingKey> = roots;
    let mut queue: VecDeque<BindingKey> = alive.iter().cloned().collect();

    while let Some(key) = queue.pop_front() {
        if let Some(deps) = edges.get(&key) {
            for dep in deps {
                if alive.insert(dep.clone()) {
                    queue.push_back(dep.clone());
                }
            }
        }
    }

    alive
}

fn collect_refs_in_node<'a, N: VisitWith<RootRefCollector<'a>>>(
    node: &N,
    candidates: &'a HashSet<BindingKey>,
    self_key: &BindingKey,
) -> HashSet<BindingKey> {
    let mut collector = RootRefCollector {
        candidates,
        found: HashSet::new(),
    };
    node.visit_with(&mut collector);
    collector.found.remove(self_key);
    collector.found
}

struct RootRefCollector<'a> {
    candidates: &'a HashSet<BindingKey>,
    found: HashSet<BindingKey>,
}

impl Visit for RootRefCollector<'_> {
    fn visit_import_decl(&mut self, _: &ImportDecl) {}

    fn visit_ident(&mut self, ident: &Ident) {
        let key = (ident.sym.clone(), ident.ctxt);
        if self.candidates.contains(&key) {
            self.found.insert(key);
        }
    }

    fn visit_prop_name(&mut self, prop: &PropName) {
        if let PropName::Computed(c) = prop {
            c.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }
}

fn is_helper_init(expr: &Expr) -> bool {
    matches!(expr, Expr::Fn(_) | Expr::Arrow(_))
}

fn collect_removable_bindings(module: &Module) -> HashSet<BindingKey> {
    let mut bindings = HashSet::new();
    let mut poisoned = HashSet::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
                let key = (fn_decl.ident.sym.clone(), fn_decl.ident.ctxt);
                if !poisoned.contains(&key) {
                    bindings.insert(key);
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
                for decl in &var_decl.decls {
                    let Pat::Ident(ident) = &decl.name else {
                        continue;
                    };
                    let key = (ident.sym.clone(), ident.ctxt);
                    let is_helper = match &decl.init {
                        Some(init) => is_helper_init(init),
                        None => false,
                    };
                    if is_helper {
                        if !poisoned.contains(&key) {
                            bindings.insert(key);
                        }
                    } else {
                        bindings.remove(&key);
                        poisoned.insert(key);
                    }
                }
            }
            _ => {}
        }
    }
    bindings
}

fn remove_dead(module: &mut Module, dead: &HashSet<BindingKey>) {
    module.body.retain_mut(|item| match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
            let key = (fn_decl.ident.sym.clone(), fn_decl.ident.ctxt);
            !dead.contains(&key)
        }
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
            strip_dead_declarators(var_decl, dead);
            !var_decl.decls.is_empty()
        }
        _ => true,
    });
}

fn strip_dead_declarators(var_decl: &mut VarDecl, dead: &HashSet<BindingKey>) {
    var_decl.decls.retain(|decl| {
        let Pat::Ident(ident) = &decl.name else {
            return true;
        };
        let is_helper = match &decl.init {
            Some(init) => is_helper_init(init),
            None => false,
        };
        if !is_helper {
            return true;
        }
        let key = (ident.sym.clone(), ident.ctxt);
        !dead.contains(&key)
    });
}

struct UninitializedDeclStripper<'a> {
    dead: &'a HashSet<BindingId>,
}

impl VisitMut for UninitializedDeclStripper<'_> {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        items.retain_mut(|item| match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                strip_unused_uninitialized_declarators(var, self.dead);
                !var.decls.is_empty()
            }
            _ => true,
        });
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        stmts.retain_mut(|stmt| match stmt {
            Stmt::Decl(Decl::Var(var)) => {
                strip_unused_uninitialized_declarators(var, self.dead);
                !var.decls.is_empty()
            }
            _ => true,
        });
    }
}

fn strip_unused_uninitialized_declarators(var_decl: &mut VarDecl, dead: &HashSet<BindingId>) {
    var_decl.decls.retain(|decl| {
        if decl.init.is_some() && !decl.init.as_deref().is_some_and(is_undefined_initializer) {
            return true;
        }
        let Pat::Ident(ident) = &decl.name else {
            return true;
        };
        !dead.contains(&binding_id(&ident.id))
    });
}

fn is_undefined_initializer(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Ident(id) if id.sym.as_ref() == "undefined")
        || matches!(
            strip_parens(expr),
            Expr::Unary(unary)
                if unary.op == swc_core::ecma::ast::UnaryOp::Void
                    && matches!(strip_parens(&unary.arg), Expr::Lit(Lit::Num(_)))
        )
}
