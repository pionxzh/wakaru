use swc_core::atoms::Atom;
use swc_core::common::Span;
use swc_core::ecma::ast::{
    Decl, DefaultDecl, Ident, ImportSpecifier, Module, ModuleDecl, ModuleItem, ObjectPatProp, Pat,
    Stmt,
};

use crate::analysis::BindingId;
use crate::collections::{HashMap, HashSet};

use super::types::BindingCorrespondence;

type SpanKey = (u32, u32);

#[derive(Debug, Clone)]
struct OriginBinding {
    evidence: Atom,
    spans: HashSet<SpanKey>,
}

/// Binding declarations captured before the readability pipeline mutates their
/// emitted names. Declaration spans are the generic origin token: normal
/// binding renames preserve them, while synthesized or replaced declarations
/// fail closed because they no longer have a matching origin.
#[derive(Debug, Clone, Default)]
pub(super) struct TopLevelBindingSnapshot {
    origins: HashMap<BindingId, OriginBinding>,
}

impl TopLevelBindingSnapshot {
    pub(super) fn collect(module: &Module) -> Self {
        let mut collector = TopLevelBindingCollector::default();
        collector.collect_module(module);
        Self {
            origins: collector
                .bindings
                .into_iter()
                .map(|(binding, spans)| {
                    (
                        binding.clone(),
                        OriginBinding {
                            evidence: binding.0,
                            spans,
                        },
                    )
                })
                .collect(),
        }
    }

    pub(super) fn correspondences(&self, module: &Module) -> Vec<BindingCorrespondence> {
        let mut readable = TopLevelBindingCollector::default();
        readable.collect_module(module);
        let mut names_by_span = HashMap::<SpanKey, Option<Atom>>::default();
        for (binding, spans) in readable.bindings {
            for span in spans {
                names_by_span
                    .entry(span)
                    .and_modify(|existing| {
                        if existing.as_ref() != Some(&binding.0) {
                            *existing = None;
                        }
                    })
                    .or_insert_with(|| Some(binding.0.clone()));
            }
        }

        let mut candidates = Vec::new();
        for origin in self.origins.values() {
            let names = origin
                .spans
                .iter()
                .filter_map(|span| names_by_span.get(span).and_then(Option::as_ref))
                .cloned()
                .collect::<HashSet<_>>();
            if names.len() != 1 {
                continue;
            }
            let readable = names
                .into_iter()
                .next()
                .expect("one readable name was established");
            candidates.push(BindingCorrespondence {
                evidence: origin.evidence.to_string(),
                readable: readable.to_string(),
            });
        }

        let mut evidence_counts = HashMap::<String, usize>::default();
        let mut readable_counts = HashMap::<String, usize>::default();
        for candidate in &candidates {
            *evidence_counts
                .entry(candidate.evidence.clone())
                .or_default() += 1;
            *readable_counts
                .entry(candidate.readable.clone())
                .or_default() += 1;
        }
        candidates.retain(|candidate| {
            evidence_counts.get(&candidate.evidence) == Some(&1)
                && readable_counts.get(&candidate.readable) == Some(&1)
        });
        candidates.sort_by(|left, right| left.evidence.cmp(&right.evidence));
        candidates
    }
}

#[derive(Default)]
struct TopLevelBindingCollector {
    bindings: HashMap<BindingId, HashSet<SpanKey>>,
}

impl TopLevelBindingCollector {
    fn collect_module(&mut self, module: &Module) {
        for item in &module.body {
            match item {
                ModuleItem::Stmt(Stmt::Decl(declaration)) => self.collect_decl(declaration),
                ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                    for specifier in &import.specifiers {
                        let local = match specifier {
                            ImportSpecifier::Named(named) => &named.local,
                            ImportSpecifier::Default(default) => &default.local,
                            ImportSpecifier::Namespace(namespace) => &namespace.local,
                        };
                        self.record(local);
                    }
                }
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                    self.collect_decl(&export.decl)
                }
                ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(export)) => {
                    match &export.decl {
                        DefaultDecl::Class(class) => {
                            if let Some(identifier) = &class.ident {
                                self.record(identifier);
                            }
                        }
                        DefaultDecl::Fn(function) => {
                            if let Some(identifier) = &function.ident {
                                self.record(identifier);
                            }
                        }
                        DefaultDecl::TsInterfaceDecl(_) => {}
                    }
                }
                _ => {}
            }
        }
    }

    fn collect_decl(&mut self, declaration: &Decl) {
        match declaration {
            Decl::Class(class) => self.record(&class.ident),
            Decl::Fn(function) => self.record(&function.ident),
            Decl::Var(variable) => {
                for declarator in &variable.decls {
                    self.collect_pattern(&declarator.name);
                }
            }
            _ => {}
        }
    }

    fn collect_pattern(&mut self, pattern: &Pat) {
        match pattern {
            Pat::Ident(binding) => self.record(&binding.id),
            Pat::Array(array) => {
                for element in array.elems.iter().flatten() {
                    self.collect_pattern(element);
                }
            }
            Pat::Object(object) => {
                for property in &object.props {
                    match property {
                        ObjectPatProp::KeyValue(property) => {
                            self.collect_pattern(property.value.as_ref())
                        }
                        ObjectPatProp::Assign(property) => self.record(&property.key.id),
                        ObjectPatProp::Rest(rest) => self.collect_pattern(rest.arg.as_ref()),
                    }
                }
            }
            Pat::Assign(assign) => self.collect_pattern(assign.left.as_ref()),
            Pat::Rest(rest) => self.collect_pattern(rest.arg.as_ref()),
            Pat::Expr(_) | Pat::Invalid(_) => {}
        }
    }

    fn record(&mut self, identifier: &Ident) {
        let span = span_key(identifier.span);
        if span == (0, 0) {
            return;
        }
        self.bindings
            .entry((identifier.sym.clone(), identifier.ctxt))
            .or_default()
            .insert(span);
    }
}

fn span_key(span: Span) -> SpanKey {
    (span.lo.0, span.hi.0)
}
