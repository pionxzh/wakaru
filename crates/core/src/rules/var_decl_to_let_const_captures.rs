//! Same-scope capture safety without recognizing invocation syntax.
//!
//! A function value reference may execute or escape. Follow references between
//! named declarations and check their captured vars at the earliest exposure.
//! Closures/classes are conservatively exposed at creation. Declaration order
//! is proved only within a containing statement-list block, not across branches.
//! Existing block-escape and loop-capture guards still apply. Export entry from
//! another module is deliberately left to the rule's existing level policy.

use super::decl_utils::BindingId;
use super::var_decl_to_let_const::collect_binding_ids_from_pat;
use crate::collections::{HashMap, HashSet};
use crate::utils::paren::strip_parens;
use swc_core::ecma::ast::*;
use swc_core::ecma::visit::{Visit, VisitWith};

#[derive(Default)]
struct Uses {
    vars: HashSet<BindingId>,
    functions: HashSet<BindingId>,
}

struct References<'a> {
    vars: &'a HashSet<BindingId>,
    functions: &'a HashMap<BindingId, usize>,
    uses: Uses,
    deferred_depth: usize,
}

impl Visit for References<'_> {
    fn visit_ident(&mut self, id: &Ident) {
        let id = id.to_id();
        if self.functions.contains_key(&id) {
            self.uses.functions.insert(id.clone());
        }
        if self.deferred_depth > 0 && self.vars.contains(&id) {
            self.uses.vars.insert(id);
        }
    }

    fn visit_function(&mut self, node: &Function) {
        self.deferred_depth += 1;
        node.visit_children_with(self);
        self.deferred_depth -= 1;
    }

    fn visit_arrow_expr(&mut self, node: &ArrowExpr) {
        self.deferred_depth += 1;
        node.visit_children_with(self);
        self.deferred_depth -= 1;
    }

    fn visit_class(&mut self, node: &Class) {
        // Include eager computed keys/static work as well as deferred members.
        self.deferred_depth += 1;
        node.visit_children_with(self);
        self.deferred_depth -= 1;
    }
}

#[derive(Default)]
struct Functions(HashMap<BindingId, usize>);

impl Functions {
    fn insert(&mut self, id: &Ident) {
        let index = self.0.len();
        self.0.entry(id.to_id()).or_insert(index);
    }
}

impl Visit for Functions {
    fn visit_fn_decl(&mut self, node: &FnDecl) {
        self.insert(&node.ident);
    }

    fn visit_export_default_decl(&mut self, node: &ExportDefaultDecl) {
        if let DefaultDecl::Fn(function) = &node.decl {
            if let Some(id) = &function.ident {
                self.insert(id);
            }
        }
    }

    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
}

pub(super) fn module(items: &[ModuleItem], vars: &HashSet<BindingId>) -> HashSet<BindingId> {
    let mut functions = Functions::default();
    items.visit_with(&mut functions);
    let mut analysis = Analysis::new(vars, &functions.0);
    for item in items {
        analysis.direct_stmt = true;
        item.visit_with(&mut analysis);
    }
    analysis.finish()
}

pub(super) fn stmts(stmts: &[Stmt], vars: &HashSet<BindingId>) -> HashSet<BindingId> {
    let mut functions = Functions::default();
    stmts.visit_with(&mut functions);
    let mut analysis = Analysis::new(vars, &functions.0);
    analysis.statements(stmts);
    analysis.finish()
}

struct Analysis<'a> {
    vars: &'a HashSet<BindingId>,
    functions: &'a HashMap<BindingId, usize>,
    summaries: Vec<Uses>,
    roots: Vec<(usize, Uses)>,
    // A declaration completes at a point inside one statement-list block.
    declarations: HashMap<BindingId, (usize, usize)>,
    block_ends: Vec<usize>,
    block: usize,
    point: usize,
    direct_stmt: bool,
}

impl<'a> Analysis<'a> {
    fn new(vars: &'a HashSet<BindingId>, functions: &'a HashMap<BindingId, usize>) -> Self {
        Self {
            vars,
            functions,
            summaries: (0..functions.len()).map(|_| Uses::default()).collect(),
            roots: Vec::new(),
            declarations: HashMap::default(),
            block_ends: vec![usize::MAX],
            block: 0,
            point: 0,
            direct_stmt: false,
        }
    }

    fn references<N: VisitWith<References<'a>>>(&self, node: &N) -> Uses {
        let mut collector = References {
            vars: self.vars,
            functions: self.functions,
            uses: Uses::default(),
            deferred_depth: 0,
        };
        node.visit_with(&mut collector);
        collector.uses
    }

    fn expose(&mut self, uses: Uses) {
        self.point += 1;
        if !uses.vars.is_empty() || !uses.functions.is_empty() {
            self.roots.push((self.point, uses));
        }
    }

    fn function(&mut self, id: &Ident, function: &Function) {
        let uses = self.references(function);
        // Retain the existing policy for captures in an earlier declaration,
        // including an exported declaration; do not invent module-entry roots.
        self.expose(Uses {
            vars: uses.vars.clone(),
            functions: HashSet::default(),
        });
        let summary = &mut self.summaries[self.functions[&id.to_id()]];
        // Duplicate declarations share an identity. Keep every possible body.
        summary.vars.extend(uses.vars);
        summary.functions.extend(uses.functions);
    }

    fn statements(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            self.direct_stmt = true;
            stmt.visit_with(self);
        }
    }

    fn declaration(&mut self, node: &VarDecl, direct: bool) {
        for decl in &node.decls {
            if let Some(init) = &decl.init {
                let mut uses = self.references(&**init);
                // A plain callable cannot execute before its own binding is
                // initialized. This exception does not apply to calls/classes.
                if matches!(strip_parens(init), Expr::Fn(_) | Expr::Arrow(_)) {
                    if let Pat::Ident(id) = &decl.name {
                        uses.vars.remove(&id.id.to_id());
                    }
                }
                self.expose(uses);
            }
            decl.name.visit_with(self);
            self.point += 1;
            if direct && node.kind == VarDeclKind::Var {
                let mut ids = HashSet::default();
                collect_binding_ids_from_pat(&decl.name, &mut ids);
                for id in ids {
                    self.declarations.insert(id, (self.point, self.block));
                }
            }
        }
    }

    fn finish(self) -> HashSet<BindingId> {
        let mut must_stay = HashSet::default();
        let mut visited = vec![false; self.summaries.len()];
        let mut pending = Vec::new();
        let mut mark =
            |vars: &HashSet<BindingId>, point| {
                for id in vars {
                    if self.declarations.get(id).is_none_or(|&(decl, block)| {
                        point <= decl || point >= self.block_ends[block]
                    }) {
                        must_stay.insert(id.clone());
                    }
                }
            };
        // Roots are emitted in traversal order. Each function summary and its
        // outgoing edges are expanded once, at the earliest possible exposure;
        // never materialize a transitive capture set for every function.
        for (point, uses) in self.roots {
            mark(&uses.vars, point);
            pending.extend(uses.functions.iter().map(|id| self.functions[id]));
            while let Some(index) = pending.pop() {
                if std::mem::replace(&mut visited[index], true) {
                    continue;
                }
                let summary = &self.summaries[index];
                mark(&summary.vars, point);
                pending.extend(summary.functions.iter().map(|id| self.functions[id]));
            }
        }
        must_stay
    }
}

impl Visit for Analysis<'_> {
    fn visit_stmt(&mut self, node: &Stmt) {
        let direct = std::mem::take(&mut self.direct_stmt);
        if let Stmt::Decl(Decl::Var(var)) = node {
            self.declaration(var, direct);
        } else {
            node.visit_children_with(self);
        }
    }

    fn visit_block_stmt(&mut self, node: &BlockStmt) {
        let parent = self.block;
        self.block = self.block_ends.len();
        self.block_ends.push(usize::MAX);
        self.statements(&node.stmts);
        self.point += 1;
        self.block_ends[self.block] = self.point;
        self.block = parent;
    }

    fn visit_var_decl(&mut self, node: &VarDecl) {
        // For-head declarations have no statement-list completion proof.
        self.declaration(node, false);
    }

    fn visit_export_decl(&mut self, node: &ExportDecl) {
        if let Decl::Var(var) = &node.decl {
            self.declaration(var, true);
        } else {
            node.visit_children_with(self);
        }
    }

    fn visit_fn_decl(&mut self, node: &FnDecl) {
        self.function(&node.ident, &node.function);
    }

    fn visit_export_default_decl(&mut self, node: &ExportDefaultDecl) {
        if let DefaultDecl::Fn(function) = &node.decl {
            if let Some(id) = &function.ident {
                self.function(id, &function.function);
                return;
            }
        }
        node.visit_children_with(self);
    }

    fn visit_expr(&mut self, node: &Expr) {
        self.expose(self.references(node));
    }

    fn visit_function(&mut self, node: &Function) {
        self.expose(self.references(node));
    }

    fn visit_class(&mut self, node: &Class) {
        self.expose(self.references(node));
    }

    fn visit_ident(&mut self, id: &Ident) {
        if self.functions.contains_key(&id.to_id()) {
            let mut uses = Uses::default();
            uses.functions.insert(id.to_id());
            self.expose(uses);
        }
    }
}
