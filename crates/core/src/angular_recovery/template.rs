use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap, Span, Spanned, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BinaryOp, BindingIdent, BlockStmt, BlockStmtOrExpr,
    CallExpr, Callee, Decl, Expr, ExprOrSpread, ExprStmt, FnDecl, Function, Ident, Lit, MemberExpr,
    MemberProp, Module, ModuleItem, Param, Pat, ReturnStmt, SimpleAssignTarget, Stmt, ThisExpr,
    UnaryOp, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_uses::BindingUseIndex;
use crate::js_names::{is_likely_generated_alias, to_valid_identifier_name};
use crate::rules::{RewriteLevel, UnOptionalChaining};

use super::artifact::{expression_references, function_references};
use super::emitter::{
    handler_expression, print_template_expression_with_aliases, TemplateExpressionAliasResolver,
};
use super::roles::{IvyInstruction, IvyRoleTable};
use super::syntax::{
    binding_key, member_prop_name, prop_name, string_lit, wtf8_to_string, BindingKey,
};
use super::{
    AngularRecoveryIssue, AngularRecoveryIssueKind, AngularRecoverySourceRange,
    AngularTemplatePhase, AngularTemplateRecoveryStats, AngularUnknownRuntimeCallShape,
};

pub(super) struct RecoveredTemplate {
    pub(super) source: String,
    pub(super) issues: Vec<AngularRecoveryIssue>,
    pub(super) stats: AngularTemplateRecoveryStats,
    pub(super) unknown_runtime_call_shapes: Vec<AngularUnknownRuntimeCallShape>,
    pub(super) artifact_references: HashSet<BindingKey>,
    pub(super) listener_methods: Vec<RecoveredListenerMethod>,
}

#[derive(Clone)]
pub(super) struct RecoveredListenerMethod {
    // This remains in the evidence AST's binding domain through support-root
    // discovery. The emitter attaches it to a cloned readable class only after
    // those roots have been resolved.
    pub(super) placeholder: String,
    pub(super) preferred_name: String,
    pub(super) function: Function,
}

#[derive(Default)]
pub(super) struct TemplateFunctionTable {
    functions: HashMap<BindingKey, Function>,
    values: HashMap<BindingKey, Box<Expr>>,
}

impl TemplateFunctionTable {
    pub(super) fn collect(module: &Module) -> Self {
        let binding_uses = BindingUseIndex::collect(module);
        let uninitialized_bindings = binding_uses.uninitialized_bindings();
        let mut collector = TemplateFunctionCollector::new(&binding_uses, &uninitialized_bindings);
        module.visit_with(&mut collector);
        Self {
            functions: collector.functions,
            values: collector.values,
        }
    }

    fn resolve(&self, expression: &Expr) -> Option<ResolvedTemplateFunction> {
        let mut expression = strip_parentheses(expression);
        let mut visited = HashSet::new();
        for _ in 0..32 {
            match expression {
                Expr::Ident(identifier) => {
                    let key = binding_key(identifier);
                    if let Some(function) = self.functions.get(&key) {
                        return Some(ResolvedTemplateFunction {
                            key: Some(key),
                            function: function.clone(),
                        });
                    }
                    if !visited.insert(key.clone()) {
                        return None;
                    }
                    let value = self.values.get(&key)?;
                    expression = strip_parentheses(value.as_ref());
                }
                Expr::Fn(function) => {
                    return Some(ResolvedTemplateFunction {
                        key: None,
                        function: function.function.as_ref().clone(),
                    });
                }
                _ => return None,
            }
        }
        None
    }

    pub(super) fn resolve_expression(&self, expression: &Expr) -> Box<Expr> {
        let mut expression = strip_parentheses(expression);
        let mut visited = HashSet::new();
        for _ in 0..32 {
            let Expr::Ident(identifier) = expression else {
                break;
            };
            let key = binding_key(identifier);
            if !visited.insert(key.clone()) {
                break;
            }
            let Some(value) = self.values.get(&key) else {
                break;
            };
            expression = strip_parentheses(value.as_ref());
        }
        Box::new(expression.clone())
    }
}

struct TemplateFunctionCollector<'a> {
    binding_uses: &'a BindingUseIndex,
    uninitialized_bindings: &'a HashSet<BindingKey>,
    functions: HashMap<BindingKey, Function>,
    values: HashMap<BindingKey, Box<Expr>>,
    definitions: HashSet<BindingKey>,
    ambiguous: HashSet<BindingKey>,
}

impl<'a> TemplateFunctionCollector<'a> {
    fn new(
        binding_uses: &'a BindingUseIndex,
        uninitialized_bindings: &'a HashSet<BindingKey>,
    ) -> Self {
        Self {
            binding_uses,
            uninitialized_bindings,
            functions: HashMap::new(),
            values: HashMap::new(),
            definitions: HashSet::new(),
            ambiguous: HashSet::new(),
        }
    }

    fn begin_definition(&mut self, key: &BindingKey) -> bool {
        if self.ambiguous.contains(key) {
            return false;
        }
        if self.definitions.insert(key.clone()) {
            return true;
        }
        self.functions.remove(key);
        self.values.remove(key);
        self.ambiguous.insert(key.clone());
        false
    }

    fn record_function(&mut self, key: BindingKey, function: Function) {
        if self.begin_definition(&key) {
            self.functions.insert(key, function);
        }
    }

    fn record_value(&mut self, key: BindingKey, value: Box<Expr>) {
        if !self.begin_definition(&key) {
            return;
        }
        if let Expr::Fn(function) = strip_parentheses(value.as_ref()) {
            self.functions
                .insert(key.clone(), function.function.as_ref().clone());
        }
        self.values.insert(key, value);
    }
}

impl Visit for TemplateFunctionCollector<'_> {
    fn visit_fn_decl(&mut self, declaration: &FnDecl) {
        let key = binding_key(&declaration.ident);
        if self.binding_uses.direct_write_count(&key) == 0 {
            self.record_function(key, declaration.function.as_ref().clone());
        }
        declaration.function.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Pat::Ident(binding), Some(value)) = (&declarator.name, declarator.init.as_ref()) {
            let key = binding_key(&binding.id);
            if self.binding_uses.direct_write_count(&key) == 0 {
                self.record_value(key, value.clone());
            }
        }
        declarator.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        if assignment.op == AssignOp::Assign {
            if let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &assignment.left {
                let key = binding_key(&binding.id);
                let stable_assignment = self.uninitialized_bindings.contains(&key)
                    && self.binding_uses.direct_write_count(&key) == 1;
                let supported_value = matches!(
                    strip_parentheses(assignment.right.as_ref()),
                    Expr::Fn(_) | Expr::Arrow(_) | Expr::Ident(_)
                );
                if stable_assignment && supported_value {
                    self.record_value(key, assignment.right.clone());
                }
            }
        }
        assignment.visit_children_with(self);
    }
}

struct ResolvedTemplateFunction {
    key: Option<BindingKey>,
    function: Function,
}

struct TemplateRecoveryEnvironment<'a> {
    constants: &'a TemplateConstants,
    projection_selectors: &'a [String],
    roles: &'a IvyRoleTable,
    template_functions: &'a TemplateFunctionTable,
    implicit_view_context_properties: &'a HashSet<String>,
    unresolved_ctxt: SyntaxContext,
    source_start_pos: u32,
    cm: Lrc<SourceMap>,
}

pub(super) struct TemplateRecoveryContext {
    pub(super) unresolved_ctxt: SyntaxContext,
    pub(super) source_start_pos: u32,
    pub(super) cm: Lrc<SourceMap>,
}

#[derive(Clone)]
struct TemplateOperationProvenance {
    view_id: usize,
    phase: AngularTemplatePhase,
    operation_index: usize,
    source_range: Option<AngularRecoverySourceRange>,
    actual_callee: Option<String>,
}

#[derive(Clone)]
struct InstructionCall {
    instruction: IvyInstruction,
    args: Vec<Box<Expr>>,
    result_binding: Option<BindingKey>,
    provenance: TemplateOperationProvenance,
}

struct InlineReferenceAlias {
    binding: BindingKey,
    slot: usize,
    context_depth: usize,
    provenance: TemplateOperationProvenance,
}

struct PendingReferenceAlias {
    binding: BindingKey,
    call: InstructionCall,
    context_depth: usize,
    structural_candidate: bool,
}

struct PendingAliasDeclaration {
    binding: BindingKey,
    span: Span,
    phase: Option<u8>,
}

#[derive(Clone, Default)]
struct ViewContextScope {
    is_component: bool,
    local_properties: HashMap<String, String>,
}

#[derive(Clone)]
struct ViewLetAliasHint {
    context_depth: usize,
    slot: usize,
    name: String,
}

#[derive(Default)]
struct TemplateProgram {
    view_id: usize,
    is_component_view: bool,
    next_operation_index: usize,
    create: Vec<InstructionCall>,
    update: Vec<InstructionCall>,
    issues: Vec<AngularRecoveryIssue>,
    stats: AngularTemplateRecoveryStats,
    unknown_runtime_call_shapes: HashMap<(AngularTemplatePhase, Vec<usize>), (usize, usize)>,
    component_contexts: HashSet<BindingKey>,
    view_context: Option<BindingKey>,
    saved_views: HashSet<BindingKey>,
    update_context_depth: usize,
    repeater_item_name: Option<String>,
    reference_aliases: Vec<PendingReferenceAlias>,
    inline_reference_aliases: Vec<InlineReferenceAlias>,
    local_reference_names: HashMap<BindingKey, String>,
    pipes: HashMap<usize, String>,
    pending_i18n_expressions: Vec<String>,
    artifact_references: HashSet<BindingKey>,
    pending_alias_declarations: Vec<PendingAliasDeclaration>,
    resolved_alias_declarations: HashSet<BindingKey>,
    member_object_bindings: HashSet<BindingKey>,
    implicit_view_context_properties: HashSet<String>,
    ancestor_contexts: Vec<ViewContextScope>,
    local_context_bindings: HashMap<BindingKey, HashMap<String, String>>,
    let_alias_hints: Vec<ViewLetAliasHint>,
    listener_methods: Vec<RecoveredListenerMethod>,
}

impl TemplateProgram {
    fn new(
        view_id: usize,
        is_component_view: bool,
        ancestor_contexts: &[ViewContextScope],
    ) -> Self {
        Self {
            view_id,
            is_component_view,
            ancestor_contexts: ancestor_contexts.to_vec(),
            ..Self::default()
        }
    }

    fn current_context_scope(&self) -> ViewContextScope {
        if self.is_component_view {
            return ViewContextScope {
                is_component: true,
                local_properties: HashMap::new(),
            };
        }
        let Some(item) = self.repeater_item_name.as_ref() else {
            return ViewContextScope::default();
        };
        let mut local_properties = self
            .implicit_view_context_properties
            .iter()
            .map(|property| (property.clone(), item.clone()))
            .collect::<HashMap<_, _>>();
        local_properties.insert("$implicit".to_string(), item.clone());
        ViewContextScope {
            is_component: false,
            local_properties,
        }
    }

    fn context_scope_at_depth(&self, context_depth: usize) -> Option<ViewContextScope> {
        if context_depth == 0 {
            Some(self.current_context_scope())
        } else {
            self.ancestor_contexts.get(context_depth - 1).cloned()
        }
    }

    fn context_property_name(&self, context_depth: usize, property: &str) -> Option<String> {
        let scope = self.context_scope_at_depth(context_depth)?;
        if scope.is_component {
            Some(to_valid_identifier_name(property))
        } else {
            scope.local_properties.get(property).cloned()
        }
    }

    fn context_property_name_for_binding(
        &self,
        context_depth: usize,
        property: &str,
        binding: &BindingKey,
    ) -> Option<String> {
        self.context_property_name(context_depth, property)
            .or_else(|| {
                let scope = self.context_scope_at_depth(context_depth)?;
                (!scope.is_component
                    && is_likely_generated_alias(property)
                    && self.member_object_bindings.contains(binding))
                .then(|| scope.local_properties.get("$implicit").cloned())
                .flatten()
            })
    }

    fn next_operation(
        &mut self,
        phase: AngularTemplatePhase,
        span: Span,
        actual_callee: Option<String>,
        source_start_pos: u32,
    ) -> TemplateOperationProvenance {
        let operation_index = self.next_operation_index;
        self.next_operation_index = self.next_operation_index.saturating_add(1);
        TemplateOperationProvenance {
            view_id: self.view_id,
            phase,
            operation_index,
            source_range: relative_source_range(span, source_start_pos),
            actual_callee,
        }
    }
}

type ReferenceScope = HashMap<usize, String>;

#[derive(Default)]
struct TemplateConstants {
    attributes: Vec<Vec<TemplateAttribute>>,
    local_references: Vec<Vec<String>>,
    i18n_messages: Vec<Option<String>>,
}

#[derive(Clone)]
struct TemplateAttribute {
    name: String,
    value: Option<String>,
}

enum I18nToken {
    Text(String),
    Interpolation(usize),
    ElementStart(usize),
    ElementEnd(usize),
}

enum TemplateNodeKind {
    Element {
        tag: String,
        attributes: Vec<TemplateAttribute>,
    },
    Text {
        value: String,
    },
    Let {
        name: String,
        value: Option<String>,
        provenance: TemplateOperationProvenance,
    },
    EmbeddedView {
        tree: Box<TemplateTree>,
        attributes: Vec<TemplateAttribute>,
        branch: Option<ConditionalBranch>,
    },
    Defer {
        primary: Box<TemplateTree>,
        loading: Option<Box<TemplateTree>>,
        placeholder: Option<Box<TemplateTree>>,
        error: Option<Box<TemplateTree>>,
        triggers: Vec<String>,
    },
    Repeater {
        body: Box<TemplateTree>,
        empty: Option<Box<TemplateTree>>,
        item: String,
        track: String,
        collection: Option<String>,
    },
    Projection {
        selector: Option<String>,
        attributes: Vec<TemplateAttribute>,
        fallback: Option<Box<TemplateTree>>,
    },
    I18nRegion {
        tokens: Vec<I18nToken>,
        expressions: Vec<String>,
    },
    UnsupportedRegion {
        comment: String,
        placement_unknown: bool,
    },
    Consumed,
}

enum ConditionalBranch {
    If(String),
    ElseIf(String),
    Else,
}

struct TemplateNode {
    kind: TemplateNodeKind,
    children: Vec<usize>,
}

#[derive(Default)]
struct TemplateTree {
    nodes: Vec<TemplateNode>,
    roots: Vec<usize>,
    stack: Vec<usize>,
    index_to_node: HashMap<usize, usize>,
    local_reference_slots: HashMap<usize, String>,
    let_names: HashMap<usize, String>,
    placed_issue_operations: HashSet<(AngularTemplatePhase, usize)>,
    cursor: usize,
    pending_defer: Option<usize>,
}

#[derive(Clone, Copy)]
struct TreeInsertionPoint {
    parent: Option<usize>,
    position: usize,
}

fn record_issue(issues: &mut Vec<AngularRecoveryIssue>, issue: AngularRecoveryIssue) {
    issues.push(issue);
}

fn issue(
    kind: AngularRecoveryIssueKind,
    instruction: Option<String>,
    detail: Option<String>,
) -> AngularRecoveryIssue {
    AngularRecoveryIssue::new(kind, instruction, detail)
}

fn issue_at_operation(
    mut issue: AngularRecoveryIssue,
    provenance: &TemplateOperationProvenance,
) -> AngularRecoveryIssue {
    issue.view_id = Some(provenance.view_id);
    issue.phase = Some(provenance.phase);
    issue.operation_index = Some(provenance.operation_index);
    issue.source_range = provenance.source_range;
    issue.actual_callee.clone_from(&provenance.actual_callee);
    issue
}

fn record_program_issue(
    program: &mut TemplateProgram,
    issue: AngularRecoveryIssue,
    phase: Option<u8>,
    span: Span,
    actual_callee: Option<&Expr>,
    environment: &TemplateRecoveryEnvironment<'_>,
) {
    let provenance = program.next_operation(
        template_phase(phase),
        span,
        actual_callee.and_then(concise_callee_spelling),
        environment.source_start_pos,
    );
    record_issue(&mut program.issues, issue_at_operation(issue, &provenance));
}

fn call_provenances(
    program: &mut TemplateProgram,
    phase: Option<u8>,
    call: &CallExpr,
    root: &Expr,
    count: usize,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> Vec<TemplateOperationProvenance> {
    let actual_callee = concise_callee_spelling(root);
    (0..count)
        .map(|_| {
            program.next_operation(
                template_phase(phase),
                call.span,
                actual_callee.clone(),
                environment.source_start_pos,
            )
        })
        .collect()
}

fn template_phase(phase: Option<u8>) -> AngularTemplatePhase {
    match phase {
        Some(1) => AngularTemplatePhase::Creation,
        Some(2) => AngularTemplatePhase::Update,
        _ => AngularTemplatePhase::OutsideRender,
    }
}

fn relative_source_range(span: Span, source_start_pos: u32) -> Option<AngularRecoverySourceRange> {
    if span.lo.0 == 0 || span.hi.0 == 0 {
        return None;
    }
    let start = span.lo.0.checked_sub(source_start_pos)?;
    let end = span.hi.0.checked_sub(source_start_pos)?;
    (start <= end).then_some(AngularRecoverySourceRange { start, end })
}

fn concise_callee_spelling(expression: &Expr) -> Option<String> {
    fn collect(expression: &Expr, parts: &mut Vec<String>) -> Option<()> {
        match strip_parentheses(expression) {
            Expr::Ident(identifier) => {
                parts.push(identifier.sym.to_string());
                Some(())
            }
            Expr::Member(member) => {
                collect(member.obj.as_ref(), parts)?;
                parts.push(member_prop_name(&member.prop)?.to_string());
                Some(())
            }
            _ => None,
        }
    }

    let mut parts = Vec::new();
    collect(expression, &mut parts)?;
    let spelling = parts.join(".");
    (!spelling.is_empty() && spelling.len() <= 128).then_some(spelling)
}

fn issue_comment(issue: &AngularRecoveryIssue) -> String {
    let instruction = issue.instruction.as_deref().unwrap_or("unknown");
    match issue.kind {
        AngularRecoveryIssueKind::UnsupportedTemplateParameters => {
            "Unsupported Ivy template parameters".to_string()
        }
        AngularRecoveryIssueKind::UnsupportedStatement => format!(
            "Unsupported Ivy statement: {}",
            issue.detail.as_deref().unwrap_or("unknown")
        ),
        AngularRecoveryIssueKind::UnsupportedExpression => format!(
            "Unsupported Ivy expression: {}",
            issue.detail.as_deref().unwrap_or("unknown")
        ),
        AngularRecoveryIssueKind::UnsupportedInstruction => {
            format!("Unsupported Ivy instruction: {instruction}")
        }
        AngularRecoveryIssueKind::UnknownRuntimeInstruction => {
            "Unsupported Ivy instruction: unknown-runtime-instruction".to_string()
        }
        AngularRecoveryIssueKind::MalformedInstruction => format!(
            "Malformed Ivy instruction: {instruction} ({})",
            issue.detail.as_deref().unwrap_or("unsupported arguments")
        ),
        AngularRecoveryIssueKind::MissingTargetNode => format!(
            "Missing Ivy target node: {instruction} ({})",
            issue.detail.as_deref().unwrap_or("unknown target")
        ),
        AngularRecoveryIssueKind::MalformedTemplateStructure => format!(
            "Malformed Ivy template structure: {}",
            issue.detail.as_deref().unwrap_or("unknown")
        ),
    }
}

fn issue_operation_key(issue: &AngularRecoveryIssue) -> Option<(AngularTemplatePhase, usize)> {
    Some((issue.phase?, issue.operation_index?))
}

fn place_new_operation_issues(
    tree: &mut TemplateTree,
    issues: &[AngularRecoveryIssue],
    issue_start: usize,
    view_id: usize,
    insertion_point: Option<TreeInsertionPoint>,
) {
    let Some(insertion_point) = insertion_point else {
        return;
    };
    let regions = issues[issue_start..]
        .iter()
        .filter(|issue| issue.view_id == Some(view_id))
        .filter_map(|issue| {
            let key = issue_operation_key(issue)?;
            Some((key, issue_comment(issue)))
        })
        .collect::<Vec<_>>();
    if regions.is_empty() {
        return;
    }
    tree.placed_issue_operations
        .extend(regions.iter().map(|(key, _)| *key));
    tree.insert_unsupported_regions(
        insertion_point,
        regions.into_iter().map(|(_, comment)| comment),
        false,
    );
}

fn place_remaining_view_issues(
    tree: &mut TemplateTree,
    issues: &[AngularRecoveryIssue],
    view_id: usize,
) {
    let comments = issues
        .iter()
        .filter(|issue| issue.view_id == Some(view_id))
        .filter(|issue| {
            issue_operation_key(issue)
                .is_none_or(|key| !tree.placed_issue_operations.contains(&key))
        })
        .map(issue_comment)
        .collect::<Vec<_>>();
    if comments.is_empty() {
        return;
    }
    tree.insert_unsupported_regions(
        TreeInsertionPoint {
            parent: None,
            position: tree.roots.len(),
        },
        comments,
        true,
    );
}

pub(super) fn recover_template(
    template: &Function,
    constant_table: Option<&Expr>,
    projection_selectors: &[String],
    roles: &IvyRoleTable,
    template_functions: &TemplateFunctionTable,
    context: TemplateRecoveryContext,
) -> Result<RecoveredTemplate> {
    let constants = constant_table
        .map(decode_component_constant_table)
        .unwrap_or_default();
    let implicit_view_context_properties = discover_implicit_view_context_properties(
        template_functions,
        roles,
        context.unresolved_ctxt,
    );
    let environment = TemplateRecoveryEnvironment {
        constants: &constants,
        projection_selectors,
        roles,
        template_functions,
        implicit_view_context_properties: &implicit_view_context_properties,
        unresolved_ctxt: context.unresolved_ctxt,
        source_start_pos: context.source_start_pos,
        cm: context.cm,
    };
    let mut active_templates = HashSet::new();
    let mut next_view_id = 0;
    let (tree, program) = recover_template_tree(
        template,
        &environment,
        true,
        None,
        ViewRecoveryState {
            active_templates: &mut active_templates,
            next_view_id: &mut next_view_id,
            ancestor_references: &[],
            ancestor_contexts: &[],
            depth: 0,
        },
    )?;

    let source = render_tree(&tree);
    let mut unknown_runtime_call_shapes = program
        .unknown_runtime_call_shapes
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
    Ok(RecoveredTemplate {
        source: if source.is_empty() {
            "<!-- Empty Ivy template -->".to_string()
        } else {
            source
        },
        issues: program.issues,
        stats: program.stats,
        unknown_runtime_call_shapes,
        artifact_references: program.artifact_references,
        listener_methods: program.listener_methods,
    })
}

fn recover_template_tree(
    template: &Function,
    environment: &TemplateRecoveryEnvironment<'_>,
    is_component_view: bool,
    initial_repeater_item_name: Option<&str>,
    recovery: ViewRecoveryState<'_>,
) -> Result<(TemplateTree, TemplateProgram)> {
    let ViewRecoveryState {
        active_templates,
        next_view_id,
        ancestor_references,
        ancestor_contexts,
        depth,
    } = recovery;
    let view_id = *next_view_id;
    *next_view_id = view_id.saturating_add(1);
    let mut program = TemplateProgram::new(view_id, is_component_view, ancestor_contexts);
    program.repeater_item_name = initial_repeater_item_name.map(str::to_string);
    program
        .implicit_view_context_properties
        .extend(environment.implicit_view_context_properties.iter().cloned());
    let Some(render_flags) = function_param_binding(template, 0) else {
        record_program_issue(
            &mut program,
            issue(
                AngularRecoveryIssueKind::UnsupportedTemplateParameters,
                None,
                None,
            ),
            None,
            template.span,
            None,
            environment,
        );
        return Ok((TemplateTree::default(), program));
    };
    if let Some(context) = function_param_binding(template, 1) {
        program.view_context = Some(context.clone());
        if is_component_view {
            program.component_contexts.insert(context);
        }
    }
    if let Some(body) = &template.body {
        program.member_object_bindings = member_object_bindings(body);
        program.saved_views.extend(inlined_current_view_captures(
            body,
            environment.roles,
            environment.unresolved_ctxt,
        ));
        collect_statements(&body.stmts, None, &render_flags, environment, &mut program);
        record_unresolved_alias_declarations(&mut program, environment);
    }

    let mut tree = TemplateTree::default();
    seed_local_reference_slots(&program.create, &mut tree, environment);
    tree.let_names = seed_let_names(&program.update);
    for instruction in program.create.clone() {
        let insertion_point = tree.next_insertion_point();
        let issue_start = program.issues.len();
        apply_create_instruction(
            &instruction,
            &mut tree,
            &mut program,
            environment,
            ViewRecoveryState {
                active_templates,
                next_view_id,
                ancestor_references,
                ancestor_contexts,
                depth,
            },
        )?;
        place_new_operation_issues(
            &mut tree,
            &program.issues,
            issue_start,
            program.view_id,
            Some(insertion_point),
        );
    }
    resolve_reference_aliases(&mut program, &tree, ancestor_references);
    for instruction in program.update.clone() {
        let insertion_point = tree.update_issue_insertion_point(instruction.instruction);
        let issue_start = program.issues.len();
        apply_update_instruction(&instruction, &mut tree, &mut program, environment)?;
        place_new_operation_issues(
            &mut tree,
            &program.issues,
            issue_start,
            program.view_id,
            insertion_point,
        );
    }
    apply_local_let_alias_hints(&mut tree, &program.let_alias_hints);
    let uninitialized_lets = tree
        .nodes
        .iter()
        .filter_map(|node| match &node.kind {
            TemplateNodeKind::Let {
                value: None,
                provenance,
                ..
            } => Some(provenance.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for provenance in uninitialized_lets {
        program.stats.malformed_instruction_calls += 1;
        record_issue(
            &mut program.issues,
            issue_at_operation(
                issue(
                    AngularRecoveryIssueKind::MalformedInstruction,
                    Some("ɵɵdeclareLet".to_string()),
                    Some("let declaration has no matching stored value".to_string()),
                ),
                &provenance,
            ),
        );
    }
    if !tree.stack.is_empty() {
        record_program_issue(
            &mut program,
            issue(
                AngularRecoveryIssueKind::MalformedTemplateStructure,
                None,
                Some(format!("{} unclosed element(s)", tree.stack.len())),
            ),
            Some(1),
            template.span,
            None,
            environment,
        );
    }
    place_remaining_view_issues(&mut tree, &program.issues, program.view_id);
    Ok((tree, program))
}

fn apply_local_let_alias_hints(tree: &mut TemplateTree, hints: &[ViewLetAliasHint]) {
    let mut candidates = HashMap::<usize, HashSet<String>>::new();
    for hint in hints.iter().filter(|hint| hint.context_depth == 0) {
        candidates
            .entry(hint.slot)
            .or_default()
            .insert(hint.name.clone());
    }
    for (slot, names) in candidates {
        let mut names = names.into_iter();
        let Some(name) = names.next().filter(|_| names.next().is_none()) else {
            continue;
        };
        tree.let_names.insert(slot, name.clone());
        let Some(&node) = tree.index_to_node.get(&slot) else {
            continue;
        };
        if let TemplateNodeKind::Let {
            name: node_name, ..
        } = &mut tree.nodes[node].kind
        {
            *node_name = name;
        }
    }
}

fn resolve_reference_aliases(
    program: &mut TemplateProgram,
    tree: &TemplateTree,
    ancestor_references: &[ReferenceScope],
) {
    for alias in std::mem::take(&mut program.reference_aliases) {
        let PendingReferenceAlias {
            binding,
            call,
            context_depth,
            structural_candidate,
        } = alias;
        let Some(slot) = numeric_arg(&call.args, 0).filter(|_| call.args.len() == 1) else {
            if structural_candidate {
                record_unresolved_reference_candidate(
                    &call,
                    "candidate call does not have one numeric slot",
                    program,
                );
            } else {
                record_malformed_instruction(
                    &call,
                    "expected one numeric local-reference slot",
                    &mut program.issues,
                    &mut program.stats,
                );
            }
            continue;
        };
        let Some(name) = reference_name_at_depth(tree, ancestor_references, context_depth, slot)
        else {
            if structural_candidate {
                record_unresolved_reference_candidate(
                    &call,
                    &format!("no local reference at slot {slot} in context depth {context_depth}"),
                    program,
                );
            } else {
                record_missing_target(
                    &call,
                    &format!("no local reference at slot {slot} in context depth {context_depth}"),
                    &mut program.issues,
                    &mut program.stats,
                );
            }
            continue;
        };
        program.local_reference_names.insert(binding, name);
        program.stats.rendered_instruction_calls += 1;
    }
    for alias in std::mem::take(&mut program.inline_reference_aliases) {
        let Some(name) =
            reference_name_at_depth(tree, ancestor_references, alias.context_depth, alias.slot)
        else {
            record_issue(
                &mut program.issues,
                issue_at_operation(
                    issue(
                        AngularRecoveryIssueKind::MissingTargetNode,
                        Some("inlined ɵɵreference".to_string()),
                        Some(format!(
                            "no local reference at slot {} in context depth {}",
                            alias.slot, alias.context_depth
                        )),
                    ),
                    &alias.provenance,
                ),
            );
            continue;
        };
        program.local_reference_names.insert(alias.binding, name);
    }
}

fn record_unresolved_reference_candidate(
    call: &InstructionCall,
    detail: &str,
    program: &mut TemplateProgram,
) {
    program.stats.unsupported_runtime_calls += 1;
    record_issue(
        &mut program.issues,
        issue_at_operation(
            issue(
                AngularRecoveryIssueKind::UnknownRuntimeInstruction,
                None,
                Some(detail.to_string()),
            ),
            &call.provenance,
        ),
    );
}

fn reference_name_at_depth(
    tree: &TemplateTree,
    ancestor_references: &[ReferenceScope],
    context_depth: usize,
    slot: usize,
) -> Option<String> {
    let scope = if context_depth == 0 {
        &tree.local_reference_slots
    } else {
        ancestor_references.get(context_depth - 1)?
    };
    scope.get(&slot).cloned()
}

pub(super) fn ivy_template_score(
    template: &Function,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
) -> usize {
    struct Counter<'a> {
        roles: &'a IvyRoleTable,
        unresolved_ctxt: SyntaxContext,
        score: usize,
    }

    impl Visit for Counter<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            let score = call_chain(call)
                .and_then(|(root, _)| self.roles.instruction_for_expr(root, self.unresolved_ctxt))
                .map(|instruction| match instruction {
                    IvyInstruction::ElementStart
                    | IvyInstruction::Element
                    | IvyInstruction::ElementContainerStart
                    | IvyInstruction::ElementContainer
                    | IvyInstruction::Text
                    | IvyInstruction::Template
                    | IvyInstruction::Defer
                    | IvyInstruction::RepeaterCreate
                    | IvyInstruction::DeclareLet
                    | IvyInstruction::Projection => 3,
                    IvyInstruction::ElementEnd
                    | IvyInstruction::ElementContainerEnd
                    | IvyInstruction::NamespaceHtml
                    | IvyInstruction::NamespaceSvg
                    | IvyInstruction::NamespaceMathMl
                    | IvyInstruction::Listener
                    | IvyInstruction::AnimateEnter
                    | IvyInstruction::AnimateEnterListener
                    | IvyInstruction::AnimateLeave
                    | IvyInstruction::AnimateLeaveListener
                    | IvyInstruction::TwoWayProperty
                    | IvyInstruction::TwoWayListener
                    | IvyInstruction::TwoWayBindingSet
                    | IvyInstruction::DeferOnIdle
                    | IvyInstruction::Conditional
                    | IvyInstruction::Repeater
                    | IvyInstruction::RepeaterTrackByIndex
                    | IvyInstruction::RepeaterTrackByIdentity
                    | IvyInstruction::NextContext
                    | IvyInstruction::GetCurrentView
                    | IvyInstruction::RestoreView
                    | IvyInstruction::ResetView
                    | IvyInstruction::ProjectionDef
                    | IvyInstruction::Reference
                    | IvyInstruction::StoreLet
                    | IvyInstruction::ReadContextLet
                    | IvyInstruction::Pipe
                    | IvyInstruction::PipeBind1
                    | IvyInstruction::PipeBind2
                    | IvyInstruction::PipeBind3
                    | IvyInstruction::PipeBind4
                    | IvyInstruction::PipeBindV
                    | IvyInstruction::PureFunction0
                    | IvyInstruction::PureFunction1
                    | IvyInstruction::PureFunction2
                    | IvyInstruction::PureFunction3
                    | IvyInstruction::PureFunction4
                    | IvyInstruction::PureFunction5
                    | IvyInstruction::PureFunction6
                    | IvyInstruction::PureFunction7
                    | IvyInstruction::PureFunction8
                    | IvyInstruction::PureFunctionV
                    | IvyInstruction::I18n
                    | IvyInstruction::I18nStart
                    | IvyInstruction::I18nEnd
                    | IvyInstruction::I18nExp
                    | IvyInstruction::I18nApply
                    | IvyInstruction::Advance
                    | IvyInstruction::Interpolate
                    | IvyInstruction::Interpolate1
                    | IvyInstruction::Interpolate2
                    | IvyInstruction::Interpolate3
                    | IvyInstruction::Interpolate4
                    | IvyInstruction::Interpolate5
                    | IvyInstruction::Interpolate6
                    | IvyInstruction::Interpolate7
                    | IvyInstruction::Interpolate8
                    | IvyInstruction::InterpolateV
                    | IvyInstruction::TextInterpolate
                    | IvyInstruction::TextInterpolate1
                    | IvyInstruction::TextInterpolate2
                    | IvyInstruction::TextInterpolate3
                    | IvyInstruction::TextInterpolate4
                    | IvyInstruction::TextInterpolate5
                    | IvyInstruction::TextInterpolate6
                    | IvyInstruction::TextInterpolate7
                    | IvyInstruction::TextInterpolate8
                    | IvyInstruction::Property
                    | IvyInstruction::AriaProperty
                    | IvyInstruction::Attribute
                    | IvyInstruction::ClassMap
                    | IvyInstruction::ClassProp
                    | IvyInstruction::StyleMap
                    | IvyInstruction::StyleProp => 1,
                    IvyInstruction::DefineComponent
                    | IvyInstruction::ViewQuerySignal
                    | IvyInstruction::ContentQuerySignal => 0,
                })
                .unwrap_or(0);
            self.score += score;
            call.visit_children_with(self);
        }
    }

    let mut counter = Counter {
        roles,
        unresolved_ctxt,
        score: 0,
    };
    template.visit_with(&mut counter);
    counter.score
}

fn function_param_binding(function: &Function, index: usize) -> Option<BindingKey> {
    let Pat::Ident(binding) = &function.params.get(index)?.pat else {
        return None;
    };
    Some(binding_key(&binding.id))
}

fn collect_statements(
    statements: &[Stmt],
    phase: Option<u8>,
    render_flags: &BindingKey,
    environment: &TemplateRecoveryEnvironment<'_>,
    program: &mut TemplateProgram,
) {
    for statement in statements {
        collect_statement(statement, phase, render_flags, environment, program);
    }
}

fn collect_statement(
    statement: &Stmt,
    phase: Option<u8>,
    render_flags: &BindingKey,
    environment: &TemplateRecoveryEnvironment<'_>,
    program: &mut TemplateProgram,
) {
    match statement {
        Stmt::Empty(_) => {}
        Stmt::Block(block) => {
            collect_statements(&block.stmts, phase, render_flags, environment, program)
        }
        Stmt::If(if_statement) => {
            let (branch_phase, is_render_guard) = collect_if_test(
                if_statement.test.as_ref(),
                phase,
                render_flags,
                environment,
                program,
            );
            if !is_render_guard {
                record_program_issue(
                    program,
                    issue(
                        AngularRecoveryIssueKind::UnsupportedStatement,
                        None,
                        Some("conditional control flow".to_string()),
                    ),
                    phase,
                    if_statement.span,
                    None,
                    environment,
                );
            }
            collect_statement(
                if_statement.cons.as_ref(),
                branch_phase,
                render_flags,
                environment,
                program,
            );
            if let Some(alternate) = &if_statement.alt {
                if is_render_guard {
                    record_program_issue(
                        program,
                        issue(
                            AngularRecoveryIssueKind::UnsupportedStatement,
                            None,
                            Some("render-flag alternate branch".to_string()),
                        ),
                        phase,
                        alternate.span(),
                        None,
                        environment,
                    );
                }
                collect_statement(
                    alternate.as_ref(),
                    phase,
                    render_flags,
                    environment,
                    program,
                );
            }
        }
        Stmt::Expr(expression) => collect_expression(
            expression.expr.as_ref(),
            phase,
            render_flags,
            environment,
            program,
        ),
        Stmt::Decl(Decl::Var(declaration)) => {
            collect_variable_declaration(declaration, phase, render_flags, environment, program)
        }
        _ => record_program_issue(
            program,
            issue(
                AngularRecoveryIssueKind::UnsupportedStatement,
                None,
                Some(statement_kind(statement).to_string()),
            ),
            phase,
            statement.span(),
            None,
            environment,
        ),
    }
}

fn collect_variable_declaration(
    declaration: &swc_core::ecma::ast::VarDecl,
    phase: Option<u8>,
    render_flags: &BindingKey,
    environment: &TemplateRecoveryEnvironment<'_>,
    program: &mut TemplateProgram,
) {
    for declarator in &declaration.decls {
        if let (Pat::Ident(binding), None) = (&declarator.name, declarator.init.as_deref()) {
            program
                .pending_alias_declarations
                .push(PendingAliasDeclaration {
                    binding: binding_key(&binding.id),
                    span: declarator.span,
                    phase,
                });
            continue;
        }

        let supported_compiler_alias = match (&declarator.name, declarator.init.as_deref()) {
            (Pat::Ident(binding), Some(Expr::Call(call))) => {
                collect_store_let_alias(&binding.id, call, phase, environment, program)
                    || collect_read_context_let_alias(
                        &binding.id,
                        call,
                        phase,
                        environment,
                        program,
                    )
                    || collect_current_view_alias(&binding.id, call, phase, environment, program)
                    || collect_next_context_alias(&binding.id, call, phase, environment, program)
                    || collect_reference_alias(&binding.id, call, phase, environment, program)
                    || collect_reference_candidate_alias(
                        &binding.id,
                        call,
                        phase,
                        environment,
                        program,
                    )
            }
            (Pat::Ident(binding), Some(initializer)) => {
                is_inlined_current_view_alias(&binding.id, initializer, phase, program)
                    || collect_inlined_reference_alias(
                        &binding.id,
                        initializer,
                        phase,
                        declarator.span,
                        environment,
                        program,
                    )
                    || collect_view_context_alias(&binding.id, initializer, phase, program)
                    || collect_component_context_member_alias(
                        &binding.id,
                        initializer,
                        phase,
                        program,
                    )
                    || collect_next_context_member_expression_alias(
                        &binding.id,
                        initializer,
                        phase,
                        environment,
                        program,
                    )
            }
            _ => false,
        };
        if supported_compiler_alias {
            continue;
        }

        record_program_issue(
            program,
            issue(
                AngularRecoveryIssueKind::UnsupportedStatement,
                None,
                Some("declaration".to_string()),
            ),
            phase,
            declarator.span,
            None,
            environment,
        );
        if let Some(initializer) = &declarator.init {
            collect_expression(
                initializer.as_ref(),
                phase,
                render_flags,
                environment,
                program,
            );
        }
    }
}

fn record_unresolved_alias_declarations(
    program: &mut TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
) {
    for declaration in std::mem::take(&mut program.pending_alias_declarations) {
        if program
            .resolved_alias_declarations
            .contains(&declaration.binding)
        {
            continue;
        }
        record_program_issue(
            program,
            issue(
                AngularRecoveryIssueKind::UnsupportedStatement,
                None,
                Some("declaration".to_string()),
            ),
            declaration.phase,
            declaration.span,
            None,
            environment,
        );
    }
}

fn inlined_current_view_captures(
    body: &BlockStmt,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
) -> HashSet<BindingKey> {
    struct Collector<'a> {
        roles: &'a IvyRoleTable,
        unresolved_ctxt: SyntaxContext,
        member_initializers: HashSet<BindingKey>,
        restored_bindings: HashSet<BindingKey>,
    }

    impl Visit for Collector<'_> {
        fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
            if let (Pat::Ident(binding), Some(initializer)) =
                (&declarator.name, declarator.init.as_deref())
            {
                if matches!(strip_parentheses(initializer), Expr::Member(_)) {
                    self.member_initializers.insert(binding_key(&binding.id));
                }
            }
            declarator.visit_children_with(self);
        }

        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Some((root, argument_lists)) = call_chain(call) {
                if self.roles.instruction_for_expr(root, self.unresolved_ctxt)
                    == Some(IvyInstruction::RestoreView)
                    && argument_lists.len() == 1
                    && argument_lists[0].len() == 1
                {
                    if let Expr::Ident(saved_view) =
                        strip_parentheses(argument_lists[0][0].expr.as_ref())
                    {
                        self.restored_bindings.insert(binding_key(saved_view));
                    }
                }
            }
            call.visit_children_with(self);
        }
    }

    let mut collector = Collector {
        roles,
        unresolved_ctxt,
        member_initializers: HashSet::new(),
        restored_bindings: HashSet::new(),
    };
    body.visit_with(&mut collector);
    collector
        .member_initializers
        .intersection(&collector.restored_bindings)
        .cloned()
        .collect()
}

fn member_object_bindings(body: &BlockStmt) -> HashSet<BindingKey> {
    #[derive(Default)]
    struct Collector {
        bindings: HashSet<BindingKey>,
    }

    impl Visit for Collector {
        fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
            if let Expr::Ident(object) = strip_parentheses(member.obj.as_ref()) {
                self.bindings.insert(binding_key(object));
            }
            member.visit_children_with(self);
        }
    }

    let mut collector = Collector::default();
    body.visit_with(&mut collector);
    collector.bindings
}

fn discover_implicit_view_context_properties(
    template_functions: &TemplateFunctionTable,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
) -> HashSet<String> {
    struct Collector<'a> {
        context: &'a BindingKey,
        member_objects: &'a HashSet<BindingKey>,
        properties: HashSet<String>,
    }

    impl Collector<'_> {
        fn collect_alias(&mut self, binding: &swc_core::ecma::ast::Ident, initializer: &Expr) {
            if !self.member_objects.contains(&binding_key(binding)) {
                return;
            }
            let Expr::Member(member) = strip_parentheses(initializer) else {
                return;
            };
            let Expr::Ident(context) = strip_parentheses(member.obj.as_ref()) else {
                return;
            };
            if binding_key(context) != *self.context {
                return;
            }
            let Some(property) = member_prop_name(&member.prop) else {
                return;
            };
            let property = property.to_string();
            if property == "$implicit" || is_likely_generated_alias(&property) {
                self.properties.insert(property);
            }
        }
    }

    impl Visit for Collector<'_> {
        fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
            if let (Pat::Ident(binding), Some(initializer)) =
                (&declarator.name, declarator.init.as_deref())
            {
                self.collect_alias(&binding.id, initializer);
            }
            declarator.visit_children_with(self);
        }

        fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
            if assignment.op == AssignOp::Assign {
                if let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &assignment.left {
                    self.collect_alias(&binding.id, assignment.right.as_ref());
                }
            }
            assignment.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &swc_core::ecma::ast::ArrowExpr) {}
    }

    let mut properties = HashSet::new();
    for function in template_functions.functions.values() {
        if ivy_template_score(function, roles, unresolved_ctxt) < 3 {
            continue;
        }
        let Some(context) = function_param_binding(function, 1) else {
            continue;
        };
        let Some(body) = function.body.as_ref() else {
            continue;
        };
        let member_objects = member_object_bindings(body);
        let mut collector = Collector {
            context: &context,
            member_objects: &member_objects,
            properties: HashSet::new(),
        };
        body.visit_with(&mut collector);
        properties.extend(collector.properties);
    }
    properties
}

fn is_inlined_current_view_alias(
    binding: &swc_core::ecma::ast::Ident,
    initializer: &Expr,
    phase: Option<u8>,
    program: &TemplateProgram,
) -> bool {
    phase == Some(1)
        && program.saved_views.contains(&binding_key(binding))
        && matches!(strip_parentheses(initializer), Expr::Member(_))
}

fn collect_current_view_alias(
    binding: &swc_core::ecma::ast::Ident,
    call: &CallExpr,
    phase: Option<u8>,
    environment: &TemplateRecoveryEnvironment<'_>,
    program: &mut TemplateProgram,
) -> bool {
    if phase != Some(1) {
        return false;
    }
    let Some((root, argument_lists)) = call_chain(call).filter(|(root, _)| {
        environment
            .roles
            .instruction_for_expr(root, environment.unresolved_ctxt)
            == Some(IvyInstruction::GetCurrentView)
    }) else {
        return false;
    };

    program.stats.runtime_calls_observed += argument_lists.len();
    let provenances = call_provenances(
        program,
        phase,
        call,
        root,
        argument_lists.len(),
        environment,
    );
    if argument_lists.len() == 1 && argument_lists[0].is_empty() {
        program.stats.rendered_instruction_calls += 1;
        program.saved_views.insert(binding_key(binding));
    } else {
        program.stats.malformed_instruction_calls += argument_lists.len();
        for provenance in provenances {
            record_issue(
                &mut program.issues,
                issue_at_operation(
                    issue(
                        AngularRecoveryIssueKind::MalformedInstruction,
                        Some("ɵɵgetCurrentView".to_string()),
                        Some("expected no arguments".to_string()),
                    ),
                    &provenance,
                ),
            );
        }
    }
    true
}

fn collect_view_context_alias(
    binding: &swc_core::ecma::ast::Ident,
    initializer: &Expr,
    phase: Option<u8>,
    program: &mut TemplateProgram,
) -> bool {
    if phase != Some(2) {
        return false;
    }
    let Expr::Member(member) = strip_parentheses(initializer) else {
        return false;
    };
    let Expr::Ident(context) = strip_parentheses(member.obj.as_ref()) else {
        return false;
    };
    if program.component_contexts.contains(&binding_key(context)) {
        return false;
    }
    if program
        .view_context
        .as_ref()
        .is_none_or(|view_context| binding_key(context) != *view_context)
    {
        return false;
    }
    let Some(property) = member_prop_name(&member.prop) else {
        return false;
    };
    let property = property.to_string();
    let inferred_implicit = property == "$implicit"
        || program.implicit_view_context_properties.contains(&property)
        || (is_likely_generated_alias(&property)
            && !program.component_contexts.contains(&binding_key(context))
            && program
                .member_object_bindings
                .contains(&binding_key(binding)));
    let fallback = if inferred_implicit {
        program.repeater_item_name.as_deref().unwrap_or("item")
    } else if property.starts_with('$') || !is_likely_generated_alias(&property) {
        property.as_str()
    } else {
        return false;
    };
    let name = recovered_view_alias_name(binding.sym.as_ref(), fallback);
    program
        .local_reference_names
        .insert(binding_key(binding), name.clone());
    if inferred_implicit {
        program.implicit_view_context_properties.insert(property);
        if program
            .repeater_item_name
            .as_ref()
            .is_none_or(|current| current == "item" && name != "item")
        {
            program.repeater_item_name = Some(name);
        }
    }
    true
}

fn collect_component_context_member_alias(
    binding: &swc_core::ecma::ast::Ident,
    initializer: &Expr,
    phase: Option<u8>,
    program: &mut TemplateProgram,
) -> bool {
    if phase != Some(2) {
        return false;
    }
    let Expr::Member(member) = strip_parentheses(initializer) else {
        return false;
    };
    let Expr::Ident(context) = strip_parentheses(member.obj.as_ref()) else {
        return false;
    };
    if !program.component_contexts.contains(&binding_key(context)) {
        return false;
    }
    let Some(property) = member_prop_name(&member.prop) else {
        return false;
    };
    program.local_reference_names.insert(
        binding_key(binding),
        to_valid_identifier_name(property.as_ref()),
    );
    true
}

fn collect_next_context_member_expression_alias(
    binding: &swc_core::ecma::ast::Ident,
    initializer: &Expr,
    phase: Option<u8>,
    environment: &TemplateRecoveryEnvironment<'_>,
    program: &mut TemplateProgram,
) -> bool {
    if phase != Some(2) {
        return false;
    }
    let Expr::Member(member) = strip_parentheses(initializer) else {
        return false;
    };
    let Expr::Call(call) = strip_parentheses(member.obj.as_ref()) else {
        return false;
    };
    let Some((root, argument_lists)) = call_chain(call).filter(|(root, _)| {
        environment
            .roles
            .instruction_for_expr(root, environment.unresolved_ctxt)
            == Some(IvyInstruction::NextContext)
    }) else {
        return false;
    };

    program.stats.runtime_calls_observed += argument_lists.len();
    let provenances = call_provenances(
        program,
        phase,
        call,
        root,
        argument_lists.len(),
        environment,
    );
    let Some(property) = member_prop_name(&member.prop) else {
        program.stats.malformed_instruction_calls += argument_lists.len();
        for provenance in provenances {
            record_issue(
                &mut program.issues,
                issue_at_operation(
                    issue(
                        AngularRecoveryIssueKind::MalformedInstruction,
                        Some("ɵɵnextContext".to_string()),
                        Some("context member has a computed property".to_string()),
                    ),
                    &provenance,
                ),
            );
        }
        return true;
    };
    let Some(context_hop) = context_hop(&argument_lists) else {
        program.stats.malformed_instruction_calls += argument_lists.len();
        for provenance in provenances {
            record_issue(
                &mut program.issues,
                issue_at_operation(
                    issue(
                        AngularRecoveryIssueKind::MalformedInstruction,
                        Some("ɵɵnextContext".to_string()),
                        Some("unexpected context-depth arguments".to_string()),
                    ),
                    &provenance,
                ),
            );
        }
        return true;
    };

    let destination_depth = program.update_context_depth.saturating_add(context_hop);
    let binding_key = binding_key(binding);
    let recovered_name = program
        .context_property_name_for_binding(destination_depth, property.as_ref(), &binding_key)
        .unwrap_or_else(|| to_valid_identifier_name(property.as_ref()));
    if destination_depth > 0 {
        if let Some(scope) = program.ancestor_contexts.get_mut(destination_depth - 1) {
            let inferred_implicit = scope
                .local_properties
                .get("$implicit")
                .is_some_and(|implicit| implicit == &recovered_name);
            if inferred_implicit {
                scope
                    .local_properties
                    .insert(property.to_string(), recovered_name.clone());
            }
        }
    }
    program.stats.rendered_instruction_calls += argument_lists.len();
    program
        .local_reference_names
        .insert(binding_key, recovered_name);
    program.update_context_depth = destination_depth;
    true
}

fn collect_inlined_reference_alias(
    binding: &swc_core::ecma::ast::Ident,
    initializer: &Expr,
    phase: Option<u8>,
    span: Span,
    environment: &TemplateRecoveryEnvironment<'_>,
    program: &mut TemplateProgram,
) -> bool {
    if phase != Some(2) {
        return false;
    }
    let Some(slot) = inlined_reference_slot(initializer) else {
        return false;
    };
    let provenance = program.next_operation(
        AngularTemplatePhase::Update,
        span,
        None,
        environment.source_start_pos,
    );
    program.inline_reference_aliases.push(InlineReferenceAlias {
        binding: binding_key(binding),
        slot,
        context_depth: program.update_context_depth,
        provenance,
    });
    true
}

fn inlined_reference_slot(initializer: &Expr) -> Option<usize> {
    let Expr::Member(member) = strip_parentheses(initializer) else {
        return None;
    };
    if !matches!(strip_parentheses(member.obj.as_ref()), Expr::Member(_)) {
        return None;
    }
    let MemberProp::Computed(computed) = &member.prop else {
        return None;
    };
    numeric_expr(computed.expr.as_ref())?.checked_sub(27)
}

fn recovered_view_alias_name(binding: &str, fallback: &str) -> String {
    let authored = binding
        .rsplit_once("_r")
        .filter(|(prefix, suffix)| {
            !prefix.is_empty()
                && !suffix.is_empty()
                && suffix.chars().all(|character| character.is_ascii_digit())
        })
        .map_or(binding, |(prefix, _)| prefix);
    if is_likely_generated_alias(authored) {
        fallback.to_string()
    } else {
        to_valid_identifier_name(authored)
    }
}

fn collect_next_context_alias(
    binding: &swc_core::ecma::ast::Ident,
    call: &CallExpr,
    phase: Option<u8>,
    environment: &TemplateRecoveryEnvironment<'_>,
    program: &mut TemplateProgram,
) -> bool {
    if phase != Some(2) {
        return false;
    }
    let Some((root, argument_lists)) = call_chain(call).filter(|(root, _)| {
        environment
            .roles
            .instruction_for_expr(root, environment.unresolved_ctxt)
            == Some(IvyInstruction::NextContext)
    }) else {
        return false;
    };

    program.stats.runtime_calls_observed += argument_lists.len();
    let provenances = call_provenances(
        program,
        phase,
        call,
        root,
        argument_lists.len(),
        environment,
    );
    if let Some(context_hop) = context_hop(&argument_lists) {
        program.stats.rendered_instruction_calls += 1;
        let destination_depth = program.update_context_depth.saturating_add(context_hop);
        match program.context_scope_at_depth(destination_depth) {
            Some(scope) if scope.is_component => {
                program.component_contexts.insert(binding_key(binding));
            }
            Some(scope) if !scope.local_properties.is_empty() => {
                program
                    .local_context_bindings
                    .insert(binding_key(binding), scope.local_properties);
            }
            Some(_) => {}
            None => {
                program.component_contexts.insert(binding_key(binding));
            }
        }
        program.update_context_depth = destination_depth;
    } else {
        program.stats.malformed_instruction_calls += argument_lists.len();
        for provenance in provenances {
            record_issue(
                &mut program.issues,
                issue_at_operation(
                    issue(
                        AngularRecoveryIssueKind::MalformedInstruction,
                        Some("ɵɵnextContext".to_string()),
                        Some("unexpected context-depth arguments".to_string()),
                    ),
                    &provenance,
                ),
            );
        }
    }
    true
}

fn context_hop(argument_lists: &[&[ExprOrSpread]]) -> Option<usize> {
    let [arguments] = argument_lists else {
        return None;
    };
    match *arguments {
        [] => Some(1),
        [argument] => numeric_expr(argument.expr.as_ref()),
        _ => None,
    }
}

fn collect_store_let_alias(
    binding: &swc_core::ecma::ast::Ident,
    call: &CallExpr,
    phase: Option<u8>,
    environment: &TemplateRecoveryEnvironment<'_>,
    program: &mut TemplateProgram,
) -> bool {
    if phase != Some(2) {
        return false;
    }
    let Some((root, argument_lists)) = call_chain(call).filter(|(root, _)| {
        environment
            .roles
            .instruction_for_expr(root, environment.unresolved_ctxt)
            == Some(IvyInstruction::StoreLet)
    }) else {
        return false;
    };

    program.stats.runtime_calls_observed += argument_lists.len();
    let provenances = call_provenances(
        program,
        phase,
        call,
        root,
        argument_lists.len(),
        environment,
    );
    program.update.extend(
        argument_lists
            .into_iter()
            .zip(provenances)
            .map(|(args, provenance)| InstructionCall {
                instruction: IvyInstruction::StoreLet,
                args: args.iter().map(|argument| argument.expr.clone()).collect(),
                result_binding: Some(binding_key(binding)),
                provenance,
            }),
    );
    true
}

fn collect_read_context_let_alias(
    binding: &swc_core::ecma::ast::Ident,
    call: &CallExpr,
    phase: Option<u8>,
    environment: &TemplateRecoveryEnvironment<'_>,
    program: &mut TemplateProgram,
) -> bool {
    if phase != Some(2) {
        return false;
    }
    let Some((root, argument_lists)) = call_chain(call).filter(|(root, _)| {
        environment
            .roles
            .instruction_for_expr(root, environment.unresolved_ctxt)
            == Some(IvyInstruction::ReadContextLet)
    }) else {
        return false;
    };

    program.stats.runtime_calls_observed += argument_lists.len();
    let provenances = call_provenances(
        program,
        phase,
        call,
        root,
        argument_lists.len(),
        environment,
    );
    let [arguments] = argument_lists.as_slice() else {
        program.stats.malformed_instruction_calls += argument_lists.len();
        for provenance in provenances {
            record_issue(
                &mut program.issues,
                issue_at_operation(
                    issue(
                        AngularRecoveryIssueKind::MalformedInstruction,
                        Some("ɵɵreadContextLet".to_string()),
                        Some("unexpected chained invocation".to_string()),
                    ),
                    &provenance,
                ),
            );
        }
        return true;
    };
    let [slot] = *arguments else {
        program.stats.malformed_instruction_calls += 1;
        record_issue(
            &mut program.issues,
            issue_at_operation(
                issue(
                    AngularRecoveryIssueKind::MalformedInstruction,
                    Some("ɵɵreadContextLet".to_string()),
                    Some("expected one numeric let slot".to_string()),
                ),
                provenances
                    .first()
                    .expect("a call chain always contains one invocation"),
            ),
        );
        return true;
    };
    let Some(slot) = numeric_expr(slot.expr.as_ref()) else {
        program.stats.malformed_instruction_calls += 1;
        record_issue(
            &mut program.issues,
            issue_at_operation(
                issue(
                    AngularRecoveryIssueKind::MalformedInstruction,
                    Some("ɵɵreadContextLet".to_string()),
                    Some("expected one numeric let slot".to_string()),
                ),
                provenances
                    .first()
                    .expect("a call chain always contains one invocation"),
            ),
        );
        return true;
    };
    let name = recovered_let_name(Some(binding.sym.as_ref()), slot);
    program
        .local_reference_names
        .insert(binding_key(binding), name.clone());
    program.let_alias_hints.push(ViewLetAliasHint {
        context_depth: program.update_context_depth,
        slot,
        name,
    });
    program.stats.rendered_instruction_calls += 1;
    true
}

fn recovered_let_name(binding: Option<&str>, slot: usize) -> String {
    let fallback = if slot == 0 {
        "value".to_string()
    } else {
        format!("value{slot}")
    };
    binding.map_or_else(
        || fallback.clone(),
        |binding| recovered_view_alias_name(binding, &fallback),
    )
}

fn collect_reference_alias(
    binding: &swc_core::ecma::ast::Ident,
    call: &CallExpr,
    phase: Option<u8>,
    environment: &TemplateRecoveryEnvironment<'_>,
    program: &mut TemplateProgram,
) -> bool {
    if phase != Some(2) {
        return false;
    }
    let Some((root, argument_lists)) = call_chain(call).filter(|(root, _)| {
        environment
            .roles
            .instruction_for_expr(root, environment.unresolved_ctxt)
            == Some(IvyInstruction::Reference)
    }) else {
        return false;
    };

    program.stats.runtime_calls_observed += argument_lists.len();
    let mut provenances = call_provenances(
        program,
        phase,
        call,
        root,
        argument_lists.len(),
        environment,
    );
    if argument_lists.len() != 1 {
        program.stats.malformed_instruction_calls += argument_lists.len();
        for provenance in provenances {
            record_issue(
                &mut program.issues,
                issue_at_operation(
                    issue(
                        AngularRecoveryIssueKind::MalformedInstruction,
                        Some("ɵɵreference".to_string()),
                        Some("unexpected chained invocation".to_string()),
                    ),
                    &provenance,
                ),
            );
        }
        return true;
    }
    let provenance = provenances
        .pop()
        .expect("a call chain always contains one invocation");
    program.reference_aliases.push(PendingReferenceAlias {
        binding: binding_key(binding),
        call: InstructionCall {
            instruction: IvyInstruction::Reference,
            args: argument_lists[0]
                .iter()
                .map(|argument| argument.expr.clone())
                .collect(),
            result_binding: None,
            provenance,
        },
        context_depth: program.update_context_depth,
        structural_candidate: false,
    });
    true
}

fn collect_reference_candidate_alias(
    binding: &swc_core::ecma::ast::Ident,
    call: &CallExpr,
    phase: Option<u8>,
    environment: &TemplateRecoveryEnvironment<'_>,
    program: &mut TemplateProgram,
) -> bool {
    if phase != Some(2) {
        return false;
    }
    let Some((root, argument_lists)) = call_chain(call).filter(|(root, _)| {
        environment
            .roles
            .is_reference_candidate_expr(root, environment.unresolved_ctxt)
    }) else {
        return false;
    };

    program.stats.runtime_calls_observed += argument_lists.len();
    let mut provenances = call_provenances(
        program,
        phase,
        call,
        root,
        argument_lists.len(),
        environment,
    );
    let provenance = provenances
        .pop()
        .expect("a call chain always contains one invocation");
    let args = if let [arguments] = argument_lists.as_slice() {
        arguments
            .iter()
            .map(|argument| argument.expr.clone())
            .collect()
    } else {
        Vec::new()
    };
    program.reference_aliases.push(PendingReferenceAlias {
        binding: binding_key(binding),
        call: InstructionCall {
            instruction: IvyInstruction::Reference,
            args,
            result_binding: None,
            provenance,
        },
        context_depth: program.update_context_depth,
        structural_candidate: true,
    });
    true
}

fn collect_if_test(
    test: &Expr,
    phase: Option<u8>,
    render_flags: &BindingKey,
    environment: &TemplateRecoveryEnvironment<'_>,
    program: &mut TemplateProgram,
) -> (Option<u8>, bool) {
    let test = strip_parentheses(test);
    let Expr::Seq(sequence) = test else {
        let mask = render_flag_mask(test, render_flags);
        return (mask.or(phase), mask.is_some());
    };
    let Some((condition, effects)) = sequence.exprs.split_last() else {
        return (phase, false);
    };
    for effect in effects {
        collect_expression(effect.as_ref(), phase, render_flags, environment, program);
    }
    let mask = render_flag_mask(strip_parentheses(condition), render_flags);
    (mask.or(phase), mask.is_some())
}

fn collect_expression(
    expression: &Expr,
    phase: Option<u8>,
    render_flags: &BindingKey,
    environment: &TemplateRecoveryEnvironment<'_>,
    program: &mut TemplateProgram,
) {
    match expression {
        Expr::Paren(paren) => collect_expression(
            paren.expr.as_ref(),
            phase,
            render_flags,
            environment,
            program,
        ),
        Expr::Seq(sequence) => {
            for expression in &sequence.exprs {
                collect_expression(
                    expression.as_ref(),
                    phase,
                    render_flags,
                    environment,
                    program,
                );
            }
        }
        Expr::Bin(binary) if binary.op == BinaryOp::LogicalAnd => {
            let mask = render_flag_mask(binary.left.as_ref(), render_flags);
            if mask.is_none() {
                record_program_issue(
                    program,
                    issue(
                        AngularRecoveryIssueKind::UnsupportedExpression,
                        None,
                        Some("conditional logical-and".to_string()),
                    ),
                    phase,
                    binary.span,
                    None,
                    environment,
                );
            }
            let branch_phase = mask.or(phase);
            collect_expression(
                binary.right.as_ref(),
                branch_phase,
                render_flags,
                environment,
                program,
            );
        }
        Expr::Assign(assignment) => {
            if phase == Some(1)
                && environment
                    .roles
                    .is_namespace_html_reset_assignment(assignment, environment.unresolved_ctxt)
            {
                return;
            }
            let pending_alias_binding = match &assignment.left {
                AssignTarget::Simple(SimpleAssignTarget::Ident(binding))
                    if assignment.op == AssignOp::Assign =>
                {
                    let key = binding_key(&binding.id);
                    program
                        .pending_alias_declarations
                        .iter()
                        .any(|declaration| declaration.binding == key)
                        .then_some(key)
                }
                _ => None,
            };
            let supported_context_alias = match (&assignment.left, assignment.right.as_ref()) {
                (AssignTarget::Simple(SimpleAssignTarget::Ident(binding)), right)
                    if assignment.op == AssignOp::Assign =>
                {
                    collect_view_context_alias(&binding.id, right, phase, program)
                        || collect_inlined_reference_alias(
                            &binding.id,
                            right,
                            phase,
                            assignment.span,
                            environment,
                            program,
                        )
                        || collect_next_context_member_expression_alias(
                            &binding.id,
                            right,
                            phase,
                            environment,
                            program,
                        )
                        || match right {
                            Expr::Call(call) => {
                                collect_store_let_alias(
                                    &binding.id,
                                    call,
                                    phase,
                                    environment,
                                    program,
                                ) || collect_read_context_let_alias(
                                    &binding.id,
                                    call,
                                    phase,
                                    environment,
                                    program,
                                ) || collect_next_context_alias(
                                    &binding.id,
                                    call,
                                    phase,
                                    environment,
                                    program,
                                ) || collect_reference_alias(
                                    &binding.id,
                                    call,
                                    phase,
                                    environment,
                                    program,
                                ) || collect_reference_candidate_alias(
                                    &binding.id,
                                    call,
                                    phase,
                                    environment,
                                    program,
                                )
                            }
                            _ => false,
                        }
                        || pending_alias_binding.as_ref().is_some_and(|key| {
                            !program.resolved_alias_declarations.contains(key)
                                && collect_component_context_member_alias(
                                    &binding.id,
                                    right,
                                    phase,
                                    program,
                                )
                        })
                }
                _ => false,
            };
            if supported_context_alias {
                if let Some(binding) = pending_alias_binding {
                    program.resolved_alias_declarations.insert(binding);
                }
            } else {
                record_program_issue(
                    program,
                    issue(
                        AngularRecoveryIssueKind::UnsupportedExpression,
                        None,
                        Some("assignment".to_string()),
                    ),
                    phase,
                    assignment.span,
                    None,
                    environment,
                );
                collect_expression(
                    assignment.right.as_ref(),
                    phase,
                    render_flags,
                    environment,
                    program,
                );
            }
        }
        Expr::Call(call) => {
            let Some((root, argument_lists)) = call_chain(call) else {
                record_program_issue(
                    program,
                    issue(
                        AngularRecoveryIssueKind::UnsupportedExpression,
                        None,
                        Some("non-expression call target".to_string()),
                    ),
                    phase,
                    call.span,
                    None,
                    environment,
                );
                return;
            };
            let Some(instruction) = environment
                .roles
                .instruction_for_expr(root, environment.unresolved_ctxt)
            else {
                if let Some(name) = environment
                    .roles
                    .ivy_name_for_expr(root, environment.unresolved_ctxt)
                {
                    program.stats.runtime_calls_observed += argument_lists.len();
                    program.stats.unsupported_runtime_calls += argument_lists.len();
                    for provenance in call_provenances(
                        program,
                        phase,
                        call,
                        root,
                        argument_lists.len(),
                        environment,
                    ) {
                        record_issue(
                            &mut program.issues,
                            issue_at_operation(
                                issue(
                                    AngularRecoveryIssueKind::UnsupportedInstruction,
                                    Some(name.clone()),
                                    phase.map(|phase| format!("render phase {phase}")),
                                ),
                                &provenance,
                            ),
                        );
                    }
                } else if matches!(phase, Some(1 | 2))
                    || environment
                        .roles
                        .is_known_runtime_member(root, environment.unresolved_ctxt)
                {
                    program.stats.runtime_calls_observed += argument_lists.len();
                    program.stats.unsupported_runtime_calls += argument_lists.len();
                    let template_phase = template_phase(phase);
                    let argument_counts = argument_lists
                        .iter()
                        .map(|arguments| arguments.len())
                        .collect::<Vec<_>>();
                    let shape = program
                        .unknown_runtime_call_shapes
                        .entry((template_phase, argument_counts))
                        .or_default();
                    shape.0 += 1;
                    shape.1 += argument_lists.len();
                    for provenance in call_provenances(
                        program,
                        phase,
                        call,
                        root,
                        argument_lists.len(),
                        environment,
                    ) {
                        record_issue(
                            &mut program.issues,
                            issue_at_operation(
                                issue(
                                    AngularRecoveryIssueKind::UnknownRuntimeInstruction,
                                    None,
                                    phase.map(|phase| {
                                        format!(
                                            "render phase {phase}, {} argument list(s)",
                                            argument_lists.len()
                                        )
                                    }),
                                ),
                                &provenance,
                            ),
                        );
                    }
                } else {
                    record_program_issue(
                        program,
                        issue(
                            AngularRecoveryIssueKind::UnsupportedExpression,
                            None,
                            Some("call outside a render phase".to_string()),
                        ),
                        phase,
                        call.span,
                        Some(root),
                        environment,
                    );
                }
                return;
            };
            let Some(phase) = phase else {
                program.stats.runtime_calls_observed += argument_lists.len();
                program.stats.unsupported_runtime_calls += argument_lists.len();
                for provenance in
                    call_provenances(program, None, call, root, argument_lists.len(), environment)
                {
                    record_issue(
                        &mut program.issues,
                        issue_at_operation(
                            issue(
                                AngularRecoveryIssueKind::UnsupportedInstruction,
                                Some(instruction.canonical_export_name().to_string()),
                                Some("outside a creation or update phase".to_string()),
                            ),
                            &provenance,
                        ),
                    );
                }
                return;
            };
            if instruction == IvyInstruction::NextContext && phase == 2 {
                program.stats.runtime_calls_observed += argument_lists.len();
                let provenances = call_provenances(
                    program,
                    Some(phase),
                    call,
                    root,
                    argument_lists.len(),
                    environment,
                );
                if let Some(context_hop) = context_hop(&argument_lists) {
                    program.stats.rendered_instruction_calls += argument_lists.len();
                    program.update_context_depth =
                        program.update_context_depth.saturating_add(context_hop);
                } else {
                    program.stats.malformed_instruction_calls += argument_lists.len();
                    for provenance in provenances {
                        record_issue(
                            &mut program.issues,
                            issue_at_operation(
                                issue(
                                    AngularRecoveryIssueKind::MalformedInstruction,
                                    Some("ɵɵnextContext".to_string()),
                                    Some("unexpected context-depth arguments".to_string()),
                                ),
                                &provenance,
                            ),
                        );
                    }
                }
                return;
            }
            if !instruction_supported_in_phase(instruction, phase) {
                program.stats.runtime_calls_observed += argument_lists.len();
                program.stats.unsupported_runtime_calls += argument_lists.len();
                for provenance in call_provenances(
                    program,
                    Some(phase),
                    call,
                    root,
                    argument_lists.len(),
                    environment,
                ) {
                    record_issue(
                        &mut program.issues,
                        issue_at_operation(
                            issue(
                                AngularRecoveryIssueKind::UnsupportedInstruction,
                                Some(instruction.canonical_export_name().to_string()),
                                Some(format!("unsupported in render phase {phase}")),
                            ),
                            &provenance,
                        ),
                    );
                }
                return;
            }
            program.stats.runtime_calls_observed += argument_lists.len();
            let provenances = call_provenances(
                program,
                Some(phase),
                call,
                root,
                argument_lists.len(),
                environment,
            );
            let target = if phase == 1 {
                &mut program.create
            } else {
                &mut program.update
            };
            target.extend(
                argument_lists
                    .into_iter()
                    .zip(provenances)
                    .map(|(args, provenance)| InstructionCall {
                        instruction,
                        args: args.iter().map(|arg| arg.expr.clone()).collect(),
                        result_binding: None,
                        provenance,
                    }),
            );
        }
        _ => record_program_issue(
            program,
            issue(
                AngularRecoveryIssueKind::UnsupportedExpression,
                None,
                Some(expression_kind(expression).to_string()),
            ),
            phase,
            expression.span(),
            None,
            environment,
        ),
    }
}

fn instruction_supported_in_phase(instruction: IvyInstruction, phase: u8) -> bool {
    match phase {
        1 => matches!(
            instruction,
            IvyInstruction::ElementStart
                | IvyInstruction::ElementEnd
                | IvyInstruction::Element
                | IvyInstruction::ElementContainerStart
                | IvyInstruction::ElementContainerEnd
                | IvyInstruction::ElementContainer
                | IvyInstruction::NamespaceHtml
                | IvyInstruction::NamespaceSvg
                | IvyInstruction::NamespaceMathMl
                | IvyInstruction::Text
                | IvyInstruction::Listener
                | IvyInstruction::AnimateEnter
                | IvyInstruction::AnimateEnterListener
                | IvyInstruction::AnimateLeave
                | IvyInstruction::AnimateLeaveListener
                | IvyInstruction::TwoWayListener
                | IvyInstruction::Template
                | IvyInstruction::Defer
                | IvyInstruction::DeferOnIdle
                | IvyInstruction::RepeaterCreate
                | IvyInstruction::ProjectionDef
                | IvyInstruction::Projection
                | IvyInstruction::Pipe
                | IvyInstruction::DeclareLet
                | IvyInstruction::I18n
                | IvyInstruction::I18nStart
                | IvyInstruction::I18nEnd
        ),
        2 => matches!(
            instruction,
            IvyInstruction::Advance
                | IvyInstruction::TextInterpolate
                | IvyInstruction::TextInterpolate1
                | IvyInstruction::TextInterpolate2
                | IvyInstruction::TextInterpolate3
                | IvyInstruction::TextInterpolate4
                | IvyInstruction::TextInterpolate5
                | IvyInstruction::TextInterpolate6
                | IvyInstruction::TextInterpolate7
                | IvyInstruction::TextInterpolate8
                | IvyInstruction::Property
                | IvyInstruction::AriaProperty
                | IvyInstruction::Attribute
                | IvyInstruction::ClassMap
                | IvyInstruction::ClassProp
                | IvyInstruction::StyleMap
                | IvyInstruction::StyleProp
                | IvyInstruction::TwoWayProperty
                | IvyInstruction::Conditional
                | IvyInstruction::Repeater
                | IvyInstruction::StoreLet
                | IvyInstruction::I18nExp
                | IvyInstruction::I18nApply
        ),
        _ => false,
    }
}

fn statement_kind(statement: &Stmt) -> &'static str {
    match statement {
        Stmt::Block(_) => "block",
        Stmt::Empty(_) => "empty",
        Stmt::Debugger(_) => "debugger",
        Stmt::With(_) => "with",
        Stmt::Return(_) => "return",
        Stmt::Labeled(_) => "labeled",
        Stmt::Break(_) => "break",
        Stmt::Continue(_) => "continue",
        Stmt::If(_) => "if",
        Stmt::Switch(_) => "switch",
        Stmt::Throw(_) => "throw",
        Stmt::Try(_) => "try",
        Stmt::While(_) => "while",
        Stmt::DoWhile(_) => "do-while",
        Stmt::For(_) => "for",
        Stmt::ForIn(_) => "for-in",
        Stmt::ForOf(_) => "for-of",
        Stmt::Decl(_) => "declaration",
        Stmt::Expr(_) => "expression",
    }
}

fn expression_kind(expression: &Expr) -> &'static str {
    match expression {
        Expr::This(_) => "this",
        Expr::Array(_) => "array",
        Expr::Object(_) => "object",
        Expr::Fn(_) => "function",
        Expr::Unary(_) => "unary",
        Expr::Update(_) => "update",
        Expr::Bin(_) => "binary",
        Expr::Assign(_) => "assignment",
        Expr::Member(_) => "member",
        Expr::SuperProp(_) => "super-property",
        Expr::Cond(_) => "conditional",
        Expr::Call(_) => "call",
        Expr::New(_) => "new",
        Expr::Seq(_) => "sequence",
        Expr::Ident(_) => "identifier",
        Expr::Lit(_) => "literal",
        Expr::Tpl(_) => "template-literal",
        Expr::TaggedTpl(_) => "tagged-template",
        Expr::Arrow(_) => "arrow",
        Expr::Class(_) => "class",
        Expr::Yield(_) => "yield",
        Expr::MetaProp(_) => "meta-property",
        Expr::Await(_) => "await",
        Expr::Paren(_) => "parenthesized",
        Expr::JSXMember(_)
        | Expr::JSXNamespacedName(_)
        | Expr::JSXEmpty(_)
        | Expr::JSXElement(_)
        | Expr::JSXFragment(_) => "jsx",
        Expr::TsTypeAssertion(_)
        | Expr::TsConstAssertion(_)
        | Expr::TsNonNull(_)
        | Expr::TsAs(_)
        | Expr::TsInstantiation(_)
        | Expr::TsSatisfies(_) => "typescript",
        Expr::PrivateName(_) => "private-name",
        Expr::OptChain(_) => "optional-chain",
        Expr::Invalid(_) => "invalid",
    }
}

fn strip_parentheses(mut expression: &Expr) -> &Expr {
    while let Expr::Paren(parenthesized) = expression {
        expression = parenthesized.expr.as_ref();
    }
    expression
}

fn render_flag_mask(expression: &Expr, render_flags: &BindingKey) -> Option<u8> {
    let Expr::Bin(binary) = expression else {
        return None;
    };
    if binary.op != BinaryOp::BitAnd {
        return None;
    }
    let (Expr::Ident(ident), Expr::Lit(Lit::Num(mask))) =
        (binary.left.as_ref(), binary.right.as_ref())
    else {
        return None;
    };
    (binding_key(ident) == *render_flags && (mask.value == 1.0 || mask.value == 2.0))
        .then_some(mask.value as u8)
}

pub(super) fn call_chain(call: &CallExpr) -> Option<(&Expr, Vec<&[ExprOrSpread]>)> {
    let mut argument_lists = vec![call.args.as_slice()];
    let mut callee = &call.callee;
    loop {
        let Callee::Expr(expression) = callee else {
            return None;
        };
        match expression.as_ref() {
            Expr::Call(inner) => {
                argument_lists.push(inner.args.as_slice());
                callee = &inner.callee;
            }
            root => {
                argument_lists.reverse();
                return Some((root, argument_lists));
            }
        }
    }
}

struct RecoveredViewHandler {
    source: String,
    runtime_calls: usize,
    artifact_references: HashSet<BindingKey>,
    let_alias_hints: Vec<ViewLetAliasHint>,
    listener_method: Option<RecoveredListenerMethod>,
}

fn recover_view_listener_handler(
    handler: &Expr,
    event: &str,
    tree: &TemplateTree,
    ancestor_references: &[ReferenceScope],
    program: &TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> std::result::Result<Option<RecoveredViewHandler>, String> {
    match recover_inline_view_listener_handler(
        handler,
        tree,
        ancestor_references,
        program,
        environment,
    ) {
        Ok(recovered) => Ok(recovered),
        Err(_) => recover_structured_view_listener_handler(
            handler,
            event,
            tree,
            ancestor_references,
            program,
            environment,
        ),
    }
}

struct StructuredListenerState {
    // Lexical identity comes from SWC SyntaxContexts. Context depth is a
    // separate Angular view cursor and must not be used as a JS scope model.
    component_contexts: HashSet<BindingKey>,
    local_names: HashMap<BindingKey, String>,
    local_context_bindings: HashMap<BindingKey, HashMap<String, String>>,
    expression_aliases: HashMap<BindingKey, Box<Expr>>,
    let_alias_hints: Vec<ViewLetAliasHint>,
    binding_uses: BindingUseIndex,
    next_parameter_marker: usize,
    runtime_calls: usize,
    context_depth: usize,
}

fn recover_structured_view_listener_handler(
    handler: &Expr,
    event: &str,
    tree: &TemplateTree,
    ancestor_references: &[ReferenceScope],
    program: &TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> std::result::Result<Option<RecoveredViewHandler>, String> {
    let Some(original_block) = handler_block(handler) else {
        return Ok(None);
    };
    if !original_block.stmts.iter().any(|statement| {
        statement_restore_view_call(statement, environment)
            .is_some_and(|call| is_instruction_call(call, IvyInstruction::RestoreView, environment))
    }) {
        return Ok(None);
    }

    let handler = normalize_listener_optional_chaining(handler, environment.unresolved_ctxt);
    let Some(block) = handler_block(handler.as_ref()) else {
        return Ok(None);
    };
    let mut state = StructuredListenerState {
        component_contexts: program.component_contexts.clone(),
        local_names: program.local_reference_names.clone(),
        local_context_bindings: program.local_context_bindings.clone(),
        expression_aliases: HashMap::new(),
        let_alias_hints: Vec::new(),
        binding_uses: BindingUseIndex::collect_stmts(&block.stmts),
        next_parameter_marker: 0,
        runtime_calls: 0,
        context_depth: 0,
    };
    if let Some(event_binding) = handler_event_binding(handler.as_ref()) {
        state
            .local_names
            .insert(event_binding, "$event".to_string());
    }

    let mut statements = Vec::new();
    let mut saw_reset_return = false;
    for (statement_index, statement) in block.stmts.iter().enumerate() {
        match statement {
            Stmt::Decl(Decl::Var(declaration)) => {
                let mut declarators = Vec::new();
                for declarator in &declaration.decls {
                    if let Some(declarator) = lower_structured_listener_declarator(
                        declarator,
                        tree,
                        ancestor_references,
                        program,
                        environment,
                        &mut state,
                    )? {
                        declarators.push(declarator);
                    }
                }
                let mut declaration = declaration.as_ref().clone();
                declaration.decls = declarators;
                if !declaration.decls.is_empty() {
                    statements.push(Stmt::Decl(Decl::Var(Box::new(declaration))));
                }
            }
            Stmt::Expr(expression) => {
                if lower_structured_listener_expression(
                    expression,
                    program,
                    environment,
                    &mut state,
                )? {
                    statements.push(statement.clone());
                }
            }
            Stmt::Return(ReturnStmt {
                arg: Some(returned),
                ..
            }) => {
                if block.stmts[statement_index + 1..]
                    .iter()
                    .any(|statement| !matches!(statement, Stmt::Empty(_)))
                {
                    return Err("ɵɵresetView return is not the final handler statement".to_string());
                }
                if saw_reset_return {
                    return Err("multiple handler returns".to_string());
                }
                lower_structured_listener_return(
                    returned.as_ref(),
                    environment,
                    &mut state,
                    &mut statements,
                )?;
                saw_reset_return = true;
            }
            Stmt::If(_) | Stmt::Block(_) => {
                validate_structured_listener_statement(statement, environment)?;
                statements.push(statement.clone());
            }
            Stmt::Empty(_) => {}
            _ => {
                return Err(format!(
                    "unsupported structured listener statement: {}",
                    statement_kind(statement)
                ));
            }
        }
    }
    if !saw_reset_return {
        return Err("restored handler has no ɵɵresetView return".to_string());
    }

    let mut body = block.clone();
    body.stmts = statements;
    if !state.expression_aliases.is_empty() {
        body.visit_mut_with(&mut TemplateExpressionAliasResolver {
            aliases: &state.expression_aliases,
            active: HashSet::new(),
        });
    }
    let mut occupied_names = ListenerBindingNameCollector::default();
    body.visit_with(&mut occupied_names);
    let mut binding_rewriter = ListenerMethodBindingRewriter {
        component_contexts: &state.component_contexts,
        local_names: &state.local_names,
        local_contexts: &state.local_context_bindings,
        occupied_names: occupied_names.names,
        parameters: Vec::new(),
        parameters_by_template_name: HashMap::new(),
    };
    body.visit_mut_with(&mut binding_rewriter);
    binding_rewriter
        .parameters
        .sort_by_key(|parameter| parameter.template_name != "$event");
    let template_arguments = binding_rewriter
        .parameters
        .iter()
        .map(|parameter| parameter.template_name.clone())
        .collect::<Vec<_>>();
    let params = binding_rewriter
        .parameters
        .into_iter()
        .map(|parameter| Param {
            span: DUMMY_SP,
            decorators: Vec::new(),
            pat: Pat::Ident(BindingIdent {
                id: parameter.identifier,
                type_ann: None,
            }),
        })
        .collect();
    let function = Function {
        params,
        decorators: Vec::new(),
        span: block.span,
        ctxt: block.ctxt,
        body: Some(body),
        is_generator: false,
        is_async: false,
        type_params: None,
        return_type: None,
    };
    let placeholder = format!(
        "__wakaru_listener_{}_{}__",
        program.view_id,
        program.listener_methods.len()
    );
    let source = format!("{placeholder}({})", template_arguments.join(", "));
    let artifact_references = function_references(&function);
    Ok(Some(RecoveredViewHandler {
        source,
        runtime_calls: state.runtime_calls,
        artifact_references,
        let_alias_hints: state.let_alias_hints,
        listener_method: Some(RecoveredListenerMethod {
            placeholder,
            preferred_name: recovered_listener_method_name(event),
            function,
        }),
    }))
}

fn preserve_reassigned_listener_alias(
    declarator: &VarDeclarator,
    initializer: Box<Expr>,
) -> Option<VarDeclarator> {
    let mut declarator = declarator.clone();
    declarator.init = Some(initializer);
    Some(declarator)
}

fn listener_parameter_marker(
    name: String,
    span: Span,
    state: &mut StructuredListenerState,
) -> Box<Expr> {
    let marker = Ident::new(
        Atom::from(format!(
            "__wakaru_listener_parameter_{}",
            state.next_parameter_marker
        )),
        span,
        SyntaxContext::empty(),
    );
    state.next_parameter_marker += 1;
    state.local_names.insert(binding_key(&marker), name);
    Box::new(Expr::Ident(marker))
}

fn lower_structured_listener_declarator(
    declarator: &VarDeclarator,
    tree: &TemplateTree,
    ancestor_references: &[ReferenceScope],
    program: &TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
    state: &mut StructuredListenerState,
) -> std::result::Result<Option<VarDeclarator>, String> {
    let Pat::Ident(binding) = &declarator.name else {
        if declarator
            .init
            .as_deref()
            .is_some_and(|initializer| contains_runtime_call(initializer, environment))
        {
            return Err("unsupported Ivy runtime call in listener-local declaration".to_string());
        }
        return Ok(Some(declarator.clone()));
    };
    let Some(initializer) = declarator.init.as_deref() else {
        return Ok(Some(declarator.clone()));
    };

    if let Some(call) = restored_view_call(initializer, environment) {
        let Some(property) = restored_view_property(initializer) else {
            return Err("restored view has no context property".to_string());
        };
        let argument_lists = validated_single_call(call, "ɵɵrestoreView")?;
        let [saved_view] = argument_lists[0] else {
            return Err("ɵɵrestoreView expected one saved view".to_string());
        };
        let Expr::Ident(saved_view) = strip_parentheses(saved_view.expr.as_ref()) else {
            return Err("ɵɵrestoreView saved view is not an identifier".to_string());
        };
        if !program.saved_views.contains(&binding_key(saved_view)) {
            return Err("ɵɵrestoreView does not reference a captured view".to_string());
        }
        let name = if property == "$implicit"
            || program.implicit_view_context_properties.contains(&property)
        {
            program
                .repeater_item_name
                .clone()
                .unwrap_or_else(|| recovered_view_alias_name(binding.id.sym.as_ref(), "item"))
        } else if !is_likely_generated_alias(&property) {
            to_valid_identifier_name(&property)
        } else {
            return Err(format!(
                "unsupported restored view context property {property}"
            ));
        };
        let binding_key = binding_key(&binding.id);
        state.runtime_calls += 1;
        state.context_depth = 0;
        if state.binding_uses.has_direct_write(&binding_key) {
            let initializer = listener_parameter_marker(name, binding.id.span, state);
            return Ok(preserve_reassigned_listener_alias(declarator, initializer));
        }
        state.local_names.insert(binding_key, name);
        return Ok(None);
    }

    if let Some((alias, context_hop)) = next_context_member_alias(initializer, environment)? {
        let destination_depth = state.context_depth.saturating_add(context_hop);
        let Expr::Ident(property) = alias.as_ref() else {
            return Err("ɵɵnextContext member alias is not an identifier".to_string());
        };
        let binding_key = binding_key(&binding.id);
        let reassigned = state.binding_uses.has_direct_write(&binding_key);
        let mut reassigned_initializer = None;
        match program.context_scope_at_depth(destination_depth) {
            Some(scope) if !scope.is_component => {
                let Some(name) = program.context_property_name_for_binding(
                    destination_depth,
                    property.sym.as_ref(),
                    &binding_key,
                ) else {
                    return Err(format!(
                        "unsupported restored view context property {}",
                        property.sym
                    ));
                };
                if reassigned {
                    reassigned_initializer =
                        Some(listener_parameter_marker(name, property.span, state));
                } else {
                    state.local_names.insert(binding_key, name);
                }
            }
            _ => {
                let name = program
                    .context_property_name_for_binding(
                        destination_depth,
                        property.sym.as_ref(),
                        &binding_key,
                    )
                    .unwrap_or_else(|| to_valid_identifier_name(property.sym.as_ref()));
                let initializer = Box::new(Expr::Member(MemberExpr {
                    span: property.span,
                    obj: Box::new(Expr::This(ThisExpr {
                        span: property.span,
                    })),
                    prop: MemberProp::Ident(swc_core::ecma::ast::IdentName::new(
                        name.into(),
                        property.span,
                    )),
                }));
                if reassigned {
                    reassigned_initializer = Some(initializer);
                } else {
                    state.expression_aliases.insert(binding_key, initializer);
                }
            }
        }
        state.runtime_calls += 1;
        state.context_depth = destination_depth;
        if let Some(initializer) = reassigned_initializer {
            return Ok(preserve_reassigned_listener_alias(declarator, initializer));
        }
        return Ok(None);
    }

    if let Some(slot) = inlined_reference_slot(initializer) {
        let Some(name) =
            reference_name_at_depth(tree, ancestor_references, state.context_depth, slot)
        else {
            return Err(format!(
                "no inlined local reference at slot {slot} in context depth {}",
                state.context_depth
            ));
        };
        let binding_key = binding_key(&binding.id);
        if state.binding_uses.has_direct_write(&binding_key) {
            let initializer = listener_parameter_marker(name, binding.id.span, state);
            return Ok(preserve_reassigned_listener_alias(declarator, initializer));
        }
        state.local_names.insert(binding_key, name);
        return Ok(None);
    }

    if let Expr::Call(call) = strip_parentheses(initializer) {
        if is_instruction_call(call, IvyInstruction::ReadContextLet, environment) {
            let argument_lists = validated_single_call(call, "ɵɵreadContextLet")?;
            let [slot] = argument_lists[0] else {
                return Err("ɵɵreadContextLet expected one let slot".to_string());
            };
            let Some(slot) = numeric_expr(slot.expr.as_ref()) else {
                return Err("ɵɵreadContextLet slot is not numeric".to_string());
            };
            let name = recovered_let_name(Some(binding.id.sym.as_ref()), slot);
            state.let_alias_hints.push(ViewLetAliasHint {
                context_depth: state.context_depth,
                slot,
                name: name.clone(),
            });
            state.runtime_calls += 1;
            let binding_key = binding_key(&binding.id);
            if state.binding_uses.has_direct_write(&binding_key) {
                let initializer = listener_parameter_marker(name, binding.id.span, state);
                return Ok(preserve_reassigned_listener_alias(declarator, initializer));
            }
            state.local_names.insert(binding_key, name);
            return Ok(None);
        }
        if is_instruction_call(call, IvyInstruction::RestoreView, environment) {
            validate_restore_view_call(call, program)?;
            state.runtime_calls += 1;
            state.context_depth = 0;
            let binding_key = binding_key(&binding.id);
            if state.binding_uses.has_direct_write(&binding_key) {
                return Ok(preserve_reassigned_listener_alias(
                    declarator,
                    Box::new(Expr::This(ThisExpr {
                        span: binding.id.span,
                    })),
                ));
            }
            state.component_contexts.insert(binding_key);
            return Ok(None);
        }
        if is_instruction_call(call, IvyInstruction::Reference, environment)
            || is_reference_candidate_call(call, environment)
        {
            let argument_lists = validated_single_call(call, "ɵɵreference")?;
            let [slot] = argument_lists[0] else {
                return Err("ɵɵreference expected one slot".to_string());
            };
            let Some(slot) = numeric_expr(slot.expr.as_ref()) else {
                return Err("ɵɵreference slot is not numeric".to_string());
            };
            let Some(name) =
                reference_name_at_depth(tree, ancestor_references, state.context_depth, slot)
            else {
                return Err(format!(
                    "no local reference at slot {slot} in context depth {}",
                    state.context_depth
                ));
            };
            state.runtime_calls += 1;
            let binding_key = binding_key(&binding.id);
            if state.binding_uses.has_direct_write(&binding_key) {
                let initializer = listener_parameter_marker(name, binding.id.span, state);
                return Ok(preserve_reassigned_listener_alias(declarator, initializer));
            }
            state.local_names.insert(binding_key, name);
            return Ok(None);
        }
        if is_instruction_call(call, IvyInstruction::NextContext, environment) {
            let argument_lists = validated_single_call(call, "ɵɵnextContext")?;
            let Some(context_hop) = context_hop(&argument_lists) else {
                return Err("ɵɵnextContext has unexpected context-depth arguments".to_string());
            };
            let destination_depth = state.context_depth.saturating_add(context_hop);
            let binding_key = binding_key(&binding.id);
            let reassigned = state.binding_uses.has_direct_write(&binding_key);
            let mut reassigned_initializer = None;
            match program.context_scope_at_depth(destination_depth) {
                Some(scope) if scope.is_component => {
                    if reassigned {
                        reassigned_initializer = Some(Box::new(Expr::This(ThisExpr {
                            span: binding.id.span,
                        })));
                    } else {
                        state.component_contexts.insert(binding_key);
                    }
                }
                Some(scope) if !scope.local_properties.is_empty() => {
                    if reassigned {
                        return Err("reassigned non-component view context alias is unsupported"
                            .to_string());
                    }
                    state
                        .local_context_bindings
                        .insert(binding_key, scope.local_properties);
                }
                Some(_) if reassigned => {
                    return Err(
                        "reassigned non-component view context alias is unsupported".to_string()
                    );
                }
                Some(_) => {}
                None => {
                    if reassigned {
                        reassigned_initializer = Some(Box::new(Expr::This(ThisExpr {
                            span: binding.id.span,
                        })));
                    } else {
                        state.component_contexts.insert(binding_key);
                    }
                }
            }
            state.runtime_calls += 1;
            state.context_depth = destination_depth;
            if let Some(initializer) = reassigned_initializer {
                return Ok(preserve_reassigned_listener_alias(declarator, initializer));
            }
            return Ok(None);
        }
    }

    if contains_runtime_call(initializer, environment) {
        return Err("unsupported Ivy runtime call in listener-local declaration".to_string());
    }
    Ok(Some(declarator.clone()))
}

fn lower_structured_listener_expression(
    expression: &ExprStmt,
    program: &TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
    state: &mut StructuredListenerState,
) -> std::result::Result<bool, String> {
    if let Expr::Call(call) = strip_parentheses(expression.expr.as_ref()) {
        if is_instruction_call(call, IvyInstruction::RestoreView, environment) {
            validate_restore_view_call(call, program)?;
            state.runtime_calls += 1;
            state.context_depth = 0;
            return Ok(false);
        }
        if is_instruction_call(call, IvyInstruction::NextContext, environment) {
            let argument_lists = validated_single_call(call, "ɵɵnextContext")?;
            let Some(context_hop) = context_hop(&argument_lists) else {
                return Err("ɵɵnextContext has unexpected context-depth arguments".to_string());
            };
            state.runtime_calls += 1;
            state.context_depth = state.context_depth.saturating_add(context_hop);
            return Ok(false);
        }
        if is_instruction_call(call, IvyInstruction::ReadContextLet, environment) {
            let argument_lists = validated_single_call(call, "ɵɵreadContextLet")?;
            let [slot] = argument_lists[0] else {
                return Err("ɵɵreadContextLet expected one let slot".to_string());
            };
            if numeric_expr(slot.expr.as_ref()).is_none() {
                return Err("ɵɵreadContextLet slot is not numeric".to_string());
            }
            state.runtime_calls += 1;
            return Ok(false);
        }
    }
    if contains_runtime_call(expression.expr.as_ref(), environment) {
        return Err("unsupported Ivy runtime call in restored handler expression".to_string());
    }
    Ok(true)
}

fn lower_structured_listener_return(
    returned: &Expr,
    environment: &TemplateRecoveryEnvironment<'_>,
    state: &mut StructuredListenerState,
    statements: &mut Vec<Stmt>,
) -> std::result::Result<(), String> {
    let returned = strip_parentheses(returned);
    let (effects, reset_expression) = match returned {
        Expr::Seq(sequence) => {
            let Some((reset_expression, effects)) = sequence.exprs.split_last() else {
                return Err("restored handler return is empty".to_string());
            };
            (effects, reset_expression.as_ref())
        }
        returned => (&[][..], returned),
    };
    let Expr::Call(call) = strip_parentheses(reset_expression) else {
        return Err("restored handler return is not ɵɵresetView".to_string());
    };
    if !is_instruction_call(call, IvyInstruction::ResetView, environment) {
        return Err("restored handler return is not ɵɵresetView".to_string());
    }
    let argument_lists = validated_single_call(call, "ɵɵresetView")?;
    let action = match argument_lists[0] {
        [] => None,
        [returned] => Some(returned.expr.as_ref()),
        _ => return Err("ɵɵresetView expected at most one return value".to_string()),
    };
    for effect in effects {
        if contains_runtime_call(effect.as_ref(), environment) {
            return Err("unsupported Ivy runtime call in restored handler expression".to_string());
        }
        statements.push(Stmt::Expr(ExprStmt {
            span: effect.span(),
            expr: effect.clone(),
        }));
    }
    if let Some(action) = action {
        if contains_runtime_call(action, environment) {
            return Err("unsupported Ivy runtime call in restored handler expression".to_string());
        }
        statements.push(Stmt::Return(ReturnStmt {
            span: action.span(),
            arg: Some(Box::new(action.clone())),
        }));
    }
    state.runtime_calls += 1;
    Ok(())
}

fn validate_structured_listener_statement(
    statement: &Stmt,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> std::result::Result<(), String> {
    match statement {
        Stmt::Block(block) => {
            for statement in &block.stmts {
                validate_structured_listener_statement(statement, environment)?;
            }
            Ok(())
        }
        Stmt::If(statement) => {
            if contains_runtime_call(statement.test.as_ref(), environment) {
                return Err(
                    "unsupported Ivy runtime call in structured listener condition".to_string(),
                );
            }
            validate_structured_listener_statement(statement.cons.as_ref(), environment)?;
            if let Some(alternate) = &statement.alt {
                validate_structured_listener_statement(alternate.as_ref(), environment)?;
            }
            Ok(())
        }
        Stmt::Decl(Decl::Var(declaration)) => {
            if declaration.decls.iter().any(|declarator| {
                declarator
                    .init
                    .as_deref()
                    .is_some_and(|initializer| contains_runtime_call(initializer, environment))
            }) {
                return Err(
                    "unsupported Ivy runtime call in listener-local declaration".to_string()
                );
            }
            Ok(())
        }
        Stmt::Expr(expression) => {
            if contains_runtime_call(expression.expr.as_ref(), environment) {
                return Err(
                    "unsupported Ivy runtime call in restored handler expression".to_string(),
                );
            }
            Ok(())
        }
        Stmt::Empty(_) => Ok(()),
        _ => Err(format!(
            "unsupported structured listener statement: {}",
            statement_kind(statement)
        )),
    }
}

#[derive(Default)]
struct ListenerBindingNameCollector {
    names: HashSet<Atom>,
}

impl Visit for ListenerBindingNameCollector {
    fn visit_binding_ident(&mut self, binding: &BindingIdent) {
        self.names.insert(binding.id.sym.clone());
    }
}

struct ListenerMethodParameter {
    template_name: String,
    identifier: Ident,
}

struct ListenerMethodBindingRewriter<'a> {
    component_contexts: &'a HashSet<BindingKey>,
    local_names: &'a HashMap<BindingKey, String>,
    local_contexts: &'a HashMap<BindingKey, HashMap<String, String>>,
    occupied_names: HashSet<Atom>,
    parameters: Vec<ListenerMethodParameter>,
    parameters_by_template_name: HashMap<String, Ident>,
}

impl VisitMut for ListenerMethodBindingRewriter<'_> {
    fn visit_mut_assign_expr(&mut self, assignment: &mut AssignExpr) {
        assignment.right.visit_mut_with(self);
        if let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &mut assignment.left {
            let key = binding_key(&binding.id);
            if let Some(name) = self.local_names.get(&key).cloned() {
                binding.id = self.parameter_for(&name, binding.id.span);
            }
            return;
        }
        let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assignment.left else {
            assignment.left.visit_mut_children_with(self);
            return;
        };
        let Expr::Ident(object) = member.obj.as_ref() else {
            assignment.left.visit_mut_children_with(self);
            return;
        };
        let MemberProp::Ident(property) = &member.prop else {
            assignment.left.visit_mut_children_with(self);
            return;
        };
        let Some(name) = self.local_context_name(object, property) else {
            assignment.left.visit_mut_children_with(self);
            return;
        };
        let identifier = self.parameter_for(&name, property.span);
        assignment.left = AssignTarget::Simple(SimpleAssignTarget::Ident(BindingIdent {
            id: identifier,
            type_ann: None,
        }));
    }

    fn visit_mut_expr(&mut self, expression: &mut Expr) {
        if let Expr::Member(member) = expression {
            if let (Expr::Ident(object), MemberProp::Ident(property)) =
                (member.obj.as_ref(), &member.prop)
            {
                if let Some(name) = self.local_context_name(object, property) {
                    *expression = Expr::Ident(self.parameter_for(&name, property.span));
                    return;
                }
            }
        }
        if let Expr::Ident(identifier) = expression {
            let key = binding_key(identifier);
            if self.component_contexts.contains(&key) {
                *expression = Expr::This(ThisExpr {
                    span: identifier.span,
                });
                return;
            }
            if let Some(name) = self.local_names.get(&key).cloned() {
                *identifier = self.parameter_for(&name, identifier.span);
                return;
            }
        }
        expression.visit_mut_children_with(self);
    }
}

impl ListenerMethodBindingRewriter<'_> {
    fn local_context_name(
        &self,
        object: &Ident,
        property: &swc_core::ecma::ast::IdentName,
    ) -> Option<String> {
        self.local_contexts
            .get(&binding_key(object))
            .and_then(|properties| properties.get(property.sym.as_ref()))
            .cloned()
    }

    fn parameter_for(&mut self, template_name: &str, span: Span) -> Ident {
        if let Some(identifier) = self.parameters_by_template_name.get(template_name) {
            return identifier.clone();
        }
        let base = to_valid_identifier_name(template_name);
        let mut name = base.clone();
        let mut suffix = 2usize;
        while self.occupied_names.contains(&Atom::from(name.as_str())) {
            name = format!("{base}{suffix}");
            suffix += 1;
        }
        self.occupied_names.insert(Atom::from(name.as_str()));
        let identifier = Ident::new(Atom::from(name.as_str()), span, SyntaxContext::empty());
        self.parameters_by_template_name
            .insert(template_name.to_string(), identifier.clone());
        self.parameters.push(ListenerMethodParameter {
            template_name: template_name.to_string(),
            identifier: identifier.clone(),
        });
        identifier
    }
}

fn recovered_listener_method_name(event: &str) -> String {
    let mut suffix = String::new();
    let mut capitalize = true;
    for character in event.chars() {
        if !character.is_alphanumeric() {
            capitalize = true;
            continue;
        }
        if capitalize {
            suffix.extend(character.to_uppercase());
            capitalize = false;
        } else {
            suffix.push(character);
        }
    }
    if suffix.is_empty() {
        suffix.push_str("Event");
    }
    to_valid_identifier_name(&format!("recovered{suffix}"))
}

fn recover_inline_view_listener_handler(
    handler: &Expr,
    tree: &TemplateTree,
    ancestor_references: &[ReferenceScope],
    program: &TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> std::result::Result<Option<RecoveredViewHandler>, String> {
    let Some(original_block) = handler_block(handler) else {
        return Ok(None);
    };
    if !original_block.stmts.iter().any(|statement| {
        statement_restore_view_call(statement, environment)
            .is_some_and(|call| is_instruction_call(call, IvyInstruction::RestoreView, environment))
    }) {
        return Ok(None);
    }

    // Work on a clone: the original handler remains available as Ivy evidence,
    // while the listener interpreter sees source-like optional chains without
    // Closure's uninitialized scratch declarations.
    let handler = normalize_listener_optional_chaining(handler, environment.unresolved_ctxt);
    let Some(block) = handler_block(handler.as_ref()) else {
        return Ok(None);
    };

    let binding_uses = BindingUseIndex::collect_stmts(&block.stmts);
    let mut component_contexts = program.component_contexts.clone();
    let mut local_names = program.local_reference_names.clone();
    let mut local_context_bindings = program.local_context_bindings.clone();
    let mut expression_aliases = HashMap::new();
    let mut let_alias_hints = Vec::new();
    let mut effects = Vec::new();
    let mut effect_references = Vec::new();
    if let Some(event) = handler_event_binding(handler.as_ref()) {
        local_names.insert(event, "$event".to_string());
    }
    let mut action = None;
    let mut saw_reset_return = false;
    let mut runtime_calls = 0;
    let mut context_depth = 0usize;
    for (statement_index, statement) in block.stmts.iter().enumerate() {
        match statement {
            Stmt::Decl(Decl::Var(declaration)) => {
                for declarator in &declaration.decls {
                    let Pat::Ident(binding) = &declarator.name else {
                        return Err("expected identifier aliases".to_string());
                    };
                    let alias_binding = binding_key(&binding.id);
                    let alias_is_reassigned = binding_uses.has_direct_write(&alias_binding);
                    let Some(initializer) = declarator.init.as_deref() else {
                        return Err("view alias has no initializer".to_string());
                    };
                    if let Some(call) = restored_view_call(initializer, environment) {
                        if alias_is_reassigned {
                            return Err("view alias is reassigned".to_string());
                        }
                        let Some(property) = restored_view_property(initializer) else {
                            return Err("restored view has no context property".to_string());
                        };
                        let argument_lists = validated_single_call(call, "ɵɵrestoreView")?;
                        let [saved_view] = argument_lists[0] else {
                            return Err("ɵɵrestoreView expected one saved view".to_string());
                        };
                        let Expr::Ident(saved_view) = strip_parentheses(saved_view.expr.as_ref())
                        else {
                            return Err("ɵɵrestoreView saved view is not an identifier".to_string());
                        };
                        if !program.saved_views.contains(&binding_key(saved_view)) {
                            return Err(
                                "ɵɵrestoreView does not reference a captured view".to_string()
                            );
                        }
                        let name = if property == "$implicit"
                            || program.implicit_view_context_properties.contains(&property)
                        {
                            program.repeater_item_name.clone().unwrap_or_else(|| {
                                recovered_view_alias_name(binding.id.sym.as_ref(), "item")
                            })
                        } else if !is_likely_generated_alias(&property) {
                            to_valid_identifier_name(&property)
                        } else {
                            return Err(format!(
                                "unsupported restored view context property {property}"
                            ));
                        };
                        local_names.insert(alias_binding, name);
                        runtime_calls += 1;
                        context_depth = 0;
                        continue;
                    }

                    if let Some((alias, context_hop)) =
                        next_context_member_alias(initializer, environment)?
                    {
                        if alias_is_reassigned {
                            return Err("view alias is reassigned".to_string());
                        }
                        let destination_depth = context_depth.saturating_add(context_hop);
                        let alias = match alias.as_ref() {
                            Expr::Ident(property) => program
                                .context_property_name_for_binding(
                                    destination_depth,
                                    property.sym.as_ref(),
                                    &alias_binding,
                                )
                                .map(|name| {
                                    Box::new(Expr::Ident(Ident::new(
                                        name.into(),
                                        property.span,
                                        SyntaxContext::empty(),
                                    )))
                                })
                                .unwrap_or(alias),
                            _ => alias,
                        };
                        expression_aliases.insert(alias_binding, alias);
                        runtime_calls += 1;
                        context_depth = destination_depth;
                        continue;
                    }

                    if let Some(slot) = inlined_reference_slot(initializer) {
                        if alias_is_reassigned {
                            return Err("view alias is reassigned".to_string());
                        }
                        let Some(name) =
                            reference_name_at_depth(tree, ancestor_references, context_depth, slot)
                        else {
                            return Err(format!(
                                "no inlined local reference at slot {slot} in context depth {context_depth}"
                            ));
                        };
                        local_names.insert(alias_binding, name);
                        continue;
                    }

                    let Expr::Call(call) = strip_parentheses(initializer) else {
                        return Err("unsupported view-local alias initializer".to_string());
                    };
                    if is_instruction_call(call, IvyInstruction::ReadContextLet, environment) {
                        if alias_is_reassigned {
                            return Err("view alias is reassigned".to_string());
                        }
                        let argument_lists = validated_single_call(call, "ɵɵreadContextLet")?;
                        let [slot] = argument_lists[0] else {
                            return Err("ɵɵreadContextLet expected one let slot".to_string());
                        };
                        let Some(slot) = numeric_expr(slot.expr.as_ref()) else {
                            return Err("ɵɵreadContextLet slot is not numeric".to_string());
                        };
                        let name = recovered_let_name(Some(binding.id.sym.as_ref()), slot);
                        local_names.insert(alias_binding, name.clone());
                        let_alias_hints.push(ViewLetAliasHint {
                            context_depth,
                            slot,
                            name,
                        });
                        runtime_calls += 1;
                        continue;
                    }
                    if is_instruction_call(call, IvyInstruction::RestoreView, environment) {
                        if alias_is_reassigned {
                            return Err("view alias is reassigned".to_string());
                        }
                        validate_restore_view_call(call, program)?;
                        component_contexts.insert(alias_binding);
                        runtime_calls += 1;
                        context_depth = 0;
                        continue;
                    }
                    if is_instruction_call(call, IvyInstruction::Reference, environment)
                        || is_reference_candidate_call(call, environment)
                    {
                        if alias_is_reassigned {
                            return Err("view alias is reassigned".to_string());
                        }
                        let argument_lists = validated_single_call(call, "ɵɵreference")?;
                        let [slot] = argument_lists[0] else {
                            return Err("ɵɵreference expected one slot".to_string());
                        };
                        let Some(slot) = numeric_expr(slot.expr.as_ref()) else {
                            return Err("ɵɵreference slot is not numeric".to_string());
                        };
                        let Some(name) =
                            reference_name_at_depth(tree, ancestor_references, context_depth, slot)
                        else {
                            return Err(format!(
                                "no local reference at slot {slot} in context depth {context_depth}"
                            ));
                        };
                        local_names.insert(alias_binding, name);
                        runtime_calls += 1;
                        continue;
                    }
                    if is_instruction_call(call, IvyInstruction::NextContext, environment) {
                        if alias_is_reassigned {
                            return Err("view alias is reassigned".to_string());
                        }
                        let argument_lists = validated_single_call(call, "ɵɵnextContext")?;
                        let Some(context_hop) = context_hop(&argument_lists) else {
                            return Err(
                                "ɵɵnextContext has unexpected context-depth arguments".to_string()
                            );
                        };
                        let destination_depth = context_depth.saturating_add(context_hop);
                        match program.context_scope_at_depth(destination_depth) {
                            Some(scope) if scope.is_component => {
                                component_contexts.insert(alias_binding);
                            }
                            Some(scope) if !scope.local_properties.is_empty() => {
                                local_context_bindings
                                    .insert(alias_binding, scope.local_properties);
                            }
                            Some(_) => {}
                            None => {
                                component_contexts.insert(alias_binding);
                            }
                        }
                        runtime_calls += 1;
                        context_depth = destination_depth;
                        continue;
                    }
                    return Err("unsupported view-local runtime alias".to_string());
                }
            }
            Stmt::Expr(expression) => {
                if let Expr::Call(call) = strip_parentheses(expression.expr.as_ref()) {
                    if is_instruction_call(call, IvyInstruction::RestoreView, environment) {
                        validate_restore_view_call(call, program)?;
                        runtime_calls += 1;
                        context_depth = 0;
                        continue;
                    }
                    if is_instruction_call(call, IvyInstruction::NextContext, environment) {
                        let argument_lists = validated_single_call(call, "ɵɵnextContext")?;
                        let Some(context_hop) = context_hop(&argument_lists) else {
                            return Err(
                                "ɵɵnextContext has unexpected context-depth arguments".to_string()
                            );
                        };
                        runtime_calls += 1;
                        context_depth = context_depth.saturating_add(context_hop);
                        continue;
                    }
                    if is_instruction_call(call, IvyInstruction::ReadContextLet, environment) {
                        let argument_lists = validated_single_call(call, "ɵɵreadContextLet")?;
                        let [slot] = argument_lists[0] else {
                            return Err("ɵɵreadContextLet expected one let slot".to_string());
                        };
                        if numeric_expr(slot.expr.as_ref()).is_none() {
                            return Err("ɵɵreadContextLet slot is not numeric".to_string());
                        }
                        runtime_calls += 1;
                        continue;
                    }
                }
                if contains_runtime_call(expression.expr.as_ref(), environment) {
                    let (rewritten, rewritten_calls, context_hops) =
                        rewrite_next_context_members(expression.expr.as_ref(), environment)?;
                    if contains_runtime_call(rewritten.as_ref(), environment) {
                        return Err(
                            "unsupported Ivy runtime call in restored handler expression"
                                .to_string(),
                        );
                    }
                    runtime_calls += rewritten_calls;
                    context_depth = context_depth.saturating_add(context_hops);
                    effect_references.push(expression.expr.clone());
                    effects.push(rewritten);
                } else {
                    effect_references.push(expression.expr.clone());
                    effects.push(expression.expr.clone());
                }
            }
            Stmt::Return(ReturnStmt {
                arg: Some(returned),
                ..
            }) => {
                if block.stmts[statement_index + 1..]
                    .iter()
                    .any(|statement| !matches!(statement, Stmt::Empty(_)))
                {
                    return Err("ɵɵresetView return is not the final handler statement".to_string());
                }
                if saw_reset_return {
                    return Err("multiple handler returns".to_string());
                }
                let returned = strip_parentheses(returned.as_ref());
                let (return_effects, reset_expression) = match returned {
                    Expr::Seq(sequence) => {
                        let Some((reset_expression, return_effects)) = sequence.exprs.split_last()
                        else {
                            return Err("restored handler return is empty".to_string());
                        };
                        (
                            return_effects
                                .iter()
                                .map(|effect| effect.as_ref())
                                .collect::<Vec<_>>(),
                            reset_expression.as_ref(),
                        )
                    }
                    returned => (Vec::new(), returned),
                };
                let Expr::Call(call) = strip_parentheses(reset_expression) else {
                    return Err("restored handler return is not ɵɵresetView".to_string());
                };
                if !is_instruction_call(call, IvyInstruction::ResetView, environment) {
                    return Err("restored handler return is not ɵɵresetView".to_string());
                }
                let argument_lists = validated_single_call(call, "ɵɵresetView")?;
                action = match argument_lists[0] {
                    [] => None,
                    [returned] => Some(returned.expr.clone()),
                    _ => return Err("ɵɵresetView expected at most one return value".to_string()),
                };
                for effect in return_effects {
                    if contains_runtime_call(effect, environment) {
                        let (rewritten, rewritten_calls, context_hops) =
                            rewrite_next_context_members(effect, environment)?;
                        if contains_runtime_call(rewritten.as_ref(), environment) {
                            return Err(
                                "unsupported Ivy runtime call in restored handler expression"
                                    .to_string(),
                            );
                        }
                        runtime_calls += rewritten_calls;
                        context_depth = context_depth.saturating_add(context_hops);
                        effect_references.push(Box::new(effect.clone()));
                        effects.push(rewritten);
                    } else {
                        effect_references.push(Box::new(effect.clone()));
                        effects.push(Box::new(effect.clone()));
                    }
                }
                saw_reset_return = true;
                runtime_calls += 1;
            }
            Stmt::Empty(_) => {}
            _ => return Err("unsupported restored handler statement".to_string()),
        }
    }
    if !saw_reset_return {
        return Err("restored handler has no ɵɵresetView return".to_string());
    }
    let action_references = action.clone();
    if let Some(raw_action) = action.take() {
        let (rewritten, rewritten_calls, _) =
            rewrite_next_context_members(raw_action.as_ref(), environment)?;
        if contains_runtime_call(rewritten.as_ref(), environment) {
            return Err("unsupported Ivy runtime call in restored handler expression".to_string());
        }
        runtime_calls += rewritten_calls;
        action = Some(rewritten);
    }
    let mut sources = Vec::with_capacity(effects.len() + usize::from(action.is_some()));
    for effect in &effects {
        sources.push(
            print_template_expression_with_aliases(
                effect.as_ref(),
                &component_contexts,
                &local_names,
                &expression_aliases,
                &local_context_bindings,
                environment.cm.clone(),
            )
            .map_err(|error| error.to_string())?,
        );
    }
    if let Some(action) = &action {
        sources.push(
            print_template_expression_with_aliases(
                action.as_ref(),
                &component_contexts,
                &local_names,
                &expression_aliases,
                &local_context_bindings,
                environment.cm.clone(),
            )
            .map_err(|error| error.to_string())?,
        );
    }
    if sources.is_empty() {
        sources.push("undefined".to_string());
    }
    let source = sources.join("; ");
    let mut artifact_references = action_references
        .as_deref()
        .map(expression_references)
        .unwrap_or_default();
    for effect in &effect_references {
        artifact_references.extend(expression_references(effect.as_ref()));
    }
    for alias in expression_aliases.values() {
        artifact_references.extend(expression_references(alias.as_ref()));
    }
    Ok(Some(RecoveredViewHandler {
        source,
        runtime_calls,
        artifact_references,
        let_alias_hints,
        listener_method: None,
    }))
}

fn rewrite_next_context_members(
    expression: &Expr,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> std::result::Result<(Box<Expr>, usize, usize), String> {
    struct Rewriter<'a> {
        environment: &'a TemplateRecoveryEnvironment<'a>,
        runtime_calls: usize,
        context_hops: usize,
        error: Option<String>,
    }

    impl VisitMut for Rewriter<'_> {
        fn visit_mut_expr(&mut self, expression: &mut Expr) {
            expression.visit_mut_children_with(self);
            if self.error.is_some() {
                return;
            }
            let Expr::Member(member) = expression else {
                return;
            };
            let Expr::Call(call) = strip_parentheses(member.obj.as_ref()) else {
                return;
            };
            if !is_instruction_call(call, IvyInstruction::NextContext, self.environment) {
                return;
            }
            let Ok(argument_lists) = validated_single_call(call, "ɵɵnextContext") else {
                self.error = Some("ɵɵnextContext has an unexpected chained invocation".to_string());
                return;
            };
            let Some(context_hop) = context_hop(&argument_lists) else {
                self.error =
                    Some("ɵɵnextContext has unexpected context-depth arguments".to_string());
                return;
            };
            let MemberProp::Ident(property) = &member.prop else {
                self.error =
                    Some("ɵɵnextContext result has a computed context property".to_string());
                return;
            };
            self.runtime_calls += argument_lists.len();
            self.context_hops = self.context_hops.saturating_add(context_hop);
            *expression = Expr::Ident(Ident::new(
                property.sym.clone(),
                property.span,
                SyntaxContext::empty(),
            ));
        }
    }

    let mut expression = Box::new(expression.clone());
    let mut rewriter = Rewriter {
        environment,
        runtime_calls: 0,
        context_hops: 0,
        error: None,
    };
    expression.visit_mut_with(&mut rewriter);
    if let Some(error) = rewriter.error {
        return Err(error);
    }
    Ok((expression, rewriter.runtime_calls, rewriter.context_hops))
}

fn normalize_listener_optional_chaining(
    handler: &Expr,
    unresolved_ctxt: SyntaxContext,
) -> Box<Expr> {
    let mut module = Module {
        span: DUMMY_SP,
        body: vec![ModuleItem::Stmt(Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(handler.clone()),
        }))],
        shebang: None,
    };
    module.visit_mut_with(&mut UnOptionalChaining::new(
        unresolved_ctxt.outer(),
        RewriteLevel::Standard,
    ));
    let Some(ModuleItem::Stmt(Stmt::Expr(statement))) = module.body.pop() else {
        return Box::new(handler.clone());
    };
    statement.expr
}

fn next_context_member_alias(
    initializer: &Expr,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> std::result::Result<Option<(Box<Expr>, usize)>, String> {
    let Expr::Member(member) = strip_parentheses(initializer) else {
        return Ok(None);
    };
    let Expr::Call(call) = strip_parentheses(member.obj.as_ref()) else {
        return Ok(None);
    };
    if !is_instruction_call(call, IvyInstruction::NextContext, environment) {
        return Ok(None);
    }
    let argument_lists = validated_single_call(call, "ɵɵnextContext")?;
    let Some(context_hop) = context_hop(&argument_lists) else {
        return Err("ɵɵnextContext has unexpected context-depth arguments".to_string());
    };
    let MemberProp::Ident(property) = &member.prop else {
        return Err("ɵɵnextContext result has a computed context property".to_string());
    };
    Ok(Some((
        Box::new(Expr::Ident(Ident::new(
            property.sym.clone(),
            property.span,
            SyntaxContext::empty(),
        ))),
        context_hop,
    )))
}

fn contains_runtime_call(expression: &Expr, environment: &TemplateRecoveryEnvironment<'_>) -> bool {
    struct Finder<'a> {
        environment: &'a TemplateRecoveryEnvironment<'a>,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            let Some((root, _)) = call_chain(call) else {
                self.found = true;
                return;
            };
            if self
                .environment
                .roles
                .ivy_name_for_expr(root, self.environment.unresolved_ctxt)
                .is_some()
                || self
                    .environment
                    .roles
                    .is_core_namespace_member(root, self.environment.unresolved_ctxt)
            {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = Finder {
        environment,
        found: false,
    };
    expression.visit_with(&mut finder);
    finder.found
}

fn handler_block(handler: &Expr) -> Option<&BlockStmt> {
    match strip_parentheses(handler) {
        Expr::Fn(function) => function.function.body.as_ref(),
        Expr::Arrow(arrow) => match arrow.body.as_ref() {
            BlockStmtOrExpr::BlockStmt(block) => Some(block),
            BlockStmtOrExpr::Expr(_) => None,
        },
        _ => None,
    }
}

fn handler_event_binding(handler: &Expr) -> Option<BindingKey> {
    let parameter = match strip_parentheses(handler) {
        Expr::Fn(function) => function.function.params.first().map(|param| &param.pat),
        Expr::Arrow(arrow) => arrow.params.first(),
        _ => None,
    };
    let Some(Pat::Ident(binding)) = parameter else {
        return None;
    };
    Some(binding_key(&binding.id))
}

fn recover_two_way_listener_target(
    handler: &Expr,
    tree: &TemplateTree,
    ancestor_references: &[ReferenceScope],
    program: &mut TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> std::result::Result<String, String> {
    let Some(event) = handler_event_binding(handler) else {
        return Err("two-way listener has no identifier event parameter".to_string());
    };
    let Some(block) = handler_block(handler) else {
        return Err("two-way listener does not have a block body".to_string());
    };
    if let Some(target) = direct_two_way_listener_target(block, &event, environment)? {
        let target = recover_template_expression(target, program, environment)
            .map_err(|error| error.to_string())?;
        program.stats.runtime_calls_observed += 1;
        program.stats.rendered_instruction_calls += 1;
        return Ok(target);
    }

    recover_restored_two_way_listener_target(
        block,
        &event,
        tree,
        ancestor_references,
        program,
        environment,
    )
}

fn direct_two_way_listener_target<'a>(
    block: &'a BlockStmt,
    event: &BindingKey,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> std::result::Result<Option<&'a Expr>, String> {
    let [Stmt::Return(ReturnStmt {
        arg: Some(returned),
        ..
    })] = block.stmts.as_slice()
    else {
        return Ok(None);
    };
    let Expr::Seq(sequence) = strip_parentheses(returned.as_ref()) else {
        return Ok(None);
    };
    let [binding_update, returned_event] = sequence.exprs.as_slice() else {
        return Ok(None);
    };
    let Expr::Ident(returned_event) = strip_parentheses(returned_event.as_ref()) else {
        return Ok(None);
    };
    if binding_key(returned_event) != *event {
        return Err("two-way listener returns a different event binding".to_string());
    }
    validate_two_way_binding_update(binding_update.as_ref(), event, environment).map(Some)
}

fn validate_two_way_binding_update<'a>(
    binding_update: &'a Expr,
    event: &BindingKey,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> std::result::Result<&'a Expr, String> {
    let Expr::Bin(binding_update) = strip_parentheses(binding_update) else {
        return Err("two-way listener update is not a logical fallback".to_string());
    };
    if binding_update.op != BinaryOp::LogicalOr {
        return Err("two-way listener update is not a logical OR".to_string());
    }
    let Expr::Call(binding_set) = strip_parentheses(binding_update.left.as_ref()) else {
        return Err("two-way listener does not call ɵɵtwoWayBindingSet".to_string());
    };
    if !is_instruction_call(binding_set, IvyInstruction::TwoWayBindingSet, environment) {
        return Err("two-way listener does not call ɵɵtwoWayBindingSet".to_string());
    }
    let argument_lists = validated_single_call(binding_set, "ɵɵtwoWayBindingSet")?;
    let [target, bound_event] = argument_lists[0] else {
        return Err("ɵɵtwoWayBindingSet expected a target and event value".to_string());
    };
    let Expr::Ident(bound_event) = strip_parentheses(bound_event.expr.as_ref()) else {
        return Err("ɵɵtwoWayBindingSet value is not the handler event".to_string());
    };
    if binding_key(bound_event) != *event {
        return Err("ɵɵtwoWayBindingSet uses a different event binding".to_string());
    }
    let Expr::Assign(fallback) = strip_parentheses(binding_update.right.as_ref()) else {
        return Err("two-way listener fallback is not an assignment".to_string());
    };
    if fallback.op != AssignOp::Assign
        || !assign_target_matches_expression(&fallback.left, target.expr.as_ref())
    {
        return Err("two-way listener fallback assigns a different target".to_string());
    }
    let Expr::Ident(fallback_event) = strip_parentheses(fallback.right.as_ref()) else {
        return Err("two-way listener fallback value is not the handler event".to_string());
    };
    if binding_key(fallback_event) != *event {
        return Err("two-way listener fallback uses a different event binding".to_string());
    }
    Ok(target.expr.as_ref())
}

fn recover_restored_two_way_listener_target(
    block: &BlockStmt,
    event: &BindingKey,
    tree: &TemplateTree,
    ancestor_references: &[ReferenceScope],
    program: &mut TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> std::result::Result<String, String> {
    let mut component_contexts = program.component_contexts.clone();
    let mut local_names = program.local_reference_names.clone();
    let mut local_context_bindings = program.local_context_bindings.clone();
    let mut binding_update = None;
    let mut saw_restore = false;
    let mut saw_reset = false;
    let mut runtime_calls = 0;
    let mut context_depth = 0usize;

    for (statement_index, statement) in block.stmts.iter().enumerate() {
        match statement {
            Stmt::Decl(Decl::Var(declaration)) => {
                for declarator in &declaration.decls {
                    let Pat::Ident(binding) = &declarator.name else {
                        return Err("two-way listener alias is not an identifier".to_string());
                    };
                    let Some(initializer) = declarator.init.as_deref() else {
                        return Err("two-way listener alias has no initializer".to_string());
                    };
                    if let Expr::Call(call) = strip_parentheses(initializer) {
                        if is_instruction_call(call, IvyInstruction::RestoreView, environment) {
                            validate_restore_view_call(call, program)?;
                            component_contexts.insert(binding_key(&binding.id));
                            saw_restore = true;
                            runtime_calls += 1;
                            context_depth = 0;
                            continue;
                        }
                        if is_instruction_call(call, IvyInstruction::NextContext, environment) {
                            let argument_lists = validated_single_call(call, "ɵɵnextContext")?;
                            let Some(context_hop) = context_hop(&argument_lists) else {
                                return Err("ɵɵnextContext has unexpected context-depth arguments"
                                    .to_string());
                            };
                            let destination_depth = context_depth.saturating_add(context_hop);
                            match program.context_scope_at_depth(destination_depth) {
                                Some(scope) if scope.is_component => {
                                    component_contexts.insert(binding_key(&binding.id));
                                }
                                Some(scope) if !scope.local_properties.is_empty() => {
                                    local_context_bindings
                                        .insert(binding_key(&binding.id), scope.local_properties);
                                }
                                Some(_) => {}
                                None => {
                                    component_contexts.insert(binding_key(&binding.id));
                                }
                            }
                            context_depth = destination_depth;
                            runtime_calls += 1;
                            continue;
                        }
                        if is_instruction_call(call, IvyInstruction::Reference, environment)
                            || is_reference_candidate_call(call, environment)
                        {
                            let argument_lists = validated_single_call(call, "ɵɵreference")?;
                            let [slot] = argument_lists[0] else {
                                return Err("ɵɵreference expected one slot".to_string());
                            };
                            let Some(slot) = numeric_expr(slot.expr.as_ref()) else {
                                return Err("ɵɵreference slot is not numeric".to_string());
                            };
                            let Some(name) = reference_name_at_depth(
                                tree,
                                ancestor_references,
                                context_depth,
                                slot,
                            ) else {
                                return Err(format!("no local reference at slot {slot}"));
                            };
                            local_names.insert(binding_key(&binding.id), name);
                            runtime_calls += 1;
                            continue;
                        }
                    }
                    if let Some(slot) = inlined_reference_slot(initializer) {
                        let Some(name) =
                            reference_name_at_depth(tree, ancestor_references, context_depth, slot)
                        else {
                            return Err(format!("no inlined local reference at slot {slot}"));
                        };
                        local_names.insert(binding_key(&binding.id), name);
                        continue;
                    }
                    return Err("unsupported two-way listener alias initializer".to_string());
                }
            }
            Stmt::Expr(expression) => {
                let expression = strip_parentheses(expression.expr.as_ref());
                if let Expr::Call(call) = expression {
                    if is_instruction_call(call, IvyInstruction::RestoreView, environment) {
                        validate_restore_view_call(call, program)?;
                        saw_restore = true;
                        runtime_calls += 1;
                        context_depth = 0;
                        continue;
                    }
                    if is_instruction_call(call, IvyInstruction::NextContext, environment) {
                        let argument_lists = validated_single_call(call, "ɵɵnextContext")?;
                        let Some(context_hop) = context_hop(&argument_lists) else {
                            return Err(
                                "ɵɵnextContext has unexpected context-depth arguments".to_string()
                            );
                        };
                        context_depth = context_depth.saturating_add(context_hop);
                        runtime_calls += 1;
                        continue;
                    }
                }
                if binding_update.replace(expression).is_some() {
                    return Err("two-way listener has multiple binding updates".to_string());
                }
            }
            Stmt::Return(ReturnStmt {
                arg: Some(returned),
                ..
            }) => {
                if block.stmts[statement_index + 1..]
                    .iter()
                    .any(|statement| !matches!(statement, Stmt::Empty(_)))
                {
                    return Err("two-way listener reset is not the final statement".to_string());
                }
                let returned = strip_parentheses(returned.as_ref());
                let reset = if let Expr::Seq(sequence) = returned {
                    let Some((reset, effects)) = sequence.exprs.split_last() else {
                        return Err("two-way listener reset sequence is empty".to_string());
                    };
                    for effect in effects {
                        if binding_update.replace(effect.as_ref()).is_some() {
                            return Err("two-way listener has multiple binding updates".to_string());
                        }
                    }
                    reset.as_ref()
                } else {
                    returned
                };
                let Expr::Call(reset) = strip_parentheses(reset) else {
                    return Err("two-way listener return is not ɵɵresetView".to_string());
                };
                if !is_instruction_call(reset, IvyInstruction::ResetView, environment) {
                    return Err("two-way listener return is not ɵɵresetView".to_string());
                }
                let argument_lists = validated_single_call(reset, "ɵɵresetView")?;
                let [returned_event] = argument_lists[0] else {
                    return Err("ɵɵresetView expected the handler event".to_string());
                };
                let Expr::Ident(returned_event) = strip_parentheses(returned_event.expr.as_ref())
                else {
                    return Err("ɵɵresetView value is not the handler event".to_string());
                };
                if binding_key(returned_event) != *event {
                    return Err("ɵɵresetView uses a different event binding".to_string());
                }
                saw_reset = true;
                runtime_calls += 1;
            }
            Stmt::Empty(_) => {}
            _ => return Err("unsupported restored two-way listener statement".to_string()),
        }
    }
    if !saw_restore || !saw_reset {
        return Err("two-way listener does not have a direct or restored-view shape".to_string());
    }
    let binding_update =
        binding_update.ok_or_else(|| "two-way listener has no binding update".to_string())?;
    let target = validate_two_way_binding_update(binding_update, event, environment)?;
    let target = print_template_expression_with_aliases(
        target,
        &component_contexts,
        &local_names,
        &HashMap::new(),
        &local_context_bindings,
        environment.cm.clone(),
    )
    .map_err(|error| error.to_string())?;
    runtime_calls += 1;
    program.stats.runtime_calls_observed += runtime_calls;
    program.stats.rendered_instruction_calls += runtime_calls;
    Ok(target)
}

fn assign_target_matches_expression(target: &AssignTarget, expression: &Expr) -> bool {
    match (target, strip_parentheses(expression)) {
        (AssignTarget::Simple(SimpleAssignTarget::Ident(target)), Expr::Ident(expression)) => {
            binding_key(&target.id) == binding_key(expression)
        }
        (AssignTarget::Simple(SimpleAssignTarget::Member(target)), Expr::Member(expression)) => {
            matches!(
                (
                    strip_parentheses(target.obj.as_ref()),
                    strip_parentheses(expression.obj.as_ref()),
                ),
                (Expr::Ident(target), Expr::Ident(expression))
                    if binding_key(target) == binding_key(expression)
            ) && member_prop_name(&target.prop) == member_prop_name(&expression.prop)
        }
        _ => false,
    }
}

fn statement_restore_view_call<'a>(
    statement: &'a Stmt,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> Option<&'a CallExpr> {
    match statement {
        Stmt::Decl(Decl::Var(declaration)) => declaration
            .decls
            .iter()
            .filter_map(|declarator| declarator.init.as_deref())
            .find_map(|initializer| {
                restored_view_call(initializer, environment).or_else(|| {
                    let Expr::Call(call) = strip_parentheses(initializer) else {
                        return None;
                    };
                    is_instruction_call(call, IvyInstruction::RestoreView, environment)
                        .then_some(call)
                })
            }),
        Stmt::Expr(expression) => {
            let Expr::Call(call) = strip_parentheses(expression.expr.as_ref()) else {
                return None;
            };
            is_instruction_call(call, IvyInstruction::RestoreView, environment).then_some(call)
        }
        _ => None,
    }
}

fn restored_view_call<'a>(
    initializer: &'a Expr,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> Option<&'a CallExpr> {
    let Expr::Member(member) = strip_parentheses(initializer) else {
        return None;
    };
    let Expr::Call(call) = strip_parentheses(member.obj.as_ref()) else {
        return None;
    };
    is_instruction_call(call, IvyInstruction::RestoreView, environment).then_some(call)
}

fn restored_view_property(initializer: &Expr) -> Option<String> {
    let Expr::Member(member) = strip_parentheses(initializer) else {
        return None;
    };
    member_prop_name(&member.prop).map(|property| property.to_string())
}

fn is_instruction_call(
    call: &CallExpr,
    instruction: IvyInstruction,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> bool {
    call_chain(call).is_some_and(|(root, _)| {
        environment
            .roles
            .instruction_for_expr(root, environment.unresolved_ctxt)
            == Some(instruction)
    })
}

fn is_reference_candidate_call(
    call: &CallExpr,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> bool {
    call_chain(call).is_some_and(|(root, _)| {
        environment
            .roles
            .is_reference_candidate_expr(root, environment.unresolved_ctxt)
    })
}

fn validate_restore_view_call(
    call: &CallExpr,
    program: &TemplateProgram,
) -> std::result::Result<(), String> {
    let argument_lists = validated_single_call(call, "ɵɵrestoreView")?;
    let [saved_view] = argument_lists[0] else {
        return Err("ɵɵrestoreView expected one saved view".to_string());
    };
    let Expr::Ident(saved_view) = strip_parentheses(saved_view.expr.as_ref()) else {
        return Err("ɵɵrestoreView saved view is not an identifier".to_string());
    };
    if !program.saved_views.contains(&binding_key(saved_view)) {
        return Err("ɵɵrestoreView does not reference a captured view".to_string());
    }
    Ok(())
}

fn validated_single_call<'a>(
    call: &'a CallExpr,
    name: &str,
) -> std::result::Result<Vec<&'a [ExprOrSpread]>, String> {
    let Some((_, argument_lists)) = call_chain(call) else {
        return Err(format!("{name} has a non-expression call target"));
    };
    if argument_lists.len() != 1 {
        return Err(format!("{name} has an unexpected chained invocation"));
    }
    Ok(argument_lists)
}

struct ViewRecoveryState<'a> {
    active_templates: &'a mut HashSet<BindingKey>,
    next_view_id: &'a mut usize,
    ancestor_references: &'a [ReferenceScope],
    ancestor_contexts: &'a [ViewContextScope],
    depth: usize,
}

fn apply_create_instruction(
    call: &InstructionCall,
    tree: &mut TemplateTree,
    program: &mut TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
    recovery: ViewRecoveryState<'_>,
) -> Result<()> {
    let ViewRecoveryState {
        active_templates,
        next_view_id,
        ancestor_references,
        ancestor_contexts,
        depth,
    } = recovery;
    if !matches!(
        call.instruction,
        IvyInstruction::Defer | IvyInstruction::DeferOnIdle
    ) {
        tree.pending_defer = None;
    }
    match call.instruction {
        IvyInstruction::ElementStart => {
            let Some(index) = numeric_arg(&call.args, 0) else {
                record_malformed_instruction(
                    call,
                    "missing numeric node index",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(tag) = call.args.get(1).and_then(|arg| string_lit(arg.as_ref())) else {
                record_malformed_instruction(
                    call,
                    "missing literal element name",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let attributes = numeric_arg(&call.args, 2)
                .and_then(|index| environment.constants.attributes.get(index).cloned())
                .unwrap_or_default();
            let node = tree.push_node(index, TemplateNodeKind::Element { tag, attributes });
            attach_local_references(call, index, node, 3, tree, environment);
            tree.stack.push(node);
        }
        IvyInstruction::Element => {
            let Some(index) = numeric_arg(&call.args, 0) else {
                record_malformed_instruction(
                    call,
                    "missing numeric node index",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(tag) = call.args.get(1).and_then(|arg| string_lit(arg.as_ref())) else {
                record_malformed_instruction(
                    call,
                    "missing literal element name",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let attributes = numeric_arg(&call.args, 2)
                .and_then(|index| environment.constants.attributes.get(index).cloned())
                .unwrap_or_default();
            let node = tree.push_node(index, TemplateNodeKind::Element { tag, attributes });
            attach_local_references(call, index, node, 3, tree, environment);
        }
        IvyInstruction::ElementEnd => {
            if tree.stack.pop().is_none() {
                record_missing_target(
                    call,
                    "no open element",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
        }
        IvyInstruction::ElementContainerStart | IvyInstruction::ElementContainer => {
            let Some(index) = numeric_arg(&call.args, 0) else {
                record_malformed_instruction(
                    call,
                    "missing numeric ng-container index",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let attributes = numeric_arg(&call.args, 1)
                .and_then(|index| environment.constants.attributes.get(index).cloned())
                .unwrap_or_default();
            let node = tree.push_node(
                index,
                TemplateNodeKind::Element {
                    tag: "ng-container".to_string(),
                    attributes,
                },
            );
            attach_local_references(call, index, node, 2, tree, environment);
            if call.instruction == IvyInstruction::ElementContainerStart {
                tree.stack.push(node);
            }
        }
        IvyInstruction::ElementContainerEnd => {
            if tree.stack.pop().is_none() {
                record_missing_target(
                    call,
                    "no open ng-container",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
        }
        IvyInstruction::NamespaceHtml
        | IvyInstruction::NamespaceSvg
        | IvyInstruction::NamespaceMathMl => {
            if !call.args.is_empty() {
                record_malformed_instruction(
                    call,
                    "namespace switch does not accept arguments",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
        }
        IvyInstruction::Text => {
            let Some(index) = numeric_arg(&call.args, 0) else {
                record_malformed_instruction(
                    call,
                    "missing numeric node index",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let value = call
                .args
                .get(1)
                .and_then(|arg| string_lit(arg.as_ref()))
                .unwrap_or_default();
            tree.push_node(index, TemplateNodeKind::Text { value });
        }
        IvyInstruction::DeclareLet => {
            if call.args.len() != 1 {
                record_malformed_instruction(
                    call,
                    "expected one let slot",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Some(index) = numeric_arg(&call.args, 0) else {
                record_malformed_instruction(
                    call,
                    "let slot is not numeric",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let name = tree
                .let_names
                .get(&index)
                .cloned()
                .unwrap_or_else(|| recovered_let_name(None, index));
            tree.push_node(
                index,
                TemplateNodeKind::Let {
                    name,
                    value: None,
                    provenance: call.provenance.clone(),
                },
            );
        }
        IvyInstruction::I18n => {
            if !matches!(call.args.len(), 2 | 3) {
                record_malformed_instruction(
                    call,
                    "expected an i18n node index, message index, and optional sub-template index",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Some(index) = numeric_arg(&call.args, 0) else {
                record_malformed_instruction(
                    call,
                    "missing numeric i18n node index",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(message_index) = numeric_arg(&call.args, 1) else {
                record_malformed_instruction(
                    call,
                    "missing numeric i18n message index",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(message) = environment
                .constants
                .i18n_messages
                .get(message_index)
                .and_then(|message| message.clone())
                .filter(|message| is_basic_i18n_message(message))
            else {
                record_unsupported_instruction(
                    call,
                    "i18n message constant is missing or contains structural opcodes",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(parent) = tree.stack.last().copied() else {
                record_missing_target(
                    call,
                    "i18n text has no containing element",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            tree.add_attribute(
                parent,
                TemplateAttribute {
                    name: "i18n".to_string(),
                    value: None,
                },
            );
            tree.push_node(index, TemplateNodeKind::Text { value: message });
        }
        IvyInstruction::I18nStart => {
            if call.args.len() != 2 {
                record_unsupported_instruction(
                    call,
                    if call.args.len() == 3 {
                        "structural i18n sub-template regions are not yet reconstructed"
                    } else {
                        "expected an i18n region index and message index"
                    },
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Some(index) = numeric_arg(&call.args, 0) else {
                record_malformed_instruction(
                    call,
                    "missing numeric i18n region index",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(message_index) = numeric_arg(&call.args, 1) else {
                record_malformed_instruction(
                    call,
                    "missing numeric i18n message index",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(tokens) = environment
                .constants
                .i18n_messages
                .get(message_index)
                .and_then(|message| message.as_deref())
                .and_then(parse_structural_i18n_message)
            else {
                record_unsupported_instruction(
                    call,
                    "i18n message constant is missing or contains unsupported structural opcodes",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(parent) = tree.stack.last().copied() else {
                record_missing_target(
                    call,
                    "i18n region has no containing element",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            tree.add_attribute(
                parent,
                TemplateAttribute {
                    name: "i18n".to_string(),
                    value: None,
                },
            );
            let node = tree.push_node(
                index,
                TemplateNodeKind::I18nRegion {
                    tokens,
                    expressions: Vec::new(),
                },
            );
            tree.stack.push(node);
        }
        IvyInstruction::I18nEnd => {
            if !call.args.is_empty() {
                record_malformed_instruction(
                    call,
                    "expected no i18n-end arguments",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Some(node) = tree.stack.last().copied() else {
                record_missing_target(
                    call,
                    "no open i18n region",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            if !matches!(tree.nodes[node].kind, TemplateNodeKind::I18nRegion { .. }) {
                record_missing_target(
                    call,
                    "the current template node is not an i18n region",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            tree.stack.pop();
        }
        IvyInstruction::AnimateEnter | IvyInstruction::AnimateLeave => {
            let Some(node) = tree.stack.last().copied() else {
                record_missing_target(
                    call,
                    "no current element",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let [value] = call.args.as_slice() else {
                record_malformed_instruction(
                    call,
                    "expected one animation binding value",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let suffix = if call.instruction == IvyInstruction::AnimateEnter {
                "enter"
            } else {
                "leave"
            };
            let (name, value) = if let Some(value) = string_lit(value.as_ref()) {
                (format!("animate.{suffix}"), value)
            } else {
                let Ok(value) = handler_expression(
                    value.as_ref(),
                    &program.component_contexts,
                    &program.local_reference_names,
                    environment.cm.clone(),
                ) else {
                    record_malformed_instruction(
                        call,
                        "animation binding expression could not be printed",
                        &mut program.issues,
                        &mut program.stats,
                    );
                    return Ok(());
                };
                (format!("[animate.{suffix}]"), value)
            };
            program
                .artifact_references
                .extend(expression_references(call.args[0].as_ref()));
            tree.add_attribute(
                node,
                TemplateAttribute {
                    name,
                    value: Some(value),
                },
            );
        }
        IvyInstruction::AnimateEnterListener | IvyInstruction::AnimateLeaveListener => {
            let Some(node) = tree.stack.last().copied() else {
                record_missing_target(
                    call,
                    "no current element",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let [handler] = call.args.as_slice() else {
                record_malformed_instruction(
                    call,
                    "expected one animation listener",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Ok(handler) = handler_expression(
                handler.as_ref(),
                &program.component_contexts,
                &program.local_reference_names,
                environment.cm.clone(),
            ) else {
                record_malformed_instruction(
                    call,
                    "animation listener could not be printed",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let suffix = if call.instruction == IvyInstruction::AnimateEnterListener {
                "enter"
            } else {
                "leave"
            };
            program
                .artifact_references
                .extend(expression_references(call.args[0].as_ref()));
            tree.add_attribute(
                node,
                TemplateAttribute {
                    name: format!("(animate.{suffix})"),
                    value: Some(handler),
                },
            );
        }
        IvyInstruction::TwoWayListener => {
            let Some(node) = tree.stack.last().copied() else {
                record_missing_target(
                    call,
                    "no current element",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let [event, handler] = call.args.as_slice() else {
                record_malformed_instruction(
                    call,
                    "expected a two-way event name and handler",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(event) = string_lit(event.as_ref()) else {
                record_malformed_instruction(
                    call,
                    "two-way event name is not a literal string",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(property) = event.strip_suffix("Change").filter(|name| !name.is_empty())
            else {
                record_malformed_instruction(
                    call,
                    "two-way event name does not end in Change",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let target = match recover_two_way_listener_target(
                handler.as_ref(),
                tree,
                ancestor_references,
                program,
                environment,
            ) {
                Ok(target) => target,
                Err(detail) => {
                    record_malformed_instruction(
                        call,
                        &detail,
                        &mut program.issues,
                        &mut program.stats,
                    );
                    return Ok(());
                }
            };
            tree.add_attribute(
                node,
                TemplateAttribute {
                    name: format!("[({property})]"),
                    value: Some(target),
                },
            );
        }
        IvyInstruction::Listener => {
            let Some(node) = tree.stack.last().copied() else {
                record_missing_target(
                    call,
                    "no current element",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(event) = call.args.first().and_then(|arg| string_lit(arg.as_ref())) else {
                record_malformed_instruction(
                    call,
                    "missing literal event name",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(handler) = call.args.get(1) else {
                record_malformed_instruction(
                    call,
                    "missing event handler",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let expression = match recover_view_listener_handler(
                handler.as_ref(),
                &event,
                tree,
                ancestor_references,
                program,
                environment,
            ) {
                Ok(Some(recovered)) => {
                    let RecoveredViewHandler {
                        source,
                        runtime_calls,
                        artifact_references,
                        let_alias_hints,
                        listener_method,
                    } = recovered;
                    program.stats.runtime_calls_observed += runtime_calls;
                    program.stats.rendered_instruction_calls += runtime_calls;
                    program.artifact_references.extend(artifact_references);
                    program.let_alias_hints.extend(let_alias_hints);
                    if let Some(listener_method) = listener_method {
                        program.listener_methods.push(listener_method);
                    }
                    source
                }
                Ok(None) => {
                    let Ok(expression) = handler_expression(
                        handler.as_ref(),
                        &program.component_contexts,
                        &program.local_reference_names,
                        environment.cm.clone(),
                    ) else {
                        record_malformed_instruction(
                            call,
                            "event handler could not be printed",
                            &mut program.issues,
                            &mut program.stats,
                        );
                        return Ok(());
                    };
                    program
                        .artifact_references
                        .extend(expression_references(handler.as_ref()));
                    expression
                }
                Err(detail) => {
                    record_malformed_instruction(
                        call,
                        &format!("event handler view restoration failed: {detail}"),
                        &mut program.issues,
                        &mut program.stats,
                    );
                    return Ok(());
                }
            };
            tree.add_attribute(
                node,
                TemplateAttribute {
                    name: format!("({event})"),
                    value: Some(expression),
                },
            );
        }
        IvyInstruction::ProjectionDef => {
            if call.args.len() > 1 {
                record_malformed_instruction(
                    call,
                    "expected zero or one projection-selector argument",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
        }
        IvyInstruction::Projection => {
            if !matches!(call.args.len(), 1..=3 | 6) {
                record_malformed_instruction(
                    call,
                    "expected projection metadata with an optional complete fallback template",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Some(index) = numeric_arg(&call.args, 0) else {
                record_malformed_instruction(
                    call,
                    "missing numeric projection index",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let selector_index = if call.args.len() == 1 {
                0
            } else {
                let Some(selector_index) = numeric_arg(&call.args, 1) else {
                    record_malformed_instruction(
                        call,
                        "projection selector index is not numeric",
                        &mut program.issues,
                        &mut program.stats,
                    );
                    return Ok(());
                };
                selector_index
            };
            let attributes = match call.args.get(2) {
                None => Vec::new(),
                Some(attributes)
                    if is_nullish_expression(attributes.as_ref(), environment.unresolved_ctxt) =>
                {
                    Vec::new()
                }
                Some(attributes) => {
                    if let Some(attributes_index) = numeric_expr(attributes.as_ref()) {
                        let Some(attributes) = environment
                            .constants
                            .attributes
                            .get(attributes_index)
                            .cloned()
                        else {
                            record_malformed_instruction(
                                call,
                                "projection attribute index is out of bounds",
                                &mut program.issues,
                                &mut program.stats,
                            );
                            return Ok(());
                        };
                        attributes
                    } else if matches!(strip_parentheses(attributes.as_ref()), Expr::Array(_)) {
                        decode_constant_attributes(attributes.as_ref())
                    } else {
                        record_malformed_instruction(
                            call,
                            "projection attributes are not a constant index, array, or null",
                            &mut program.issues,
                            &mut program.stats,
                        );
                        return Ok(());
                    }
                }
            };
            let selector = environment
                .projection_selectors
                .get(selector_index)
                .filter(|selector| selector.as_str() != "*")
                .cloned();
            if !environment.projection_selectors.is_empty()
                && selector.is_none()
                && environment
                    .projection_selectors
                    .get(selector_index)
                    .is_none()
            {
                record_malformed_instruction(
                    call,
                    "projection selector index is out of bounds",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let fallback = if call.args.len() == 6 {
                if numeric_arg(&call.args, 4).is_none() || numeric_arg(&call.args, 5).is_none() {
                    record_malformed_instruction(
                        call,
                        "projection fallback is missing declaration or binding counts",
                        &mut program.issues,
                        &mut program.stats,
                    );
                    return Ok(());
                }
                let mut staged_next_view_id = *next_view_id;
                let Some((fallback, fallback_program)) = recover_child_template(
                    call,
                    call.args[3].as_ref(),
                    "projection fallback",
                    program,
                    environment,
                    ChildViewRecovery {
                        parent_tree: tree,
                        active_templates,
                        next_view_id: &mut staged_next_view_id,
                        ancestor_references,
                        ancestor_contexts,
                        child_implicit_item_name: None,
                        depth,
                    },
                )?
                else {
                    return Ok(());
                };
                merge_template_program(program, fallback_program);
                *next_view_id = staged_next_view_id;
                Some(Box::new(fallback))
            } else {
                None
            };
            tree.push_node(
                index,
                TemplateNodeKind::Projection {
                    selector,
                    attributes,
                    fallback,
                },
            );
        }
        IvyInstruction::Pipe => {
            if call.args.len() != 2 {
                record_malformed_instruction(
                    call,
                    "expected a pipe slot and name",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Some(index) = numeric_arg(&call.args, 0) else {
                record_malformed_instruction(
                    call,
                    "missing numeric pipe slot",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(name) = call.args.get(1).and_then(|arg| string_lit(arg.as_ref())) else {
                record_malformed_instruction(
                    call,
                    "missing literal pipe name",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            if program.pipes.insert(index, name).is_some() {
                record_malformed_instruction(
                    call,
                    "duplicate pipe slot",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
        }
        IvyInstruction::RepeaterCreate => {
            if !(7..=13).contains(&call.args.len()) {
                record_malformed_instruction(
                    call,
                    "expected repeater metadata arguments",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Some(index) = numeric_arg(&call.args, 0) else {
                record_malformed_instruction(
                    call,
                    "missing numeric repeater index",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            if numeric_arg(&call.args, 2).is_none() || numeric_arg(&call.args, 3).is_none() {
                record_malformed_instruction(
                    call,
                    "missing repeater declaration or binding count",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Some(body_expression) = call.args.get(1) else {
                record_malformed_instruction(
                    call,
                    "missing repeater body template",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let mut staged_next_view_id = *next_view_id;
            let Some((body, body_program)) = recover_child_template(
                call,
                body_expression.as_ref(),
                "repeater body",
                program,
                environment,
                ChildViewRecovery {
                    parent_tree: tree,
                    active_templates,
                    next_view_id: &mut staged_next_view_id,
                    ancestor_references,
                    ancestor_contexts,
                    child_implicit_item_name: Some("item"),
                    depth,
                },
            )?
            else {
                return Ok(());
            };
            let item = body_program
                .repeater_item_name
                .clone()
                .unwrap_or_else(|| "item".to_string());

            let empty = if let Some(empty_expression) = call.args.get(8) {
                if is_nullish_expression(empty_expression.as_ref(), environment.unresolved_ctxt) {
                    None
                } else {
                    if numeric_arg(&call.args, 9).is_none() || numeric_arg(&call.args, 10).is_none()
                    {
                        record_malformed_instruction(
                            call,
                            "missing empty-view declaration or binding count",
                            &mut program.issues,
                            &mut program.stats,
                        );
                        return Ok(());
                    }
                    let Some((empty, empty_program)) = recover_child_template(
                        call,
                        empty_expression.as_ref(),
                        "repeater empty view",
                        program,
                        environment,
                        ChildViewRecovery {
                            parent_tree: tree,
                            active_templates,
                            next_view_id: &mut staged_next_view_id,
                            ancestor_references,
                            ancestor_contexts,
                            child_implicit_item_name: None,
                            depth,
                        },
                    )?
                    else {
                        return Ok(());
                    };
                    Some((Box::new(empty), empty_program))
                }
            } else {
                None
            };

            let Some(track_expression) = call.args.get(6) else {
                record_malformed_instruction(
                    call,
                    "missing repeater track expression",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let track = match recover_repeater_track_expression(
                track_expression.as_ref(),
                &item,
                program,
                environment,
            ) {
                Ok(track) => track,
                Err(detail) => {
                    record_malformed_instruction(
                        call,
                        &format!("repeater track expression could not be recovered: {detail}"),
                        &mut program.issues,
                        &mut program.stats,
                    );
                    return Ok(());
                }
            };
            merge_template_program(program, body_program);
            let empty = empty.map(|(empty, empty_program)| {
                merge_template_program(program, empty_program);
                empty
            });
            *next_view_id = staged_next_view_id;
            tree.push_node(
                index,
                TemplateNodeKind::Repeater {
                    body: Box::new(body),
                    empty,
                    item,
                    track,
                    collection: None,
                },
            );
        }
        IvyInstruction::Template => {
            let Some(index) = numeric_arg(&call.args, 0) else {
                record_malformed_instruction(
                    call,
                    "missing numeric template index",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            if numeric_arg(&call.args, 2).is_none() || numeric_arg(&call.args, 3).is_none() {
                record_malformed_instruction(
                    call,
                    "missing template declaration or binding count",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Some(template_expression) = call.args.get(1) else {
                record_malformed_instruction(
                    call,
                    "missing embedded template function",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(resolved) = environment
                .template_functions
                .resolve(template_expression.as_ref())
            else {
                record_malformed_instruction(
                    call,
                    "embedded template function could not be resolved",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            if depth >= 32
                || resolved
                    .key
                    .as_ref()
                    .is_some_and(|key| active_templates.contains(key))
            {
                record_malformed_instruction(
                    call,
                    "recursive embedded template graph",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            if let Some(key) = &resolved.key {
                active_templates.insert(key.clone());
            }
            let child_references = child_reference_scopes(tree, ancestor_references);
            let child_contexts = child_context_scopes(program, ancestor_contexts);
            let child = recover_template_tree(
                &resolved.function,
                environment,
                false,
                None,
                ViewRecoveryState {
                    active_templates,
                    next_view_id,
                    ancestor_references: &child_references,
                    ancestor_contexts: &child_contexts,
                    depth: depth + 1,
                },
            );
            if let Some(key) = &resolved.key {
                active_templates.remove(key);
            }
            let (child_tree, child_program) = child?;
            merge_template_program(program, child_program);
            let attributes = numeric_arg(&call.args, 5)
                .and_then(|index| environment.constants.attributes.get(index).cloned())
                .unwrap_or_default();
            let node = tree.push_node(
                index,
                TemplateNodeKind::EmbeddedView {
                    tree: Box::new(child_tree),
                    attributes,
                    branch: None,
                },
            );
            attach_local_references(call, index, node, 6, tree, environment);
        }
        IvyInstruction::Defer => {
            if !(3..=10).contains(&call.args.len()) {
                record_malformed_instruction(
                    call,
                    "expected defer metadata arguments",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Some(index) = numeric_arg(&call.args, 0) else {
                record_malformed_instruction(
                    call,
                    "missing numeric defer index",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(primary_index) = numeric_arg(&call.args, 1) else {
                record_malformed_instruction(
                    call,
                    "missing primary defer-template index",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            if !is_nullish_or_callable_expression(
                call.args[2].as_ref(),
                environment.unresolved_ctxt,
            ) {
                record_malformed_instruction(
                    call,
                    "defer dependency resolver is not callable or null",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let loading_index =
                match optional_defer_template_index(&call.args, 3, environment.unresolved_ctxt) {
                    Ok(index) => index,
                    Err(detail) => {
                        record_malformed_instruction(
                            call,
                            detail,
                            &mut program.issues,
                            &mut program.stats,
                        );
                        return Ok(());
                    }
                };
            let placeholder_index =
                match optional_defer_template_index(&call.args, 4, environment.unresolved_ctxt) {
                    Ok(index) => index,
                    Err(detail) => {
                        record_malformed_instruction(
                            call,
                            detail,
                            &mut program.issues,
                            &mut program.stats,
                        );
                        return Ok(());
                    }
                };
            let error_index =
                match optional_defer_template_index(&call.args, 5, environment.unresolved_ctxt) {
                    Ok(index) => index,
                    Err(detail) => {
                        record_malformed_instruction(
                            call,
                            detail,
                            &mut program.issues,
                            &mut program.stats,
                        );
                        return Ok(());
                    }
                };
            let view_indices = [
                Some(primary_index),
                loading_index,
                placeholder_index,
                error_index,
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            let trees = match tree.consume_trailing_embedded_views(&view_indices) {
                Ok(trees) => trees,
                Err(detail) => {
                    record_missing_target(call, &detail, &mut program.issues, &mut program.stats);
                    return Ok(());
                }
            };
            let mut trees = trees.into_iter();
            let primary = trees
                .next()
                .expect("the validated primary defer view is present");
            let loading = loading_index.map(|_| {
                trees
                    .next()
                    .expect("the validated loading defer view is present")
            });
            let placeholder = placeholder_index.map(|_| {
                trees
                    .next()
                    .expect("the validated placeholder defer view is present")
            });
            let error = error_index.map(|_| {
                trees
                    .next()
                    .expect("the validated error defer view is present")
            });
            let node = tree.push_node(
                index,
                TemplateNodeKind::Defer {
                    primary: Box::new(primary),
                    loading: loading.map(Box::new),
                    placeholder: placeholder.map(Box::new),
                    error: error.map(Box::new),
                    triggers: Vec::new(),
                },
            );
            tree.pending_defer = Some(node);

            if call.args.iter().skip(6).any(|argument| {
                !is_nullish_expression(argument.as_ref(), environment.unresolved_ctxt)
            }) {
                record_unsupported_instruction(
                    call,
                    "defer timing or hydration metadata is not yet rendered",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
        }
        IvyInstruction::DeferOnIdle => {
            if !call.args.is_empty() {
                record_unsupported_instruction(
                    call,
                    "idle trigger metadata is not yet rendered",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Some(node) = tree.pending_defer else {
                record_missing_target(
                    call,
                    "no immediately preceding defer block",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let TemplateNodeKind::Defer { triggers, .. } = &mut tree.nodes[node].kind else {
                record_missing_target(
                    call,
                    "the pending node is not a defer block",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            if !triggers.iter().any(|trigger| trigger == "on idle") {
                triggers.push("on idle".to_string());
            }
        }
        _ => {
            record_unsupported_instruction(
                call,
                "unsupported in creation phase",
                &mut program.issues,
                &mut program.stats,
            );
            return Ok(());
        }
    }
    program.stats.rendered_instruction_calls += 1;
    Ok(())
}

fn child_reference_scopes(
    tree: &TemplateTree,
    ancestor_references: &[ReferenceScope],
) -> Vec<ReferenceScope> {
    let mut scopes = Vec::with_capacity(ancestor_references.len() + 1);
    scopes.push(tree.local_reference_slots.clone());
    scopes.extend_from_slice(ancestor_references);
    scopes
}

fn child_context_scopes(
    program: &TemplateProgram,
    ancestor_contexts: &[ViewContextScope],
) -> Vec<ViewContextScope> {
    let mut scopes = Vec::with_capacity(ancestor_contexts.len() + 1);
    scopes.push(program.current_context_scope());
    scopes.extend_from_slice(ancestor_contexts);
    scopes
}

struct ChildViewRecovery<'a> {
    parent_tree: &'a TemplateTree,
    active_templates: &'a mut HashSet<BindingKey>,
    next_view_id: &'a mut usize,
    ancestor_references: &'a [ReferenceScope],
    ancestor_contexts: &'a [ViewContextScope],
    child_implicit_item_name: Option<&'a str>,
    depth: usize,
}

fn recover_child_template(
    call: &InstructionCall,
    expression: &Expr,
    description: &str,
    program: &mut TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
    recovery: ChildViewRecovery<'_>,
) -> Result<Option<(TemplateTree, TemplateProgram)>> {
    let ChildViewRecovery {
        parent_tree,
        active_templates,
        next_view_id,
        ancestor_references,
        ancestor_contexts,
        child_implicit_item_name,
        depth,
    } = recovery;
    let Some(resolved) = environment.template_functions.resolve(expression) else {
        record_malformed_instruction(
            call,
            &format!("{description} function could not be resolved"),
            &mut program.issues,
            &mut program.stats,
        );
        return Ok(None);
    };
    if depth >= 32
        || resolved
            .key
            .as_ref()
            .is_some_and(|key| active_templates.contains(key))
    {
        record_malformed_instruction(
            call,
            &format!("recursive {description} graph"),
            &mut program.issues,
            &mut program.stats,
        );
        return Ok(None);
    }
    if let Some(key) = &resolved.key {
        active_templates.insert(key.clone());
    }
    let child_references = child_reference_scopes(parent_tree, ancestor_references);
    let child_contexts = child_context_scopes(program, ancestor_contexts);
    let child = recover_template_tree(
        &resolved.function,
        environment,
        false,
        child_implicit_item_name,
        ViewRecoveryState {
            active_templates,
            next_view_id,
            ancestor_references: &child_references,
            ancestor_contexts: &child_contexts,
            depth: depth + 1,
        },
    );
    if let Some(key) = &resolved.key {
        active_templates.remove(key);
    }
    child.map(Some)
}

fn recover_repeater_track_expression(
    expression: &Expr,
    item: &str,
    program: &mut TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> std::result::Result<String, String> {
    if let Some(instruction) = environment
        .roles
        .instruction_for_expr(strip_parentheses(expression), environment.unresolved_ctxt)
    {
        return match instruction {
            IvyInstruction::RepeaterTrackByIndex => Ok("$index".to_string()),
            IvyInstruction::RepeaterTrackByIdentity => Ok(item.to_string()),
            _ => Err(format!(
                "unexpected runtime helper {}",
                instruction.canonical_export_name()
            )),
        };
    }

    let resolved = environment
        .template_functions
        .resolve_expression(expression);
    let (parameters, body) = match strip_parentheses(resolved.as_ref()) {
        Expr::Arrow(arrow) => {
            let body = match arrow.body.as_ref() {
                BlockStmtOrExpr::Expr(expression) => expression.as_ref(),
                BlockStmtOrExpr::BlockStmt(block) => single_return_value(block)
                    .ok_or_else(|| "track function is not a single expression".to_string())?,
            };
            (arrow.params.as_slice(), body)
        }
        Expr::Fn(function) => {
            let body = function
                .function
                .body
                .as_ref()
                .and_then(single_return_value)
                .ok_or_else(|| "track function is not a single expression".to_string())?;
            let parameters = function
                .function
                .params
                .iter()
                .map(|parameter| &parameter.pat)
                .collect::<Vec<_>>();
            return print_repeater_track_body(body, &parameters, item, program, environment);
        }
        _ => return Err("track value is not a resolvable function".to_string()),
    };
    let parameters = parameters.iter().collect::<Vec<_>>();
    print_repeater_track_body(body, &parameters, item, program, environment)
}

fn print_repeater_track_body(
    body: &Expr,
    parameters: &[&Pat],
    item: &str,
    program: &mut TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> std::result::Result<String, String> {
    let [Pat::Ident(index_binding), Pat::Ident(item_binding)] = parameters else {
        return Err("track function does not have two identifier parameters".to_string());
    };
    let mut local_names = HashMap::from([
        (binding_key(&index_binding.id), "$index".to_string()),
        (binding_key(&item_binding.id), item.to_string()),
    ]);
    local_names.extend(program.local_reference_names.clone());
    let printed = print_template_expression_with_aliases(
        body,
        &program.component_contexts,
        &local_names,
        &HashMap::new(),
        &program.local_context_bindings,
        environment.cm.clone(),
    )
    .map_err(|error| error.to_string())?;
    let parameter_keys = HashSet::from([
        binding_key(&index_binding.id),
        binding_key(&item_binding.id),
    ]);
    program.artifact_references.extend(
        expression_references(body)
            .into_iter()
            .filter(|reference| !parameter_keys.contains(reference)),
    );
    Ok(printed)
}

fn single_return_value(block: &BlockStmt) -> Option<&Expr> {
    let [Stmt::Return(ReturnStmt {
        arg: Some(expression),
        ..
    })] = block.stmts.as_slice()
    else {
        return None;
    };
    Some(expression.as_ref())
}

fn is_nullish_expression(expression: &Expr, unresolved_ctxt: SyntaxContext) -> bool {
    match strip_parentheses(expression) {
        Expr::Lit(Lit::Null(_)) => true,
        Expr::Ident(identifier) => {
            identifier.sym == "undefined" && identifier.ctxt == unresolved_ctxt
        }
        Expr::Unary(unary) if unary.op == UnaryOp::Void => true,
        _ => false,
    }
}

fn is_nullish_or_callable_expression(expression: &Expr, unresolved_ctxt: SyntaxContext) -> bool {
    is_nullish_expression(expression, unresolved_ctxt)
        || matches!(
            strip_parentheses(expression),
            Expr::Ident(_) | Expr::Member(_) | Expr::Fn(_) | Expr::Arrow(_)
        )
}

fn optional_defer_template_index(
    arguments: &[Box<Expr>],
    index: usize,
    unresolved_ctxt: SyntaxContext,
) -> std::result::Result<Option<usize>, &'static str> {
    let Some(argument) = arguments.get(index) else {
        return Ok(None);
    };
    if is_nullish_expression(argument.as_ref(), unresolved_ctxt) {
        return Ok(None);
    }
    numeric_expr(argument.as_ref())
        .map(Some)
        .ok_or("defer child-template index is not numeric or null")
}

fn seed_local_reference_slots(
    calls: &[InstructionCall],
    tree: &mut TemplateTree,
    environment: &TemplateRecoveryEnvironment<'_>,
) {
    for call in calls {
        let reference_argument = match call.instruction {
            IvyInstruction::ElementStart | IvyInstruction::Element => {
                if call
                    .args
                    .get(1)
                    .and_then(|argument| string_lit(argument.as_ref()))
                    .is_none()
                {
                    continue;
                }
                3
            }
            IvyInstruction::ElementContainerStart | IvyInstruction::ElementContainer => 2,
            IvyInstruction::Template => {
                if call.args.get(1).is_none() {
                    continue;
                }
                6
            }
            _ => continue,
        };
        let Some(node_index) = numeric_arg(&call.args, 0) else {
            continue;
        };
        let Some(references) = local_references_for_call(call, reference_argument, environment)
        else {
            continue;
        };
        for (offset, name) in references.iter().enumerate() {
            tree.local_reference_slots
                .insert(node_index + offset + 1, name.clone());
        }
    }
}

fn seed_let_names(calls: &[InstructionCall]) -> HashMap<usize, String> {
    let mut cursor = 0usize;
    let mut names = HashMap::new();
    for call in calls {
        match call.instruction {
            IvyInstruction::Advance => {
                let amount = if call.args.is_empty() {
                    Some(1)
                } else if call.args.len() == 1 {
                    numeric_arg(&call.args, 0)
                } else {
                    None
                };
                if let Some(amount) = amount {
                    cursor = cursor.saturating_add(amount);
                }
            }
            IvyInstruction::StoreLet => {
                let binding = call
                    .result_binding
                    .as_ref()
                    .map(|binding| binding.0.as_ref());
                names.insert(cursor, recovered_let_name(binding, cursor));
            }
            _ => {}
        }
    }
    names
}

fn attach_local_references(
    call: &InstructionCall,
    node_index: usize,
    node: usize,
    reference_argument: usize,
    tree: &mut TemplateTree,
    environment: &TemplateRecoveryEnvironment<'_>,
) {
    let Some(references) = local_references_for_call(call, reference_argument, environment) else {
        return;
    };
    for (offset, name) in references.iter().enumerate() {
        tree.add_attribute(
            node,
            TemplateAttribute {
                name: format!("#{name}"),
                value: None,
            },
        );
        tree.local_reference_slots
            .insert(node_index + offset + 1, name.clone());
    }
}

fn local_references_for_call<'a>(
    call: &InstructionCall,
    reference_argument: usize,
    environment: &'a TemplateRecoveryEnvironment<'_>,
) -> Option<&'a [String]> {
    numeric_arg(&call.args, reference_argument)
        .and_then(|index| environment.constants.local_references.get(index))
        .map(Vec::as_slice)
}

fn apply_update_instruction(
    call: &InstructionCall,
    tree: &mut TemplateTree,
    program: &mut TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> Result<()> {
    match call.instruction {
        IvyInstruction::Advance => {
            let amount = if call.args.is_empty() {
                1
            } else {
                let Some(amount) = numeric_arg(&call.args, 0) else {
                    record_malformed_instruction(
                        call,
                        "advance amount is not a non-negative integer",
                        &mut program.issues,
                        &mut program.stats,
                    );
                    return Ok(());
                };
                amount
            };
            tree.cursor = tree.cursor.saturating_add(amount);
        }
        IvyInstruction::StoreLet => {
            if call.args.len() != 1 {
                record_malformed_instruction(
                    call,
                    "expected one let value",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Ok(value) =
                recover_template_expression(call.args[0].as_ref(), program, environment)
            else {
                record_malformed_instruction(
                    call,
                    "let expression could not be printed",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(&node) = tree.index_to_node.get(&tree.cursor) else {
                record_missing_target(
                    call,
                    &format!("no let declaration at cursor {}", tree.cursor),
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let TemplateNodeKind::Let {
                name,
                value: current,
                ..
            } = &mut tree.nodes[node].kind
            else {
                record_missing_target(
                    call,
                    &format!(
                        "cursor {} does not reference a let declaration",
                        tree.cursor
                    ),
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            *current = Some(value);
            if let Some(binding) = &call.result_binding {
                program
                    .local_reference_names
                    .insert(binding.clone(), name.clone());
            }
        }
        IvyInstruction::TextInterpolate
        | IvyInstruction::TextInterpolate1
        | IvyInstruction::TextInterpolate2
        | IvyInstruction::TextInterpolate3
        | IvyInstruction::TextInterpolate4
        | IvyInstruction::TextInterpolate5
        | IvyInstruction::TextInterpolate6
        | IvyInstruction::TextInterpolate7
        | IvyInstruction::TextInterpolate8 => {
            if !valid_interpolation_arity(call) {
                record_malformed_instruction(
                    call,
                    "unexpected interpolation argument count",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Ok(value) = interpolation_value(call, program, environment) else {
                record_malformed_instruction(
                    call,
                    "interpolation expression could not be printed",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(&node) = tree.index_to_node.get(&tree.cursor) else {
                record_missing_target(
                    call,
                    &format!("no text node at cursor {}", tree.cursor),
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let TemplateNodeKind::Text { value: current } = &mut tree.nodes[node].kind else {
                record_missing_target(
                    call,
                    &format!("cursor {} does not reference text", tree.cursor),
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            *current = value;
        }
        IvyInstruction::TwoWayProperty => {
            let Some(&node) = tree.index_to_node.get(&tree.cursor) else {
                record_missing_target(
                    call,
                    &format!("no element at cursor {}", tree.cursor),
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            if !matches!(call.args.len(), 2 | 3) {
                record_malformed_instruction(
                    call,
                    "expected a two-way property name, value, and optional sanitizer",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Some(name) = call.args.first().and_then(|arg| string_lit(arg.as_ref())) else {
                record_malformed_instruction(
                    call,
                    "two-way property name is not a literal string",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Ok(value) =
                recover_template_expression(call.args[1].as_ref(), program, environment)
            else {
                record_malformed_instruction(
                    call,
                    "two-way property expression could not be printed",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let expected_name = format!("[({name})]");
            let attributes = match &tree.nodes[node].kind {
                TemplateNodeKind::Element { attributes, .. }
                | TemplateNodeKind::EmbeddedView { attributes, .. }
                | TemplateNodeKind::Projection { attributes, .. } => attributes,
                _ => {
                    record_missing_target(
                        call,
                        &format!("cursor {} does not reference an element", tree.cursor),
                        &mut program.issues,
                        &mut program.stats,
                    );
                    return Ok(());
                }
            };
            if !attributes.iter().any(|attribute| {
                attribute.name == expected_name && attribute.value.as_deref() == Some(&value)
            }) {
                record_missing_target(
                    call,
                    "no matching two-way listener binding",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
        }
        IvyInstruction::ClassMap | IvyInstruction::StyleMap => {
            let Some(&node) = tree.index_to_node.get(&tree.cursor) else {
                record_missing_target(
                    call,
                    &format!("no element at cursor {}", tree.cursor),
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            if !matches!(
                tree.nodes[node].kind,
                TemplateNodeKind::Element { .. } | TemplateNodeKind::EmbeddedView { .. }
            ) {
                record_missing_target(
                    call,
                    &format!("cursor {} does not reference an element", tree.cursor),
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let [value] = call.args.as_slice() else {
                record_malformed_instruction(
                    call,
                    "expected one styling map value",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Ok(expression) = recover_template_expression(value.as_ref(), program, environment)
            else {
                record_malformed_instruction(
                    call,
                    "styling map expression could not be printed",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            tree.add_attribute(
                node,
                TemplateAttribute {
                    name: match call.instruction {
                        IvyInstruction::ClassMap => "[class]".to_string(),
                        IvyInstruction::StyleMap => "[style]".to_string(),
                        _ => unreachable!(),
                    },
                    value: Some(expression),
                },
            );
        }
        IvyInstruction::Property
        | IvyInstruction::AriaProperty
        | IvyInstruction::Attribute
        | IvyInstruction::ClassProp
        | IvyInstruction::StyleProp => {
            let Some(&node) = tree.index_to_node.get(&tree.cursor) else {
                record_missing_target(
                    call,
                    &format!("no element at cursor {}", tree.cursor),
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            if !matches!(
                tree.nodes[node].kind,
                TemplateNodeKind::Element { .. } | TemplateNodeKind::EmbeddedView { .. }
            ) {
                record_missing_target(
                    call,
                    &format!("cursor {} does not reference an element", tree.cursor),
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            if call.instruction == IvyInstruction::AriaProperty
                && (call.args.len() != 2
                    || !call
                        .args
                        .first()
                        .and_then(|argument| string_lit(argument.as_ref()))
                        .is_some_and(|name| name.starts_with("aria-")))
            {
                record_malformed_instruction(
                    call,
                    "expected an ARIA name and one binding value",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Some(name) = call.args.first().and_then(|arg| string_lit(arg.as_ref())) else {
                record_malformed_instruction(
                    call,
                    "missing literal binding name",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(value) = call.args.get(1) else {
                record_malformed_instruction(
                    call,
                    "missing binding value",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Ok(expression) = recover_template_expression(value.as_ref(), program, environment)
            else {
                record_malformed_instruction(
                    call,
                    "binding expression could not be printed",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let prefix = match call.instruction {
                IvyInstruction::Property | IvyInstruction::AriaProperty => "",
                IvyInstruction::Attribute => "attr.",
                IvyInstruction::ClassProp => "class.",
                IvyInstruction::StyleProp => "style.",
                _ => unreachable!(),
            };
            let suffix = if call.instruction == IvyInstruction::StyleProp {
                match call.args.get(2) {
                    Some(suffix) => {
                        let Some(suffix) = string_lit(suffix.as_ref()) else {
                            record_malformed_instruction(
                                call,
                                "style unit suffix is not a literal string",
                                &mut program.issues,
                                &mut program.stats,
                            );
                            return Ok(());
                        };
                        format!(".{suffix}")
                    }
                    None => String::new(),
                }
            } else {
                String::new()
            };
            tree.add_attribute(
                node,
                TemplateAttribute {
                    name: format!("[{prefix}{name}{suffix}]"),
                    value: Some(expression),
                },
            );
        }
        IvyInstruction::I18nExp => {
            if call.args.len() != 1 {
                record_malformed_instruction(
                    call,
                    "expected one i18n binding expression",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Ok(expression) =
                recover_template_expression(call.args[0].as_ref(), program, environment)
            else {
                record_malformed_instruction(
                    call,
                    "i18n binding expression could not be printed",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            program.pending_i18n_expressions.push(expression);
        }
        IvyInstruction::I18nApply => {
            if call.args.len() != 1 {
                record_malformed_instruction(
                    call,
                    "expected one i18n node index",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let Some(index) = numeric_arg(&call.args, 0) else {
                record_malformed_instruction(
                    call,
                    "i18n node index is not numeric",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let Some(&node) = tree.index_to_node.get(&index) else {
                record_missing_target(
                    call,
                    &format!("no i18n node at index {index}"),
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            match &mut tree.nodes[node].kind {
                TemplateNodeKind::Text { value } => {
                    let Some(rendered) =
                        render_basic_i18n_message(value, &program.pending_i18n_expressions)
                    else {
                        record_malformed_instruction(
                            call,
                            "i18n placeholders do not match the collected binding expressions",
                            &mut program.issues,
                            &mut program.stats,
                        );
                        program.pending_i18n_expressions.clear();
                        return Ok(());
                    };
                    *value = rendered;
                }
                TemplateNodeKind::I18nRegion {
                    tokens,
                    expressions,
                } => {
                    if tokens.iter().any(|token| {
                        matches!(
                            token,
                            I18nToken::Interpolation(index)
                                if program.pending_i18n_expressions.get(*index).is_none()
                        )
                    }) {
                        record_malformed_instruction(
                            call,
                            "i18n placeholders do not match the collected binding expressions",
                            &mut program.issues,
                            &mut program.stats,
                        );
                        program.pending_i18n_expressions.clear();
                        return Ok(());
                    }
                    expressions.clone_from(&program.pending_i18n_expressions);
                }
                _ => {
                    record_missing_target(
                        call,
                        &format!("i18n index {index} does not reference an i18n node"),
                        &mut program.issues,
                        &mut program.stats,
                    );
                    return Ok(());
                }
            }
            program.pending_i18n_expressions.clear();
        }
        IvyInstruction::Conditional => {
            if !apply_conditional_instruction(call, tree, environment.cm.clone(), program) {
                return Ok(());
            }
        }
        IvyInstruction::Repeater => {
            if call.args.len() != 1 {
                record_malformed_instruction(
                    call,
                    "expected one repeater collection",
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            }
            let collection =
                match recover_template_expression(call.args[0].as_ref(), program, environment) {
                    Ok(collection) => collection,
                    Err(_) => {
                        record_malformed_instruction(
                            call,
                            "repeater collection could not be printed",
                            &mut program.issues,
                            &mut program.stats,
                        );
                        return Ok(());
                    }
                };
            let Some(&node) = tree.index_to_node.get(&tree.cursor) else {
                record_missing_target(
                    call,
                    &format!("no repeater at cursor {}", tree.cursor),
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            let TemplateNodeKind::Repeater {
                collection: current,
                ..
            } = &mut tree.nodes[node].kind
            else {
                record_missing_target(
                    call,
                    &format!("cursor {} does not reference a repeater", tree.cursor),
                    &mut program.issues,
                    &mut program.stats,
                );
                return Ok(());
            };
            *current = Some(collection);
        }
        _ => {
            record_unsupported_instruction(
                call,
                "unsupported in update phase",
                &mut program.issues,
                &mut program.stats,
            );
            return Ok(());
        }
    }
    program.stats.rendered_instruction_calls += 1;
    Ok(())
}

fn merge_template_program(parent: &mut TemplateProgram, child: TemplateProgram) {
    parent.let_alias_hints.extend(
        child
            .let_alias_hints
            .iter()
            .filter(|hint| hint.context_depth > 0)
            .map(|hint| ViewLetAliasHint {
                context_depth: hint.context_depth - 1,
                slot: hint.slot,
                name: hint.name.clone(),
            }),
    );
    for issue in child.issues {
        record_issue(&mut parent.issues, issue);
    }
    parent.stats.runtime_calls_observed += child.stats.runtime_calls_observed;
    parent.stats.rendered_instruction_calls += child.stats.rendered_instruction_calls;
    parent.stats.unsupported_runtime_calls += child.stats.unsupported_runtime_calls;
    parent.stats.malformed_instruction_calls += child.stats.malformed_instruction_calls;
    for (shape, (occurrences, runtime_calls)) in child.unknown_runtime_call_shapes {
        let aggregate = parent.unknown_runtime_call_shapes.entry(shape).or_default();
        aggregate.0 += occurrences;
        aggregate.1 += runtime_calls;
    }
    parent.artifact_references.extend(child.artifact_references);
    parent.listener_methods.extend(child.listener_methods);
}

fn apply_conditional_instruction(
    call: &InstructionCall,
    tree: &mut TemplateTree,
    cm: Lrc<SourceMap>,
    program: &mut TemplateProgram,
) -> bool {
    if !matches!(call.args.len(), 1 | 2) {
        record_malformed_instruction(
            call,
            "expected a template selection and optional context value",
            &mut program.issues,
            &mut program.stats,
        );
        return false;
    }
    let Some(selection) = call.args.first() else {
        record_malformed_instruction(
            call,
            "missing template selection",
            &mut program.issues,
            &mut program.stats,
        );
        return false;
    };
    let Some(branches) = decode_conditional_branches(
        selection.as_ref(),
        &program.component_contexts,
        &program.local_reference_names,
        &program.local_context_bindings,
        cm,
    ) else {
        record_malformed_instruction(
            call,
            "unsupported conditional template selection",
            &mut program.issues,
            &mut program.stats,
        );
        return false;
    };

    let mut resolved = Vec::with_capacity(branches.len());
    for (index, branch) in branches {
        let Some(&node) = tree.index_to_node.get(&index) else {
            record_missing_target(
                call,
                &format!("no embedded template at index {index}"),
                &mut program.issues,
                &mut program.stats,
            );
            return false;
        };
        if !matches!(tree.nodes[node].kind, TemplateNodeKind::EmbeddedView { .. }) {
            record_missing_target(
                call,
                &format!("index {index} does not reference an embedded template"),
                &mut program.issues,
                &mut program.stats,
            );
            return false;
        }
        resolved.push((node, branch));
    }

    let points = resolved
        .iter()
        .map(|(node, _)| tree.insertion_point_before_node(*node))
        .collect::<Option<Vec<_>>>();
    let Some(points) = points else {
        record_malformed_instruction(
            call,
            "conditional branch template has no insertion point",
            &mut program.issues,
            &mut program.stats,
        );
        return false;
    };
    let parent = points.first().and_then(|point| point.parent);
    if points.iter().any(|point| point.parent != parent) {
        record_malformed_instruction(
            call,
            "conditional branch templates are not siblings",
            &mut program.issues,
            &mut program.stats,
        );
        return false;
    }
    let mut positions = points
        .iter()
        .map(|point| point.position)
        .collect::<Vec<_>>();
    positions.sort_unstable();
    let ordered_nodes = resolved.iter().map(|(node, _)| *node).collect::<Vec<_>>();
    let siblings = match parent {
        Some(parent) => &mut tree.nodes[parent].children,
        None => &mut tree.roots,
    };
    for (position, node) in positions.into_iter().zip(ordered_nodes) {
        siblings[position] = node;
    }
    for (node, branch) in resolved {
        let TemplateNodeKind::EmbeddedView {
            branch: node_branch,
            ..
        } = &mut tree.nodes[node].kind
        else {
            unreachable!("conditional branch kinds were validated before mutation");
        };
        *node_branch = Some(branch);
    }
    program
        .artifact_references
        .extend(expression_references(selection.as_ref()));
    true
}

fn decode_conditional_branches(
    selection: &Expr,
    component_contexts: &HashSet<BindingKey>,
    local_references: &HashMap<BindingKey, String>,
    local_contexts: &HashMap<BindingKey, HashMap<String, String>>,
    cm: Lrc<SourceMap>,
) -> Option<Vec<(usize, ConditionalBranch)>> {
    struct Leaf {
        index: isize,
        conditions: Vec<(String, bool)>,
    }

    fn collect_leaves(
        selection: &Expr,
        conditions: &mut Vec<(String, bool)>,
        component_contexts: &HashSet<BindingKey>,
        local_references: &HashMap<BindingKey, String>,
        local_contexts: &HashMap<BindingKey, HashMap<String, String>>,
        cm: Lrc<SourceMap>,
        leaves: &mut Vec<Leaf>,
    ) -> Option<()> {
        let Expr::Cond(conditional) = strip_parentheses(selection) else {
            let index = signed_integer(selection)?;
            if index < -1 {
                return None;
            }
            leaves.push(Leaf {
                index,
                conditions: conditions.clone(),
            });
            return Some(());
        };
        let condition = print_template_expression_with_aliases(
            conditional.test.as_ref(),
            component_contexts,
            local_references,
            &HashMap::new(),
            local_contexts,
            cm.clone(),
        )
        .ok()?;
        conditions.push((condition.clone(), true));
        collect_leaves(
            conditional.cons.as_ref(),
            conditions,
            component_contexts,
            local_references,
            local_contexts,
            cm.clone(),
            leaves,
        )?;
        conditions.pop();
        conditions.push((condition, false));
        collect_leaves(
            conditional.alt.as_ref(),
            conditions,
            component_contexts,
            local_references,
            local_contexts,
            cm,
            leaves,
        )?;
        conditions.pop();
        Some(())
    }

    fn render_conditions(conditions: &[(String, bool)]) -> Option<String> {
        if conditions.len() == 1 {
            let (condition, positive) = &conditions[0];
            return Some(if *positive {
                condition.clone()
            } else {
                format!("!({condition})")
            });
        }
        (!conditions.is_empty()).then(|| {
            conditions
                .iter()
                .map(|(condition, positive)| {
                    if *positive {
                        format!("({condition})")
                    } else {
                        format!("!({condition})")
                    }
                })
                .collect::<Vec<_>>()
                .join(" && ")
        })
    }

    let mut leaves = Vec::new();
    collect_leaves(
        strip_parentheses(selection),
        &mut Vec::new(),
        component_contexts,
        local_references,
        local_contexts,
        cm,
        &mut leaves,
    )?;
    let has_omitted_leaf = leaves.iter().any(|leaf| leaf.index == -1);
    let visible = leaves
        .iter()
        .filter(|leaf| leaf.index >= 0)
        .collect::<Vec<_>>();
    let mut indices = HashSet::new();
    if visible.is_empty()
        || visible
            .iter()
            .any(|leaf| !indices.insert(leaf.index as usize))
    {
        return None;
    }
    let last = visible.len().saturating_sub(1);
    visible
        .into_iter()
        .enumerate()
        .map(|(position, leaf)| {
            let index = leaf.index as usize;
            let condition = render_conditions(&leaf.conditions)?;
            let branch = if position == 0 {
                ConditionalBranch::If(condition)
            } else if !has_omitted_leaf && position == last {
                ConditionalBranch::Else
            } else {
                ConditionalBranch::ElseIf(condition)
            };
            Some((index, branch))
        })
        .collect()
}

fn signed_integer(expression: &Expr) -> Option<isize> {
    match strip_parentheses(expression) {
        Expr::Lit(Lit::Num(number))
            if number.value.fract() == 0.0
                && number.value >= isize::MIN as f64
                && number.value <= isize::MAX as f64 =>
        {
            Some(number.value as isize)
        }
        Expr::Unary(unary) if unary.op == UnaryOp::Minus => {
            let Expr::Lit(Lit::Num(number)) = strip_parentheses(unary.arg.as_ref()) else {
                return None;
            };
            (number.value.fract() == 0.0 && number.value <= isize::MAX as f64)
                .then_some(-(number.value as isize))
        }
        _ => None,
    }
}

fn record_malformed_instruction(
    call: &InstructionCall,
    detail: &str,
    issues: &mut Vec<AngularRecoveryIssue>,
    stats: &mut AngularTemplateRecoveryStats,
) {
    stats.malformed_instruction_calls += 1;
    record_issue(
        issues,
        issue_at_operation(
            issue(
                AngularRecoveryIssueKind::MalformedInstruction,
                Some(call.instruction.canonical_export_name().to_string()),
                Some(detail.to_string()),
            ),
            &call.provenance,
        ),
    );
}

fn record_missing_target(
    call: &InstructionCall,
    detail: &str,
    issues: &mut Vec<AngularRecoveryIssue>,
    stats: &mut AngularTemplateRecoveryStats,
) {
    stats.malformed_instruction_calls += 1;
    record_issue(
        issues,
        issue_at_operation(
            issue(
                AngularRecoveryIssueKind::MissingTargetNode,
                Some(call.instruction.canonical_export_name().to_string()),
                Some(detail.to_string()),
            ),
            &call.provenance,
        ),
    );
}

fn record_unsupported_instruction(
    call: &InstructionCall,
    detail: &str,
    issues: &mut Vec<AngularRecoveryIssue>,
    stats: &mut AngularTemplateRecoveryStats,
) {
    stats.unsupported_runtime_calls += 1;
    record_issue(
        issues,
        issue_at_operation(
            issue(
                AngularRecoveryIssueKind::UnsupportedInstruction,
                Some(call.instruction.canonical_export_name().to_string()),
                Some(detail.to_string()),
            ),
            &call.provenance,
        ),
    );
}

fn valid_interpolation_arity(call: &InstructionCall) -> bool {
    let expected = match call.instruction {
        IvyInstruction::TextInterpolate => return !call.args.is_empty(),
        IvyInstruction::TextInterpolate1 => 3,
        IvyInstruction::TextInterpolate2 => 5,
        IvyInstruction::TextInterpolate3 => 7,
        IvyInstruction::TextInterpolate4 => 9,
        IvyInstruction::TextInterpolate5 => 11,
        IvyInstruction::TextInterpolate6 => 13,
        IvyInstruction::TextInterpolate7 => 15,
        IvyInstruction::TextInterpolate8 => 17,
        _ => return false,
    };
    matches!(call.args.len(), length if length == expected - 1 || length == expected)
}

fn interpolation_value(
    call: &InstructionCall,
    program: &mut TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> Result<String> {
    if call.instruction == IvyInstruction::TextInterpolate {
        let expression = call
            .args
            .first()
            .map(|expr| recover_template_expression(expr.as_ref(), program, environment))
            .transpose()?
            .unwrap_or_default();
        return Ok(format!("{{{{ {expression} }}}}"));
    }

    let mut output = String::new();
    for (index, argument) in call.args.iter().enumerate() {
        if index % 2 == 0 {
            output.push_str(&string_lit(argument.as_ref()).unwrap_or_default());
        } else {
            let expression = recover_template_expression(argument.as_ref(), program, environment)?;
            output.push_str("{{ ");
            output.push_str(&expression);
            output.push_str(" }}");
        }
    }
    Ok(output)
}

fn recover_template_expression(
    expression: &Expr,
    program: &mut TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> Result<String> {
    if let Expr::Call(call) = strip_parentheses(expression) {
        if let Some((root, argument_lists)) = call_chain(call) {
            if let Some(instruction) = environment
                .roles
                .instruction_for_expr(root, environment.unresolved_ctxt)
                .filter(|instruction| is_pipe_binding(*instruction))
            {
                program.stats.runtime_calls_observed += argument_lists.len();
                let mut provenances = call_provenances(
                    program,
                    Some(2),
                    call,
                    root,
                    argument_lists.len(),
                    environment,
                );
                if argument_lists.len() != 1 {
                    program.stats.malformed_instruction_calls += argument_lists.len();
                    for provenance in provenances {
                        record_issue(
                            &mut program.issues,
                            issue_at_operation(
                                issue(
                                    AngularRecoveryIssueKind::MalformedInstruction,
                                    Some(instruction.canonical_export_name().to_string()),
                                    Some("unexpected chained pipe binding".to_string()),
                                ),
                                &provenance,
                            ),
                        );
                    }
                    return Err(anyhow!("unexpected chained Ivy pipe binding"));
                }
                let nested = InstructionCall {
                    instruction,
                    args: argument_lists[0]
                        .iter()
                        .map(|argument| argument.expr.clone())
                        .collect(),
                    result_binding: None,
                    provenance: provenances
                        .pop()
                        .expect("a call chain always contains one invocation"),
                };
                let Some(values) = pipe_binding_values(&nested) else {
                    record_malformed_instruction(
                        &nested,
                        "unexpected pipe-binding arguments",
                        &mut program.issues,
                        &mut program.stats,
                    );
                    return Err(anyhow!("malformed Ivy pipe binding"));
                };
                let Some(pipe_slot) = numeric_arg(&nested.args, 0) else {
                    record_malformed_instruction(
                        &nested,
                        "missing numeric pipe slot",
                        &mut program.issues,
                        &mut program.stats,
                    );
                    return Err(anyhow!("missing Ivy pipe slot"));
                };
                let Some(pipe_name) = program.pipes.get(&pipe_slot).cloned() else {
                    record_missing_target(
                        &nested,
                        &format!("no pipe at slot {pipe_slot}"),
                        &mut program.issues,
                        &mut program.stats,
                    );
                    return Err(anyhow!("missing Ivy pipe declaration"));
                };
                let mut values = values.into_iter();
                let Some(input) = values.next() else {
                    record_malformed_instruction(
                        &nested,
                        "pipe binding has no input value",
                        &mut program.issues,
                        &mut program.stats,
                    );
                    return Err(anyhow!("missing Ivy pipe input"));
                };
                let mut output = recover_template_expression(input.as_ref(), program, environment)?;
                output.push_str(" | ");
                output.push_str(&pipe_name);
                for argument in values {
                    output.push_str(": ");
                    output.push_str(&recover_template_expression(
                        argument.as_ref(),
                        program,
                        environment,
                    )?);
                }
                program.stats.rendered_instruction_calls += 1;
                return Ok(output);
            }

            if let Some(instruction) = environment
                .roles
                .instruction_for_expr(root, environment.unresolved_ctxt)
                .filter(|instruction| is_pure_function(*instruction))
            {
                program.stats.runtime_calls_observed += argument_lists.len();
                let mut provenances = call_provenances(
                    program,
                    Some(2),
                    call,
                    root,
                    argument_lists.len(),
                    environment,
                );
                if argument_lists.len() != 1 {
                    program.stats.malformed_instruction_calls += argument_lists.len();
                    for provenance in provenances {
                        record_issue(
                            &mut program.issues,
                            issue_at_operation(
                                issue(
                                    AngularRecoveryIssueKind::MalformedInstruction,
                                    Some(instruction.canonical_export_name().to_string()),
                                    Some("unexpected chained pure-function binding".to_string()),
                                ),
                                &provenance,
                            ),
                        );
                    }
                    return Err(anyhow!("unexpected chained Ivy pure function"));
                }
                let nested = InstructionCall {
                    instruction,
                    args: argument_lists[0]
                        .iter()
                        .map(|argument| argument.expr.clone())
                        .collect(),
                    result_binding: None,
                    provenance: provenances
                        .pop()
                        .expect("a call chain always contains one invocation"),
                };
                let Some(values) = pure_function_values(&nested) else {
                    record_malformed_instruction(
                        &nested,
                        "unexpected pure-function arguments",
                        &mut program.issues,
                        &mut program.stats,
                    );
                    return Err(anyhow!("malformed Ivy pure function"));
                };
                let Some(callback) = nested.args.get(1) else {
                    record_malformed_instruction(
                        &nested,
                        "missing pure-function callback",
                        &mut program.issues,
                        &mut program.stats,
                    );
                    return Err(anyhow!("missing Ivy pure-function callback"));
                };
                let expanded = match expand_pure_function(
                    callback.as_ref(),
                    &values,
                    environment.template_functions,
                ) {
                    Ok(expanded) => expanded,
                    Err(detail) => {
                        record_malformed_instruction(
                            &nested,
                            &detail,
                            &mut program.issues,
                            &mut program.stats,
                        );
                        return Err(anyhow!("unresolved Ivy pure-function callback"));
                    }
                };
                let output = recover_template_expression(expanded.as_ref(), program, environment)?;
                program.stats.rendered_instruction_calls += 1;
                return Ok(output);
            }

            if let Some(instruction) = environment
                .roles
                .instruction_for_expr(root, environment.unresolved_ctxt)
                .filter(|instruction| is_expression_interpolation(*instruction))
            {
                program.stats.runtime_calls_observed += argument_lists.len();
                let mut provenances = call_provenances(
                    program,
                    Some(2),
                    call,
                    root,
                    argument_lists.len(),
                    environment,
                );
                if argument_lists.len() != 1 {
                    program.stats.malformed_instruction_calls += argument_lists.len();
                    for provenance in provenances {
                        record_issue(
                            &mut program.issues,
                            issue_at_operation(
                                issue(
                                    AngularRecoveryIssueKind::MalformedInstruction,
                                    Some(instruction.canonical_export_name().to_string()),
                                    Some("unexpected chained expression interpolation".to_string()),
                                ),
                                &provenance,
                            ),
                        );
                    }
                    return Err(anyhow!("unexpected chained Ivy interpolation"));
                }
                let nested = InstructionCall {
                    instruction,
                    args: argument_lists[0]
                        .iter()
                        .map(|argument| argument.expr.clone())
                        .collect(),
                    result_binding: None,
                    provenance: provenances
                        .pop()
                        .expect("a call chain always contains one invocation"),
                };
                let output = match expression_interpolation_value(&nested, program, environment) {
                    Ok(output) => output,
                    Err(detail) => {
                        record_malformed_instruction(
                            &nested,
                            &detail,
                            &mut program.issues,
                            &mut program.stats,
                        );
                        return Err(anyhow!("malformed Ivy expression interpolation"));
                    }
                };
                program.stats.rendered_instruction_calls += 1;
                return Ok(output);
            }

            if let Some(name) = environment
                .roles
                .ivy_name_for_expr(root, environment.unresolved_ctxt)
            {
                program.stats.runtime_calls_observed += argument_lists.len();
                program.stats.unsupported_runtime_calls += argument_lists.len();
                for provenance in call_provenances(
                    program,
                    Some(2),
                    call,
                    root,
                    argument_lists.len(),
                    environment,
                ) {
                    record_issue(
                        &mut program.issues,
                        issue_at_operation(
                            issue(
                                AngularRecoveryIssueKind::UnsupportedInstruction,
                                Some(name.clone()),
                                Some("nested in a template expression".to_string()),
                            ),
                            &provenance,
                        ),
                    );
                }
                return Err(anyhow!("unsupported nested Ivy instruction"));
            }
        }
    }

    let printed = print_template_expression_with_aliases(
        expression,
        &program.component_contexts,
        &program.local_reference_names,
        &HashMap::new(),
        &program.local_context_bindings,
        environment.cm.clone(),
    )?;
    program
        .artifact_references
        .extend(expression_references(expression));
    Ok(printed)
}

fn is_pipe_binding(instruction: IvyInstruction) -> bool {
    matches!(
        instruction,
        IvyInstruction::PipeBind1
            | IvyInstruction::PipeBind2
            | IvyInstruction::PipeBind3
            | IvyInstruction::PipeBind4
            | IvyInstruction::PipeBindV
    )
}

fn is_pure_function(instruction: IvyInstruction) -> bool {
    matches!(
        instruction,
        IvyInstruction::PureFunction0
            | IvyInstruction::PureFunction1
            | IvyInstruction::PureFunction2
            | IvyInstruction::PureFunction3
            | IvyInstruction::PureFunction4
            | IvyInstruction::PureFunction5
            | IvyInstruction::PureFunction6
            | IvyInstruction::PureFunction7
            | IvyInstruction::PureFunction8
            | IvyInstruction::PureFunctionV
    )
}

fn is_expression_interpolation(instruction: IvyInstruction) -> bool {
    matches!(
        instruction,
        IvyInstruction::Interpolate
            | IvyInstruction::Interpolate1
            | IvyInstruction::Interpolate2
            | IvyInstruction::Interpolate3
            | IvyInstruction::Interpolate4
            | IvyInstruction::Interpolate5
            | IvyInstruction::Interpolate6
            | IvyInstruction::Interpolate7
            | IvyInstruction::Interpolate8
            | IvyInstruction::InterpolateV
    )
}

fn expression_interpolation_value(
    call: &InstructionCall,
    program: &mut TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> std::result::Result<String, String> {
    if call.instruction == IvyInstruction::Interpolate {
        let [value] = call.args.as_slice() else {
            return Err("unexpected ɵɵinterpolate arguments".to_string());
        };
        let value = recover_template_expression(value.as_ref(), program, environment)
            .map_err(|_| "interpolation expression could not be printed".to_string())?;
        return Ok(format!("`${{{value}}}`"));
    }

    let expected_values = match call.instruction {
        IvyInstruction::Interpolate1 => Some(1),
        IvyInstruction::Interpolate2 => Some(2),
        IvyInstruction::Interpolate3 => Some(3),
        IvyInstruction::Interpolate4 => Some(4),
        IvyInstruction::Interpolate5 => Some(5),
        IvyInstruction::Interpolate6 => Some(6),
        IvyInstruction::Interpolate7 => Some(7),
        IvyInstruction::Interpolate8 => Some(8),
        IvyInstruction::InterpolateV => None,
        _ => return Err("not an Ivy expression interpolation".to_string()),
    };
    let arguments = if call.instruction == IvyInstruction::InterpolateV {
        let [values] = call.args.as_slice() else {
            return Err("unexpected ɵɵinterpolateV arguments".to_string());
        };
        let Expr::Array(values) = strip_parentheses(values.as_ref()) else {
            return Err("ɵɵinterpolateV values are not an array".to_string());
        };
        values
            .elems
            .iter()
            .map(|value| {
                value
                    .as_ref()
                    .filter(|value| value.spread.is_none())
                    .map(|value| value.expr.clone())
                    .ok_or_else(|| "ɵɵinterpolateV contains a hole or spread".to_string())
            })
            .collect::<std::result::Result<Vec<_>, _>>()?
    } else {
        call.args.clone()
    };
    if let Some(value_count) = expected_values {
        if !matches!(
            arguments.len(),
            length if length == value_count * 2 || length == value_count * 2 + 1
        ) {
            return Err("unexpected expression-interpolation arity".to_string());
        }
    } else if arguments.len() < 3 || arguments.len() % 2 == 0 {
        return Err("unexpected ɵɵinterpolateV value layout".to_string());
    }

    let mut output = String::from("`");
    for (index, argument) in arguments.iter().enumerate() {
        if index % 2 == 0 {
            let Some(text) = string_lit(argument.as_ref()) else {
                return Err("interpolation static segment is not a string".to_string());
            };
            push_template_literal_text(&mut output, &text);
        } else {
            let value = recover_template_expression(argument.as_ref(), program, environment)
                .map_err(|_| "interpolation expression could not be printed".to_string())?;
            output.push_str("${");
            output.push_str(&value);
            output.push('}');
        }
    }
    output.push('`');
    Ok(output)
}

fn push_template_literal_text(output: &mut String, text: &str) {
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' => output.push_str("\\\\"),
            '`' => output.push_str("\\`"),
            '$' if characters.peek() == Some(&'{') => output.push_str("\\$"),
            '\r' => output.push_str("\\r"),
            '\n' => output.push_str("\\n"),
            '\u{2028}' => output.push_str("\\u2028"),
            '\u{2029}' => output.push_str("\\u2029"),
            character => output.push(character),
        }
    }
}

fn pure_function_values(call: &InstructionCall) -> Option<Vec<Box<Expr>>> {
    let value_count = match call.instruction {
        IvyInstruction::PureFunction0 => 0,
        IvyInstruction::PureFunction1 => 1,
        IvyInstruction::PureFunction2 => 2,
        IvyInstruction::PureFunction3 => 3,
        IvyInstruction::PureFunction4 => 4,
        IvyInstruction::PureFunction5 => 5,
        IvyInstruction::PureFunction6 => 6,
        IvyInstruction::PureFunction7 => 7,
        IvyInstruction::PureFunction8 => 8,
        IvyInstruction::PureFunctionV => {
            if call.args.len() != 3 || numeric_arg(&call.args, 0).is_none() {
                return None;
            }
            let Expr::Array(values) = strip_parentheses(call.args[2].as_ref()) else {
                return None;
            };
            return values
                .elems
                .iter()
                .map(|value| value.as_ref().map(|value| value.expr.clone()))
                .collect();
        }
        _ => return None,
    };
    (call.args.len() == value_count + 2 && numeric_arg(&call.args, 0).is_some())
        .then(|| call.args[2..].to_vec())
}

fn expand_pure_function(
    callback: &Expr,
    values: &[Box<Expr>],
    template_functions: &TemplateFunctionTable,
) -> std::result::Result<Box<Expr>, String> {
    fn function_body(function: &Function) -> Option<(Vec<BindingKey>, Box<Expr>)> {
        if function.is_async || function.is_generator {
            return None;
        }
        let parameters = function
            .params
            .iter()
            .map(|parameter| {
                let Pat::Ident(binding) = &parameter.pat else {
                    return None;
                };
                Some(binding_key(&binding.id))
            })
            .collect::<Option<Vec<_>>>()?;
        let body = function.body.as_ref().and_then(single_return_value)?;
        Some((parameters, Box::new(body.clone())))
    }

    let resolved = template_functions.resolve_expression(callback);
    let candidate = match strip_parentheses(resolved.as_ref()) {
        Expr::Arrow(arrow) if !arrow.is_async => {
            let parameters = arrow
                .params
                .iter()
                .map(|parameter| {
                    let Pat::Ident(binding) = parameter else {
                        return None;
                    };
                    Some(binding_key(&binding.id))
                })
                .collect::<Option<Vec<_>>>();
            let body = match arrow.body.as_ref() {
                BlockStmtOrExpr::Expr(expression) => Some(expression.clone()),
                BlockStmtOrExpr::BlockStmt(block) => {
                    single_return_value(block).map(|expression| Box::new(expression.clone()))
                }
            };
            parameters.zip(body)
        }
        Expr::Fn(function) => function_body(function.function.as_ref()),
        _ => template_functions
            .resolve(callback)
            .and_then(|resolved| function_body(&resolved.function)),
    }
    .ok_or_else(|| "pure-function callback is not a single expression".to_string())?;
    let (parameters, mut body) = candidate;
    if parameters.len() != values.len() {
        return Err("pure-function callback parameter count does not match values".to_string());
    }
    let aliases = parameters
        .into_iter()
        .zip(values.iter().cloned())
        .collect::<HashMap<_, _>>();
    body.visit_mut_with(&mut PureFunctionParameterSubstituter { aliases: &aliases });
    Ok(body)
}

struct PureFunctionParameterSubstituter<'a> {
    aliases: &'a HashMap<BindingKey, Box<Expr>>,
}

impl VisitMut for PureFunctionParameterSubstituter<'_> {
    fn visit_mut_expr(&mut self, expression: &mut Expr) {
        if let Expr::Ident(identifier) = expression {
            if let Some(alias) = self.aliases.get(&binding_key(identifier)) {
                *expression = alias.as_ref().clone();
                return;
            }
        }
        expression.visit_mut_children_with(self);
    }
}

fn pipe_binding_values(call: &InstructionCall) -> Option<Vec<Box<Expr>>> {
    let fixed_values = match call.instruction {
        IvyInstruction::PipeBind1 => 1,
        IvyInstruction::PipeBind2 => 2,
        IvyInstruction::PipeBind3 => 3,
        IvyInstruction::PipeBind4 => 4,
        IvyInstruction::PipeBindV => {
            if call.args.len() != 3 {
                return None;
            }
            let Expr::Array(values) = call.args[2].as_ref() else {
                return None;
            };
            return values
                .elems
                .iter()
                .map(|value| value.as_ref().map(|value| value.expr.clone()))
                .collect();
        }
        _ => return None,
    };
    (call.args.len() == fixed_values + 2).then(|| call.args[2..].to_vec())
}

fn numeric_arg(args: &[Box<Expr>], index: usize) -> Option<usize> {
    numeric_expr(args.get(index)?.as_ref())
}

fn numeric_expr(expression: &Expr) -> Option<usize> {
    let Expr::Lit(Lit::Num(number)) = strip_parentheses(expression) else {
        return None;
    };
    (number.value >= 0.0 && number.value.fract() == 0.0).then_some(number.value as usize)
}

fn decode_component_constant_table(constants: &Expr) -> TemplateConstants {
    let Some(decoded) = decode_component_constant_entries(constants) else {
        return TemplateConstants::default();
    };
    TemplateConstants {
        attributes: decoded
            .entries
            .iter()
            .map(|entry| {
                entry
                    .as_deref()
                    .and_then(|entry| {
                        resolve_constant_expression(entry, &decoded.values, &mut HashSet::new())
                    })
                    .map(decode_constant_attributes)
                    .unwrap_or_default()
            })
            .collect(),
        local_references: decoded
            .entries
            .iter()
            .map(|entry| {
                entry
                    .as_deref()
                    .and_then(|entry| {
                        resolve_constant_expression(entry, &decoded.values, &mut HashSet::new())
                    })
                    .and_then(decode_local_references)
                    .unwrap_or_default()
            })
            .collect(),
        i18n_messages: decoded
            .entries
            .iter()
            .map(|entry| {
                entry.as_deref().and_then(|entry| {
                    decode_i18n_message_expression(
                        entry,
                        &decoded.values,
                        &mut HashSet::new(),
                        decoded.allow_unnamed_localizer,
                    )
                })
            })
            .collect(),
    }
}

struct DecodedComponentConstantEntries {
    entries: Vec<Option<Box<Expr>>>,
    values: HashMap<BindingKey, Box<Expr>>,
    allow_unnamed_localizer: bool,
}

fn decode_component_constant_entries(expression: &Expr) -> Option<DecodedComponentConstantEntries> {
    if let Expr::Array(array) = strip_parentheses(expression) {
        return Some(DecodedComponentConstantEntries {
            entries: array
                .elems
                .iter()
                .map(|element| element.as_ref().map(|element| element.expr.clone()))
                .collect(),
            values: HashMap::new(),
            allow_unnamed_localizer: false,
        });
    }

    let body = match strip_parentheses(expression) {
        Expr::Fn(function) if function.function.params.is_empty() => {
            function.function.body.as_ref()?
        }
        Expr::Arrow(arrow) => match arrow.body.as_ref() {
            BlockStmtOrExpr::BlockStmt(body) if arrow.params.is_empty() => body,
            BlockStmtOrExpr::Expr(expression) => {
                let Expr::Array(array) = strip_parentheses(expression.as_ref()) else {
                    return None;
                };
                return Some(DecodedComponentConstantEntries {
                    entries: array
                        .elems
                        .iter()
                        .map(|element| element.as_ref().map(|element| element.expr.clone()))
                        .collect(),
                    values: HashMap::new(),
                    allow_unnamed_localizer: true,
                });
            }
            _ => return None,
        },
        _ => return None,
    };

    let mut collector = ComponentConstantFactoryCollector::default();
    body.visit_with(&mut collector);
    let returned = collector.returns.last()?.as_ref();
    let returned = match strip_parentheses(returned) {
        Expr::Seq(sequence) => sequence.exprs.last()?.as_ref(),
        expression => expression,
    };
    let returned = resolve_constant_expression(returned, &collector.values, &mut HashSet::new())?;
    let Expr::Array(array) = strip_parentheses(returned) else {
        return None;
    };
    Some(DecodedComponentConstantEntries {
        entries: array
            .elems
            .iter()
            .map(|element| element.as_ref().map(|element| element.expr.clone()))
            .collect(),
        values: collector.values,
        allow_unnamed_localizer: true,
    })
}

#[derive(Default)]
struct ComponentConstantFactoryCollector {
    values: HashMap<BindingKey, Box<Expr>>,
    returns: Vec<Box<Expr>>,
}

impl Visit for ComponentConstantFactoryCollector {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Pat::Ident(binding), Some(initializer)) = (&declarator.name, &declarator.init) {
            self.values
                .insert(binding_key(&binding.id), initializer.clone());
        }
        declarator.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        if assignment.op == AssignOp::Assign {
            if let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &assignment.left {
                self.values
                    .insert(binding_key(&binding.id), assignment.right.clone());
            }
        }
        assignment.visit_children_with(self);
    }

    fn visit_return_stmt(&mut self, statement: &ReturnStmt) {
        if let Some(argument) = &statement.arg {
            self.returns.push(argument.clone());
        }
        statement.visit_children_with(self);
    }

    fn visit_function(&mut self, _function: &Function) {}

    fn visit_arrow_expr(&mut self, _arrow: &swc_core::ecma::ast::ArrowExpr) {}
}

fn resolve_constant_expression<'a>(
    expression: &'a Expr,
    values: &'a HashMap<BindingKey, Box<Expr>>,
    resolving: &mut HashSet<BindingKey>,
) -> Option<&'a Expr> {
    let expression = strip_parentheses(expression);
    let Expr::Ident(identifier) = expression else {
        return Some(expression);
    };
    let key = binding_key(identifier);
    if !resolving.insert(key.clone()) {
        return None;
    }
    let resolved = resolve_constant_expression(values.get(&key)?.as_ref(), values, resolving);
    resolving.remove(&key);
    resolved
}

fn decode_i18n_message_expression(
    expression: &Expr,
    values: &HashMap<BindingKey, Box<Expr>>,
    resolving: &mut HashSet<BindingKey>,
    allow_unnamed_localizer: bool,
) -> Option<String> {
    let expression = strip_parentheses(expression);
    if let Expr::Ident(identifier) = expression {
        let key = binding_key(identifier);
        if !resolving.insert(key.clone()) {
            return None;
        }
        let decoded = decode_i18n_message_expression(
            values.get(&key)?.as_ref(),
            values,
            resolving,
            allow_unnamed_localizer,
        );
        resolving.remove(&key);
        return decoded;
    }
    if let Some(value) = string_lit(expression) {
        return Some(value);
    }
    match expression {
        Expr::Seq(sequence) => decode_i18n_message_expression(
            sequence.exprs.last()?.as_ref(),
            values,
            resolving,
            allow_unnamed_localizer,
        ),
        Expr::Bin(binary) if binary.op == BinaryOp::Add => {
            let mut message = decode_i18n_message_expression(
                binary.left.as_ref(),
                values,
                resolving,
                allow_unnamed_localizer,
            )?;
            message.push_str(&decode_i18n_message_expression(
                binary.right.as_ref(),
                values,
                resolving,
                allow_unnamed_localizer,
            )?);
            Some(message)
        }
        Expr::TaggedTpl(tagged)
            if matches!(
                strip_parentheses(tagged.tag.as_ref()),
                Expr::Ident(identifier) if identifier.sym == "$localize"
            ) =>
        {
            decode_localized_template(
                tagged.tpl.as_ref(),
                values,
                resolving,
                allow_unnamed_localizer,
            )
        }
        Expr::Call(call) if is_goog_get_msg(call) => {
            decode_localization_call(call, values, resolving, allow_unnamed_localizer)
        }
        Expr::Call(call) if allow_unnamed_localizer => {
            decode_localization_call(call, values, resolving, allow_unnamed_localizer)
        }
        _ => None,
    }
}

fn decode_localized_template(
    template: &swc_core::ecma::ast::Tpl,
    values: &HashMap<BindingKey, Box<Expr>>,
    resolving: &mut HashSet<BindingKey>,
    allow_unnamed_localizer: bool,
) -> Option<String> {
    if template.quasis.len() != template.exprs.len() + 1 {
        return None;
    }
    let mut message = String::new();
    for (index, quasi) in template.quasis.iter().enumerate() {
        let text = quasi
            .cooked
            .as_ref()
            .map(wtf8_to_string)
            .unwrap_or_else(|| quasi.raw.to_string());
        message.push_str(strip_localize_metadata(&text));
        if let Some(expression) = template.exprs.get(index) {
            message.push_str(&decode_i18n_message_expression(
                expression.as_ref(),
                values,
                resolving,
                allow_unnamed_localizer,
            )?);
        }
    }
    Some(message)
}

fn strip_localize_metadata(value: &str) -> &str {
    let Some(metadata) = value.strip_prefix(':') else {
        return value;
    };
    let mut escaped = false;
    for (index, character) in metadata.char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == ':' {
            return &metadata[index + character.len_utf8()..];
        }
    }
    value
}

fn is_goog_get_msg(call: &CallExpr) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = strip_parentheses(callee.as_ref()) else {
        return false;
    };
    matches!(
        strip_parentheses(member.obj.as_ref()),
        Expr::Ident(identifier) if identifier.sym == "goog"
    ) && member_prop_name(&member.prop).is_some_and(|property| property == "getMsg")
}

fn decode_localization_call(
    call: &CallExpr,
    values: &HashMap<BindingKey, Box<Expr>>,
    resolving: &mut HashSet<BindingKey>,
    allow_unnamed_localizer: bool,
) -> Option<String> {
    let mut message = call
        .args
        .first()
        .and_then(|argument| string_lit(argument.expr.as_ref()))?;
    let Some(mapping) = call.args.get(1) else {
        if call.args.len() != 1 {
            return None;
        }
        return Some(message);
    };
    if call.args.len() != 2 {
        return None;
    }
    let Expr::Object(mapping) = strip_parentheses(mapping.expr.as_ref()) else {
        return None;
    };
    for property in &mapping.props {
        let swc_core::ecma::ast::PropOrSpread::Prop(property) = property else {
            return None;
        };
        let swc_core::ecma::ast::Prop::KeyValue(property) = property.as_ref() else {
            return None;
        };
        let name = prop_name(&property.key)?;
        let value = decode_i18n_message_expression(
            property.value.as_ref(),
            values,
            resolving,
            allow_unnamed_localizer,
        )?;
        message = message.replace(&format!("{{${name}}}"), &value);
    }
    Some(message)
}

fn is_basic_i18n_message(message: &str) -> bool {
    parse_i18n_markers(message).is_some()
}

fn render_basic_i18n_message(message: &str, expressions: &[String]) -> Option<String> {
    let markers = parse_i18n_markers(message)?;
    let mut rendered = String::new();
    let mut cursor = 0;
    for (start, end, expression_index) in markers {
        rendered.push_str(&message[cursor..start]);
        let expression = expressions.get(expression_index)?;
        rendered.push_str("{{ ");
        rendered.push_str(expression);
        rendered.push_str(" }}");
        cursor = end;
    }
    rendered.push_str(&message[cursor..]);
    Some(rendered)
}

fn parse_structural_i18n_message(message: &str) -> Option<Vec<I18nToken>> {
    const MARKER: char = '\u{fffd}';

    let mut tokens = Vec::new();
    let mut element_stack = Vec::new();
    let mut saw_element = false;
    let mut cursor = 0;
    while let Some(relative_start) = message[cursor..].find(MARKER) {
        let start = cursor + relative_start;
        if start > cursor {
            tokens.push(I18nToken::Text(message[cursor..start].to_string()));
        }
        let content_start = start + MARKER.len_utf8();
        let relative_end = message[content_start..].find(MARKER)?;
        let marker_end = content_start + relative_end;
        let content = &message[content_start..marker_end];
        let numeric_marker = |value: &str| {
            let mut parts = value.split(':');
            let index = parts.next()?.parse::<usize>().ok()?;
            parts
                .all(|part| {
                    !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
                })
                .then_some(index)
        };
        if let Some(value) = content.strip_prefix("/#") {
            let index = numeric_marker(value)?;
            (element_stack.pop() == Some(index)).then_some(())?;
            tokens.push(I18nToken::ElementEnd(index));
            saw_element = true;
        } else if let Some(value) = content.strip_prefix('#') {
            let index = numeric_marker(value)?;
            element_stack.push(index);
            tokens.push(I18nToken::ElementStart(index));
            saw_element = true;
        } else {
            tokens.push(I18nToken::Interpolation(numeric_marker(content)?));
        }
        cursor = marker_end + MARKER.len_utf8();
    }
    if cursor < message.len() {
        tokens.push(I18nToken::Text(message[cursor..].to_string()));
    }
    (saw_element && element_stack.is_empty()).then_some(tokens)
}

fn parse_i18n_markers(message: &str) -> Option<Vec<(usize, usize, usize)>> {
    const MARKER: char = '\u{fffd}';

    let mut markers = Vec::new();
    let mut cursor = 0;
    while let Some(relative_start) = message[cursor..].find(MARKER) {
        let start = cursor + relative_start;
        let content_start = start + MARKER.len_utf8();
        let relative_end = message[content_start..].find(MARKER)?;
        let marker_end = content_start + relative_end;
        let content = &message[content_start..marker_end];
        if content.is_empty()
            || !content
                .chars()
                .all(|character| character.is_ascii_digit() || character == ':')
        {
            return None;
        }
        let expression_index = content.split(':').next()?.parse().ok()?;
        let end = marker_end + MARKER.len_utf8();
        markers.push((start, end, expression_index));
        cursor = end;
    }
    Some(markers)
}

fn decode_local_references(expression: &Expr) -> Option<Vec<String>> {
    if let Some(values) = decode_static_string_split(expression) {
        if values.len() % 2 != 0 {
            return None;
        }
        return Some(values.chunks_exact(2).map(|pair| pair[0].clone()).collect());
    }
    let Expr::Array(array) = expression else {
        return None;
    };
    let values = array
        .elems
        .iter()
        .map(|element| {
            element
                .as_ref()
                .and_then(|element| string_lit(element.expr.as_ref()))
        })
        .collect::<Option<Vec<_>>>()?;
    if values.len() % 2 != 0 {
        return None;
    }
    Some(values.chunks_exact(2).map(|pair| pair[0].clone()).collect())
}

fn decode_constant_attributes(expression: &Expr) -> Vec<TemplateAttribute> {
    if let Some(values) = decode_static_string_split(expression) {
        return values
            .chunks_exact(2)
            .map(|pair| TemplateAttribute {
                name: pair[0].clone(),
                value: Some(pair[1].clone()),
            })
            .collect();
    }
    let Expr::Array(array) = expression else {
        return Vec::new();
    };
    let values = array
        .elems
        .iter()
        .filter_map(|element| element.as_ref().map(|element| element.expr.as_ref()))
        .collect::<Vec<_>>();
    let mut attributes = Vec::new();
    let mut classes = Vec::new();
    let mut styles = Vec::new();
    let mut index = 0;
    let mut marker = 0usize;
    while index < values.len() {
        if let Expr::Lit(Lit::Num(number)) = values[index] {
            marker = number.value as usize;
            index += 1;
            continue;
        }
        let Some(name) = string_lit(values[index]) else {
            index += 1;
            continue;
        };
        match marker {
            0 => {
                let value = values.get(index + 1).and_then(|value| string_lit(value));
                attributes.push(TemplateAttribute { name, value });
                index += 2;
            }
            1 => {
                classes.push(name);
                index += 1;
            }
            2 => {
                let value = values
                    .get(index + 1)
                    .and_then(|value| string_lit(value))
                    .unwrap_or_default();
                styles.push(format!("{name}: {value}"));
                index += 2;
            }
            _ => {
                // Binding/template markers name non-static attributes.
                index += 1;
            }
        }
    }
    if !classes.is_empty() {
        attributes.push(TemplateAttribute {
            name: "class".to_string(),
            value: Some(classes.join(" ")),
        });
    }
    if !styles.is_empty() {
        attributes.push(TemplateAttribute {
            name: "style".to_string(),
            value: Some(styles.join("; ")),
        });
    }
    attributes
}

fn decode_static_string_split(expression: &Expr) -> Option<Vec<String>> {
    let Expr::Call(call) = strip_parentheses(expression) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = strip_parentheses(callee.as_ref()) else {
        return None;
    };
    if member_prop_name(&member.prop).as_deref() != Some("split") {
        return None;
    }
    let source = string_lit(strip_parentheses(member.obj.as_ref()))?;
    let [delimiter] = call.args.as_slice() else {
        return None;
    };
    if delimiter.spread.is_some() {
        return None;
    }
    let delimiter = string_lit(strip_parentheses(delimiter.expr.as_ref()))?;
    if delimiter.is_empty() || !source.is_ascii() || !delimiter.is_ascii() {
        return None;
    }
    Some(source.split(&delimiter).map(str::to_string).collect())
}

impl TemplateTree {
    fn push_node(&mut self, index: usize, kind: TemplateNodeKind) -> usize {
        let node = self.nodes.len();
        self.nodes.push(TemplateNode {
            kind,
            children: Vec::new(),
        });
        self.index_to_node.insert(index, node);
        if let Some(parent) = self.stack.last().copied() {
            self.nodes[parent].children.push(node);
        } else {
            self.roots.push(node);
        }
        node
    }

    fn add_attribute(&mut self, node: usize, attribute: TemplateAttribute) {
        match &mut self.nodes[node].kind {
            TemplateNodeKind::Element { attributes, .. }
            | TemplateNodeKind::EmbeddedView { attributes, .. }
            | TemplateNodeKind::Projection { attributes, .. } => attributes.push(attribute),
            TemplateNodeKind::Text { .. }
            | TemplateNodeKind::Let { .. }
            | TemplateNodeKind::Defer { .. }
            | TemplateNodeKind::Repeater { .. }
            | TemplateNodeKind::I18nRegion { .. }
            | TemplateNodeKind::UnsupportedRegion { .. }
            | TemplateNodeKind::Consumed => {}
        }
    }

    fn consume_trailing_embedded_views(
        &mut self,
        indices: &[usize],
    ) -> std::result::Result<Vec<TemplateTree>, String> {
        if indices.is_empty() {
            return Err("defer block has no primary embedded view".to_string());
        }
        let nodes = indices
            .iter()
            .map(|index| {
                self.index_to_node
                    .get(index)
                    .copied()
                    .ok_or_else(|| format!("no embedded template at defer index {index}"))
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let siblings = match self.stack.last().copied() {
            Some(parent) => &self.nodes[parent].children,
            None => &self.roots,
        };
        if siblings.len() < nodes.len() || siblings[siblings.len() - nodes.len()..] != nodes {
            return Err(
                "defer child templates are not the trailing siblings in declaration order"
                    .to_string(),
            );
        }
        for node in &nodes {
            match &self.nodes[*node].kind {
                TemplateNodeKind::EmbeddedView {
                    attributes,
                    branch: None,
                    ..
                } if attributes.is_empty() => {}
                TemplateNodeKind::EmbeddedView { .. } => {
                    return Err(
                        "defer child template has attributes or a control-flow branch".to_string(),
                    );
                }
                _ => return Err("defer index does not reference an embedded template".to_string()),
            }
        }

        match self.stack.last().copied() {
            Some(parent) => {
                let new_len = self.nodes[parent].children.len() - nodes.len();
                self.nodes[parent].children.truncate(new_len);
            }
            None => self.roots.truncate(self.roots.len() - nodes.len()),
        }
        let mut trees = Vec::with_capacity(nodes.len());
        for (index, node) in indices.iter().zip(nodes) {
            self.index_to_node.remove(index);
            let kind = std::mem::replace(&mut self.nodes[node].kind, TemplateNodeKind::Consumed);
            let TemplateNodeKind::EmbeddedView { tree, .. } = kind else {
                unreachable!("defer child kinds were validated before mutation")
            };
            trees.push(*tree);
        }
        Ok(trees)
    }

    fn next_insertion_point(&self) -> TreeInsertionPoint {
        match self.stack.last().copied() {
            Some(parent) => TreeInsertionPoint {
                parent: Some(parent),
                position: self.nodes[parent].children.len(),
            },
            None => TreeInsertionPoint {
                parent: None,
                position: self.roots.len(),
            },
        }
    }

    fn update_issue_insertion_point(
        &self,
        instruction: IvyInstruction,
    ) -> Option<TreeInsertionPoint> {
        if !matches!(
            instruction,
            IvyInstruction::StoreLet
                | IvyInstruction::TextInterpolate
                | IvyInstruction::TextInterpolate1
                | IvyInstruction::TextInterpolate2
                | IvyInstruction::TextInterpolate3
                | IvyInstruction::TextInterpolate4
                | IvyInstruction::TextInterpolate5
                | IvyInstruction::TextInterpolate6
                | IvyInstruction::TextInterpolate7
                | IvyInstruction::TextInterpolate8
                | IvyInstruction::Property
                | IvyInstruction::AriaProperty
                | IvyInstruction::Attribute
                | IvyInstruction::ClassMap
                | IvyInstruction::ClassProp
                | IvyInstruction::StyleMap
                | IvyInstruction::StyleProp
                | IvyInstruction::TwoWayProperty
                | IvyInstruction::Repeater
        ) {
            return None;
        }
        let node = *self.index_to_node.get(&self.cursor)?;
        self.insertion_point_before_node(node)
    }

    fn insertion_point_before_node(&self, target: usize) -> Option<TreeInsertionPoint> {
        if let Some(position) = self.roots.iter().position(|node| *node == target) {
            return Some(TreeInsertionPoint {
                parent: None,
                position,
            });
        }
        self.nodes.iter().enumerate().find_map(|(parent, node)| {
            node.children
                .iter()
                .position(|child| *child == target)
                .map(|position| TreeInsertionPoint {
                    parent: Some(parent),
                    position,
                })
        })
    }

    fn insert_unsupported_regions(
        &mut self,
        insertion_point: TreeInsertionPoint,
        comments: impl IntoIterator<Item = String>,
        placement_unknown: bool,
    ) {
        for (offset, comment) in comments.into_iter().enumerate() {
            let node = self.nodes.len();
            self.nodes.push(TemplateNode {
                kind: TemplateNodeKind::UnsupportedRegion {
                    comment,
                    placement_unknown,
                },
                children: Vec::new(),
            });
            let siblings = match insertion_point.parent {
                Some(parent) => &mut self.nodes[parent].children,
                None => &mut self.roots,
            };
            siblings.insert(
                (insertion_point.position + offset).min(siblings.len()),
                node,
            );
        }
    }
}

fn render_tree(tree: &TemplateTree) -> String {
    render_tree_at_depth(tree, 0, &mut HashSet::new())
}

fn render_tree_at_depth(
    tree: &TemplateTree,
    depth: usize,
    rendered_issue_comments: &mut HashSet<String>,
) -> String {
    tree.roots
        .iter()
        .map(|&node| render_node(tree, node, depth, rendered_issue_comments))
        .filter(|rendered| !rendered.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_node(
    tree: &TemplateTree,
    node: usize,
    depth: usize,
    rendered_issue_comments: &mut HashSet<String>,
) -> String {
    let current = &tree.nodes[node];
    let indent = "  ".repeat(depth);
    match &current.kind {
        TemplateNodeKind::Text { value } => format!("{indent}{}", escape_text(value)),
        TemplateNodeKind::I18nRegion {
            tokens,
            expressions,
        } => render_i18n_region(tree, tokens, expressions)
            .map(|rendered| format!("{indent}{rendered}"))
            .unwrap_or_else(|| format!("{indent}<!-- Malformed structural i18n region -->")),
        TemplateNodeKind::Let { name, value, .. } => match value {
            Some(value) => format!("{indent}@let {name} = {value};"),
            None => format!("{indent}<!-- Unrecovered Angular @let {name} -->"),
        },
        TemplateNodeKind::Projection {
            selector,
            attributes,
            fallback,
        } => {
            let selector = selector
                .as_ref()
                .map(|selector| format!(" select=\"{}\"", escape_attribute(selector)))
                .unwrap_or_default();
            let attributes = attributes
                .iter()
                .map(render_attribute)
                .collect::<Vec<_>>()
                .join("");
            let Some(fallback) = fallback else {
                return format!("{indent}<ng-content{selector}{attributes} />");
            };
            let body = render_tree_at_depth(fallback, depth + 1, rendered_issue_comments);
            format!("{indent}<ng-content{selector}{attributes}>\n{body}\n{indent}</ng-content>")
        }
        TemplateNodeKind::Element { tag, attributes } => {
            let attributes = attributes
                .iter()
                .map(render_attribute)
                .collect::<Vec<_>>()
                .join("");
            if is_void_element(tag) {
                return format!("{indent}<{tag}{attributes} />");
            }
            if current.children.is_empty() {
                return format!("{indent}<{tag}{attributes}></{tag}>");
            }
            if current.children.len() == 1 {
                let child = &tree.nodes[current.children[0]];
                if let TemplateNodeKind::Text { value } = &child.kind {
                    if !value.contains('\n') {
                        return format!(
                            "{indent}<{tag}{attributes}>{}</{tag}>",
                            escape_text(value)
                        );
                    }
                }
            }
            let children = current
                .children
                .iter()
                .map(|&child| render_node(tree, child, depth + 1, rendered_issue_comments))
                .filter(|rendered| !rendered.is_empty())
                .collect::<Vec<_>>();
            if children.is_empty() {
                return format!("{indent}<{tag}{attributes}></{tag}>");
            }
            let children = children.join("\n");
            format!("{indent}<{tag}{attributes}>\n{children}\n{indent}</{tag}>")
        }
        TemplateNodeKind::EmbeddedView {
            tree: embedded,
            attributes,
            branch,
        } => {
            let body = render_tree_at_depth(embedded, depth + 1, rendered_issue_comments);
            match branch {
                Some(ConditionalBranch::If(condition)) => {
                    format!("{indent}@if ({condition}) {{\n{body}\n{indent}}}")
                }
                Some(ConditionalBranch::ElseIf(condition)) => {
                    format!("{indent}@else if ({condition}) {{\n{body}\n{indent}}}")
                }
                Some(ConditionalBranch::Else) => {
                    format!("{indent}@else {{\n{body}\n{indent}}}")
                }
                None => {
                    let attributes = attributes
                        .iter()
                        .map(render_attribute)
                        .collect::<Vec<_>>()
                        .join("");
                    format!("{indent}<ng-template{attributes}>\n{body}\n{indent}</ng-template>")
                }
            }
        }
        TemplateNodeKind::Defer {
            primary,
            loading,
            placeholder,
            error,
            triggers,
        } => {
            let trigger = if triggers.is_empty() {
                String::new()
            } else {
                format!(" ({})", triggers.join("; "))
            };
            let primary = render_tree_at_depth(primary, depth + 1, rendered_issue_comments);
            let mut rendered = format!("{indent}@defer{trigger} {{\n{primary}\n{indent}}}");
            for (name, tree) in [
                ("loading", loading.as_deref()),
                ("placeholder", placeholder.as_deref()),
                ("error", error.as_deref()),
            ] {
                if let Some(tree) = tree {
                    let body = render_tree_at_depth(tree, depth + 1, rendered_issue_comments);
                    rendered.push_str(&format!("\n{indent}@{name} {{\n{body}\n{indent}}}"));
                }
            }
            rendered
        }
        TemplateNodeKind::Repeater {
            body,
            empty,
            item,
            track,
            collection,
        } => {
            let collection = collection.as_deref().unwrap_or("/* unknown collection */");
            let body = render_tree_at_depth(body, depth + 1, rendered_issue_comments);
            let mut rendered = format!(
                "{indent}@for ({item} of {collection}; track {track}) {{\n{body}\n{indent}}}"
            );
            if let Some(empty) = empty {
                let empty = render_tree_at_depth(empty, depth + 1, rendered_issue_comments);
                rendered.push_str(&format!("\n{indent}@empty {{\n{empty}\n{indent}}}"));
            }
            rendered
        }
        TemplateNodeKind::UnsupportedRegion {
            comment,
            placement_unknown,
        } => {
            if !rendered_issue_comments.insert(comment.clone()) {
                return String::new();
            }
            let mut rendered = format!("{indent}<!-- {comment} -->");
            if *placement_unknown {
                rendered.push_str(&format!(
                    "\n{indent}<!-- Wakaru: placement unknown within this view -->"
                ));
            }
            rendered
        }
        TemplateNodeKind::Consumed => String::new(),
    }
}

fn render_i18n_region(
    tree: &TemplateTree,
    tokens: &[I18nToken],
    expressions: &[String],
) -> Option<String> {
    let mut rendered = String::new();
    for token in tokens {
        match token {
            I18nToken::Text(value) => rendered.push_str(&escape_text(value)),
            I18nToken::Interpolation(index) => {
                rendered.push_str("{{ ");
                rendered.push_str(expressions.get(*index)?);
                rendered.push_str(" }}");
            }
            I18nToken::ElementStart(index) => {
                let node = *tree.index_to_node.get(index)?;
                let TemplateNodeKind::Element { tag, attributes } = &tree.nodes[node].kind else {
                    return None;
                };
                rendered.push('<');
                rendered.push_str(tag);
                for attribute in attributes {
                    rendered.push_str(&render_attribute(attribute));
                }
                rendered.push('>');
            }
            I18nToken::ElementEnd(index) => {
                let node = *tree.index_to_node.get(index)?;
                let TemplateNodeKind::Element { tag, .. } = &tree.nodes[node].kind else {
                    return None;
                };
                rendered.push_str("</");
                rendered.push_str(tag);
                rendered.push('>');
            }
        }
    }
    Some(rendered)
}

fn render_attribute(attribute: &TemplateAttribute) -> String {
    match &attribute.value {
        Some(value) if !value.is_empty() => {
            format!(" {}=\"{}\"", attribute.name, escape_attribute(value))
        }
        _ => format!(" {}", attribute.name),
    }
}

fn escape_text(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;")
}

fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

fn is_void_element(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}
