//! Namespace decomposition: decompose default imports into named imports when
//! the imported binding is only used via property access and the target module
//! exports those properties.
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
    CatchClause, Expr, Function, Ident, ImportDecl, ImportNamedSpecifier, ImportSpecifier,
    MemberExpr, MemberProp, ModuleDecl, ModuleExportName, ModuleItem, Module, Param, Pat,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::facts::{ExportKind, ModuleFactsMap};

/// A candidate for namespace decomposition: a default import whose binding is used
/// only via property access, and the target module exports those properties.
struct DecompCandidate {
    /// Index of the import declaration in module.body
    import_index: usize,
    /// The local binding name and its SyntaxContext
    local_sym: Atom,
    local_ctxt: SyntaxContext,
    /// Properties accessed on this binding (e.g. `createStore`, `applyMiddleware`)
    accessed_props: Vec<Atom>,
}

/// Run namespace decomposition on a single module, using cross-module facts.
///
/// `module_facts` provides lookup from module specifier → facts of the target module.
/// This allows checking whether the target module actually exports the accessed names.
pub fn run_namespace_decomposition(
    module: &mut Module,
    module_facts: &ModuleFactsMap,
) {
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
    let existing_bindings = collector.bindings;

    // Step 1: Find default imports and their source modules
    let mut default_imports: Vec<(usize, Atom, SyntaxContext, Atom)> = Vec::new();
    for (idx, item) in module.body.iter().enumerate() {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        for spec in &import.specifiers {
            if let ImportSpecifier::Default(s) = spec {
                let source: Atom = import.src.value.as_str().unwrap_or("").into();
                default_imports.push((idx, s.local.sym.clone(), s.local.ctxt, source));
            }
        }
    }

    if default_imports.is_empty() {
        return Vec::new();
    }

    // Step 2: For each default import, analyze usage
    let mut candidates = Vec::new();
    for (import_index, local_sym, local_ctxt, source) in default_imports {
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

        // Check for naming collisions: skip if any accessed property name
        // would collide with an existing binding in the module (excluding the
        // candidate's own default import binding, which will be removed).
        let has_collision = analyzer.accessed_props.iter().any(|prop| {
            let is_own_binding = *prop == local_sym;
            !is_own_binding && existing_bindings.contains(prop)
        });
        if has_collision {
            continue;
        }

        let mut accessed_props: Vec<Atom> = analyzer.accessed_props.into_iter().collect();
        accessed_props.sort();

        candidates.push(DecompCandidate {
            import_index,
            local_sym,
            local_ctxt,
            accessed_props,
        });
    }

    candidates
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
}

impl Visit for UsageAnalyzer<'_> {
    fn visit_import_decl(&mut self, import: &ImportDecl) {
        // Don't count the import declaration itself as a usage
        self.in_import_decl = true;
        import.visit_children_with(self);
        self.in_import_decl = false;
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
    // Build a lookup: (sym, ctxt) → accessed_props
    let decomp_map: HashMap<(Atom, SyntaxContext), &[Atom]> = candidates
        .iter()
        .map(|c| ((c.local_sym.clone(), c.local_ctxt), c.accessed_props.as_slice()))
        .collect();

    // Rewrite import declarations: default → named
    for candidate in candidates {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) =
            &mut module.body[candidate.import_index]
        else {
            continue;
        };

        // Remove the default specifier, keep any existing named/namespace specifiers,
        // and add the new named specifiers for the decomposed properties.
        import.specifiers.retain(|s| !matches!(s, ImportSpecifier::Default(_)));
        for prop in &candidate.accessed_props {
            import.specifiers.push(ImportSpecifier::Named(ImportNamedSpecifier {
                span: DUMMY_SP,
                local: Ident::new(prop.clone(), DUMMY_SP, SyntaxContext::empty()),
                imported: None,
                is_type_only: false,
            }));
        }
    }

    // Rewrite usages: r.foo → foo
    // After decomposition, `r.foo.apply(undefined, args)` becomes
    // `foo.apply(undefined, args)` which UnArgumentSpread (Stage 3) handles
    // naturally as Pattern 1 (simple ident callee).
    let mut rewriter = UsageRewriter { decomp_map: &decomp_map };
    module.visit_mut_with(&mut rewriter);
}

/// Rewrites `r.prop` → `prop` for decomposed namespace bindings.
struct UsageRewriter<'a> {
    decomp_map: &'a HashMap<(Atom, SyntaxContext), &'a [Atom]>,
}

impl VisitMut for UsageRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Member(member) = expr else { return };
        let Expr::Ident(obj) = member.obj.as_ref() else {
            return;
        };
        let key = (obj.sym.clone(), obj.ctxt);
        let Some(props) = self.decomp_map.get(&key) else {
            return;
        };
        let MemberProp::Ident(prop) = &member.prop else {
            return;
        };
        if props.contains(&prop.sym) {
            *expr = Expr::Ident(Ident::new(prop.sym.clone(), DUMMY_SP, SyntaxContext::empty()));
        }
    }

    fn visit_mut_prop_name(&mut self, _: &mut swc_core::ecma::ast::PropName) {}

    fn visit_mut_member_prop(&mut self, prop: &mut MemberProp) {
        if let MemberProp::Computed(prop) = prop {
            prop.visit_mut_with(self);
        }
    }
}
