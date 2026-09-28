//! Top-level binding indexes and reference/write collectors.

use rayon::prelude::*;
use swc_core::atoms::Atom;
use swc_core::common::GLOBALS;
use swc_core::ecma::ast::{
    ArrowExpr, AssignTarget, AssignTargetPat, BindingIdent, ClassDecl, Decl, Expr, FnDecl, ForHead,
    Function, Ident, ImportDecl, ImportSpecifier, ModuleDecl, ModuleItem, ObjectPatProp, Pat,
    PropName, SimpleAssignTarget, Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::collections::{HashMap, HashSet};
use crate::unpacker::{module_item_declared_binding_ids, BindingId};

use super::synthesis::{filter_item_to_owned_bindings, pat_declared_binding_ids};

pub(super) fn atom_to_filename_binding_map(
    bindings: &HashMap<BindingId, String>,
) -> HashMap<Atom, (BindingId, String)> {
    let mut by_atom = HashMap::default();
    for (binding, filename) in bindings {
        by_atom
            .entry(binding.0.clone())
            .or_insert_with(|| (binding.clone(), filename.clone()));
    }
    by_atom
}

pub(super) fn atom_binding_map_from_keys<T>(
    imports: &HashMap<BindingId, T>,
) -> HashMap<Atom, BindingId> {
    let mut by_atom = HashMap::default();
    for binding in imports.keys() {
        by_atom
            .entry(binding.0.clone())
            .or_insert_with(|| binding.clone());
    }
    by_atom
}

pub(super) fn collect_top_level_decl_indices(items: &[ModuleItem]) -> HashMap<BindingId, usize> {
    let mut indices = HashMap::default();
    for (index, item) in items.iter().enumerate() {
        for binding in module_item_declared_binding_ids(item) {
            indices.entry(binding).or_insert(index);
        }
    }
    indices
}

pub(super) fn collect_top_level_decl_references(
    items: &[ModuleItem],
    decl_indices: &HashMap<BindingId, usize>,
    top_level_bindings: &HashSet<BindingId>,
    ignored_atoms: &HashSet<Atom>,
) -> HashMap<BindingId, HashSet<BindingId>> {
    // Each ownership query reads the same resolved tree and produces an
    // independent map entry. Keep the caller's hygiene context on workers.
    GLOBALS.with(|globals| {
        decl_indices
            .par_iter()
            .filter_map(|(binding, index)| {
                GLOBALS.set(globals, || {
                    if ignored_atoms.contains(&binding.0) {
                        return None;
                    }
                    let owned_atoms = HashSet::from_iter([binding.0.clone()]);
                    let item = filter_item_to_owned_bindings(&items[*index], &owned_atoms)?;
                    let mut collector = TopLevelRefCollector {
                        top_level_bindings,
                        references: HashSet::default(),
                    };
                    item.visit_with(&mut collector);
                    Some((binding.clone(), collector.references))
                })
            })
            .collect()
    })
}

pub(super) fn collect_top_level_decl_writes(
    items: &[ModuleItem],
    decl_indices: &HashMap<BindingId, usize>,
    top_level_bindings: &HashSet<BindingId>,
) -> HashMap<BindingId, HashSet<BindingId>> {
    GLOBALS.with(|globals| {
        decl_indices
            .par_iter()
            .filter_map(|(binding, index)| {
                GLOBALS.set(globals, || {
                    let owned_atoms = HashSet::from_iter([binding.0.clone()]);
                    let item = filter_item_to_owned_bindings(&items[*index], &owned_atoms)?;
                    let binding_writes = exact_write_bindings_for_item(&item, top_level_bindings);
                    Some((binding.clone(), binding_writes))
                })
            })
            .collect()
    })
}

pub(super) fn exact_write_bindings_for_item(
    item: &ModuleItem,
    top_level_bindings: &HashSet<BindingId>,
) -> HashSet<BindingId> {
    let mut writes = HashSet::default();
    if let ModuleItem::Stmt(stmt) = item {
        collect_write_bindings(stmt, top_level_bindings, &mut writes);
    }
    writes
}

pub(super) fn add_factory_atom_import(
    imports_by_filename: &mut HashMap<String, Vec<BindingId>>,
    current_filename: &str,
    source_binding: &BindingId,
    source_filename: &str,
) {
    if source_filename == current_filename {
        return;
    }
    imports_by_filename
        .entry(source_filename.to_string())
        .or_default()
        .push(source_binding.clone());
}

pub(super) fn atom_to_module_binding_map(
    bindings: &HashMap<BindingId, usize>,
) -> HashMap<Atom, (BindingId, usize)> {
    let mut by_atom = HashMap::default();
    for (binding, module_index) in bindings {
        by_atom
            .entry(binding.0.clone())
            .or_insert_with(|| (binding.clone(), *module_index));
    }
    by_atom
}

// ---------------------------------------------------------------------------
// Extracted factory info
// ---------------------------------------------------------------------------

#[derive(Default)]
pub(super) struct ItemBindingInfo {
    pub(super) declared: HashSet<BindingId>,
    pub(super) references: HashSet<BindingId>,
}

pub(super) fn build_item_binding_infos(items: &[ModuleItem]) -> Vec<ItemBindingInfo> {
    // Collect per-item declared bindings in one pass, then build the
    // union for reference filtering.  This avoids calling
    // module_item_declared_binding_ids twice per item.
    let per_item_declared: Vec<HashSet<BindingId>> = items
        .iter()
        .map(|item| module_item_declared_binding_ids(item).into_iter().collect())
        .collect();

    let top_level_bindings: HashSet<BindingId> = per_item_declared
        .iter()
        .flat_map(|s| s.iter().cloned())
        .chain(
            items
                .iter()
                .flat_map(|item| module_item_import_binding_ids(item).into_iter()),
        )
        .collect();

    items
        .iter()
        .zip(per_item_declared)
        .map(|(item, declared)| {
            let mut collector = TopLevelRefCollector {
                top_level_bindings: &top_level_bindings,
                references: HashSet::default(),
            };
            item.visit_with(&mut collector);
            ItemBindingInfo {
                declared,
                references: collector.references,
            }
        })
        .collect()
}

pub(super) fn item_binding_info_for(
    item: &ModuleItem,
    top_level_bindings: &HashSet<BindingId>,
) -> ItemBindingInfo {
    let declared: HashSet<BindingId> = module_item_declared_binding_ids(item).into_iter().collect();
    let mut collector = TopLevelRefCollector {
        top_level_bindings,
        references: HashSet::default(),
    };
    item.visit_with(&mut collector);
    ItemBindingInfo {
        declared,
        references: collector.references,
    }
}

pub(super) fn module_item_import_binding_ids(item: &ModuleItem) -> Vec<BindingId> {
    let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
        return vec![];
    };
    import
        .specifiers
        .iter()
        .map(|specifier| match specifier {
            ImportSpecifier::Named(named) => (named.local.sym.clone(), named.local.ctxt),
            ImportSpecifier::Default(default) => (default.local.sym.clone(), default.local.ctxt),
            ImportSpecifier::Namespace(namespace) => {
                (namespace.local.sym.clone(), namespace.local.ctxt)
            }
        })
        .collect()
}

pub(super) fn filter_item_excluding_bindings(
    item: &ModuleItem,
    excluded: &HashSet<BindingId>,
    excluded_atoms: &HashSet<Atom>,
) -> Option<ModuleItem> {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
            let mut filtered = var_decl.clone();
            filtered.decls.retain(|decl| {
                let ids = pat_declared_binding_ids(&decl.name);
                ids.is_empty()
                    || !ids
                        .iter()
                        .all(|id| excluded.contains(id) || excluded_atoms.contains(&id.0))
            });
            if filtered.decls.is_empty() {
                None
            } else {
                Some(ModuleItem::Stmt(Stmt::Decl(Decl::Var(filtered))))
            }
        }
        _ => {
            let declared = module_item_declared_binding_ids(item);
            if !declared.is_empty()
                && declared
                    .iter()
                    .all(|id| excluded.contains(id) || excluded_atoms.contains(&id.0))
            {
                None
            } else {
                Some(item.clone())
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct ExternalImport {
    pub(super) decl: ImportDecl,
    pub(super) specifier: ImportSpecifier,
}

pub(super) fn collect_external_imports(
    analysis_items: &[ModuleItem],
    source_items: &[ModuleItem],
) -> HashMap<BindingId, ExternalImport> {
    let mut imports = HashMap::default();
    for (analysis_item, source_item) in analysis_items.iter().zip(source_items) {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(analysis_import)) = analysis_item else {
            continue;
        };
        let ModuleItem::ModuleDecl(ModuleDecl::Import(source_import)) = source_item else {
            continue;
        };
        for (analysis_specifier, source_specifier) in analysis_import
            .specifiers
            .iter()
            .zip(source_import.specifiers.iter())
        {
            let binding = import_specifier_binding(analysis_specifier);
            imports.entry(binding).or_insert_with(|| ExternalImport {
                decl: source_import.clone(),
                specifier: source_specifier.clone(),
            });
        }
    }
    imports
}

fn import_specifier_binding(specifier: &ImportSpecifier) -> BindingId {
    match specifier {
        ImportSpecifier::Named(named) => (named.local.sym.clone(), named.local.ctxt),
        ImportSpecifier::Default(default) => (default.local.sym.clone(), default.local.ctxt),
        ImportSpecifier::Namespace(namespace) => {
            (namespace.local.sym.clone(), namespace.local.ctxt)
        }
    }
}

pub(super) struct TopLevelRefCollector<'a> {
    pub(super) top_level_bindings: &'a HashSet<BindingId>,
    pub(super) references: HashSet<BindingId>,
}

pub(super) struct AtomRefCollector<'a> {
    pub(super) candidate_atoms: &'a HashSet<Atom>,
    pub(super) references: HashSet<Atom>,
    pub(super) shadowed_atoms: Vec<HashSet<Atom>>,
}

impl AtomRefCollector<'_> {
    fn visit_assignment_array(&mut self, elems: &[Option<Pat>]) {
        for elem in elems.iter().flatten() {
            self.visit_assignment_pat(elem);
        }
    }

    fn visit_assignment_object(&mut self, props: &[ObjectPatProp]) {
        for prop in props {
            match prop {
                ObjectPatProp::KeyValue(prop) => {
                    prop.key.visit_with(self);
                    self.visit_assignment_pat(&prop.value);
                }
                ObjectPatProp::Assign(prop) => {
                    self.visit_ident(&prop.key.id);
                    prop.value.visit_with(self);
                }
                ObjectPatProp::Rest(rest) => self.visit_assignment_pat(&rest.arg),
            }
        }
    }

    fn visit_assignment_pat(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(ident) => self.visit_ident(&ident.id),
            Pat::Array(array) => self.visit_assignment_array(&array.elems),
            Pat::Object(object) => self.visit_assignment_object(&object.props),
            Pat::Assign(assign) => {
                self.visit_assignment_pat(&assign.left);
                assign.right.visit_with(self);
            }
            Pat::Rest(rest) => self.visit_assignment_pat(&rest.arg),
            Pat::Expr(expr) => expr.visit_with(self),
            Pat::Invalid(_) => {}
        }
    }

    fn visit_assignment_target_pat(&mut self, pat: &AssignTargetPat) {
        match pat {
            AssignTargetPat::Array(array) => self.visit_assignment_array(&array.elems),
            AssignTargetPat::Object(object) => self.visit_assignment_object(&object.props),
            AssignTargetPat::Invalid(_) => {}
        }
    }
}

impl Visit for AtomRefCollector<'_> {
    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        if let Some(scope) = self.shadowed_atoms.last_mut() {
            scope.insert(ident.id.sym.clone());
        }
    }

    fn visit_function(&mut self, function: &Function) {
        self.shadowed_atoms.push(HashSet::default());
        function.visit_children_with(self);
        self.shadowed_atoms.pop();
    }

    fn visit_arrow_expr(&mut self, expr: &ArrowExpr) {
        self.shadowed_atoms.push(HashSet::default());
        expr.visit_children_with(self);
        self.shadowed_atoms.pop();
    }

    fn visit_assign_target(&mut self, target: &AssignTarget) {
        match target {
            // SWC represents an identifier assignment target with
            // `BindingIdent`, but it is a use of an existing binding, not a
            // declaration that shadows later references in this scope.
            AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) => {
                self.visit_ident(&ident.id);
            }
            AssignTarget::Simple(target) => target.visit_children_with(self),
            AssignTarget::Pat(target) => self.visit_assignment_target_pat(target),
        }
    }

    fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
        if self.candidate_atoms.contains(&ident.sym)
            && !self
                .shadowed_atoms
                .iter()
                .any(|scope| scope.contains(&ident.sym))
        {
            self.references.insert(ident.sym.clone());
        }
    }

    fn visit_member_expr(&mut self, expr: &swc_core::ecma::ast::MemberExpr) {
        expr.obj.visit_with(self);
        if let swc_core::ecma::ast::MemberProp::Computed(c) = &expr.prop {
            c.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, prop: &swc_core::ecma::ast::MemberProp) {
        if let swc_core::ecma::ast::MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }

    fn visit_prop_name(&mut self, prop: &PropName) {
        if let PropName::Computed(c) = prop {
            c.visit_with(self);
        }
    }
}

impl TopLevelRefCollector<'_> {
    fn visit_binding_pat_defaults(&mut self, pat: &Pat) {
        match pat {
            Pat::Array(array) => {
                for elem in array.elems.iter().flatten() {
                    self.visit_binding_pat_defaults(elem);
                }
            }
            Pat::Object(object) => {
                for prop in &object.props {
                    match prop {
                        swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                            self.visit_binding_pat_defaults(&kv.value);
                        }
                        swc_core::ecma::ast::ObjectPatProp::Assign(assign) => {
                            if let Some(value) = &assign.value {
                                value.visit_with(self);
                            }
                        }
                        swc_core::ecma::ast::ObjectPatProp::Rest(rest) => {
                            self.visit_binding_pat_defaults(&rest.arg);
                        }
                    }
                }
            }
            Pat::Assign(assign) => {
                assign.right.visit_with(self);
                self.visit_binding_pat_defaults(&assign.left);
            }
            Pat::Rest(rest) => self.visit_binding_pat_defaults(&rest.arg),
            _ => {}
        }
    }
}

impl Visit for TopLevelRefCollector<'_> {
    fn visit_binding_ident(&mut self, _: &BindingIdent) {}

    fn visit_pat(&mut self, pat: &Pat) {
        self.visit_binding_pat_defaults(pat);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        self.visit_binding_pat_defaults(&declarator.name);
        if let Some(init) = &declarator.init {
            init.visit_with(self);
        }
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        decl.function.visit_with(self);
    }

    fn visit_class_decl(&mut self, decl: &ClassDecl) {
        decl.class.visit_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
        if let Some(ident) = assign.left.as_ident() {
            let binding = (ident.sym.clone(), ident.ctxt);
            if self.top_level_bindings.contains(&binding) {
                self.references.insert(binding);
            }
        }
        assign.left.visit_with(self);
        assign.right.visit_with(self);
    }

    fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
        if let Expr::Ident(ident) = &*update.arg {
            let binding = (ident.sym.clone(), ident.ctxt);
            if self.top_level_bindings.contains(&binding) {
                self.references.insert(binding);
            }
        }
    }

    fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
        let binding = (ident.sym.clone(), ident.ctxt);
        if self.top_level_bindings.contains(&binding) {
            self.references.insert(binding);
        }
    }

    fn visit_member_expr(&mut self, expr: &swc_core::ecma::ast::MemberExpr) {
        expr.obj.visit_with(self);
        if let swc_core::ecma::ast::MemberProp::Computed(c) = &expr.prop {
            c.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, prop: &swc_core::ecma::ast::MemberProp) {
        if let swc_core::ecma::ast::MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }

    fn visit_prop_name(&mut self, name: &PropName) {
        if let PropName::Computed(c) = name {
            c.visit_with(self);
        }
    }

    fn visit_super_prop(&mut self, prop: &swc_core::ecma::ast::SuperProp) {
        if let swc_core::ecma::ast::SuperProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }
}

/// Collect top-level bindings that appear as assignment targets in a statement.
/// This detects `X = expr` and `X = expr, Y = expr` patterns where X/Y are
/// top-level bindings (not local declarations).
pub(super) fn collect_write_bindings(
    stmt: &Stmt,
    top_level_bindings: &HashSet<BindingId>,
    out: &mut HashSet<BindingId>,
) {
    struct WriteCollector<'a> {
        top_level_bindings: &'a HashSet<BindingId>,
        writes: &'a mut HashSet<BindingId>,
    }

    impl WriteCollector<'_> {
        fn record_ident(&mut self, ident: &Ident) {
            let binding = (ident.sym.clone(), ident.ctxt);
            if self.top_level_bindings.contains(&binding) {
                self.writes.insert(binding);
            }
        }

        fn record_assignment_pat(&mut self, pat: &Pat) {
            match pat {
                Pat::Ident(ident) => self.record_ident(&ident.id),
                Pat::Array(array) => {
                    for elem in array.elems.iter().flatten() {
                        self.record_assignment_pat(elem);
                    }
                }
                Pat::Object(object) => {
                    for prop in &object.props {
                        match prop {
                            ObjectPatProp::KeyValue(prop) => {
                                self.record_assignment_pat(&prop.value);
                            }
                            ObjectPatProp::Assign(prop) => self.record_ident(&prop.key.id),
                            ObjectPatProp::Rest(rest) => {
                                self.record_assignment_pat(&rest.arg);
                            }
                        }
                    }
                }
                Pat::Assign(assign) => self.record_assignment_pat(&assign.left),
                Pat::Rest(rest) => self.record_assignment_pat(&rest.arg),
                // A member target writes the property, not the object binding.
                // SWC can represent a direct identifier target either as
                // `Pat::Ident` or as an expression after parser recovery.
                Pat::Expr(expr) => {
                    if let Expr::Ident(ident) = &**expr {
                        self.record_ident(ident);
                    }
                }
                Pat::Invalid(_) => {}
            }
        }

        fn record_assignment_target_pat(&mut self, pat: &AssignTargetPat) {
            match pat {
                AssignTargetPat::Array(array) => {
                    for elem in array.elems.iter().flatten() {
                        self.record_assignment_pat(elem);
                    }
                }
                AssignTargetPat::Object(object) => {
                    for prop in &object.props {
                        match prop {
                            ObjectPatProp::KeyValue(prop) => {
                                self.record_assignment_pat(&prop.value);
                            }
                            ObjectPatProp::Assign(prop) => self.record_ident(&prop.key.id),
                            ObjectPatProp::Rest(rest) => {
                                self.record_assignment_pat(&rest.arg);
                            }
                        }
                    }
                }
                AssignTargetPat::Invalid(_) => {}
            }
        }
    }

    impl Visit for WriteCollector<'_> {
        fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
            if let Some(ident) = assign.left.as_ident() {
                self.record_ident(ident);
            } else if let swc_core::ecma::ast::AssignTarget::Pat(pat) = &assign.left {
                self.record_assignment_target_pat(pat);
            }
            // Computed member targets and destructuring defaults can contain
            // nested assignments of their own.
            assign.left.visit_children_with(self);
            assign.right.visit_with(self);
        }

        fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
            if let Expr::Ident(ident) = &*update.arg {
                self.record_ident(ident);
            }
            update.arg.visit_with(self);
        }

        fn visit_for_head(&mut self, head: &ForHead) {
            if let ForHead::Pat(pat) = head {
                self.record_assignment_pat(pat);
            }
            // Keep traversing the iterable declaration/default expressions so
            // nested assignments are inventoried as well.
            head.visit_children_with(self);
        }
    }

    let mut collector = WriteCollector {
        top_level_bindings,
        writes: out,
    };
    stmt.visit_with(&mut collector);
}

pub(super) fn scope_write_atoms_for_item(
    item: &ModuleItem,
    top_level_bindings: &HashSet<BindingId>,
    top_level_atoms: &HashSet<Atom>,
) -> HashSet<Atom> {
    let mut local_collector = NonTopLevelBindingCollector {
        top_level_bindings,
        local_bindings: HashSet::default(),
    };
    item.visit_with(&mut local_collector);

    let mut write_collector = ScopeWriteAtomCollector {
        top_level_bindings,
        top_level_atoms,
        local_bindings: &local_collector.local_bindings,
        writes: HashSet::default(),
    };
    item.visit_with(&mut write_collector);
    write_collector.writes
}

struct NonTopLevelBindingCollector<'a> {
    pub(super) top_level_bindings: &'a HashSet<BindingId>,
    local_bindings: HashSet<BindingId>,
}

impl Visit for NonTopLevelBindingCollector<'_> {
    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        let binding = (ident.id.sym.clone(), ident.id.ctxt);
        if !self.top_level_bindings.contains(&binding) {
            self.local_bindings.insert(binding);
        }
    }
}

struct ScopeWriteAtomCollector<'a> {
    pub(super) top_level_bindings: &'a HashSet<BindingId>,
    pub(super) top_level_atoms: &'a HashSet<Atom>,
    local_bindings: &'a HashSet<BindingId>,
    pub(super) writes: HashSet<Atom>,
}

impl ScopeWriteAtomCollector<'_> {
    fn record_ident(&mut self, ident: &Ident) {
        let binding = (ident.sym.clone(), ident.ctxt);
        if self.top_level_bindings.contains(&binding)
            || (self.top_level_atoms.contains(&ident.sym)
                && !self.local_bindings.contains(&binding))
        {
            self.writes.insert(ident.sym.clone());
        }
    }
}

impl Visit for ScopeWriteAtomCollector<'_> {
    fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
        collect_scope_write_target(&assign.left, self);
        assign.right.visit_with(self);
    }

    fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
        if let Expr::Ident(ident) = update.arg.as_ref() {
            self.record_ident(ident);
        }
    }
}

fn collect_scope_write_target(target: &AssignTarget, collector: &mut ScopeWriteAtomCollector<'_>) {
    match target {
        AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) => {
            collector.record_ident(&ident.id);
        }
        AssignTarget::Simple(simple) => {
            simple.visit_with(collector);
        }
        AssignTarget::Pat(pat) => collect_scope_write_pat_target(pat, collector),
    }
}

fn collect_scope_write_pat_target(
    target: &AssignTargetPat,
    collector: &mut ScopeWriteAtomCollector<'_>,
) {
    match target {
        AssignTargetPat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_scope_write_pat(elem, collector);
            }
        }
        AssignTargetPat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => collect_scope_write_pat(&kv.value, collector),
                    ObjectPatProp::Assign(assign) => collector.record_ident(&assign.key),
                    ObjectPatProp::Rest(rest) => collect_scope_write_pat(&rest.arg, collector),
                }
            }
        }
        AssignTargetPat::Invalid(_) => {}
    }
}

fn collect_scope_write_pat(pat: &Pat, collector: &mut ScopeWriteAtomCollector<'_>) {
    match pat {
        Pat::Ident(ident) => collector.record_ident(&ident.id),
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_scope_write_pat(elem, collector);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::Assign(assign) => collector.record_ident(&assign.key),
                    ObjectPatProp::KeyValue(kv) => collect_scope_write_pat(&kv.value, collector),
                    ObjectPatProp::Rest(rest) => collect_scope_write_pat(&rest.arg, collector),
                }
            }
        }
        Pat::Rest(rest) => collect_scope_write_pat(&rest.arg, collector),
        Pat::Assign(assign) => {
            collect_scope_write_pat(&assign.left, collector);
            assign.right.visit_with(collector);
        }
        Pat::Expr(expr) => expr.visit_with(collector),
        Pat::Invalid(_) => {}
    }
}

// ---------------------------------------------------------------------------
// Import / export synthesis for scope-hoisted modules
// ---------------------------------------------------------------------------
