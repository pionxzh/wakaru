//! Same-scope capture safety without recognizing invocation syntax.
//!
//! A function value reference may execute or escape. Follow references between
//! named declarations and check their captured vars at the earliest exposure.
//! Closures and classes with eager work are exposed at creation. Simple class
//! declarations defer captures until a value reference. Declaration order
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
    deferred: HashSet<BindingId>,
}

struct References<'a> {
    vars: &'a HashSet<BindingId>,
    deferred: &'a HashMap<BindingId, usize>,
    uses: Uses,
    deferred_depth: usize,
}

impl Visit for References<'_> {
    fn visit_ident(&mut self, id: &Ident) {
        let id = id.to_id();
        if self.deferred.contains_key(&id) {
            self.uses.deferred.insert(id.clone());
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

// Only declarations whose creation cannot invoke their bodies qualify. Keep
// expressions, inheritance, computed keys, decorators and static initialization
// on the conservative path; `this.method()` in a static block need not mention
// the class binding at all.
fn is_deferred_class(class: &Class) -> bool {
    let undecorated = |function: &Function| {
        function.decorators.is_empty()
            && function
                .params
                .iter()
                .all(|param| param.decorators.is_empty())
    };
    class.super_class.is_none()
        && class.decorators.is_empty()
        && class.body.iter().all(|member| match member {
            ClassMember::Constructor(ctor) => {
                !matches!(ctor.key, PropName::Computed(_))
                    && ctor.params.iter().all(|param| {
                        matches!(param, ParamOrTsParamProp::Param(param) if param.decorators.is_empty())
                    })
            }
            ClassMember::Method(method) => {
                !matches!(method.key, PropName::Computed(_)) && undecorated(&method.function)
            }
            ClassMember::PrivateMethod(method) => undecorated(&method.function),
            ClassMember::ClassProp(prop) => {
                !prop.is_static
                    && !matches!(prop.key, PropName::Computed(_))
                    && prop.decorators.is_empty()
            }
            ClassMember::PrivateProp(prop) => !prop.is_static && prop.decorators.is_empty(),
            ClassMember::Empty(_) => true,
            _ => false,
        })
}

#[derive(Default)]
struct DeferredDeclarations(HashMap<BindingId, usize>);

impl DeferredDeclarations {
    fn insert(&mut self, id: &Ident) {
        let index = self.0.len();
        self.0.entry(id.to_id()).or_insert(index);
    }
}

impl Visit for DeferredDeclarations {
    fn visit_fn_decl(&mut self, node: &FnDecl) {
        self.insert(&node.ident);
    }

    fn visit_class_decl(&mut self, node: &ClassDecl) {
        if is_deferred_class(&node.class) {
            self.insert(&node.ident);
        }
    }

    fn visit_export_default_decl(&mut self, node: &ExportDefaultDecl) {
        match &node.decl {
            DefaultDecl::Fn(function) => {
                if let Some(id) = &function.ident {
                    self.insert(id);
                }
            }
            DefaultDecl::Class(class) if is_deferred_class(&class.class) => {
                if let Some(id) = &class.ident {
                    self.insert(id);
                }
            }
            _ => {}
        }
    }

    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
}

pub(super) fn module(items: &[ModuleItem], vars: &HashSet<BindingId>) -> HashSet<BindingId> {
    let mut deferred = DeferredDeclarations::default();
    items.visit_with(&mut deferred);
    let mut analysis = Analysis::new(vars, &deferred.0);
    for item in items {
        analysis.direct_stmt = true;
        item.visit_with(&mut analysis);
    }
    analysis.finish()
}

pub(super) fn stmts(stmts: &[Stmt], vars: &HashSet<BindingId>) -> HashSet<BindingId> {
    let mut deferred = DeferredDeclarations::default();
    stmts.visit_with(&mut deferred);
    let mut analysis = Analysis::new(vars, &deferred.0);
    analysis.statements(stmts);
    analysis.finish()
}

enum Root {
    Exposure(Uses),
    ClassInitialized(usize),
}

struct Analysis<'a> {
    vars: &'a HashSet<BindingId>,
    deferred: &'a HashMap<BindingId, usize>,
    summaries: Vec<Uses>,
    roots: Vec<(usize, Root)>,
    class_initialization: Vec<usize>,
    // A declaration completes at a point inside one statement-list block.
    declarations: HashMap<BindingId, (usize, usize)>,
    block_ends: Vec<usize>,
    block: usize,
    point: usize,
    direct_stmt: bool,
}

impl<'a> Analysis<'a> {
    fn new(vars: &'a HashSet<BindingId>, deferred: &'a HashMap<BindingId, usize>) -> Self {
        Self {
            vars,
            deferred,
            summaries: (0..deferred.len()).map(|_| Uses::default()).collect(),
            roots: Vec::new(),
            class_initialization: vec![0; deferred.len()],
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
            deferred: self.deferred,
            uses: Uses::default(),
            deferred_depth: 0,
        };
        node.visit_with(&mut collector);
        collector.uses
    }

    fn expose(&mut self, uses: Uses) {
        self.point += 1;
        if !uses.vars.is_empty() || !uses.deferred.is_empty() {
            self.roots.push((self.point, Root::Exposure(uses)));
        }
    }

    fn function(&mut self, id: &Ident, function: &Function) {
        let uses = self.references(function);
        // Retain the existing policy for captures in an earlier declaration,
        // including an exported declaration; do not invent module-entry roots.
        self.expose(Uses {
            vars: uses.vars.clone(),
            deferred: HashSet::default(),
        });
        let summary = &mut self.summaries[self.deferred[&id.to_id()]];
        // Duplicate declarations share an identity. Keep every possible body.
        summary.vars.extend(uses.vars);
        summary.deferred.extend(uses.deferred);
    }

    fn class_declaration(&mut self, id: &Ident, class: &Class) {
        if let Some(&index) = self.deferred.get(&id.to_id()) {
            self.summaries[index] = self.references(class);
            self.point += 1;
            self.class_initialization[index] = self.point;
            self.roots.push((self.point, Root::ClassInitialized(index)));
        } else {
            self.expose(self.references(class));
        }
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
        let mut referenced = vec![false; self.summaries.len()];
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
        // Roots are emitted in traversal order. A simple class's own TDZ prevents
        // its captures from running before initialization. Queue early references
        // for its initialization event, then expand each summary/edge just once.
        // No priority queue or per-declaration transitive capture set is needed.
        for (point, root) in self.roots {
            match root {
                Root::Exposure(uses) => {
                    mark(&uses.vars, point);
                    pending.extend(uses.deferred.iter().map(|id| self.deferred[id]));
                }
                Root::ClassInitialized(index) if referenced[index] => pending.push(index),
                Root::ClassInitialized(_) => {}
            }
            while let Some(index) = pending.pop() {
                referenced[index] = true;
                if point < self.class_initialization[index] {
                    continue;
                }
                if std::mem::replace(&mut visited[index], true) {
                    continue;
                }
                let summary = &self.summaries[index];
                mark(&summary.vars, point);
                pending.extend(summary.deferred.iter().map(|id| self.deferred[id]));
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

    fn visit_named_export(&mut self, _: &NamedExport) {
        // Linking a name does not evaluate its value. Cross-module entry is
        // governed by the existing export policy, not this local exposure graph.
    }

    fn visit_class_decl(&mut self, node: &ClassDecl) {
        self.class_declaration(&node.ident, &node.class);
    }

    fn visit_export_default_decl(&mut self, node: &ExportDefaultDecl) {
        match &node.decl {
            DefaultDecl::Fn(function) => {
                if let Some(id) = &function.ident {
                    self.function(id, &function.function);
                    return;
                }
            }
            DefaultDecl::Class(class) => {
                if let Some(id) = &class.ident {
                    self.class_declaration(id, &class.class);
                    return;
                }
            }
            _ => {}
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
        if self.deferred.contains_key(&id.to_id()) {
            let mut uses = Uses::default();
            uses.deferred.insert(id.to_id());
            self.expose(uses);
        }
    }
}
