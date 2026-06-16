//! Strip unused import specifiers. Runs late in the pipeline, after all
//! rewrites that might remove usages (JSX synthesis, SmartInline, etc.).
//!
//! For each `import` declaration:
//! - Specifiers whose local binding has no reference in the module body are
//!   removed.
//! - If all specifiers are stripped, the declaration becomes a side-effect
//!   import `import "./x.js";` — we don't delete it outright because the
//!   source module may have side effects on evaluation.
//! - Duplicate side-effect imports are collapsed. If any remaining binding
//!   import exists for the same source, side-effect-only imports for that source
//!   are removed because the binding import still evaluates the module.
//!
//! Property-name positions (`obj.foo`, `{foo: ...}` keys, JSX attribute names)
//! are not counted as references.

use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{BytePos, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    Ident, ImportDecl, ImportSpecifier, MemberProp, Module, ModuleDecl, ModuleItem, PropName, Str,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

pub struct DeadImports {
    pre_dead_spans: Option<HashSet<(BytePos, BytePos)>>,
}

impl DeadImports {
    pub fn full() -> Self {
        Self {
            pre_dead_spans: None,
        }
    }

    pub fn delta(pre_dead_spans: &HashSet<(BytePos, BytePos)>) -> Self {
        Self {
            pre_dead_spans: Some(pre_dead_spans.clone()),
        }
    }
}

impl VisitMut for DeadImports {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let referenced = collect_references(module);

        for item in &mut module.body {
            let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
                continue;
            };
            strip_unused_specifiers(import, &referenced, self.pre_dead_spans.as_ref());
        }
        dedup_side_effect_imports(module);

        module.visit_mut_children_with(self);
    }
}

fn collect_references(module: &Module) -> HashSet<(Atom, SyntaxContext)> {
    let mut collector = ReferenceCollector {
        refs: HashSet::new(),
    };
    module.visit_with(&mut collector);
    collector.refs
}

pub(crate) fn compute_pre_dead_import_spans(module: &Module) -> HashSet<(BytePos, BytePos)> {
    let referenced = collect_references(module);
    let mut dead_spans = HashSet::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        for spec in &import.specifiers {
            let (sym, ctxt, span) = match spec {
                ImportSpecifier::Default(s) => (s.local.sym.clone(), s.local.ctxt, s.local.span),
                ImportSpecifier::Namespace(s) => (s.local.sym.clone(), s.local.ctxt, s.local.span),
                ImportSpecifier::Named(s) => (s.local.sym.clone(), s.local.ctxt, s.local.span),
            };
            if !referenced.contains(&(sym, ctxt)) && span != DUMMY_SP {
                dead_spans.insert((span.lo, span.hi));
            }
        }
    }
    dead_spans
}

fn strip_unused_specifiers(
    import: &mut ImportDecl,
    referenced: &HashSet<(Atom, SyntaxContext)>,
    pre_dead_spans: Option<&HashSet<(BytePos, BytePos)>>,
) {
    import.specifiers.retain(|spec| {
        let (sym, ctxt, span) = match spec {
            ImportSpecifier::Default(s) => (s.local.sym.clone(), s.local.ctxt, s.local.span),
            ImportSpecifier::Namespace(s) => (s.local.sym.clone(), s.local.ctxt, s.local.span),
            ImportSpecifier::Named(s) => (s.local.sym.clone(), s.local.ctxt, s.local.span),
        };
        if referenced.contains(&(sym, ctxt)) {
            return true;
        }
        if let Some(pre_dead) = pre_dead_spans {
            if span != DUMMY_SP && pre_dead.contains(&(span.lo, span.hi)) {
                return true;
            }
        }
        false
    });
}

fn dedup_side_effect_imports(module: &mut Module) {
    let sources_with_bindings: HashSet<String> = module
        .body
        .iter()
        .filter_map(|item| {
            let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
                return None;
            };
            if import.specifiers.is_empty() {
                None
            } else {
                Some(import_source_key(&import.src))
            }
        })
        .collect();

    let mut seen_side_effect_sources = HashSet::new();
    module.body.retain(|item| {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            return true;
        };
        if !import.specifiers.is_empty() {
            return true;
        }

        let source = import_source_key(&import.src);
        if sources_with_bindings.contains(&source) {
            return false;
        }
        seen_side_effect_sources.insert(source)
    });
}

fn import_source_key(src: &Str) -> String {
    src.value.as_str().unwrap_or("").to_string()
}

/// Collects every (sym, ctxt) pair that appears as a reference in the module.
/// Skips import declarations (bindings aren't references) and property-name
/// positions in member access, object literals, and JSX attributes.
struct ReferenceCollector {
    refs: HashSet<(Atom, SyntaxContext)>,
}

impl Visit for ReferenceCollector {
    fn visit_import_decl(&mut self, _: &ImportDecl) {
        // Import specifier locals are bindings, not references.
    }

    fn visit_ident(&mut self, ident: &Ident) {
        self.refs.insert((ident.sym.clone(), ident.ctxt));
    }

    fn visit_prop_name(&mut self, prop: &PropName) {
        // Only a computed key is a real reference; identifier/string keys are
        // labels, not variable uses.
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
