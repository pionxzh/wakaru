//! Best-effort recovery of production Angular Ivy component artifacts.
//!
//! The analyzer consumes ordinary resolved JavaScript modules. Bundle-format
//! concerns stay in unpackers; this module knows only module ASTs and semantic
//! Ivy instruction identities.

mod emitter;
mod roles;
mod syntax;
mod template;
mod workspace;

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, SyntaxContext, GLOBALS};
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, Class, ClassDecl, Expr, Function, Module, ObjectLit, Pat, Prop,
    PropOrSpread, SimpleAssignTarget, VarDeclarator,
};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};

use crate::js_names::{is_likely_generated_alias, to_valid_identifier_name};
use emitter::{emit_component_source, ComponentEmitInput};
use roles::{symbol_identity, IvyInstruction, IvyRoleTable, SymbolIdentity};
use syntax::{prop_name, string_lit};
use template::{ivy_template_score, recover_template, TemplateFunctionTable};

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
    pub instruction: Option<String>,
    pub detail: Option<String>,
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
    cm: Lrc<SourceMap>,
}

#[derive(Clone)]
struct ComponentClass {
    name: Atom,
    class: Box<Class>,
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
    template: Function,
    constants: Option<Box<Expr>>,
}

pub fn recover_angular_components_from_js(
    source: &str,
    options: AngularRecoveryOptions,
) -> Result<Vec<RecoveredAngularComponent>> {
    Ok(analyze_angular_components_from_js(source, options)?.components)
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

pub fn analyze_angular_components_from_modules(
    sources: &[AngularModuleSource<'_>],
    _options: AngularRecoveryOptions,
) -> Result<AngularRecoveryReport> {
    GLOBALS.set(&Default::default(), || {
        let modules = sources
            .iter()
            .map(prepare_module)
            .collect::<Result<Vec<_>>>()?;
        recover_prepared_modules(&modules, None)
    })
}

pub fn recover_angular_components_from_module_views(
    views: &[AngularModuleView<'_>],
    options: AngularRecoveryOptions,
) -> Result<Vec<RecoveredAngularComponent>> {
    Ok(analyze_angular_components_from_module_views(views, options)?.components)
}

pub fn analyze_angular_components_from_module_views(
    views: &[AngularModuleView<'_>],
    _options: AngularRecoveryOptions,
) -> Result<AngularRecoveryReport> {
    GLOBALS.set(&Default::default(), || {
        let evidence_modules = views
            .iter()
            .map(|view| {
                prepare_module(&AngularModuleSource {
                    filename: view.filename,
                    source: view.evidence_source,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let readable_modules = views
            .iter()
            .map(|view| {
                prepare_module(&AngularModuleSource {
                    filename: view.filename,
                    source: view.readable_source,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        recover_prepared_modules(&evidence_modules, Some(&readable_modules))
    })
}

fn recover_prepared_modules(
    evidence_modules: &[PreparedAngularModule],
    readable_modules: Option<&[PreparedAngularModule]>,
) -> Result<AngularRecoveryReport> {
    let readable_modules = readable_modules.unwrap_or(evidence_modules);
    let roles = IvyRoleTable::collect(evidence_modules);
    let readable_classes = readable_modules
        .iter()
        .map(|prepared| {
            collect_portable_component_classes(&prepared.module, prepared.unresolved_ctxt)
        })
        .collect::<Vec<_>>();

    let mut recovered = Vec::new();
    let mut stats = AngularRecoveryStats {
        modules_analyzed: evidence_modules.len(),
        ..AngularRecoveryStats::default()
    };
    let mut unknown_runtime_call_shapes =
        HashMap::<(AngularTemplatePhase, Vec<usize>), (usize, usize)>::new();
    for (module_index, prepared) in evidence_modules.iter().enumerate() {
        let classes = collect_component_classes(&prepared.module, prepared.unresolved_ctxt);
        let template_functions = TemplateFunctionTable::collect(&prepared.module);
        let mut calls = roles::IvyCallCollector::new(&roles, prepared.unresolved_ctxt);
        prepared.module.visit_with(&mut calls);

        for candidate in &calls.define_component_calls {
            stats.component_candidates += 1;
            let call = &candidate.call;
            let Some(descriptor) =
                parse_component_descriptor(call, &classes, &roles, prepared.unresolved_ctxt)
            else {
                stats.rejected_component_candidates += 1;
                continue;
            };
            let recovered_template = recover_template(
                &descriptor.template,
                descriptor.constants.as_deref(),
                &roles,
                &template_functions,
                prepared.unresolved_ctxt,
                prepared.cm.clone(),
            )?;
            let readable_class = readable_classes[module_index]
                .get(&descriptor.class.portable_identity)
                .unwrap_or(&descriptor.class);
            let name = recovered_component_name(readable_class.name.as_ref(), &descriptor.selector);
            let source = emit_component_source(
                ComponentEmitInput {
                    name: &name,
                    selector: &descriptor.selector,
                    styles: &descriptor.styles,
                    class: &readable_class.class,
                    roles: &roles,
                    unresolved_ctxt: prepared.unresolved_ctxt,
                    template: &recovered_template,
                    definition_field: candidate.definition_field.as_ref(),
                },
                readable_modules[module_index].cm.clone(),
            )?;
            for shape in &recovered_template.unknown_runtime_call_shapes {
                let aggregate = unknown_runtime_call_shapes
                    .entry((shape.phase, shape.argument_counts.clone()))
                    .or_default();
                aggregate.0 += shape.occurrences;
                aggregate.1 += shape.runtime_calls;
            }
            recovered.push(RecoveredAngularComponent {
                name,
                selector: descriptor.selector,
                source,
                completeness: if recovered_template.issues.is_empty() {
                    stats.complete_components += 1;
                    AngularRecoveryCompleteness::Complete
                } else {
                    stats.partial_components += 1;
                    AngularRecoveryCompleteness::Partial
                },
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
    }

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
        stats,
        unknown_runtime_call_shapes,
    })
}

fn prepare_module(source: &AngularModuleSource<'_>) -> Result<PreparedAngularModule> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        FileName::Custom(source.filename.to_string()).into(),
        source.source.to_string(),
    );
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
        cm,
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
            identity,
            ComponentClass {
                name,
                class: Box::new(class.clone()),
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
    let selector = descriptor_selector(object)?;
    let styles = descriptor_styles(object);
    let constants = descriptor_constants(object);

    Some(ComponentDescriptor {
        class,
        selector,
        styles,
        template,
        constants,
    })
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
            .then(|| first_selector(key_value.value.as_ref()))
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

fn descriptor_constants(object: &ObjectLit) -> Option<Box<Expr>> {
    if let Some(constants) = object.props.iter().find_map(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            return None;
        };
        (prop_name(&key_value.key).as_deref() == Some("consts")).then(|| key_value.value.clone())
    }) {
        return Some(constants);
    }

    descriptor_expression_values(object)
        .filter_map(|expression| attribute_table_score(expression).map(|score| (score, expression)))
        .max_by_key(|(score, _)| *score)
        .map(|(_, expression)| Box::new(expression.clone()))
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
    let Expr::Array(outer) = expr else {
        return None;
    };
    if outer.elems.is_empty() {
        return None;
    }
    let selectors = outer
        .elems
        .iter()
        .map(|element| {
            let Expr::Array(selector) = element.as_ref()?.expr.as_ref() else {
                return None;
            };
            let first = string_lit(selector.elems.first()?.as_ref()?.expr.as_ref())?;
            Some((first, selector.elems.len()))
        })
        .collect::<Option<Vec<_>>>()?;
    let (selector, width) = selectors.first()?.clone();
    let mut score = 1;
    if selectors.len() == 1 {
        score += 2;
    }
    if width == 1 {
        score += 5;
    }
    if selector.contains('-') || selector.is_empty() {
        score += 3;
    }
    (score >= 4).then_some((selector, score))
}

fn attribute_table_score(expr: &Expr) -> Option<usize> {
    let Expr::Array(table) = expr else {
        return None;
    };
    if table.elems.is_empty() {
        return None;
    }
    let mut score = 0;
    for entry in &table.elems {
        let Expr::Array(attributes) = entry.as_ref()?.expr.as_ref() else {
            return None;
        };
        if attributes.elems.len() < 2 {
            return None;
        }
        score += attributes.elems.len();
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
    Some(score)
}

fn first_selector(expr: &Expr) -> Option<String> {
    let Expr::Array(outer) = expr else {
        return None;
    };
    let Expr::Array(selector) = outer.elems.first()?.as_ref()?.expr.as_ref() else {
        return None;
    };
    string_lit(selector.elems.first()?.as_ref()?.expr.as_ref())
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
