//! Best-effort recovery of production Angular Ivy component and module
//! inspection artifacts.
//!
//! The analyzer consumes ordinary resolved JavaScript modules. Bundle-format
//! concerns stay in unpackers; this module knows only module ASTs and semantic
//! Ivy instruction identities.

mod artifact;
mod emitter;
mod roles;
mod syntax;
mod template;
mod workspace;

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use rayon::prelude::*;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, Globals, Mark, SourceMap, SyntaxContext, GLOBALS};
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, CallExpr, Class, ClassDecl, Expr, Function, Module, ObjectLit, Pat,
    Prop, PropOrSpread, SimpleAssignTarget, VarDeclarator,
};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};

use crate::js_names::{is_likely_generated_alias, to_valid_identifier_name};
use crate::rules::rename_utils::BindingRename;
use artifact::{class_references, dependency_binding, ArtifactSymbolTable};
use emitter::{
    clean_component_class, emit_angular_module_source, emit_component_source, ComponentEmitInput,
    ModuleComponentEmitInput,
};
use roles::{symbol_identity, IvyInstruction, IvyRoleTable, SymbolIdentity};
use syntax::{prop_name, string_lit, wtf8_to_string, BindingKey};
use template::{
    ivy_template_score, recover_template, TemplateFunctionTable, TemplateRecoveryContext,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AngularRecoveryCompleteness {
    Complete,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AngularRecoveryIssueKind {
    UnsupportedTemplateParameters,
    UnsupportedStatement,
    UnsupportedExpression,
    UnsupportedInstruction,
    UnknownRuntimeInstruction,
    MalformedInstruction,
    MissingTargetNode,
    MalformedTemplateStructure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AngularRecoveryIssue {
    pub kind: AngularRecoveryIssueKind,
    /// Index of the source module containing the affected component.
    pub module_index: Option<usize>,
    /// Recovered component identity, when component discovery succeeded.
    pub component: Option<String>,
    /// Deterministic depth-first view identity within the component.
    pub view_id: Option<usize>,
    /// Render phase containing the affected operation, when known.
    pub phase: Option<AngularTemplatePhase>,
    /// Zero-based operation ordinal within the affected view.
    pub operation_index: Option<usize>,
    /// Module-relative, end-exclusive byte range of the affected source.
    pub source_range: Option<AngularRecoverySourceRange>,
    /// Canonical Ivy role, when known.
    pub instruction: Option<String>,
    /// Concise callee spelling observed in the compiled source, when known.
    pub actual_callee: Option<String>,
    /// Concise reason the operation could not be recovered.
    pub detail: Option<String>,
}

impl AngularRecoveryIssue {
    fn new(
        kind: AngularRecoveryIssueKind,
        instruction: Option<String>,
        detail: Option<String>,
    ) -> Self {
        Self {
            kind,
            module_index: None,
            component: None,
            view_id: None,
            phase: None,
            operation_index: None,
            source_range: None,
            instruction,
            actual_callee: None,
            detail,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct AngularRecoverySourceRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct AngularTemplateRecoveryStats {
    pub runtime_calls_observed: usize,
    pub rendered_instruction_calls: usize,
    pub unsupported_runtime_calls: usize,
    pub malformed_instruction_calls: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum AngularTemplatePhase {
    Creation,
    Update,
    OutsideRender,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AngularUnknownRuntimeCallShape {
    pub phase: AngularTemplatePhase,
    pub argument_counts: Vec<usize>,
    pub occurrences: usize,
    pub runtime_calls: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecoveredAngularComponent {
    pub name: String,
    pub selector: String,
    pub source: String,
    pub completeness: AngularRecoveryCompleteness,
    pub issues: Vec<AngularRecoveryIssue>,
    pub stats: AngularTemplateRecoveryStats,
    pub unknown_runtime_call_shapes: Vec<AngularUnknownRuntimeCallShape>,
    /// Index into the `AngularModuleSource` slice that contained the
    /// component definition.
    pub module_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecoveredAngularModule {
    pub source: String,
    pub completeness: AngularRecoveryCompleteness,
    /// Indices into `AngularRecoveryReport::components`.
    pub component_indices: Vec<usize>,
    /// Proven component edges to recovered artifacts in other source modules.
    pub dependencies: Vec<RecoveredAngularModuleDependency>,
    pub issues: Vec<AngularRecoveryIssue>,
    /// Index into the analyzed `AngularModuleSource`/`AngularModuleView` slice.
    pub module_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecoveredAngularModuleDependency {
    /// Index of the component containing the dependency.
    pub component_index: usize,
    /// Index of the recovered target component.
    pub target_component_index: usize,
    pub target_module_index: usize,
    /// Exported class name in the target artifact.
    pub target_name: String,
    /// Collision-free local name used by this artifact.
    pub local_name: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct AngularRecoveryStats {
    pub modules_analyzed: usize,
    pub component_candidates: usize,
    pub recovered_components: usize,
    pub rejected_component_candidates: usize,
    pub complete_components: usize,
    pub partial_components: usize,
    pub runtime_calls_observed: usize,
    pub rendered_instruction_calls: usize,
    pub unsupported_runtime_calls: usize,
    pub malformed_instruction_calls: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AngularRecoveryReport {
    pub components: Vec<RecoveredAngularComponent>,
    pub modules: Vec<RecoveredAngularModule>,
    pub stats: AngularRecoveryStats,
    pub unknown_runtime_call_shapes: Vec<AngularUnknownRuntimeCallShape>,
}

#[derive(Debug, Clone, Copy)]
pub struct AngularModuleSource<'a> {
    pub filename: &'a str,
    pub source: &'a str,
}

/// Two views of one module used by root artifact recovery.
///
/// `evidence_source` is captured before readability rewrites can erase
/// compiler-runtime shapes. `readable_source` is the finalized JavaScript
/// whose class body should be used when the same component binding can be
/// matched conservatively.
#[derive(Debug, Clone, Copy)]
pub struct AngularModuleView<'a> {
    pub filename: &'a str,
    pub evidence_source: &'a str,
    pub readable_source: &'a str,
}

#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct AngularRecoveryOptions {}

struct PreparedAngularModule {
    filename: String,
    module: Module,
    unresolved_ctxt: SyntaxContext,
    source_start_pos: u32,
}

#[derive(Clone)]
struct ComponentClass {
    name: Atom,
    class: Box<Class>,
    identity: SymbolIdentity,
    portable_identity: PortableSymbolIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PortableSymbolIdentity {
    LocalBinding(Atom),
    LocalMember { object: Atom, property: Atom },
    GlobalBinding(Atom),
    GlobalMember { object: Atom, property: Atom },
}

struct ComponentDescriptor {
    class: ComponentClass,
    selector: String,
    styles: Vec<String>,
    projection_selectors: Vec<String>,
    dependencies: Vec<Box<Expr>>,
    template: Function,
    constants: Option<Box<Expr>>,
}

struct RecoveredModuleComponentDraft {
    component_index: usize,
    name: String,
    selector: String,
    styles: Vec<String>,
    class: Box<Class>,
    template_source: String,
    readable_class_roots: HashSet<BindingKey>,
    template_roots: HashSet<BindingKey>,
    dependencies: Vec<Box<Expr>>,
    evidence_class_identity: SymbolIdentity,
    readable_class_identity: SymbolIdentity,
    completeness: AngularRecoveryCompleteness,
    issues: Vec<AngularRecoveryIssue>,
}

#[derive(Clone)]
struct RecoveredComponentTarget {
    component_index: usize,
    module_index: usize,
    name: String,
}

struct ComponentAliasResolver {
    aliases: workspace::WorkspaceAliasIndex,
    targets_by_group: HashMap<usize, Vec<RecoveredComponentTarget>>,
    targets_by_symbol: HashMap<workspace::WorkspaceSymbol, Vec<RecoveredComponentTarget>>,
}

struct ComponentRelationshipIndex {
    evidence: ComponentAliasResolver,
    readable: ComponentAliasResolver,
}

pub fn recover_angular_components_from_js(
    source: &str,
    options: AngularRecoveryOptions,
) -> Result<Vec<RecoveredAngularComponent>> {
    Ok(analyze_angular_components_from_js(source, options)?.components)
}

pub fn recover_angular_modules_from_js(
    source: &str,
    options: AngularRecoveryOptions,
) -> Result<Vec<RecoveredAngularModule>> {
    Ok(analyze_angular_components_from_js(source, options)?.modules)
}

pub fn analyze_angular_components_from_js(
    source: &str,
    options: AngularRecoveryOptions,
) -> Result<AngularRecoveryReport> {
    analyze_angular_components_from_modules(
        &[AngularModuleSource {
            filename: "angular-recovery.js",
            source,
        }],
        options,
    )
}

pub fn recover_angular_components_from_modules(
    sources: &[AngularModuleSource<'_>],
    options: AngularRecoveryOptions,
) -> Result<Vec<RecoveredAngularComponent>> {
    Ok(analyze_angular_components_from_modules(sources, options)?.components)
}

pub fn recover_angular_modules_from_modules(
    sources: &[AngularModuleSource<'_>],
    options: AngularRecoveryOptions,
) -> Result<Vec<RecoveredAngularModule>> {
    Ok(analyze_angular_components_from_modules(sources, options)?.modules)
}

pub fn analyze_angular_components_from_modules(
    sources: &[AngularModuleSource<'_>],
    _options: AngularRecoveryOptions,
) -> Result<AngularRecoveryReport> {
    let globals = Globals::new();
    GLOBALS.set(&globals, || {
        let modules = {
            let span = tracing::info_span!("angular: prepare modules", count = sources.len());
            let _enter = span.enter();
            sources
                .par_iter()
                .map(|source| GLOBALS.set(&globals, || prepare_module(source)))
                .collect::<Result<Vec<_>>>()?
        };
        recover_prepared_modules(&modules, None)
    })
}

pub fn recover_angular_components_from_module_views(
    views: &[AngularModuleView<'_>],
    options: AngularRecoveryOptions,
) -> Result<Vec<RecoveredAngularComponent>> {
    Ok(analyze_angular_components_from_module_views(views, options)?.components)
}

pub fn recover_angular_modules_from_module_views(
    views: &[AngularModuleView<'_>],
    options: AngularRecoveryOptions,
) -> Result<Vec<RecoveredAngularModule>> {
    Ok(analyze_angular_components_from_module_views(views, options)?.modules)
}

pub fn analyze_angular_components_from_module_views(
    views: &[AngularModuleView<'_>],
    _options: AngularRecoveryOptions,
) -> Result<AngularRecoveryReport> {
    let globals = Globals::new();
    GLOBALS.set(&globals, || {
        let (evidence_modules, readable_modules) = {
            let span = tracing::info_span!("angular: prepare module views", count = views.len());
            let _enter = span.enter();
            rayon::join(
                || {
                    views
                        .par_iter()
                        .map(|view| {
                            GLOBALS.set(&globals, || {
                                prepare_module(&AngularModuleSource {
                                    filename: view.filename,
                                    source: view.evidence_source,
                                })
                            })
                        })
                        .collect::<Result<Vec<_>>>()
                },
                || {
                    views
                        .par_iter()
                        .map(|view| {
                            GLOBALS.set(&globals, || {
                                prepare_module(&AngularModuleSource {
                                    filename: view.filename,
                                    source: view.readable_source,
                                })
                            })
                        })
                        .collect::<Result<Vec<_>>>()
                },
            )
        };
        let evidence_modules = evidence_modules?;
        let readable_modules = readable_modules?;
        recover_prepared_modules(&evidence_modules, Some(&readable_modules))
    })
}

fn recover_prepared_modules(
    evidence_modules: &[PreparedAngularModule],
    readable_modules: Option<&[PreparedAngularModule]>,
) -> Result<AngularRecoveryReport> {
    let recovery_span = tracing::info_span!(
        "angular: recover prepared modules",
        count = evidence_modules.len()
    );
    let _recovery_enter = recovery_span.enter();
    let readable_modules = readable_modules.unwrap_or(evidence_modules);
    let roles = {
        let span = tracing::info_span!("angular: infer Ivy roles");
        let _enter = span.enter();
        IvyRoleTable::collect(evidence_modules)
    };
    let (evidence_artifact_symbols, readable_artifact_symbols, readable_classes) = {
        let span = tracing::info_span!("angular: index artifact symbols");
        let _enter = span.enter();
        let evidence_artifact_symbols = evidence_modules
            .iter()
            .map(|prepared| ArtifactSymbolTable::collect(&prepared.module))
            .collect::<Vec<_>>();
        let readable_artifact_symbols = readable_modules
            .iter()
            .map(|prepared| ArtifactSymbolTable::collect(&prepared.module))
            .collect::<Vec<_>>();
        let readable_classes = readable_modules
            .iter()
            .map(|prepared| {
                collect_portable_component_classes(&prepared.module, prepared.unresolved_ctxt)
            })
            .collect::<Vec<_>>();
        (
            evidence_artifact_symbols,
            readable_artifact_symbols,
            readable_classes,
        )
    };

    let mut recovered = Vec::new();
    let mut recovered_module_drafts = Vec::with_capacity(evidence_modules.len());
    let mut stats = AngularRecoveryStats {
        modules_analyzed: evidence_modules.len(),
        ..AngularRecoveryStats::default()
    };
    let mut unknown_runtime_call_shapes =
        HashMap::<(AngularTemplatePhase, Vec<usize>), (usize, usize)>::new();
    let component_span = tracing::info_span!("angular: recover components");
    let _component_enter = component_span.enter();
    for (module_index, prepared) in evidence_modules.iter().enumerate() {
        let emit_cm: Lrc<SourceMap> = Default::default();
        let mut module_drafts = Vec::new();
        let mut recovered_names = HashSet::new();
        let classes = collect_component_classes(&prepared.module, prepared.unresolved_ctxt);
        let template_functions = TemplateFunctionTable::collect(&prepared.module);
        let mut calls = roles::IvyCallCollector::new(&roles, prepared.unresolved_ctxt);
        prepared.module.visit_with(&mut calls);

        for candidate in &calls.define_component_calls {
            stats.component_candidates += 1;
            let call = &candidate.call;
            let Some(descriptor) = parse_component_descriptor(
                call,
                &classes,
                &roles,
                &template_functions,
                prepared.unresolved_ctxt,
            ) else {
                stats.rejected_component_candidates += 1;
                continue;
            };
            let mut recovered_template = recover_template(
                &descriptor.template,
                descriptor.constants.as_deref(),
                &descriptor.projection_selectors,
                &roles,
                &template_functions,
                TemplateRecoveryContext {
                    unresolved_ctxt: prepared.unresolved_ctxt,
                    source_start_pos: prepared.source_start_pos,
                    cm: emit_cm.clone(),
                },
            )?;
            let readable_class = readable_classes[module_index]
                .get(&descriptor.class.portable_identity)
                .unwrap_or(&descriptor.class);
            let name = unique_recovered_component_name(
                recovered_component_name(readable_class.name.as_ref(), &descriptor.selector),
                &mut recovered_names,
            );
            for issue in &mut recovered_template.issues {
                issue.module_index = Some(module_index);
                issue.component = Some(name.clone());
            }
            let class = clean_component_class(
                &readable_class.class,
                candidate.definition_field.as_ref(),
                &roles,
                prepared.unresolved_ctxt,
            );
            let reserved_names = HashSet::from([
                Atom::from("Component"),
                readable_class.name.clone(),
                Atom::from(name.as_str()),
            ]);
            let mut class_roots = class_references(&class);
            class_roots.retain(|root| root.0 != readable_class.name);
            let mut support = readable_artifact_symbols[module_index].recover(
                &class_roots,
                &reserved_names,
                true,
            );
            support.merge(evidence_artifact_symbols[module_index].recover(
                &recovered_template.artifact_references,
                &reserved_names,
                true,
            ));
            let dependency_roots = descriptor
                .dependencies
                .iter()
                .filter_map(|dependency| dependency_binding(dependency.as_ref()))
                .collect::<HashSet<_>>();
            let dependency_support = evidence_artifact_symbols[module_index].recover(
                &dependency_roots,
                &reserved_names,
                false,
            );
            let dependencies = descriptor
                .dependencies
                .iter()
                .filter_map(|dependency| {
                    let binding = dependency_binding(dependency.as_ref())?;
                    dependency_support
                        .provides(&binding)
                        .then(|| binding.0.to_string())
                })
                .collect::<Vec<_>>();
            support.merge(dependency_support);
            let source = emit_component_source(
                ComponentEmitInput {
                    name: &name,
                    selector: &descriptor.selector,
                    styles: &descriptor.styles,
                    class: &class,
                    template: &recovered_template,
                    support: &support,
                    dependencies: &dependencies,
                },
                emit_cm.clone(),
            )?;
            for shape in &recovered_template.unknown_runtime_call_shapes {
                let aggregate = unknown_runtime_call_shapes
                    .entry((shape.phase, shape.argument_counts.clone()))
                    .or_default();
                aggregate.0 += shape.occurrences;
                aggregate.1 += shape.runtime_calls;
            }
            let completeness = if recovered_template.issues.is_empty() {
                stats.complete_components += 1;
                AngularRecoveryCompleteness::Complete
            } else {
                stats.partial_components += 1;
                AngularRecoveryCompleteness::Partial
            };
            let component_index = recovered.len();
            module_drafts.push(RecoveredModuleComponentDraft {
                component_index,
                name: name.clone(),
                selector: descriptor.selector.clone(),
                styles: descriptor.styles.clone(),
                class: class.clone(),
                template_source: recovered_template.source.clone(),
                readable_class_roots: class_roots,
                template_roots: recovered_template.artifact_references.clone(),
                dependencies: descriptor.dependencies.clone(),
                evidence_class_identity: descriptor.class.identity.clone(),
                readable_class_identity: readable_class.identity.clone(),
                completeness,
                issues: recovered_template.issues.clone(),
            });
            recovered.push(RecoveredAngularComponent {
                name,
                selector: descriptor.selector,
                source,
                completeness,
                issues: recovered_template.issues,
                stats: recovered_template.stats,
                unknown_runtime_call_shapes: recovered_template.unknown_runtime_call_shapes,
                module_index,
            });
            stats.recovered_components += 1;
            stats.runtime_calls_observed += recovered_template.stats.runtime_calls_observed;
            stats.rendered_instruction_calls += recovered_template.stats.rendered_instruction_calls;
            stats.unsupported_runtime_calls += recovered_template.stats.unsupported_runtime_calls;
            stats.malformed_instruction_calls +=
                recovered_template.stats.malformed_instruction_calls;
        }
        recovered_module_drafts.push(module_drafts);
    }
    drop(_component_enter);

    let recovered_modules = {
        let span = tracing::info_span!("angular: link module artifacts");
        let _enter = span.enter();
        let relationships = ComponentRelationshipIndex::new(
            evidence_modules,
            readable_modules,
            &recovered_module_drafts,
        );
        recovered_module_drafts
            .iter()
            .enumerate()
            .filter(|(_, drafts)| !drafts.is_empty())
            .map(|(module_index, drafts)| {
                emit_recovered_angular_module(
                    module_index,
                    drafts,
                    &evidence_artifact_symbols[module_index],
                    &readable_artifact_symbols[module_index],
                    &relationships,
                    Default::default(),
                )
            })
            .collect::<Result<Vec<_>>>()?
    };

    let mut unknown_runtime_call_shapes = unknown_runtime_call_shapes
        .into_iter()
        .map(|((phase, argument_counts), (occurrences, runtime_calls))| {
            AngularUnknownRuntimeCallShape {
                phase,
                argument_counts,
                occurrences,
                runtime_calls,
            }
        })
        .collect::<Vec<_>>();
    unknown_runtime_call_shapes.sort_by(|left, right| {
        left.phase
            .cmp(&right.phase)
            .then_with(|| left.argument_counts.cmp(&right.argument_counts))
    });

    Ok(AngularRecoveryReport {
        components: recovered,
        modules: recovered_modules,
        stats,
        unknown_runtime_call_shapes,
    })
}

impl ComponentRelationshipIndex {
    fn new(
        evidence_modules: &[PreparedAngularModule],
        readable_modules: &[PreparedAngularModule],
        drafts: &[Vec<RecoveredModuleComponentDraft>],
    ) -> Self {
        Self {
            evidence: ComponentAliasResolver::new(evidence_modules, drafts, false),
            readable: ComponentAliasResolver::new(readable_modules, drafts, true),
        }
    }
}

impl ComponentAliasResolver {
    fn new(
        modules: &[PreparedAngularModule],
        drafts: &[Vec<RecoveredModuleComponentDraft>],
        readable: bool,
    ) -> Self {
        let aliases = workspace::WorkspaceAliasIndex::collect(modules);
        let mut targets_by_group = HashMap::<usize, Vec<RecoveredComponentTarget>>::new();
        let mut targets_by_symbol =
            HashMap::<workspace::WorkspaceSymbol, Vec<RecoveredComponentTarget>>::new();
        for (module_index, module_drafts) in drafts.iter().enumerate() {
            for draft in module_drafts {
                let identity = if readable {
                    &draft.readable_class_identity
                } else {
                    &draft.evidence_class_identity
                };
                let Some(symbol) = workspace_symbol(identity) else {
                    continue;
                };
                let target = RecoveredComponentTarget {
                    component_index: draft.component_index,
                    module_index,
                    name: draft.name.clone(),
                };
                if let Some(group) = aliases.group(&symbol) {
                    targets_by_group.entry(group).or_default().push(target);
                } else {
                    targets_by_symbol.entry(symbol).or_default().push(target);
                }
            }
        }
        Self {
            aliases,
            targets_by_group,
            targets_by_symbol,
        }
    }

    fn resolve_binding(&self, binding: &BindingKey) -> Option<RecoveredComponentTarget> {
        self.resolve_symbol(&workspace::WorkspaceSymbol::Binding(binding.clone()))
    }

    fn resolve_symbol(
        &self,
        symbol: &workspace::WorkspaceSymbol,
    ) -> Option<RecoveredComponentTarget> {
        let candidates = if let Some(group) = self.aliases.group(symbol) {
            self.targets_by_group.get(&group)?
        } else {
            self.targets_by_symbol.get(symbol)?
        };
        let first = candidates.first()?;
        candidates
            .iter()
            .all(|candidate| candidate.component_index == first.component_index)
            .then(|| first.clone())
    }
}

fn workspace_symbol(identity: &SymbolIdentity) -> Option<workspace::WorkspaceSymbol> {
    match identity {
        SymbolIdentity::LocalBinding(binding) => {
            Some(workspace::WorkspaceSymbol::Binding(binding.clone()))
        }
        SymbolIdentity::LocalMember { object, property } => {
            Some(workspace::WorkspaceSymbol::Member {
                object: object.clone(),
                property: property.clone(),
            })
        }
        SymbolIdentity::GlobalBinding(_) | SymbolIdentity::GlobalMember { .. } => None,
    }
}

fn emit_recovered_angular_module(
    module_index: usize,
    drafts: &[RecoveredModuleComponentDraft],
    evidence_symbols: &ArtifactSymbolTable,
    readable_symbols: &ArtifactSymbolTable,
    relationships: &ComponentRelationshipIndex,
    cm: Lrc<SourceMap>,
) -> Result<RecoveredAngularModule> {
    let mut evidence_component_names = HashMap::<BindingKey, String>::new();
    let mut readable_component_bindings = HashSet::new();
    let mut evidence_component_bindings = HashSet::new();
    let mut renames = Vec::new();
    let mut renamed_bindings = HashSet::new();
    let mut reserved_names = HashSet::from([Atom::from("Component")]);

    for draft in drafts {
        let recovered_name = Atom::from(draft.name.as_str());
        reserved_names.insert(recovered_name.clone());
        for identity in [
            &draft.evidence_class_identity,
            &draft.readable_class_identity,
        ] {
            let Some(binding) = local_identity_binding(identity) else {
                continue;
            };
            reserved_names.insert(binding.0.clone());
            if renamed_bindings.insert(binding.clone()) {
                renames.push(BindingRename {
                    old: binding.clone(),
                    new: recovered_name.clone(),
                });
            }
        }
        if let Some(binding) = local_identity_binding(&draft.evidence_class_identity) {
            evidence_component_bindings.insert(binding.clone());
            evidence_component_names.insert(binding.clone(), draft.name.clone());
        }
        if let Some(binding) = local_identity_binding(&draft.readable_class_identity) {
            readable_component_bindings.insert(binding.clone());
        }
    }

    let readable_roots = drafts
        .iter()
        .flat_map(|draft| draft.readable_class_roots.iter().cloned())
        .collect::<HashSet<_>>();
    let template_roots = drafts
        .iter()
        .flat_map(|draft| draft.template_roots.iter().cloned())
        .collect::<HashSet<_>>();
    let dependency_roots = drafts
        .iter()
        .flat_map(|draft| {
            draft
                .dependencies
                .iter()
                .filter_map(|dependency| dependency_binding(dependency.as_ref()))
        })
        .collect::<HashSet<_>>();

    let readable_target_bindings = readable_symbols
        .bindings()
        .filter_map(|binding| {
            relationships
                .readable
                .resolve_binding(binding)
                .map(|target| (target.component_index, binding.clone()))
        })
        .fold(
            HashMap::<usize, Vec<BindingKey>>::new(),
            |mut bindings, (component_index, binding)| {
                bindings.entry(component_index).or_default().push(binding);
                bindings
            },
        );
    let mut linked_dependencies = HashMap::<BindingKey, (RecoveredComponentTarget, String)>::new();
    let mut local_names_by_target = HashMap::<usize, String>::new();
    let mut module_dependencies = Vec::new();
    let mut seen_module_dependencies = HashSet::new();
    for draft in drafts {
        for dependency in &draft.dependencies {
            let Some(binding) = dependency_binding(dependency.as_ref()) else {
                continue;
            };
            if evidence_component_names.contains_key(&binding) || template_roots.contains(&binding)
            {
                continue;
            }
            let Some(target) = relationships.evidence.resolve_binding(&binding) else {
                continue;
            };
            if target.module_index == module_index {
                continue;
            }
            if let Some((existing, _)) = linked_dependencies.get(&binding) {
                if existing.component_index != target.component_index {
                    continue;
                }
            }
            let local_name = if let Some(name) = local_names_by_target.get(&target.component_index)
            {
                name.clone()
            } else {
                let name = reserve_unique_artifact_name(&target.name, &mut reserved_names);
                local_names_by_target.insert(target.component_index, name.clone());
                name
            };
            linked_dependencies.insert(binding.clone(), (target.clone(), local_name.clone()));
            evidence_component_bindings.insert(binding.clone());
            reserved_names.insert(binding.0.clone());
            if renamed_bindings.insert(binding.clone()) {
                renames.push(BindingRename {
                    old: binding,
                    new: Atom::from(local_name.as_str()),
                });
            }
            if let Some(readable_bindings) = readable_target_bindings.get(&target.component_index) {
                for readable_binding in readable_bindings {
                    readable_component_bindings.insert(readable_binding.clone());
                    reserved_names.insert(readable_binding.0.clone());
                    if renamed_bindings.insert(readable_binding.clone()) {
                        renames.push(BindingRename {
                            old: readable_binding.clone(),
                            new: Atom::from(local_name.as_str()),
                        });
                    }
                }
            }
            if seen_module_dependencies.insert((draft.component_index, target.component_index)) {
                module_dependencies.push(RecoveredAngularModuleDependency {
                    component_index: draft.component_index,
                    target_component_index: target.component_index,
                    target_module_index: target.module_index,
                    target_name: target.name,
                    local_name,
                });
            }
        }
    }

    let mut support = readable_symbols.recover_with_provided(
        &readable_roots,
        &reserved_names,
        &readable_component_bindings,
        true,
    );
    support.merge(evidence_symbols.recover_with_provided(
        &template_roots,
        &reserved_names,
        &evidence_component_bindings,
        true,
    ));
    let dependency_support = evidence_symbols.recover_with_provided(
        &dependency_roots,
        &reserved_names,
        &evidence_component_bindings,
        false,
    );
    let dependency_names = drafts
        .iter()
        .map(|draft| {
            let mut seen = HashSet::new();
            draft
                .dependencies
                .iter()
                .filter_map(|dependency| {
                    let binding = dependency_binding(dependency.as_ref())?;
                    evidence_component_names
                        .get(&binding)
                        .cloned()
                        .or_else(|| {
                            linked_dependencies
                                .get(&binding)
                                .map(|(_, local_name)| local_name.clone())
                        })
                        .or_else(|| {
                            dependency_support
                                .provides(&binding)
                                .then(|| binding.0.to_string())
                        })
                })
                .filter(|name| seen.insert(name.clone()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    support.merge(dependency_support);

    let components = drafts
        .iter()
        .zip(&dependency_names)
        .map(|(draft, dependencies)| ModuleComponentEmitInput {
            name: &draft.name,
            selector: &draft.selector,
            styles: &draft.styles,
            class: &draft.class,
            template_source: &draft.template_source,
            dependencies,
        })
        .collect::<Vec<_>>();
    let source = emit_angular_module_source(&components, &support, &renames, cm)?;
    let completeness = if drafts
        .iter()
        .all(|draft| draft.completeness == AngularRecoveryCompleteness::Complete)
    {
        AngularRecoveryCompleteness::Complete
    } else {
        AngularRecoveryCompleteness::Partial
    };
    Ok(RecoveredAngularModule {
        source,
        completeness,
        component_indices: drafts.iter().map(|draft| draft.component_index).collect(),
        dependencies: module_dependencies,
        issues: drafts
            .iter()
            .flat_map(|draft| draft.issues.iter().cloned())
            .collect(),
        module_index,
    })
}

fn local_identity_binding(identity: &SymbolIdentity) -> Option<&BindingKey> {
    let SymbolIdentity::LocalBinding(binding) = identity else {
        return None;
    };
    Some(binding)
}

fn reserve_unique_artifact_name(preferred: &str, reserved_names: &mut HashSet<Atom>) -> String {
    if reserved_names.insert(Atom::from(preferred)) {
        return preferred.to_string();
    }
    for suffix in 2usize.. {
        let candidate = format!("{preferred}_{suffix}");
        if reserved_names.insert(Atom::from(candidate.as_str())) {
            return candidate;
        }
    }
    unreachable!("the artifact-name suffix space is unbounded")
}

fn prepare_module(source: &AngularModuleSource<'_>) -> Result<PreparedAngularModule> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        FileName::Custom(source.filename.to_string()).into(),
        source.source.to_string(),
    );
    let source_start_pos = fm.start_pos.0;
    let lexer = Lexer::new(
        Syntax::Es(EsSyntax {
            jsx: true,
            decorators: true,
            ..Default::default()
        }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let mut module = parser
        .parse_module()
        .map_err(|error| anyhow!("failed to parse {}: {error:?}", source.filename))?;
    let errors = parser.take_errors();
    if !errors.is_empty() {
        return Err(anyhow!(
            "failed to parse {} without recovery errors: {:?}",
            source.filename,
            errors[0]
        ));
    }

    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
    workspace::canonicalize_immediate_iife_namespace_aliases(
        &mut module,
        SyntaxContext::empty().apply_mark(unresolved_mark),
    );

    Ok(PreparedAngularModule {
        filename: source.filename.to_string(),
        module,
        unresolved_ctxt: SyntaxContext::empty().apply_mark(unresolved_mark),
        source_start_pos,
    })
}

fn collect_component_classes(
    module: &Module,
    unresolved_ctxt: SyntaxContext,
) -> HashMap<SymbolIdentity, ComponentClass> {
    let mut collector = ComponentClassCollector {
        unresolved_ctxt,
        classes: HashMap::new(),
    };
    module.visit_with(&mut collector);
    collector.classes
}

fn collect_portable_component_classes(
    module: &Module,
    unresolved_ctxt: SyntaxContext,
) -> HashMap<PortableSymbolIdentity, ComponentClass> {
    let mut classes = HashMap::new();
    let mut ambiguous = std::collections::HashSet::new();
    for class in collect_component_classes(module, unresolved_ctxt).into_values() {
        let identity = class.portable_identity.clone();
        if ambiguous.contains(&identity) {
            continue;
        }
        match classes.entry(identity.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(class);
            }
            std::collections::hash_map::Entry::Occupied(entry)
                if entry.get().name == class.name && entry.get().class.span == class.class.span => {
            }
            std::collections::hash_map::Entry::Occupied(entry) => {
                entry.remove();
                ambiguous.insert(identity);
            }
        }
    }
    classes
}

struct ComponentClassCollector {
    unresolved_ctxt: SyntaxContext,
    classes: HashMap<SymbolIdentity, ComponentClass>,
}

impl ComponentClassCollector {
    fn record(&mut self, expression: &Expr, fallback_name: &str, class: &Class) {
        let Some(identity) = symbol_identity(expression, self.unresolved_ctxt) else {
            return;
        };
        let portable_identity = portable_symbol_identity(&identity);
        let name = Atom::from(to_valid_identifier_name(fallback_name));
        self.classes.insert(
            identity.clone(),
            ComponentClass {
                name,
                class: Box::new(class.clone()),
                identity,
                portable_identity,
            },
        );
    }
}

fn portable_symbol_identity(identity: &SymbolIdentity) -> PortableSymbolIdentity {
    match identity {
        SymbolIdentity::LocalBinding(binding) => {
            PortableSymbolIdentity::LocalBinding(binding.0.clone())
        }
        SymbolIdentity::LocalMember { object, property } => PortableSymbolIdentity::LocalMember {
            object: object.0.clone(),
            property: property.clone(),
        },
        SymbolIdentity::GlobalBinding(binding) => {
            PortableSymbolIdentity::GlobalBinding(binding.clone())
        }
        SymbolIdentity::GlobalMember { object, property } => PortableSymbolIdentity::GlobalMember {
            object: object.clone(),
            property: property.clone(),
        },
    }
}

impl Visit for ComponentClassCollector {
    fn visit_class_decl(&mut self, declaration: &ClassDecl) {
        self.record(
            &Expr::Ident(declaration.ident.clone()),
            declaration.ident.sym.as_ref(),
            declaration.class.as_ref(),
        );
        declaration.class.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Pat::Ident(binding), Some(Expr::Class(class))) =
            (&declarator.name, declarator.init.as_deref())
        {
            self.record(
                &Expr::Ident(binding.id.clone()),
                binding.id.sym.as_ref(),
                class.class.as_ref(),
            );
            if let Some(inner) = &class.ident {
                self.record(
                    &Expr::Ident(inner.clone()),
                    binding.id.sym.as_ref(),
                    class.class.as_ref(),
                );
            }
        }
        declarator.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        if let Expr::Class(class) = assignment.right.as_ref() {
            if let Some((target, name)) = class_assignment_target(&assignment.left) {
                self.record(&target, name.as_ref(), class.class.as_ref());
                if let Some(inner) = &class.ident {
                    self.record(
                        &Expr::Ident(inner.clone()),
                        name.as_ref(),
                        class.class.as_ref(),
                    );
                }
            }
        }
        assignment.visit_children_with(self);
    }
}

fn class_assignment_target(target: &AssignTarget) -> Option<(Expr, Atom)> {
    match target {
        AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
            Some((Expr::Ident(binding.id.clone()), binding.id.sym.clone()))
        }
        AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
            let name = syntax::member_prop_name(&member.prop)?;
            Some((Expr::Member(member.clone()), name))
        }
        AssignTarget::Simple(SimpleAssignTarget::Paren(paren)) => {
            let identity_name = match paren.expr.as_ref() {
                Expr::Ident(ident) => ident.sym.clone(),
                Expr::Member(member) => syntax::member_prop_name(&member.prop)?,
                _ => return None,
            };
            Some((paren.expr.as_ref().clone(), identity_name))
        }
        _ => None,
    }
}

fn parse_component_descriptor(
    call: &swc_core::ecma::ast::CallExpr,
    classes: &HashMap<SymbolIdentity, ComponentClass>,
    roles: &IvyRoleTable,
    template_functions: &TemplateFunctionTable,
    unresolved_ctxt: SyntaxContext,
) -> Option<ComponentDescriptor> {
    if roles.instruction_for_callee(&call.callee, unresolved_ctxt)
        != Some(IvyInstruction::DefineComponent)
    {
        return None;
    }
    let Expr::Object(object) = call.args.first()?.expr.as_ref() else {
        return None;
    };

    let class = descriptor_class(object, classes, unresolved_ctxt)?;
    let template = descriptor_template(object, roles, unresolved_ctxt)?;
    let contains_i18n =
        template_contains_instruction(&template, roles, unresolved_ctxt, IvyInstruction::I18n);
    let selector = descriptor_selector(object)?;
    let styles = descriptor_styles(object);
    let projection_selectors = descriptor_projection_selectors(
        object,
        &template,
        roles,
        template_functions,
        unresolved_ctxt,
    );
    let dependencies = descriptor_dependencies(object, template_functions);
    let constants = descriptor_constants(object, template_functions, contains_i18n);

    Some(ComponentDescriptor {
        class,
        selector,
        styles,
        projection_selectors,
        dependencies,
        template,
        constants,
    })
}

fn template_contains_instruction(
    template: &Function,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
    expected: IvyInstruction,
) -> bool {
    struct Finder<'a> {
        roles: &'a IvyRoleTable,
        unresolved_ctxt: SyntaxContext,
        expected: IvyInstruction,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if self
                .roles
                .instruction_for_callee(&call.callee, self.unresolved_ctxt)
                == Some(self.expected)
            {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = Finder {
        roles,
        unresolved_ctxt,
        expected,
        found: false,
    };
    template.visit_with(&mut finder);
    finder.found
}

fn descriptor_class(
    object: &ObjectLit,
    classes: &HashMap<SymbolIdentity, ComponentClass>,
    unresolved_ctxt: SyntaxContext,
) -> Option<ComponentClass> {
    let candidates = object.props.iter().filter_map(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            return None;
        };
        let identity = symbol_identity(key_value.value.as_ref(), unresolved_ctxt)?;
        classes
            .get(&identity)
            .map(|class| (prop_name(&key_value.key), identity, class))
    });

    if let Some(class) = candidates
        .clone()
        .find_map(|(name, _, class)| (name.as_deref() == Some("type")).then(|| class.clone()))
    {
        return Some(class);
    }

    let mut structural = candidates.map(|(_, identity, class)| (identity, class.clone()));
    let (first_identity, first) = structural.next()?;
    structural
        .all(|(identity, _)| identity == first_identity)
        .then_some(first)
}

fn descriptor_template(
    object: &ObjectLit,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
) -> Option<Function> {
    let candidates = object
        .props
        .iter()
        .filter_map(descriptor_function_property)
        .collect::<Vec<_>>();
    if let Some((_, function)) = candidates
        .iter()
        .find(|(name, _)| name.as_deref() == Some("template"))
    {
        return Some(function.clone());
    }

    let mut best: Option<(usize, &Function)> = None;
    let mut tied = false;
    for (_, function) in &candidates {
        let score = ivy_template_score(function, roles, unresolved_ctxt);
        if score == 0 {
            continue;
        }
        match best {
            Some((best_score, _)) if score < best_score => {}
            Some((best_score, _)) if score == best_score => tied = true,
            _ => {
                best = Some((score, function));
                tied = false;
            }
        }
    }
    (!tied).then(|| best.map(|(_, function)| function.clone()))?
}

fn descriptor_function_property(prop: &PropOrSpread) -> Option<(Option<String>, Function)> {
    let PropOrSpread::Prop(prop) = prop else {
        return None;
    };
    match prop.as_ref() {
        Prop::KeyValue(key_value) => {
            let Expr::Fn(function) = key_value.value.as_ref() else {
                return None;
            };
            Some((
                prop_name(&key_value.key),
                function.function.as_ref().clone(),
            ))
        }
        Prop::Method(method) => Some((prop_name(&method.key), method.function.as_ref().clone())),
        _ => None,
    }
}

fn descriptor_selector(object: &ObjectLit) -> Option<String> {
    if let Some(selector) = object.props.iter().find_map(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            return None;
        };
        (prop_name(&key_value.key).as_deref() == Some("selectors"))
            .then(|| selector_list_string(key_value.value.as_ref()).map(|decoded| decoded.selector))
            .flatten()
    }) {
        return Some(selector);
    }

    let mut best: Option<(usize, String)> = None;
    let mut tied = false;
    for expression in descriptor_expression_values(object) {
        let Some((selector, score)) = selector_shape(expression) else {
            continue;
        };
        match &best {
            Some((best_score, _)) if score < *best_score => {}
            Some((best_score, _)) if score == *best_score => tied = true,
            _ => {
                best = Some((score, selector));
                tied = false;
            }
        }
    }
    (!tied).then(|| best.map(|(_, selector)| selector))?
}

fn descriptor_styles(object: &ObjectLit) -> Vec<String> {
    if let Some(styles) = object.props.iter().find_map(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            return None;
        };
        (prop_name(&key_value.key).as_deref() == Some("styles"))
            .then(|| string_array(key_value.value.as_ref()))
            .flatten()
    }) {
        return styles;
    }

    let mut candidates = descriptor_expression_values(object).filter_map(|expression| {
        let styles = string_array(expression)?;
        (!styles.is_empty()
            && styles
                .iter()
                .all(|style| style.contains('{') && style.contains('}')))
        .then_some(styles)
    });
    let first = candidates.next().unwrap_or_default();
    if candidates.next().is_some() {
        Vec::new()
    } else {
        first
    }
}

fn descriptor_projection_selectors(
    object: &ObjectLit,
    template: &Function,
    roles: &IvyRoleTable,
    template_functions: &TemplateFunctionTable,
    unresolved_ctxt: SyntaxContext,
) -> Vec<String> {
    if let Some(selectors) = object.props.iter().find_map(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            return None;
        };
        (prop_name(&key_value.key).as_deref() == Some("ngContentSelectors"))
            .then(|| {
                let value = template_functions.resolve_expression(key_value.value.as_ref());
                string_array(value.as_ref())
            })
            .flatten()
    }) {
        return selectors;
    }

    let mut projection_calls = ProjectionCallCollector {
        roles,
        unresolved_ctxt,
        found: false,
    };
    template.visit_with(&mut projection_calls);
    if !projection_calls.found {
        return Vec::new();
    }

    let mut candidates = descriptor_expression_values(object)
        .filter_map(|expression| {
            let expression = template_functions.resolve_expression(expression);
            let selectors = string_array(expression.as_ref())?;
            (!selectors.is_empty()
                && selectors
                    .iter()
                    .all(|selector| !selector.contains('{') && !selector.contains('}')))
            .then_some(selectors)
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();
    if candidates.len() == 1 {
        candidates.pop().unwrap_or_default()
    } else {
        Vec::new()
    }
}

struct ProjectionCallCollector<'a> {
    roles: &'a IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
    found: bool,
}

impl Visit for ProjectionCallCollector<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if matches!(
            self.roles
                .instruction_for_callee(&call.callee, self.unresolved_ctxt),
            Some(IvyInstruction::ProjectionDef | IvyInstruction::Projection)
        ) {
            self.found = true;
            return;
        }
        call.visit_children_with(self);
    }
}

fn descriptor_dependencies(
    object: &ObjectLit,
    template_functions: &TemplateFunctionTable,
) -> Vec<Box<Expr>> {
    object
        .props
        .iter()
        .find_map(|prop| {
            let PropOrSpread::Prop(prop) = prop else {
                return None;
            };
            let Prop::KeyValue(key_value) = prop.as_ref() else {
                return None;
            };
            (prop_name(&key_value.key).as_deref() == Some("dependencies")).then(|| {
                let value = template_functions.resolve_expression(key_value.value.as_ref());
                let Expr::Array(array) = value.as_ref() else {
                    return Vec::new();
                };
                array
                    .elems
                    .iter()
                    .filter_map(|element| {
                        let element = element.as_ref()?;
                        element.spread.is_none().then(|| element.expr.clone())
                    })
                    .collect()
            })
        })
        .unwrap_or_default()
}

fn descriptor_constants(
    object: &ObjectLit,
    template_functions: &TemplateFunctionTable,
    contains_i18n: bool,
) -> Option<Box<Expr>> {
    if let Some(constants) = object.props.iter().find_map(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            return None;
        };
        (prop_name(&key_value.key).as_deref() == Some("consts")).then(|| key_value.value.clone())
    }) {
        return Some(template_functions.resolve_expression(constants.as_ref()));
    }

    let mut constant_factories = descriptor_expression_values(object)
        .map(|expression| template_functions.resolve_expression(expression))
        .filter_map(|expression| {
            component_constant_factory_score(expression.as_ref()).map(|score| (score, expression))
        })
        .collect::<Vec<_>>();
    constant_factories.sort_by_key(|(score, _)| *score);
    if let Some((best_score, best)) = constant_factories.pop() {
        if constant_factories
            .last()
            .is_none_or(|(score, _)| *score < best_score)
        {
            return Some(best);
        }
    }

    if contains_i18n {
        let mut array_factories = descriptor_expression_values(object)
            .map(|expression| template_functions.resolve_expression(expression))
            .filter(|expression| is_zero_parameter_array_factory(expression.as_ref()))
            .collect::<Vec<_>>();
        if array_factories.len() == 1 {
            return array_factories.pop();
        }
    }

    let mut i18n_factories = descriptor_expression_values(object)
        .filter_map(|expression| {
            let expression = template_functions.resolve_expression(expression);
            i18n_constant_factory_score(expression.as_ref()).map(|score| (score, expression))
        })
        .collect::<Vec<_>>();
    i18n_factories.sort_by_key(|(score, _)| *score);
    if let Some((best_score, best)) = i18n_factories.pop() {
        if i18n_factories
            .last()
            .is_none_or(|(score, _)| *score < best_score)
        {
            return Some(best);
        }
    }

    let mut constant_tables = descriptor_expression_values(object)
        .filter_map(|expression| {
            let expression = template_functions.resolve_expression(expression);
            component_constant_table_score(expression.as_ref()).map(|score| (score, expression))
        })
        .collect::<Vec<_>>();
    constant_tables.sort_by_key(|(score, _)| *score);
    let (best_score, best) = constant_tables.pop()?;
    constant_tables
        .last()
        .is_none_or(|(score, _)| *score < best_score)
        .then_some(best)
}

fn is_zero_parameter_array_factory(expression: &Expr) -> bool {
    fn is_array_return(expression: &Expr) -> bool {
        match expression {
            Expr::Array(_) => true,
            Expr::Paren(parenthesized) => is_array_return(parenthesized.expr.as_ref()),
            Expr::Seq(sequence) => sequence
                .exprs
                .last()
                .is_some_and(|expression| is_array_return(expression.as_ref())),
            _ => false,
        }
    }

    #[derive(Default)]
    struct ReturnEvidence {
        array_returns: usize,
        other_returns: usize,
    }

    impl Visit for ReturnEvidence {
        fn visit_return_stmt(&mut self, statement: &swc_core::ecma::ast::ReturnStmt) {
            if statement.arg.as_deref().is_some_and(is_array_return) {
                self.array_returns += 1;
            } else {
                self.other_returns += 1;
            }
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &swc_core::ecma::ast::ArrowExpr) {}
    }

    let body = match expression {
        Expr::Paren(parenthesized) => {
            return is_zero_parameter_array_factory(parenthesized.expr.as_ref());
        }
        Expr::Fn(function) if function.function.params.is_empty() => {
            function.function.body.as_ref()
        }
        Expr::Arrow(arrow) if arrow.params.is_empty() => match arrow.body.as_ref() {
            swc_core::ecma::ast::BlockStmtOrExpr::Expr(expression) => {
                return is_array_return(expression.as_ref());
            }
            swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(body) => Some(body),
        },
        _ => None,
    };
    let Some(body) = body else {
        return false;
    };
    let mut evidence = ReturnEvidence::default();
    body.visit_with(&mut evidence);
    evidence.array_returns == 1 && evidence.other_returns == 0
}

fn component_constant_factory_score(expression: &Expr) -> Option<usize> {
    fn strip(expression: &Expr) -> &Expr {
        match expression {
            Expr::Paren(parenthesized) => strip(parenthesized.expr.as_ref()),
            Expr::Seq(sequence) => sequence
                .exprs
                .last()
                .map_or(expression, |expression| strip(expression.as_ref())),
            expression => expression,
        }
    }

    fn returned_array(body: &swc_core::ecma::ast::BlockStmt) -> Option<&Expr> {
        let mut returns = body.stmts.iter().filter_map(|statement| {
            let swc_core::ecma::ast::Stmt::Return(statement) = statement else {
                return None;
            };
            statement.arg.as_deref()
        });
        let returned = returns.next()?;
        returns.next().is_none().then_some(returned)
    }

    let returned = match strip(expression) {
        Expr::Fn(function) if function.function.params.is_empty() => {
            returned_array(function.function.body.as_ref()?)?
        }
        Expr::Arrow(arrow) if arrow.params.is_empty() => match arrow.body.as_ref() {
            swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(body) => returned_array(body)?,
            swc_core::ecma::ast::BlockStmtOrExpr::Expr(expression) => expression.as_ref(),
        },
        _ => return None,
    };
    let Expr::Array(table) = strip(returned) else {
        return None;
    };

    let score = table
        .elems
        .iter()
        .filter_map(|entry| {
            let Expr::Array(entry) = strip(entry.as_ref()?.expr.as_ref()) else {
                return None;
            };
            let primitive_values = entry
                .elems
                .iter()
                .filter(|value| {
                    matches!(
                        value.as_ref().map(|value| strip(value.expr.as_ref())),
                        Some(Expr::Lit(_))
                    )
                })
                .count();
            (primitive_values >= 2).then_some(primitive_values + 4)
        })
        .sum::<usize>();
    (score > 0).then_some(score)
}

fn i18n_constant_factory_score(expression: &Expr) -> Option<usize> {
    let body = match expression {
        Expr::Fn(function) if function.function.params.is_empty() => {
            function.function.body.as_ref()?
        }
        Expr::Arrow(arrow) if arrow.params.is_empty() => {
            let swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(body) = arrow.body.as_ref() else {
                return None;
            };
            body
        }
        _ => return None,
    };

    #[derive(Default)]
    struct Evidence {
        returns_array: bool,
        localized_messages: usize,
        strings: usize,
    }

    impl Visit for Evidence {
        fn visit_return_stmt(&mut self, statement: &swc_core::ecma::ast::ReturnStmt) {
            let Some(argument) = &statement.arg else {
                return;
            };
            let expression = match argument.as_ref() {
                Expr::Paren(paren) => paren.expr.as_ref(),
                expression => expression,
            };
            let expression = match expression {
                Expr::Seq(sequence) => sequence.exprs.last().map(Box::as_ref),
                expression => Some(expression),
            };
            self.returns_array |= matches!(expression, Some(Expr::Array(_)));
            statement.visit_children_with(self);
        }

        fn visit_tagged_tpl(&mut self, tagged: &swc_core::ecma::ast::TaggedTpl) {
            if matches!(tagged.tag.as_ref(), Expr::Ident(identifier) if identifier.sym == "$localize")
            {
                self.localized_messages += 1;
            }
            tagged.visit_children_with(self);
        }

        fn visit_call_expr(&mut self, call: &CallExpr) {
            if matches!(
                &call.callee,
                swc_core::ecma::ast::Callee::Expr(callee)
                    if matches!(
                        callee.as_ref(),
                        Expr::Member(member)
                            if matches!(
                                member.obj.as_ref(),
                                Expr::Ident(identifier) if identifier.sym == "goog"
                            )
                                && syntax::member_prop_name(&member.prop)
                                    .is_some_and(|property| property == "getMsg")
                    )
            ) {
                self.localized_messages += 1;
            }
            call.visit_children_with(self);
        }

        fn visit_lit(&mut self, literal: &swc_core::ecma::ast::Lit) {
            if matches!(literal, swc_core::ecma::ast::Lit::Str(_)) {
                self.strings += 1;
            }
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &swc_core::ecma::ast::ArrowExpr) {}
    }

    let mut evidence = Evidence::default();
    body.visit_with(&mut evidence);
    (evidence.returns_array && (evidence.localized_messages > 0 || evidence.strings > 0))
        .then_some(evidence.localized_messages * 10 + evidence.strings)
}

fn descriptor_expression_values(object: &ObjectLit) -> impl Iterator<Item = &Expr> {
    object.props.iter().filter_map(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            return None;
        };
        Some(key_value.value.as_ref())
    })
}

fn selector_shape(expr: &Expr) -> Option<(String, usize)> {
    let decoded = selector_list_string(expr)?;
    let mut score = 1;
    if decoded.rows == 1 {
        score += 2;
    }
    if decoded.first_width == 1 {
        score += 5;
    }
    if decoded.first_element.contains('-') || decoded.first_element.is_empty() {
        score += 3;
    }
    if decoded.has_marker || decoded.has_empty_attribute {
        score += 4;
    }
    (score >= 4).then_some((decoded.selector, score))
}

struct DecodedSelectorList {
    selector: String,
    rows: usize,
    first_width: usize,
    first_element: String,
    has_marker: bool,
    has_empty_attribute: bool,
}

fn selector_list_string(expr: &Expr) -> Option<DecodedSelectorList> {
    let Expr::Array(outer) = expr else {
        return None;
    };
    if outer.elems.is_empty() {
        return None;
    }

    let mut selectors = Vec::with_capacity(outer.elems.len());
    let mut first_width = 0;
    let mut first_element = String::new();
    let mut has_marker = false;
    let mut has_empty_attribute = false;
    for (row_index, element) in outer.elems.iter().enumerate() {
        let Expr::Array(row) = element.as_ref()?.expr.as_ref() else {
            return None;
        };
        let decoded = selector_row_string(row)?;
        if row_index == 0 {
            first_width = row.elems.len();
            first_element.clone_from(&decoded.element);
        }
        has_marker |= decoded.has_marker;
        has_empty_attribute |= decoded.has_empty_attribute;
        selectors.push(decoded.selector);
    }

    Some(DecodedSelectorList {
        selector: selectors.join(","),
        rows: selectors.len(),
        first_width,
        first_element,
        has_marker,
        has_empty_attribute,
    })
}

struct DecodedSelectorRow {
    selector: String,
    element: String,
    has_marker: bool,
    has_empty_attribute: bool,
}

fn selector_row_string(row: &swc_core::ecma::ast::ArrayLit) -> Option<DecodedSelectorRow> {
    let values = row
        .elems
        .iter()
        .map(|element| Some(element.as_ref()?.expr.as_ref()))
        .collect::<Option<Vec<_>>>()?;
    let element = string_lit(*values.first()?)?;
    let mut selector = element.clone();
    let mut chunk = String::new();
    let mut mode = 2u8;
    let mut negative = false;
    let mut has_marker = false;
    let mut has_empty_attribute = false;
    let mut index = 1;
    while index < values.len() {
        match values[index] {
            Expr::Lit(swc_core::ecma::ast::Lit::Str(value)) => {
                let value = wtf8_to_string(&value.value);
                if mode & 2 != 0 {
                    let attribute_value = string_lit(*values.get(index + 1)?)?;
                    chunk.push('[');
                    chunk.push_str(&value);
                    if !attribute_value.is_empty() {
                        chunk.push_str("=\"");
                        chunk.push_str(&escape_css_selector_value(&attribute_value));
                        chunk.push('"');
                    } else {
                        has_empty_attribute = true;
                    }
                    chunk.push(']');
                    index += 2;
                    continue;
                }
                if mode & 8 != 0 {
                    chunk.push('.');
                    chunk.push_str(&value);
                } else if mode & 4 != 0 {
                    chunk.push(' ');
                    chunk.push_str(&value);
                } else {
                    return None;
                }
            }
            Expr::Lit(swc_core::ecma::ast::Lit::Num(number))
                if number.value.fract() == 0.0
                    && matches!(number.value as u8, 2 | 3 | 4 | 5 | 8 | 9) =>
            {
                let marker = number.value as u8;
                if !chunk.is_empty() && marker & 1 != 0 {
                    append_selector_chunk(&mut selector, &mut chunk, negative);
                }
                mode = marker;
                negative |= marker & 1 != 0;
                has_marker = true;
            }
            _ => return None,
        }
        index += 1;
    }
    append_selector_chunk(&mut selector, &mut chunk, negative);

    Some(DecodedSelectorRow {
        selector,
        element,
        has_marker,
        has_empty_attribute,
    })
}

fn append_selector_chunk(selector: &mut String, chunk: &mut String, negative: bool) {
    if chunk.is_empty() {
        return;
    }
    if negative {
        selector.push_str(":not(");
        selector.push_str(chunk.trim());
        selector.push(')');
    } else {
        selector.push_str(chunk);
    }
    chunk.clear();
}

fn escape_css_selector_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn component_constant_table_score(expr: &Expr) -> Option<usize> {
    let Expr::Array(table) = expr else {
        return None;
    };
    if table.elems.is_empty() {
        return None;
    }
    let mut score = 0;
    for entry in &table.elems {
        let Some(Expr::Array(attributes)) = entry.as_ref().map(|entry| entry.expr.as_ref()) else {
            continue;
        };
        let primitive_values = attributes
            .elems
            .iter()
            .filter(|element| {
                matches!(
                    element.as_ref().map(|element| element.expr.as_ref()),
                    Some(Expr::Lit(_))
                )
            })
            .count();
        if primitive_values < 2 {
            continue;
        }
        score += primitive_values + 4;
        score += attributes
            .elems
            .iter()
            .filter(|element| {
                matches!(
                    element.as_ref().map(|element| element.expr.as_ref()),
                    Some(Expr::Lit(swc_core::ecma::ast::Lit::Num(_)))
                )
            })
            .count()
            * 3;
    }
    (score > 0).then_some(score)
}

fn string_array(expr: &Expr) -> Option<Vec<String>> {
    let Expr::Array(array) = expr else {
        return None;
    };
    array
        .elems
        .iter()
        .map(|element| string_lit(element.as_ref()?.expr.as_ref()))
        .collect()
}

fn recovered_component_name(binding: &str, selector: &str) -> String {
    if !is_likely_generated_alias(binding) {
        return binding
            .strip_prefix('_')
            .filter(|name| name.ends_with("Component"))
            .unwrap_or(binding)
            .to_string();
    }

    selector_component_name(selector).unwrap_or_else(|| binding.to_string())
}

fn unique_recovered_component_name(
    preferred: String,
    recovered_names: &mut HashSet<String>,
) -> String {
    if recovered_names.insert(preferred.clone()) {
        return preferred;
    }

    for suffix in 2usize.. {
        let candidate = format!("{preferred}_{suffix}");
        if recovered_names.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("the component-name suffix space is unbounded")
}

fn selector_component_name(selector: &str) -> Option<String> {
    if selector.is_empty()
        || !selector
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return None;
    }

    let mut name = String::new();
    for segment in selector
        .split(['-', '_'])
        .filter(|segment| !segment.is_empty())
    {
        let mut characters = segment.chars();
        name.extend(characters.next()?.to_uppercase());
        name.extend(characters);
    }
    if name.is_empty() {
        return None;
    }
    if !name.ends_with("Component") {
        name.push_str("Component");
    }
    Some(to_valid_identifier_name(&name))
}

#[cfg(test)]
#[path = "angular_recovery/tests.rs"]
mod tests;
