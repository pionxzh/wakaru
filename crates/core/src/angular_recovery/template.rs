use std::collections::{HashMap, HashSet};

use anyhow::Result;
use swc_core::common::{sync::Lrc, SourceMap, SyntaxContext};
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BinaryOp, CallExpr, Callee, Decl, Expr, ExprOrSpread, FnDecl, Function,
    Lit, Module, Pat, SimpleAssignTarget, Stmt, UnaryOp, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::emitter::{handler_expression, print_template_expression};
use super::roles::{IvyInstruction, IvyRoleTable};
use super::syntax::{binding_key, string_lit, BindingKey};
use super::{
    AngularRecoveryIssue, AngularRecoveryIssueKind, AngularTemplatePhase,
    AngularTemplateRecoveryStats, AngularUnknownRuntimeCallShape,
};

pub(super) struct RecoveredTemplate {
    pub(super) source: String,
    pub(super) issues: Vec<AngularRecoveryIssue>,
    pub(super) stats: AngularTemplateRecoveryStats,
    pub(super) unknown_runtime_call_shapes: Vec<AngularUnknownRuntimeCallShape>,
}

#[derive(Default)]
pub(super) struct TemplateFunctionTable {
    functions: HashMap<BindingKey, Function>,
}

impl TemplateFunctionTable {
    pub(super) fn collect(module: &Module) -> Self {
        let mut collector = TemplateFunctionCollector::default();
        module.visit_with(&mut collector);
        Self {
            functions: collector.functions,
        }
    }

    fn resolve(&self, expression: &Expr) -> Option<ResolvedTemplateFunction> {
        match strip_parentheses(expression) {
            Expr::Ident(identifier) => {
                let key = binding_key(identifier);
                self.functions
                    .get(&key)
                    .cloned()
                    .map(|function| ResolvedTemplateFunction {
                        key: Some(key),
                        function,
                    })
            }
            Expr::Fn(function) => Some(ResolvedTemplateFunction {
                key: None,
                function: function.function.as_ref().clone(),
            }),
            _ => None,
        }
    }
}

#[derive(Default)]
struct TemplateFunctionCollector {
    functions: HashMap<BindingKey, Function>,
}

impl Visit for TemplateFunctionCollector {
    fn visit_fn_decl(&mut self, declaration: &FnDecl) {
        self.functions.insert(
            binding_key(&declaration.ident),
            declaration.function.as_ref().clone(),
        );
        declaration.function.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Pat::Ident(binding), Some(Expr::Fn(function))) =
            (&declarator.name, declarator.init.as_deref())
        {
            self.functions
                .insert(binding_key(&binding.id), function.function.as_ref().clone());
        }
        declarator.visit_children_with(self);
    }
}

struct ResolvedTemplateFunction {
    key: Option<BindingKey>,
    function: Function,
}

struct TemplateRecoveryEnvironment<'a> {
    constants: &'a [Vec<TemplateAttribute>],
    roles: &'a IvyRoleTable,
    template_functions: &'a TemplateFunctionTable,
    unresolved_ctxt: SyntaxContext,
    cm: Lrc<SourceMap>,
}

#[derive(Clone)]
struct InstructionCall {
    instruction: IvyInstruction,
    args: Vec<Box<Expr>>,
}

#[derive(Default)]
struct TemplateProgram {
    create: Vec<InstructionCall>,
    update: Vec<InstructionCall>,
    issues: Vec<AngularRecoveryIssue>,
    stats: AngularTemplateRecoveryStats,
    unknown_runtime_call_shapes: HashMap<(AngularTemplatePhase, Vec<usize>), (usize, usize)>,
    component_contexts: HashSet<BindingKey>,
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
    cursor: usize,
}

fn record_issue(issues: &mut Vec<AngularRecoveryIssue>, issue: AngularRecoveryIssue) {
    if !issues.contains(&issue) {
        issues.push(issue);
    }
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

pub(super) fn recover_template(
    template: &Function,
    constant_table: Option<&Expr>,
    roles: &IvyRoleTable,
    template_functions: &TemplateFunctionTable,
    unresolved_ctxt: SyntaxContext,
    cm: Lrc<SourceMap>,
) -> Result<RecoveredTemplate> {
    let constants = constant_table
        .map(decode_component_constant_table)
        .unwrap_or_default();
    let environment = TemplateRecoveryEnvironment {
        constants: &constants,
        roles,
        template_functions,
        unresolved_ctxt,
        cm,
    };
    let mut active_templates = HashSet::new();
    let (tree, program) =
        recover_template_tree(template, &environment, true, &mut active_templates, 0)?;

    let mut source = render_tree(&tree);
    let mut rendered_issue_comments = HashSet::new();
    for issue in &program.issues {
        let comment = issue_comment(issue);
        if !rendered_issue_comments.insert(comment.clone()) {
            continue;
        }
        if !source.is_empty() {
            source.push('\n');
        }
        source.push_str("<!-- ");
        source.push_str(&comment);
        source.push_str(" -->");
    }
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
    })
}

fn recover_template_tree(
    template: &Function,
    environment: &TemplateRecoveryEnvironment<'_>,
    is_component_view: bool,
    active_templates: &mut HashSet<BindingKey>,
    depth: usize,
) -> Result<(TemplateTree, TemplateProgram)> {
    let mut program = TemplateProgram::default();
    let Some(render_flags) = function_param_binding(template, 0) else {
        record_issue(
            &mut program.issues,
            AngularRecoveryIssue {
                kind: AngularRecoveryIssueKind::UnsupportedTemplateParameters,
                instruction: None,
                detail: None,
            },
        );
        return Ok((TemplateTree::default(), program));
    };
    if is_component_view {
        if let Some(context) = function_param_binding(template, 1) {
            program.component_contexts.insert(context);
        }
    }
    if let Some(body) = &template.body {
        collect_statements(
            &body.stmts,
            None,
            &render_flags,
            environment.roles,
            environment.unresolved_ctxt,
            &mut program,
        );
    }

    let mut tree = TemplateTree::default();
    for instruction in program.create.clone() {
        apply_create_instruction(
            &instruction,
            &mut tree,
            &mut program,
            environment,
            active_templates,
            depth,
        )?;
    }
    for instruction in program.update.clone() {
        apply_update_instruction(
            &instruction,
            &mut tree,
            environment.cm.clone(),
            &mut program,
        )?;
    }
    if !tree.stack.is_empty() {
        record_issue(
            &mut program.issues,
            AngularRecoveryIssue {
                kind: AngularRecoveryIssueKind::MalformedTemplateStructure,
                instruction: None,
                detail: Some(format!("{} unclosed element(s)", tree.stack.len())),
            },
        );
    }
    Ok((tree, program))
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
                    | IvyInstruction::Template => 3,
                    IvyInstruction::ElementEnd
                    | IvyInstruction::Listener
                    | IvyInstruction::Conditional
                    | IvyInstruction::NextContext
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
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
    program: &mut TemplateProgram,
) {
    for statement in statements {
        collect_statement(
            statement,
            phase,
            render_flags,
            roles,
            unresolved_ctxt,
            program,
        );
    }
}

fn collect_statement(
    statement: &Stmt,
    phase: Option<u8>,
    render_flags: &BindingKey,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
    program: &mut TemplateProgram,
) {
    match statement {
        Stmt::Empty(_) => {}
        Stmt::Block(block) => collect_statements(
            &block.stmts,
            phase,
            render_flags,
            roles,
            unresolved_ctxt,
            program,
        ),
        Stmt::If(if_statement) => {
            let (branch_phase, is_render_guard) = collect_if_test(
                if_statement.test.as_ref(),
                phase,
                render_flags,
                roles,
                unresolved_ctxt,
                program,
            );
            if !is_render_guard {
                record_issue(
                    &mut program.issues,
                    AngularRecoveryIssue {
                        kind: AngularRecoveryIssueKind::UnsupportedStatement,
                        instruction: None,
                        detail: Some("conditional control flow".to_string()),
                    },
                );
            }
            collect_statement(
                if_statement.cons.as_ref(),
                branch_phase,
                render_flags,
                roles,
                unresolved_ctxt,
                program,
            );
            if let Some(alternate) = &if_statement.alt {
                if is_render_guard {
                    record_issue(
                        &mut program.issues,
                        AngularRecoveryIssue {
                            kind: AngularRecoveryIssueKind::UnsupportedStatement,
                            instruction: None,
                            detail: Some("render-flag alternate branch".to_string()),
                        },
                    );
                }
                collect_statement(
                    alternate.as_ref(),
                    phase,
                    render_flags,
                    roles,
                    unresolved_ctxt,
                    program,
                );
            }
        }
        Stmt::Expr(expression) => collect_expression(
            expression.expr.as_ref(),
            phase,
            render_flags,
            roles,
            unresolved_ctxt,
            program,
        ),
        Stmt::Decl(Decl::Var(declaration)) => collect_variable_declaration(
            declaration,
            phase,
            render_flags,
            roles,
            unresolved_ctxt,
            program,
        ),
        _ => record_issue(
            &mut program.issues,
            AngularRecoveryIssue {
                kind: AngularRecoveryIssueKind::UnsupportedStatement,
                instruction: None,
                detail: Some(statement_kind(statement).to_string()),
            },
        ),
    }
}

fn collect_variable_declaration(
    declaration: &swc_core::ecma::ast::VarDecl,
    phase: Option<u8>,
    render_flags: &BindingKey,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
    program: &mut TemplateProgram,
) {
    for declarator in &declaration.decls {
        let supported_context_alias = match (&declarator.name, declarator.init.as_deref()) {
            (Pat::Ident(binding), Some(Expr::Call(call))) if phase == Some(2) => {
                collect_next_context_alias(
                    &binding.id,
                    call,
                    phase,
                    roles,
                    unresolved_ctxt,
                    program,
                )
            }
            _ => false,
        };
        if supported_context_alias {
            continue;
        }

        record_issue(
            &mut program.issues,
            AngularRecoveryIssue {
                kind: AngularRecoveryIssueKind::UnsupportedStatement,
                instruction: None,
                detail: Some("declaration".to_string()),
            },
        );
        if let Some(initializer) = &declarator.init {
            collect_expression(
                initializer.as_ref(),
                phase,
                render_flags,
                roles,
                unresolved_ctxt,
                program,
            );
        }
    }
}

fn collect_next_context_alias(
    binding: &swc_core::ecma::ast::Ident,
    call: &CallExpr,
    phase: Option<u8>,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
    program: &mut TemplateProgram,
) -> bool {
    if phase != Some(2) {
        return false;
    }
    let Some((_, argument_lists)) = call_chain(call).filter(|(root, _)| {
        roles.instruction_for_expr(root, unresolved_ctxt) == Some(IvyInstruction::NextContext)
    }) else {
        return false;
    };

    program.stats.runtime_calls_observed += argument_lists.len();
    let valid = argument_lists.len() == 1
        && argument_lists[0].len() <= 1
        && argument_lists[0].first().is_none_or(|argument| {
            matches!(
                argument.expr.as_ref(),
                Expr::Lit(Lit::Num(number))
                    if number.value >= 0.0 && number.value.fract() == 0.0
            )
        });
    if valid {
        program.stats.rendered_instruction_calls += 1;
        program.component_contexts.insert(binding_key(binding));
    } else {
        program.stats.malformed_instruction_calls += argument_lists.len();
        record_issue(
            &mut program.issues,
            AngularRecoveryIssue {
                kind: AngularRecoveryIssueKind::MalformedInstruction,
                instruction: Some("ɵɵnextContext".to_string()),
                detail: Some("unexpected context-depth arguments".to_string()),
            },
        );
    }
    true
}

fn collect_if_test(
    test: &Expr,
    phase: Option<u8>,
    render_flags: &BindingKey,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
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
        collect_expression(
            effect.as_ref(),
            phase,
            render_flags,
            roles,
            unresolved_ctxt,
            program,
        );
    }
    let mask = render_flag_mask(strip_parentheses(condition), render_flags);
    (mask.or(phase), mask.is_some())
}

fn collect_expression(
    expression: &Expr,
    phase: Option<u8>,
    render_flags: &BindingKey,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
    program: &mut TemplateProgram,
) {
    match expression {
        Expr::Paren(paren) => collect_expression(
            paren.expr.as_ref(),
            phase,
            render_flags,
            roles,
            unresolved_ctxt,
            program,
        ),
        Expr::Seq(sequence) => {
            for expression in &sequence.exprs {
                collect_expression(
                    expression.as_ref(),
                    phase,
                    render_flags,
                    roles,
                    unresolved_ctxt,
                    program,
                );
            }
        }
        Expr::Bin(binary) if binary.op == BinaryOp::LogicalAnd => {
            let mask = render_flag_mask(binary.left.as_ref(), render_flags);
            if mask.is_none() {
                record_issue(
                    &mut program.issues,
                    AngularRecoveryIssue {
                        kind: AngularRecoveryIssueKind::UnsupportedExpression,
                        instruction: None,
                        detail: Some("conditional logical-and".to_string()),
                    },
                );
            }
            let branch_phase = mask.or(phase);
            collect_expression(
                binary.right.as_ref(),
                branch_phase,
                render_flags,
                roles,
                unresolved_ctxt,
                program,
            );
        }
        Expr::Assign(assignment) => {
            let supported_context_alias = match (&assignment.left, assignment.right.as_ref()) {
                (AssignTarget::Simple(SimpleAssignTarget::Ident(binding)), Expr::Call(call))
                    if assignment.op == AssignOp::Assign =>
                {
                    collect_next_context_alias(
                        &binding.id,
                        call,
                        phase,
                        roles,
                        unresolved_ctxt,
                        program,
                    )
                }
                _ => false,
            };
            if !supported_context_alias {
                record_issue(
                    &mut program.issues,
                    AngularRecoveryIssue {
                        kind: AngularRecoveryIssueKind::UnsupportedExpression,
                        instruction: None,
                        detail: Some("assignment".to_string()),
                    },
                );
                collect_expression(
                    assignment.right.as_ref(),
                    phase,
                    render_flags,
                    roles,
                    unresolved_ctxt,
                    program,
                );
            }
        }
        Expr::Call(call) => {
            let Some((root, argument_lists)) = call_chain(call) else {
                record_issue(
                    &mut program.issues,
                    AngularRecoveryIssue {
                        kind: AngularRecoveryIssueKind::UnsupportedExpression,
                        instruction: None,
                        detail: Some("non-expression call target".to_string()),
                    },
                );
                return;
            };
            let Some(instruction) = roles.instruction_for_expr(root, unresolved_ctxt) else {
                if let Some(name) = roles.ivy_name_for_expr(root, unresolved_ctxt) {
                    program.stats.runtime_calls_observed += argument_lists.len();
                    program.stats.unsupported_runtime_calls += argument_lists.len();
                    record_issue(
                        &mut program.issues,
                        AngularRecoveryIssue {
                            kind: AngularRecoveryIssueKind::UnsupportedInstruction,
                            instruction: Some(name),
                            detail: phase.map(|phase| format!("render phase {phase}")),
                        },
                    );
                } else if matches!(phase, Some(1 | 2))
                    || roles.is_known_runtime_member(root, unresolved_ctxt)
                {
                    program.stats.runtime_calls_observed += argument_lists.len();
                    program.stats.unsupported_runtime_calls += argument_lists.len();
                    let template_phase = match phase {
                        Some(1) => AngularTemplatePhase::Creation,
                        Some(2) => AngularTemplatePhase::Update,
                        _ => AngularTemplatePhase::OutsideRender,
                    };
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
                    record_issue(
                        &mut program.issues,
                        AngularRecoveryIssue {
                            kind: AngularRecoveryIssueKind::UnknownRuntimeInstruction,
                            instruction: None,
                            detail: phase.map(|phase| {
                                format!(
                                    "render phase {phase}, {} argument list(s)",
                                    argument_lists.len()
                                )
                            }),
                        },
                    );
                } else {
                    record_issue(
                        &mut program.issues,
                        AngularRecoveryIssue {
                            kind: AngularRecoveryIssueKind::UnsupportedExpression,
                            instruction: None,
                            detail: Some("call outside a render phase".to_string()),
                        },
                    );
                }
                return;
            };
            let Some(phase) = phase else {
                program.stats.runtime_calls_observed += argument_lists.len();
                program.stats.unsupported_runtime_calls += argument_lists.len();
                record_issue(
                    &mut program.issues,
                    AngularRecoveryIssue {
                        kind: AngularRecoveryIssueKind::UnsupportedInstruction,
                        instruction: Some(instruction.canonical_export_name().to_string()),
                        detail: Some("outside a creation or update phase".to_string()),
                    },
                );
                return;
            };
            if !instruction_supported_in_phase(instruction, phase) {
                program.stats.runtime_calls_observed += argument_lists.len();
                program.stats.unsupported_runtime_calls += argument_lists.len();
                record_issue(
                    &mut program.issues,
                    AngularRecoveryIssue {
                        kind: AngularRecoveryIssueKind::UnsupportedInstruction,
                        instruction: Some(instruction.canonical_export_name().to_string()),
                        detail: Some(format!("unsupported in render phase {phase}")),
                    },
                );
                return;
            }
            program.stats.runtime_calls_observed += argument_lists.len();
            let target = if phase == 1 {
                &mut program.create
            } else {
                &mut program.update
            };
            target.extend(argument_lists.into_iter().map(|args| InstructionCall {
                instruction,
                args: args.iter().map(|arg| arg.expr.clone()).collect(),
            }));
        }
        _ => record_issue(
            &mut program.issues,
            AngularRecoveryIssue {
                kind: AngularRecoveryIssueKind::UnsupportedExpression,
                instruction: None,
                detail: Some(expression_kind(expression).to_string()),
            },
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

fn apply_create_instruction(
    call: &InstructionCall,
    tree: &mut TemplateTree,
    program: &mut TemplateProgram,
    environment: &TemplateRecoveryEnvironment<'_>,
    active_templates: &mut HashSet<BindingKey>,
    depth: usize,
) -> Result<()> {
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
                .and_then(|index| environment.constants.get(index).cloned())
                .unwrap_or_default();
            let node = tree.push_node(index, TemplateNodeKind::Element { tag, attributes });
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
                .and_then(|index| environment.constants.get(index).cloned())
                .unwrap_or_default();
            tree.push_node(index, TemplateNodeKind::Element { tag, attributes });
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
            let Ok(expression) = handler_expression(
                handler.as_ref(),
                &program.component_contexts,
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
            tree.add_attribute(
                node,
                TemplateAttribute {
                    name: format!("({event})"),
                    value: Some(expression),
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
            let child = recover_template_tree(
                &resolved.function,
                environment,
                false,
                active_templates,
                depth + 1,
            );
            if let Some(key) = &resolved.key {
                active_templates.remove(key);
            }
            let (child_tree, child_program) = child?;
            merge_template_program(program, child_program);
            let attributes = numeric_arg(&call.args, 5)
                .and_then(|index| environment.constants.get(index).cloned())
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

fn apply_update_instruction(
    call: &InstructionCall,
    tree: &mut TemplateTree,
    cm: Lrc<SourceMap>,
    program: &mut TemplateProgram,
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
            let Ok(value) = interpolation_value(call, &program.component_contexts, cm) else {
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
            let Ok(expression) =
                print_template_expression(value.as_ref(), &program.component_contexts, cm)
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
            if !apply_conditional_instruction(call, tree, cm, program) {
                return Ok(());
            }
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
    let Some(branches) =
        decode_conditional_branches(selection.as_ref(), &program.component_contexts, cm)
    else {
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
    true
}

fn decode_conditional_branches(
    selection: &Expr,
    component_contexts: &HashSet<BindingKey>,
    cm: Lrc<SourceMap>,
) -> Option<Vec<(usize, ConditionalBranch)>> {
    let mut branches = Vec::new();
    decode_conditional_branch(
        strip_parentheses(selection),
        true,
        component_contexts,
        cm,
        &mut branches,
    )?;
    (!branches.is_empty()).then_some(branches)
}

fn decode_conditional_branch(
    selection: &Expr,
    first: bool,
    component_contexts: &HashSet<BindingKey>,
    cm: Lrc<SourceMap>,
    branches: &mut Vec<(usize, ConditionalBranch)>,
) -> Option<()> {
    let Expr::Cond(conditional) = strip_parentheses(selection) else {
        return None;
    };
    let condition =
        print_template_expression(conditional.test.as_ref(), component_contexts, cm.clone())
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
        AngularRecoveryIssue {
            kind: AngularRecoveryIssueKind::MalformedInstruction,
            instruction: Some(call.instruction.canonical_export_name().to_string()),
            detail: Some(detail.to_string()),
        },
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
        AngularRecoveryIssue {
            kind: AngularRecoveryIssueKind::MissingTargetNode,
            instruction: Some(call.instruction.canonical_export_name().to_string()),
            detail: Some(detail.to_string()),
        },
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
        AngularRecoveryIssue {
            kind: AngularRecoveryIssueKind::UnsupportedInstruction,
            instruction: Some(call.instruction.canonical_export_name().to_string()),
            detail: Some(detail.to_string()),
        },
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
    call.args.len() == expected
}

fn interpolation_value(
    call: &InstructionCall,
    component_contexts: &HashSet<BindingKey>,
    cm: Lrc<SourceMap>,
) -> Result<String> {
    if call.instruction == IvyInstruction::TextInterpolate {
        let expression = call
            .args
            .first()
            .map(|expr| print_template_expression(expr.as_ref(), component_contexts, cm))
            .transpose()?
            .unwrap_or_default();
        return Ok(format!("{{{{ {expression} }}}}"));
    }

    let mut output = String::new();
    for (index, argument) in call.args.iter().enumerate() {
        if index % 2 == 0 {
            output.push_str(&string_lit(argument.as_ref()).unwrap_or_default());
        } else {
            let expression =
                print_template_expression(argument.as_ref(), component_contexts, cm.clone())?;
            output.push_str("{{ ");
            output.push_str(&expression);
            output.push_str(" }}");
        }
    }
    Ok(output)
}

fn numeric_arg(args: &[Box<Expr>], index: usize) -> Option<usize> {
    let Expr::Lit(Lit::Num(number)) = args.get(index)?.as_ref() else {
        return None;
    };
    (number.value >= 0.0 && number.value.fract() == 0.0).then_some(number.value as usize)
}

fn decode_component_constant_table(constants: &Expr) -> Vec<Vec<TemplateAttribute>> {
    let Expr::Array(table) = constants else {
        return Vec::new();
    };
    table
        .elems
        .iter()
        .map(|entry| {
            let Some(entry) = entry else {
                return Vec::new();
            };
            decode_constant_attributes(entry.expr.as_ref())
        })
        .collect()
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
            TemplateNodeKind::Text { .. } => {}
        }
    }
}

fn render_tree(tree: &TemplateTree) -> String {
    render_tree_at_depth(tree, 0)
}

fn render_tree_at_depth(tree: &TemplateTree, depth: usize) -> String {
    tree.roots
        .iter()
        .map(|&node| render_node(tree, node, depth))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_node(tree: &TemplateTree, node: usize, depth: usize) -> String {
    let current = &tree.nodes[node];
    let indent = "  ".repeat(depth);
    match &current.kind {
        TemplateNodeKind::Text { value } => format!("{indent}{}", escape_text(value)),
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
                .map(|&child| render_node(tree, child, depth + 1))
                .collect::<Vec<_>>()
                .join("\n");
            format!("{indent}<{tag}{attributes}>\n{children}\n{indent}</{tag}>")
        }
        TemplateNodeKind::EmbeddedView {
            tree: embedded,
            attributes,
            branch,
        } => {
            let body = render_tree_at_depth(embedded, depth + 1);
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
