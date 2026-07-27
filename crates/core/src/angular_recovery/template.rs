use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use swc_core::common::{sync::Lrc, SourceMap, Span, Spanned, SyntaxContext};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BinaryOp, BlockStmt, BlockStmtOrExpr, CallExpr, Callee,
    Decl, Expr, ExprOrSpread, FnDecl, Function, Ident, Lit, MemberProp, Module, Pat, ReturnStmt,
    SimpleAssignTarget, Stmt, UnaryOp, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::analysis::binding_uses::BindingUseIndex;
use crate::js_names::{is_likely_generated_alias, to_valid_identifier_name};

use super::artifact::expression_references;
use super::emitter::{
    handler_expression, print_template_expression, print_template_expression_with_aliases,
};
use super::roles::{IvyInstruction, IvyRoleTable};
use super::syntax::{binding_key, member_prop_name, string_lit, BindingKey};
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
                    Expr::Fn(_) | Expr::Ident(_)
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
    provenance: TemplateOperationProvenance,
}

#[derive(Default)]
struct TemplateProgram {
    view_id: usize,
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
    reference_aliases: Vec<(BindingKey, InstructionCall, usize)>,
    local_reference_names: HashMap<BindingKey, String>,
    pipes: HashMap<usize, String>,
    artifact_references: HashSet<BindingKey>,
}

impl TemplateProgram {
    fn new(view_id: usize) -> Self {
        Self {
            view_id,
            ..Self::default()
        }
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
}

#[derive(Clone)]
struct TemplateAttribute {
    name: String,
    value: Option<String>,
}

enum TemplateNodeKind {
    Element {
        tag: String,
        attributes: Vec<TemplateAttribute>,
    },
    Text {
        value: String,
    },
    EmbeddedView {
        tree: Box<TemplateTree>,
        attributes: Vec<TemplateAttribute>,
        branch: Option<ConditionalBranch>,
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
    },
    UnsupportedRegion {
        comment: String,
        placement_unknown: bool,
    },
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
    placed_issue_operations: HashSet<(AngularTemplatePhase, usize)>,
    cursor: usize,
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
    let environment = TemplateRecoveryEnvironment {
        constants: &constants,
        projection_selectors,
        roles,
        template_functions,
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
        &mut active_templates,
        &mut next_view_id,
        &[],
        0,
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
    })
}

fn recover_template_tree(
    template: &Function,
    environment: &TemplateRecoveryEnvironment<'_>,
    is_component_view: bool,
    active_templates: &mut HashSet<BindingKey>,
    next_view_id: &mut usize,
    ancestor_references: &[ReferenceScope],
    depth: usize,
) -> Result<(TemplateTree, TemplateProgram)> {
    let view_id = *next_view_id;
    *next_view_id = view_id.saturating_add(1);
    let mut program = TemplateProgram::new(view_id);
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
        program.saved_views.extend(inlined_current_view_captures(
            body,
            environment.roles,
            environment.unresolved_ctxt,
        ));
        collect_statements(&body.stmts, None, &render_flags, environment, &mut program);
    }

    let mut tree = TemplateTree::default();
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

fn resolve_reference_aliases(
    program: &mut TemplateProgram,
    tree: &TemplateTree,
    ancestor_references: &[ReferenceScope],
) {
    for (binding, call, context_depth) in std::mem::take(&mut program.reference_aliases) {
        let Some(slot) = numeric_arg(&call.args, 0).filter(|_| call.args.len() == 1) else {
            record_malformed_instruction(
                &call,
                "expected one numeric local-reference slot",
                &mut program.issues,
                &mut program.stats,
            );
            continue;
        };
        let Some(name) = reference_name_at_depth(tree, ancestor_references, context_depth, slot)
        else {
            record_missing_target(
                &call,
                &format!("no local reference at slot {slot} in context depth {context_depth}"),
                &mut program.issues,
                &mut program.stats,
            );
            continue;
        };
        program.local_reference_names.insert(binding, name);
        program.stats.rendered_instruction_calls += 1;
    }
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
                    | IvyInstruction::Text
                    | IvyInstruction::Template
                    | IvyInstruction::RepeaterCreate
                    | IvyInstruction::Projection => 3,
                    IvyInstruction::ElementEnd
                    | IvyInstruction::Listener
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
                    | IvyInstruction::Pipe
                    | IvyInstruction::PipeBind1
                    | IvyInstruction::PipeBind2
                    | IvyInstruction::PipeBind3
                    | IvyInstruction::PipeBind4
                    | IvyInstruction::PipeBindV
                    | IvyInstruction::Advance
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
                    | IvyInstruction::Attribute
                    | IvyInstruction::ClassProp
                    | IvyInstruction::StyleProp => 1,
                    IvyInstruction::DefineComponent => 0,
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
        let supported_compiler_alias = match (&declarator.name, declarator.init.as_deref()) {
            (Pat::Ident(binding), Some(Expr::Call(call))) => {
                collect_current_view_alias(&binding.id, call, phase, environment, program)
                    || collect_next_context_alias(&binding.id, call, phase, environment, program)
                    || collect_reference_alias(&binding.id, call, phase, environment, program)
            }
            (Pat::Ident(binding), Some(initializer)) => {
                is_inlined_current_view_alias(&binding.id, initializer, phase, program)
                    || collect_view_context_alias(&binding.id, initializer, phase, program)
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
    if property != "$implicit" && !property.starts_with('$') {
        return false;
    }

    let fallback = if property == "$implicit" {
        "item"
    } else {
        property.as_str()
    };
    let name = recovered_view_alias_name(binding.sym.as_ref(), fallback);
    program
        .local_reference_names
        .insert(binding_key(binding), name.clone());
    if property == "$implicit" {
        program.repeater_item_name.get_or_insert(name);
    }
    true
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
        program.component_contexts.insert(binding_key(binding));
        program.update_context_depth = program.update_context_depth.saturating_add(context_hop);
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
    program.reference_aliases.push((
        binding_key(binding),
        InstructionCall {
            instruction: IvyInstruction::Reference,
            args: argument_lists[0]
                .iter()
                .map(|argument| argument.expr.clone())
                .collect(),
            provenance,
        },
        program.update_context_depth,
    ));
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
            let supported_context_alias = match (&assignment.left, assignment.right.as_ref()) {
                (AssignTarget::Simple(SimpleAssignTarget::Ident(binding)), Expr::Call(call))
                    if assignment.op == AssignOp::Assign =>
                {
                    collect_next_context_alias(&binding.id, call, phase, environment, program)
                        || collect_reference_alias(&binding.id, call, phase, environment, program)
                }
                _ => false,
            };
            if !supported_context_alias {
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
                | IvyInstruction::Text
                | IvyInstruction::Listener
                | IvyInstruction::Template
                | IvyInstruction::RepeaterCreate
                | IvyInstruction::ProjectionDef
                | IvyInstruction::Projection
                | IvyInstruction::Pipe
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
                | IvyInstruction::Attribute
                | IvyInstruction::ClassProp
                | IvyInstruction::StyleProp
                | IvyInstruction::Conditional
                | IvyInstruction::Repeater
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

fn call_chain(call: &CallExpr) -> Option<(&Expr, Vec<&[ExprOrSpread]>)> {
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
}

fn recover_view_listener_handler(
    handler: &Expr,
    tree: &TemplateTree,
    ancestor_references: &[ReferenceScope],
    program: &TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
) -> std::result::Result<Option<RecoveredViewHandler>, String> {
    let Some(block) = handler_block(handler) else {
        return Ok(None);
    };
    if !block.stmts.iter().any(|statement| {
        statement_restore_view_call(statement, environment)
            .is_some_and(|call| is_instruction_call(call, IvyInstruction::RestoreView, environment))
    }) {
        return Ok(None);
    }

    let mut component_contexts = program.component_contexts.clone();
    let mut local_names = program.local_reference_names.clone();
    let mut expression_aliases = HashMap::new();
    let mut effects = Vec::new();
    if let Some(event) = handler_event_binding(handler) {
        local_names.insert(event, "$event".to_string());
    }
    let mut action = None;
    let mut saw_reset_return = false;
    let mut runtime_calls = 0;
    let mut context_depth = 0usize;
    for statement in &block.stmts {
        match statement {
            Stmt::Decl(Decl::Var(declaration)) => {
                for declarator in &declaration.decls {
                    let Pat::Ident(binding) = &declarator.name else {
                        return Err("expected identifier aliases".to_string());
                    };
                    let Some(initializer) = declarator.init.as_deref() else {
                        return Err("view alias has no initializer".to_string());
                    };
                    if let Some(call) = restored_view_call(initializer, environment) {
                        let Some(property) = restored_view_property(initializer) else {
                            return Err("restored view has no context property".to_string());
                        };
                        if property != "$implicit" {
                            return Err(format!(
                                "unsupported restored view context property {property}"
                            ));
                        }
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
                        let item = program.repeater_item_name.clone().unwrap_or_else(|| {
                            recovered_view_alias_name(binding.id.sym.as_ref(), "item")
                        });
                        local_names.insert(binding_key(&binding.id), item);
                        runtime_calls += 1;
                        context_depth = 0;
                        continue;
                    }

                    if let Some((alias, context_hop)) =
                        next_context_member_alias(initializer, environment)?
                    {
                        expression_aliases.insert(binding_key(&binding.id), alias);
                        runtime_calls += 1;
                        context_depth = context_depth.saturating_add(context_hop);
                        continue;
                    }

                    let Expr::Call(call) = strip_parentheses(initializer) else {
                        return Err("unsupported view-local alias initializer".to_string());
                    };
                    if is_instruction_call(call, IvyInstruction::RestoreView, environment) {
                        validate_restore_view_call(call, program)?;
                        component_contexts.insert(binding_key(&binding.id));
                        runtime_calls += 1;
                        context_depth = 0;
                        continue;
                    }
                    if is_instruction_call(call, IvyInstruction::Reference, environment) {
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
                        local_names.insert(binding_key(&binding.id), name);
                        runtime_calls += 1;
                        continue;
                    }
                    if is_instruction_call(call, IvyInstruction::NextContext, environment) {
                        let argument_lists = validated_single_call(call, "ɵɵnextContext")?;
                        let Some(context_hop) = context_hop(&argument_lists) else {
                            return Err(
                                "ɵɵnextContext has unexpected context-depth arguments".to_string()
                            );
                        };
                        component_contexts.insert(binding_key(&binding.id));
                        runtime_calls += 1;
                        context_depth = context_depth.saturating_add(context_hop);
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
                }
                if contains_runtime_call(expression.expr.as_ref(), environment) {
                    return Err(
                        "unsupported Ivy runtime call in restored handler expression".to_string(),
                    );
                }
                effects.push(expression.expr.clone());
            }
            Stmt::Return(ReturnStmt {
                arg: Some(returned),
                ..
            }) => {
                if saw_reset_return {
                    return Err("multiple handler returns".to_string());
                }
                let Expr::Call(call) = strip_parentheses(returned.as_ref()) else {
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
    let mut sources = Vec::with_capacity(effects.len() + usize::from(action.is_some()));
    for effect in &effects {
        sources.push(
            print_template_expression_with_aliases(
                effect.as_ref(),
                &component_contexts,
                &local_names,
                &expression_aliases,
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
                environment.cm.clone(),
            )
            .map_err(|error| error.to_string())?,
        );
    }
    if sources.is_empty() {
        sources.push("undefined".to_string());
    }
    let source = sources.join("; ");
    let mut artifact_references = action
        .as_deref()
        .map(expression_references)
        .unwrap_or_default();
    for effect in &effects {
        artifact_references.extend(expression_references(effect.as_ref()));
    }
    for alias in expression_aliases.values() {
        artifact_references.extend(expression_references(alias.as_ref()));
    }
    Ok(Some(RecoveredViewHandler {
        source,
        runtime_calls,
        artifact_references,
    }))
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
                    .is_known_runtime_member(root, self.environment.unresolved_ctxt)
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
        depth,
    } = recovery;
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
            attach_local_references(call, index, node, tree, environment);
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
            attach_local_references(call, index, node, tree, environment);
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
                tree,
                ancestor_references,
                program,
                environment,
            ) {
                Ok(Some(recovered)) => {
                    program.stats.runtime_calls_observed += recovered.runtime_calls;
                    program.stats.rendered_instruction_calls += recovered.runtime_calls;
                    program
                        .artifact_references
                        .extend(recovered.artifact_references);
                    recovered.source
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
            if !matches!(call.args.len(), 1 | 2) {
                record_unsupported_instruction(
                    call,
                    "projection fallback content is not yet supported",
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
            tree.push_node(index, TemplateNodeKind::Projection { selector });
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
            if !(8..=13).contains(&call.args.len()) {
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
            let Some((body, body_program)) = recover_child_template(
                call,
                body_expression.as_ref(),
                "repeater body",
                program,
                environment,
                ChildViewRecovery {
                    parent_tree: tree,
                    active_templates,
                    next_view_id,
                    ancestor_references,
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
            merge_template_program(program, body_program);

            let empty = if let Some(empty_expression) = call.args.get(8) {
                if is_nullish_expression(empty_expression.as_ref()) {
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
                            next_view_id,
                            ancestor_references,
                            depth,
                        },
                    )?
                    else {
                        return Ok(());
                    };
                    merge_template_program(program, empty_program);
                    Some(Box::new(empty))
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
            let child = recover_template_tree(
                &resolved.function,
                environment,
                false,
                active_templates,
                next_view_id,
                &child_references,
                depth + 1,
            );
            if let Some(key) = &resolved.key {
                active_templates.remove(key);
            }
            let (child_tree, child_program) = child?;
            merge_template_program(program, child_program);
            let attributes = numeric_arg(&call.args, 5)
                .and_then(|index| environment.constants.attributes.get(index).cloned())
                .unwrap_or_default();
            tree.push_node(
                index,
                TemplateNodeKind::EmbeddedView {
                    tree: Box::new(child_tree),
                    attributes,
                    branch: None,
                },
            );
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

struct ChildViewRecovery<'a> {
    parent_tree: &'a TemplateTree,
    active_templates: &'a mut HashSet<BindingKey>,
    next_view_id: &'a mut usize,
    ancestor_references: &'a [ReferenceScope],
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
    let child = recover_template_tree(
        &resolved.function,
        environment,
        false,
        active_templates,
        next_view_id,
        &child_references,
        depth + 1,
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
    let printed = print_template_expression(
        body,
        &program.component_contexts,
        &local_names,
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

fn is_nullish_expression(expression: &Expr) -> bool {
    match strip_parentheses(expression) {
        Expr::Lit(Lit::Null(_)) => true,
        Expr::Ident(identifier) => identifier.sym == "undefined",
        _ => false,
    }
}

fn attach_local_references(
    call: &InstructionCall,
    node_index: usize,
    node: usize,
    tree: &mut TemplateTree,
    environment: &TemplateRecoveryEnvironment<'_>,
) {
    let Some(reference_index) = numeric_arg(&call.args, 3) else {
        return;
    };
    let Some(references) = environment.constants.local_references.get(reference_index) else {
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
        IvyInstruction::Property
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
                IvyInstruction::Property => "",
                IvyInstruction::Attribute => "attr.",
                IvyInstruction::ClassProp => "class.",
                IvyInstruction::StyleProp => "style.",
                _ => unreachable!(),
            };
            tree.add_attribute(
                node,
                TemplateAttribute {
                    name: format!("[{prefix}{name}]"),
                    value: Some(expression),
                },
            );
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
        let TemplateNodeKind::EmbeddedView {
            branch: node_branch,
            ..
        } = &mut tree.nodes[node].kind
        else {
            record_missing_target(
                call,
                &format!("index {index} does not reference an embedded template"),
                &mut program.issues,
                &mut program.stats,
            );
            return false;
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
    cm: Lrc<SourceMap>,
) -> Option<Vec<(usize, ConditionalBranch)>> {
    let mut branches = Vec::new();
    decode_conditional_branch(
        strip_parentheses(selection),
        true,
        component_contexts,
        local_references,
        cm,
        &mut branches,
    )?;
    (!branches.is_empty()).then_some(branches)
}

fn decode_conditional_branch(
    selection: &Expr,
    first: bool,
    component_contexts: &HashSet<BindingKey>,
    local_references: &HashMap<BindingKey, String>,
    cm: Lrc<SourceMap>,
    branches: &mut Vec<(usize, ConditionalBranch)>,
) -> Option<()> {
    let Expr::Cond(conditional) = strip_parentheses(selection) else {
        return None;
    };
    let condition = print_template_expression(
        conditional.test.as_ref(),
        component_contexts,
        local_references,
        cm.clone(),
    )
    .ok()?;
    let index = signed_integer(conditional.cons.as_ref())?;
    if index < 0 {
        return None;
    }
    branches.push((
        index as usize,
        if first {
            ConditionalBranch::If(condition)
        } else {
            ConditionalBranch::ElseIf(condition)
        },
    ));

    match strip_parentheses(conditional.alt.as_ref()) {
        Expr::Cond(_) => decode_conditional_branch(
            conditional.alt.as_ref(),
            false,
            component_contexts,
            local_references,
            cm,
            branches,
        ),
        alternate => {
            let index = signed_integer(alternate)?;
            if index >= 0 {
                branches.push((index as usize, ConditionalBranch::Else));
            } else if index != -1 {
                return None;
            }
            Some(())
        }
    }
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
            if environment
                .roles
                .is_known_runtime_member(root, environment.unresolved_ctxt)
            {
                program.stats.runtime_calls_observed += argument_lists.len();
                program.stats.unsupported_runtime_calls += argument_lists.len();
                let argument_counts = argument_lists
                    .iter()
                    .map(|arguments| arguments.len())
                    .collect::<Vec<_>>();
                let shape = program
                    .unknown_runtime_call_shapes
                    .entry((AngularTemplatePhase::Update, argument_counts))
                    .or_default();
                shape.0 += 1;
                shape.1 += argument_lists.len();
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
                                AngularRecoveryIssueKind::UnknownRuntimeInstruction,
                                None,
                                Some("nested in a template expression".to_string()),
                            ),
                            &provenance,
                        ),
                    );
                }
                return Err(anyhow!("unknown nested Ivy instruction"));
            }
        }
    }

    let printed = print_template_expression(
        expression,
        &program.component_contexts,
        &program.local_reference_names,
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
    let Expr::Array(table) = constants else {
        return TemplateConstants::default();
    };
    let entries = table
        .elems
        .iter()
        .map(|entry| entry.as_ref().map(|entry| entry.expr.as_ref()))
        .collect::<Vec<_>>();
    TemplateConstants {
        attributes: entries
            .iter()
            .map(|entry| entry.map(decode_constant_attributes).unwrap_or_default())
            .collect(),
        local_references: entries
            .iter()
            .map(|entry| entry.and_then(decode_local_references).unwrap_or_default())
            .collect(),
    }
}

fn decode_local_references(expression: &Expr) -> Option<Vec<String>> {
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
            | TemplateNodeKind::EmbeddedView { attributes, .. } => attributes.push(attribute),
            TemplateNodeKind::Text { .. }
            | TemplateNodeKind::Repeater { .. }
            | TemplateNodeKind::Projection { .. }
            | TemplateNodeKind::UnsupportedRegion { .. } => {}
        }
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
            IvyInstruction::TextInterpolate
                | IvyInstruction::TextInterpolate1
                | IvyInstruction::TextInterpolate2
                | IvyInstruction::TextInterpolate3
                | IvyInstruction::TextInterpolate4
                | IvyInstruction::TextInterpolate5
                | IvyInstruction::TextInterpolate6
                | IvyInstruction::TextInterpolate7
                | IvyInstruction::TextInterpolate8
                | IvyInstruction::Property
                | IvyInstruction::Attribute
                | IvyInstruction::ClassProp
                | IvyInstruction::StyleProp
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
        TemplateNodeKind::Projection { selector } => match selector {
            Some(selector) => {
                format!(
                    "{indent}<ng-content select=\"{}\" />",
                    escape_attribute(selector)
                )
            }
            None => format!("{indent}<ng-content />"),
        },
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
    }
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
