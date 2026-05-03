//! Namespace decomposition: decompose default or namespace imports into named
//! imports when the imported binding is only used via property access and the
//! target module exports those properties.
//!
//! Runs at the Stage 2 barrier (after `UnEsm`, before `UnTemplateLiteral`),
//! using cross-module `ModuleFacts` to verify that the target module actually
//! exports the accessed names.
//!
//! After decomposition, patterns like `r.fn.apply(undefined, args)` become
//! `fn.apply(undefined, args)`, which `UnArgumentSpread` (Stage 3) handles
//! naturally as Pattern 1.

use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, CatchClause, Expr, Function, Ident, ImportDecl, ImportNamedSpecifier,
    ImportSpecifier, MemberExpr, MemberProp, Module, ModuleDecl, ModuleExportName, ModuleItem,
    Param, Pat, SimpleAssignTarget, UnaryExpr, UnaryOp, UpdateExpr,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::facts::{ExportKind, ModuleFactsMap};

/// A single property to decompose: maps the exported name to its local alias.
#[derive(Debug, Clone)]
struct DecompProp {
    /// The exported property name (e.g. `createStore`)
    exported: Atom,
    /// The local binding name — equals `exported` unless aliased to avoid collision
    local: Atom,
    /// `SyntaxContext` to stamp on rewritten usage idents. When we reuse an
    /// existing named specifier's local, this is that specifier's real ctxt, so
    /// downstream `(sym, ctxt)` matching sees our rewrites as refs to the same
    /// binding. When we add a fresh specifier, the binding is new and carries
    /// `SyntaxContext::empty()`.
    local_ctxt: SyntaxContext,
    /// True when this prop needed a synthesized alias only to avoid a local
    /// binding collision.
    collision_alias: bool,
}

/// A candidate for namespace decomposition: a default import whose binding is used
/// only via property access, and the target module exports those properties.
struct DecompCandidate {
    /// Index of the import declaration in module.body
    import_index: usize,
    /// The local binding name and its SyntaxContext
    local_sym: Atom,
    local_ctxt: SyntaxContext,
    /// Properties to decompose with their local aliases
    props: Vec<DecompProp>,
    /// Whether the original default/namespace specifier can be removed. Partial
    /// decompositions keep it for properties that still need namespace access.
    remove_original: bool,
}

/// Run namespace decomposition on a single module, using cross-module facts.
///
/// `module_facts` provides lookup from module specifier → facts of the target module.
/// This allows checking whether the target module actually exports the accessed names.
pub fn run_namespace_decomposition(module: &mut Module, module_facts: &ModuleFactsMap) {
    let candidates = find_decomposition_candidates(module, module_facts);
    if candidates.is_empty() {
        return;
    }
    apply_decompositions(module, &candidates);
}

/// Find default imports that can be decomposed into named imports.
fn find_decomposition_candidates(
    module: &Module,
    module_facts: &ModuleFactsMap,
) -> Vec<DecompCandidate> {
    // Collect ALL bindings at every scope level (including function params,
    // catch clauses, inner var/let/const, etc.) to detect naming collisions.
    // A property name that shadows an inner binding would produce wrong code
    // if we rewrote `r.foo` → `foo` where `foo` is already bound in that scope.
    let mut collector = AllBindingsCollector {
        bindings: HashSet::new(),
    };
    module.visit_with(&mut collector);
    let mut existing_bindings = collector.bindings;

    // Step 1: Find default and namespace imports and their source modules.
    // Both shapes expose a module-namespace-like binding that can be decomposed
    // into direct named specifiers when usage is property-access only.
    let mut namespace_like_imports: Vec<(usize, Atom, SyntaxContext, Atom)> = Vec::new();
    for (idx, item) in module.body.iter().enumerate() {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        let source: Atom = import.src.value.as_str().unwrap_or("").into();
        for spec in &import.specifiers {
            match spec {
                ImportSpecifier::Default(s) => {
                    namespace_like_imports.push((
                        idx,
                        s.local.sym.clone(),
                        s.local.ctxt,
                        source.clone(),
                    ));
                }
                ImportSpecifier::Namespace(s) => {
                    namespace_like_imports.push((
                        idx,
                        s.local.sym.clone(),
                        s.local.ctxt,
                        source.clone(),
                    ));
                }
                ImportSpecifier::Named(_) => {}
            }
        }
    }

    if namespace_like_imports.is_empty() {
        return Vec::new();
    }

    // Step 2: For each default/namespace import, analyze usage
    let mut candidates = Vec::new();
    for (import_index, local_sym, local_ctxt, source) in namespace_like_imports {
        // Check if target module's exports are known
        let Some(target_facts) = module_facts.get(source.as_ref()) else {
            continue;
        };

        let mut analyzer = UsageAnalyzer {
            target_sym: &local_sym,
            target_ctxt: local_ctxt,
            accessed_props: HashSet::new(),
            safe: true,
            in_import_decl: false,
        };
        module.visit_with(&mut analyzer);

        if !analyzer.safe || analyzer.accessed_props.is_empty() {
            continue;
        }

        // Check if ALL accessed properties are exported by the target module
        let exported_names: HashSet<&str> = target_facts
            .exports
            .iter()
            .filter(|e| e.kind == ExportKind::Named)
            .map(|e| e.exported.as_ref())
            .collect();

        let all_exported = analyzer
            .accessed_props
            .iter()
            .all(|prop| exported_names.contains(prop.as_ref()));

        if !all_exported {
            continue;
        }

        // Map `exported_name → local_name` for named specifiers already on this
        // import. Keying by *exported* name is essential: `import { foo as bar }`
        // binds local `bar` to export `foo`, so if we later decompose `React.bar`
        // we must NOT reuse the existing `bar` — that local points at export `foo`.
        // `existing_import_locals` tracks locals that came from this import; we use
        // it to avoid removing pre-existing bindings during the readability-skip
        // undo below.
        let mut exported_to_local: HashMap<Atom, (Atom, SyntaxContext)> = HashMap::new();
        let mut existing_import_locals: HashSet<Atom> = HashSet::new();
        if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = &module.body[import_index] {
            for spec in &import.specifiers {
                if let ImportSpecifier::Named(s) = spec {
                    existing_import_locals.insert(s.local.sym.clone());
                    // Skip string-named imports (`import { "x" as y }`) — we can't
                    // reuse them for identifier-keyed property access anyway.
                    let exported = match &s.imported {
                        Some(ModuleExportName::Ident(id)) => id.sym.clone(),
                        Some(ModuleExportName::Str(_)) => continue,
                        None => s.local.sym.clone(),
                    };
                    exported_to_local.insert(exported, (s.local.sym.clone(), s.local.ctxt));
                }
            }
        }

        // Build decomposition props, aliasing on collision.
        // For each accessed export name, determine its local name:
        // - if that *export* is already imported (even under an alias), reuse the
        //   existing local binding
        // - else if the preferred local name collides with an existing binding,
        //   synthesize a safe alias
        // - else use the property name directly
        let mut props: Vec<DecompProp> = Vec::new();
        let mut alias_count = 0usize;
        let mut reused_existing = 0usize;
        let mut inserted_locals: HashSet<Atom> = HashSet::new();
        let mut sorted_accessed: Vec<Atom> = analyzer.accessed_props.into_iter().collect();
        sorted_accessed.sort();
        for prop in &sorted_accessed {
            if let Some((existing_local, existing_ctxt)) = exported_to_local.get(prop) {
                props.push(DecompProp {
                    exported: prop.clone(),
                    local: existing_local.clone(),
                    local_ctxt: *existing_ctxt,
                    collision_alias: false,
                });
                reused_existing += 1;
                continue;
            }
            let is_own_binding = *prop == local_sym;
            let has_collision = existing_bindings.contains(prop);
            if !is_own_binding && has_collision {
                let alias = synthesize_alias(prop, &existing_bindings);
                existing_bindings.insert(alias.clone());
                inserted_locals.insert(alias.clone());
                alias_count += 1;
                props.push(DecompProp {
                    exported: prop.clone(),
                    local: alias,
                    local_ctxt: SyntaxContext::empty(),
                    collision_alias: true,
                });
            } else {
                if existing_bindings.insert(prop.clone()) {
                    inserted_locals.insert(prop.clone());
                }
                props.push(DecompProp {
                    exported: prop.clone(),
                    local: prop.clone(),
                    local_ctxt: SyntaxContext::empty(),
                    collision_alias: false,
                });
            }
        }

        let mut remove_original = true;

        // If too many aliases are needed, keep the namespace import and decompose
        // only properties that can use clean local names. This still enables
        // downstream rules such as `fn.apply(undefined, args)` → `fn(...args)`.
        let new_props = sorted_accessed.len() - reused_existing;
        if new_props > 1 && alias_count * 2 > new_props {
            let mut partial_props = Vec::new();
            for prop in props {
                if prop.collision_alias || prop.local == local_sym {
                    if inserted_locals.contains(&prop.local) {
                        existing_bindings.remove(&prop.local);
                    }
                } else {
                    partial_props.push(prop);
                }
            }
            if partial_props.is_empty() {
                continue;
            }
            props = partial_props;
            remove_original = false;
        }

        candidates.push(DecompCandidate {
            import_index,
            local_sym,
            local_ctxt,
            props,
            remove_original,
        });
    }

    candidates
}

/// Generate a safe alias for a name that collides with existing bindings.
/// Appends `_1`, `_2`, etc. until a unique name is found.
fn synthesize_alias(name: &Atom, existing: &HashSet<Atom>) -> Atom {
    for i in 1.. {
        let candidate: Atom = format!("{name}_{i}").into();
        if !existing.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// Collects all binding names in the module, at every scope depth.
/// Used to detect collisions with inner-scope variables, parameters, etc.
struct AllBindingsCollector {
    bindings: HashSet<Atom>,
}

impl AllBindingsCollector {
    fn collect_pat(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(b) => {
                self.bindings.insert(b.id.sym.clone());
            }
            Pat::Array(a) => {
                for elem in a.elems.iter().flatten() {
                    self.collect_pat(elem);
                }
            }
            Pat::Object(o) => {
                for prop in &o.props {
                    match prop {
                        swc_core::ecma::ast::ObjectPatProp::Assign(a) => {
                            self.bindings.insert(a.key.sym.clone());
                        }
                        swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                            self.collect_pat(&kv.value);
                        }
                        swc_core::ecma::ast::ObjectPatProp::Rest(r) => {
                            self.collect_pat(&r.arg);
                        }
                    }
                }
            }
            Pat::Rest(r) => {
                self.collect_pat(&r.arg);
            }
            Pat::Assign(a) => {
                self.collect_pat(&a.left);
            }
            Pat::Expr(_) | Pat::Invalid(_) => {}
        }
    }
}

impl Visit for AllBindingsCollector {
    fn visit_import_specifier(&mut self, spec: &ImportSpecifier) {
        let local = match spec {
            ImportSpecifier::Default(s) => &s.local.sym,
            ImportSpecifier::Namespace(s) => &s.local.sym,
            ImportSpecifier::Named(s) => &s.local.sym,
        };
        self.bindings.insert(local.clone());
    }

    fn visit_var_declarator(&mut self, decl: &swc_core::ecma::ast::VarDeclarator) {
        self.collect_pat(&decl.name);
        decl.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, f: &swc_core::ecma::ast::FnDecl) {
        self.bindings.insert(f.ident.sym.clone());
        f.visit_children_with(self);
    }

    fn visit_class_decl(&mut self, c: &swc_core::ecma::ast::ClassDecl) {
        self.bindings.insert(c.ident.sym.clone());
        c.visit_children_with(self);
    }

    fn visit_param(&mut self, param: &Param) {
        self.collect_pat(&param.pat);
        param.visit_children_with(self);
    }

    fn visit_function(&mut self, f: &Function) {
        for param in &f.params {
            self.collect_pat(&param.pat);
        }
        f.visit_children_with(self);
    }

    fn visit_catch_clause(&mut self, c: &CatchClause) {
        if let Some(param) = &c.param {
            self.collect_pat(param);
        }
        c.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        for param in &arrow.params {
            self.collect_pat(param);
        }
        arrow.visit_children_with(self);
    }
}

/// Visitor that checks whether a binding is used only via property access.
struct UsageAnalyzer<'a> {
    target_sym: &'a Atom,
    target_ctxt: SyntaxContext,
    accessed_props: HashSet<Atom>,
    safe: bool,
    in_import_decl: bool,
}

impl UsageAnalyzer<'_> {
    fn is_target(&self, ident: &Ident) -> bool {
        ident.sym == *self.target_sym && ident.ctxt == self.target_ctxt
    }

    fn is_target_member(&self, member: &MemberExpr) -> bool {
        matches!(member.obj.as_ref(), Expr::Ident(obj) if self.is_target(obj))
    }

    fn is_target_member_expr(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::Member(member) if self.is_target_member(member))
    }

    fn is_target_member_assign_target(&self, target: &AssignTarget) -> bool {
        matches!(
            target,
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) if self.is_target_member(member)
        )
    }
}

impl Visit for UsageAnalyzer<'_> {
    fn visit_import_decl(&mut self, import: &ImportDecl) {
        // Don't count the import declaration itself as a usage
        self.in_import_decl = true;
        import.visit_children_with(self);
        self.in_import_decl = false;
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if self.is_target_member_assign_target(&assign.left) {
            self.safe = false;
            assign.right.visit_with(self);
            return;
        }
        assign.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        if self.is_target_member_expr(update.arg.as_ref()) {
            self.safe = false;
            return;
        }
        update.visit_children_with(self);
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        if unary.op == UnaryOp::Delete && self.is_target_member_expr(unary.arg.as_ref()) {
            self.safe = false;
            return;
        }
        unary.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if let Expr::Ident(obj) = member.obj.as_ref() {
            if self.is_target(obj) {
                match &member.prop {
                    MemberProp::Ident(prop) => {
                        self.accessed_props.insert(prop.sym.clone());
                        // Don't recurse into obj — we've handled it
                        return;
                    }
                    MemberProp::Computed(_) => {
                        // Computed access like r[expr] — not safe
                        self.safe = false;
                        return;
                    }
                    _ => {
                        self.safe = false;
                        return;
                    }
                }
            }
        }
        member.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if self.in_import_decl {
            return;
        }
        if self.is_target(ident) {
            // Bare reference to the binding (not via member access) — not safe
            self.safe = false;
        }
    }

    // Don't visit into property name position (object literal keys, etc.)
    fn visit_prop_name(&mut self, _: &swc_core::ecma::ast::PropName) {}

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(prop) = prop {
            prop.visit_with(self);
        }
    }
}

/// Apply the decomposition rewrites.
fn apply_decompositions(module: &mut Module, candidates: &[DecompCandidate]) {
    // Build a lookup: (sym, ctxt) → map of exported_name → (local_name, local_ctxt).
    // Reused-existing specifiers carry their real ctxt; freshly-added ones use
    // `SyntaxContext::empty()`. Propagating the ctxt lets downstream rules match
    // rewritten usages against the binding.
    let decomp_map: HashMap<(Atom, SyntaxContext), HashMap<Atom, (Atom, SyntaxContext)>> =
        candidates
            .iter()
            .map(|c| {
                let prop_map: HashMap<Atom, (Atom, SyntaxContext)> = c
                    .props
                    .iter()
                    .map(|p| (p.exported.clone(), (p.local.clone(), p.local_ctxt)))
                    .collect();
                ((c.local_sym.clone(), c.local_ctxt), prop_map)
            })
            .collect();

    // Rewrite import declarations: remove default specifier, add named specifiers.
    // If a namespace specifier remains (`import * as ns`), named specifiers must
    // be emitted in a separate import declaration because `import * as ns, { x }`
    // is not valid JavaScript.
    let mut extra_named_imports: HashMap<usize, Vec<ImportDecl>> = HashMap::new();
    for candidate in candidates {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) =
            &mut module.body[candidate.import_index]
        else {
            continue;
        };

        // Collect names already present as named imports to avoid duplicates
        let already_imported: HashSet<Atom> = import
            .specifiers
            .iter()
            .filter_map(|s| match s {
                ImportSpecifier::Named(n) => Some(n.local.sym.clone()),
                _ => None,
            })
            .collect();

        // Remove the specific specifier we're decomposing (default or namespace).
        // Other specifiers on the same import — including a sibling default when
        // we decompose a namespace, or named specifiers — are left intact.
        if candidate.remove_original {
            import.specifiers.retain(|s| match s {
                ImportSpecifier::Default(d) => d.local.sym != candidate.local_sym,
                ImportSpecifier::Namespace(n) => n.local.sym != candidate.local_sym,
                ImportSpecifier::Named(_) => true,
            });
        }

        // Add new named specifiers for decomposed properties (skip if already present)
        let mut named_specifiers = Vec::new();
        for prop in &candidate.props {
            if already_imported.contains(&prop.local) {
                continue;
            }
            let imported = if prop.exported != prop.local {
                // Aliased import: `import { exported as local } from "..."`
                Some(ModuleExportName::Ident(Ident::new(
                    prop.exported.clone(),
                    DUMMY_SP,
                    SyntaxContext::empty(),
                )))
            } else {
                None
            };
            named_specifiers.push(ImportSpecifier::Named(ImportNamedSpecifier {
                span: DUMMY_SP,
                local: Ident::new(prop.local.clone(), DUMMY_SP, SyntaxContext::empty()),
                imported,
                is_type_only: false,
            }));
        }

        if named_specifiers.is_empty() {
            continue;
        }

        let has_remaining_namespace = import
            .specifiers
            .iter()
            .any(|s| matches!(s, ImportSpecifier::Namespace(_)));
        if has_remaining_namespace {
            extra_named_imports
                .entry(candidate.import_index)
                .or_default()
                .push(ImportDecl {
                    span: import.span,
                    specifiers: named_specifiers,
                    src: import.src.clone(),
                    type_only: import.type_only,
                    with: import.with.clone(),
                    phase: import.phase.clone(),
                });
        } else {
            import.specifiers.extend(named_specifiers);
        }
    }

    if !extra_named_imports.is_empty() {
        let mut new_body = Vec::with_capacity(module.body.len() + extra_named_imports.len());
        for (index, item) in std::mem::take(&mut module.body).into_iter().enumerate() {
            new_body.push(item);
            if let Some(imports) = extra_named_imports.remove(&index) {
                new_body.extend(
                    imports
                        .into_iter()
                        .map(|import| ModuleItem::ModuleDecl(ModuleDecl::Import(import))),
                );
            }
        }
        module.body = new_body;
    }

    // Rewrite usages: r.foo → local_name (which is `foo` or `foo_1` if aliased)
    let mut rewriter = UsageRewriter {
        decomp_map: &decomp_map,
    };
    module.visit_mut_with(&mut rewriter);
}

/// Rewrites `r.prop` → `local_name` for decomposed namespace bindings.
struct UsageRewriter<'a> {
    /// Maps (namespace_sym, ctxt) → { exported_name → (local_name, local_ctxt) }
    decomp_map: &'a HashMap<(Atom, SyntaxContext), HashMap<Atom, (Atom, SyntaxContext)>>,
}

impl VisitMut for UsageRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Member(member) = expr else { return };
        let Expr::Ident(obj) = member.obj.as_ref() else {
            return;
        };
        let key = (obj.sym.clone(), obj.ctxt);
        let Some(prop_map) = self.decomp_map.get(&key) else {
            return;
        };
        let MemberProp::Ident(prop) = &member.prop else {
            return;
        };
        if let Some((local_name, local_ctxt)) = prop_map.get(&prop.sym) {
            *expr = Expr::Ident(Ident::new(local_name.clone(), DUMMY_SP, *local_ctxt));
        }
    }

    fn visit_mut_prop_name(&mut self, _: &mut swc_core::ecma::ast::PropName) {}

    fn visit_mut_member_prop(&mut self, prop: &mut MemberProp) {
        if let MemberProp::Computed(prop) = prop {
            prop.visit_mut_with(self);
        }
    }
}
