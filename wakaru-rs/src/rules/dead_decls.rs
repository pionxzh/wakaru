use std::collections::{HashMap, HashSet, VecDeque};

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    Decl, Expr, Ident, ImportDecl, MemberProp, Module, ModuleItem, Pat, PropName, Stmt,
    VarDecl,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitWith};

type BindingKey = (Atom, SyntaxContext);

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
