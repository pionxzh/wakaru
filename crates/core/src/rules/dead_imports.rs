//! Strip unused import specifiers. Runs late in the pipeline, after all
//! rewrites that might remove usages (JSX synthesis, SmartInline, etc.).
//!
//! In full DCE mode, specifiers whose local binding has no reference in the
//! module body are removed. Transform-only DCE removes only newly synthesized
//! specifiers: original default/named imports are observable ESM link-time
//! export checks even when a rewrite removes their last runtime read.
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
use swc_core::ecma::ast::{ImportDecl, ImportSpecifier, Module, ModuleDecl, ModuleItem, Str};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use crate::analysis::binding_uses::BindingUseIndex;

pub struct DeadImports {
    pre_existing_spans: Option<HashSet<(BytePos, BytePos)>>,
}

impl DeadImports {
    pub fn full() -> Self {
        Self {
            pre_existing_spans: None,
        }
    }

    pub fn delta(pre_existing_spans: &HashSet<(BytePos, BytePos)>) -> Self {
        Self {
            pre_existing_spans: Some(pre_existing_spans.clone()),
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
            strip_unused_specifiers(import, &referenced, self.pre_existing_spans.as_ref());
        }
        dedup_side_effect_imports(module);

        module.visit_mut_children_with(self);
    }
}

fn collect_references(module: &Module) -> HashSet<(Atom, SyntaxContext)> {
    BindingUseIndex::collect(module).referenced_bindings()
}

pub(crate) fn compute_pre_existing_import_spans(module: &Module) -> HashSet<(BytePos, BytePos)> {
    let mut spans = HashSet::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        for spec in &import.specifiers {
            let span = match spec {
                ImportSpecifier::Default(s) => s.local.span,
                ImportSpecifier::Namespace(s) => s.local.span,
                ImportSpecifier::Named(s) => s.local.span,
            };
            if span != DUMMY_SP {
                spans.insert((span.lo, span.hi));
            }
        }
    }
    spans
}

fn strip_unused_specifiers(
    import: &mut ImportDecl,
    referenced: &HashSet<(Atom, SyntaxContext)>,
    pre_existing_spans: Option<&HashSet<(BytePos, BytePos)>>,
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
        if let Some(pre_existing) = pre_existing_spans {
            if span != DUMMY_SP && pre_existing.contains(&(span.lo, span.hi)) {
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
