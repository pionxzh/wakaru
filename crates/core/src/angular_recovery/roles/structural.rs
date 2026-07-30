use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, AssignTarget, BinaryOp, BlockStmt, BlockStmtOrExpr, CallExpr,
    Callee, CatchClause, ClassDecl, Expr, ExprOrSpread, FnDecl, ForHead, ForInStmt, ForOfStmt,
    Function, ImportDecl, ImportSpecifier, Lit, MemberProp, ObjectPatProp, Pat, Prop, PropName,
    ReturnStmt, SimpleAssignTarget, Stmt, UnaryExpr, UnaryOp, UpdateExpr, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::{symbol_identity, IvyInstruction, IvyRoleTable, SymbolIdentity};
use crate::angular_recovery::syntax::{binding_key, member_prop_name, BindingKey};
use crate::angular_recovery::PreparedAngularModule;

pub(super) struct StructuralRoleEvidence {
    functions: Vec<RuntimeFunction>,
    definition_counts: HashMap<SymbolIdentity, usize>,
    invalid_values: HashSet<SymbolIdentity>,
    assignment_definitions: HashMap<SymbolIdentity, Vec<(usize, u32)>>,
}

impl StructuralRoleEvidence {
    pub(super) fn collect(modules: &[PreparedAngularModule]) -> Self {
        collect_runtime_functions(modules)
    }

    pub(super) fn is_stable_export_reference(
        &self,
        identity: &SymbolIdentity,
        module_index: usize,
        position: u32,
    ) -> bool {
        self.is_stable_at(identity, module_index, position)
            && match identity {
                SymbolIdentity::LocalMember { object, .. } => self.is_stable_at(
                    &SymbolIdentity::LocalBinding(object.clone()),
                    module_index,
                    position,
                ),
                SymbolIdentity::GlobalMember { object, .. } => {
                    let root = object
                        .split('.')
                        .next()
                        .map(Atom::from)
                        .expect("global member paths are non-empty");
                    self.is_stable_at(&SymbolIdentity::GlobalBinding(root), module_index, position)
                }
                SymbolIdentity::LocalBinding(_) | SymbolIdentity::GlobalBinding(_) => true,
            }
    }

    fn is_stable_at(&self, identity: &SymbolIdentity, module_index: usize, position: u32) -> bool {
        if self.invalid_values.contains(identity) {
            return false;
        }
        match self.definition_counts.get(identity).copied().unwrap_or(0) {
            0 => true,
            1 => self
                .assignment_definitions
                .get(identity)
                .is_none_or(|assignments| {
                    matches!(
                        assignments.as_slice(),
                        [(assignment_module, assignment_position)]
                            if *assignment_module == module_index
                                && *assignment_position <= position
                    )
                }),
            _ => false,
        }
    }

    pub(super) fn infer_ivy_roles(&self) -> Vec<(SymbolIdentity, &'static str)> {
        let mut inferred = self
            .functions
            .iter()
            .filter(|function| is_define_component_shape(function))
            .map(|function| (function.identity.clone(), "ɵɵdefineComponent"))
            .collect::<Vec<_>>();
        inferred.extend(infer_element_family(&self.functions));
        inferred
    }

    pub(super) fn infer_template_roles(
        &self,
        modules: &[PreparedAngularModule],
        roles: &IvyRoleTable,
    ) -> Vec<(SymbolIdentity, &'static str)> {
        let function_index = RuntimeFunctionIndex::new(&self.functions, roles);
        let mut observations = Vec::new();
        let mut next_view_id = 0;
        for prepared in modules {
            let mut collector = TemplateFunctionCollector {
                roles,
                function_index: &function_index,
                unresolved_ctxt: prepared.unresolved_ctxt,
                observations: Vec::new(),
                next_view_id,
            };
            prepared.module.visit_with(&mut collector);
            next_view_id = collector.next_view_id;
            observations.extend(collector.observations);
        }

        let mut by_identity: HashMap<SymbolIdentity, Vec<TemplateCallObservation>> = HashMap::new();
        for observation in observations {
            by_identity
                .entry(observation.identity.clone())
                .or_default()
                .push(observation);
        }

        let mut inferred = infer_specialized_element_pair(&function_index, &by_identity);
        inferred.extend(infer_text_interpolation_family(
            &function_index,
            &by_identity,
        ));
        inferred.extend(infer_embedded_template_continuation_family(
            &function_index,
            &by_identity,
        ));
        inferred.extend(infer_defer_role_family(&function_index, &by_identity));
        inferred.extend(infer_repeater_role_family(&function_index, &by_identity));
        inferred.extend(infer_view_state_role_family(&self.functions, modules));
        for (identity, observations) in &by_identity {
            let Some(definition) = function_index.unique(identity) else {
                continue;
            };

            let mut matches = Vec::new();
            if is_text_shape(definition, observations) {
                matches.push("ɵɵtext");
            }
            if is_listener_shape(definition, observations) {
                matches.push("ɵɵlistener");
            }
            if is_advance_shape(definition, observations) {
                matches.push("ɵɵadvance");
            }
            if is_property_shape(definition, observations) {
                matches.push("ɵɵproperty");
            }
            if is_embedded_template_shape(definition, observations) {
                matches.push("ɵɵtemplate");
            }
            if is_conditional_shape(definition, observations) {
                matches.push("ɵɵconditional");
            }
            if is_next_context_shape(definition, observations) {
                matches.push("ɵɵnextContext");
            }
            if is_projection_def_shape(definition, observations) {
                matches.push("ɵɵprojectionDef");
            }
            if is_projection_shape(definition, observations) {
                matches.push("ɵɵprojection");
            }
            if is_reference_shape(definition, observations) {
                matches.push("ɵɵreference");
            }
            if is_pipe_shape(definition, observations) {
                matches.push("ɵɵpipe");
            }
            if let Some(name) = pipe_binding_shape(definition, observations) {
                matches.push(name);
            }
            if let [name] = matches.as_slice() {
                inferred.push((definition.identity.clone(), *name));
            }
        }
        inferred
    }
}

fn collect_runtime_functions(modules: &[PreparedAngularModule]) -> StructuralRoleEvidence {
    let mut functions = Vec::new();
    let mut definition_counts = HashMap::<SymbolIdentity, usize>::new();
    let mut invalid_values = HashSet::new();
    let mut assignment_definitions = HashMap::<SymbolIdentity, Vec<(usize, u32)>>::new();
    for (module_index, prepared) in modules.iter().enumerate() {
        let mut collector = RuntimeFunctionCollector {
            module_index,
            unresolved_ctxt: prepared.unresolved_ctxt,
            functions: Vec::new(),
            definition_counts: HashMap::new(),
            invalid_values: HashSet::new(),
            assignment_definitions: HashMap::new(),
        };
        prepared.module.visit_with(&mut collector);
        functions.extend(collector.functions);
        for (identity, count) in collector.definition_counts {
            *definition_counts.entry(identity).or_default() += count;
        }
        invalid_values.extend(collector.invalid_values);
        for (identity, assignments) in collector.assignment_definitions {
            assignment_definitions
                .entry(identity)
                .or_default()
                .extend(assignments);
        }
    }

    let stable_values = definition_counts
        .iter()
        .filter(|(identity, count)| {
            **count == 1
                && !invalid_values.contains(*identity)
                && has_stable_container(identity, &definition_counts, &invalid_values)
        })
        .map(|(identity, _)| identity.clone())
        .collect::<HashSet<_>>();
    functions.retain(|function| stable_values.contains(&function.identity));

    StructuralRoleEvidence {
        functions,
        definition_counts,
        invalid_values,
        assignment_definitions,
    }
}

fn has_stable_container(
    identity: &SymbolIdentity,
    definition_counts: &HashMap<SymbolIdentity, usize>,
    invalid_values: &HashSet<SymbolIdentity>,
) -> bool {
    match identity {
        SymbolIdentity::LocalMember { object, .. } => {
            let container = SymbolIdentity::LocalBinding(object.clone());
            definition_counts.get(&container) == Some(&1) && !invalid_values.contains(&container)
        }
        SymbolIdentity::GlobalMember { object, .. } => {
            let root = object
                .split('.')
                .next()
                .map(Atom::from)
                .expect("global member paths are non-empty");
            let container = SymbolIdentity::GlobalBinding(root);
            definition_counts
                .get(&container)
                .is_none_or(|count| *count <= 1)
                && !invalid_values.contains(&container)
        }
        SymbolIdentity::LocalBinding(_) | SymbolIdentity::GlobalBinding(_) => true,
    }
}

#[derive(Clone)]
struct RuntimeFunction {
    identity: SymbolIdentity,
    params: Vec<Pat>,
    body: BlockStmt,
    unresolved_ctxt: SyntaxContext,
}

struct RuntimeFunctionIndex<'a> {
    exact: HashMap<&'a SymbolIdentity, Vec<&'a RuntimeFunction>>,
    aliases: HashMap<usize, Vec<&'a RuntimeFunction>>,
    roles: &'a IvyRoleTable,
}

impl<'a> RuntimeFunctionIndex<'a> {
    fn new(functions: &'a [RuntimeFunction], roles: &'a IvyRoleTable) -> Self {
        let mut exact = HashMap::new();
        let mut aliases = HashMap::new();
        for function in functions {
            if let Some(group) = roles.alias_group_index(&function.identity) {
                aliases.entry(group).or_insert_with(Vec::new).push(function);
            } else {
                exact
                    .entry(&function.identity)
                    .or_insert_with(Vec::new)
                    .push(function);
            }
        }
        Self {
            exact,
            aliases,
            roles,
        }
    }

    fn unique(&self, identity: &SymbolIdentity) -> Option<&'a RuntimeFunction> {
        let candidates = if let Some(group) = self.roles.alias_group_index(identity) {
            self.aliases.get(&group)?
        } else {
            self.exact.get(identity)?
        };
        let [candidate] = candidates.as_slice() else {
            return None;
        };
        Some(*candidate)
    }
}

struct RuntimeFunctionCollector {
    module_index: usize,
    unresolved_ctxt: SyntaxContext,
    functions: Vec<RuntimeFunction>,
    definition_counts: HashMap<SymbolIdentity, usize>,
    invalid_values: HashSet<SymbolIdentity>,
    assignment_definitions: HashMap<SymbolIdentity, Vec<(usize, u32)>>,
}

struct TemplateCallObservation {
    identity: SymbolIdentity,
    phase: u8,
    arguments: Vec<Box<Expr>>,
    usage: TemplateCallUsage,
    view_id: usize,
    call_order: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TemplateCallUsage {
    Effect,
    Initializer,
}

struct TemplateFunctionCollector<'a> {
    roles: &'a IvyRoleTable,
    function_index: &'a RuntimeFunctionIndex<'a>,
    unresolved_ctxt: SyntaxContext,
    observations: Vec<TemplateCallObservation>,
    next_view_id: usize,
}

impl Visit for TemplateFunctionCollector<'_> {
    fn visit_function(&mut self, function: &Function) {
        let view_id = self.next_view_id;
        self.next_view_id += 1;
        let Some(render_flags) = function_param_binding(function, 0) else {
            function.visit_children_with(self);
            return;
        };
        let mut observer = TemplateCallObserver {
            roles: self.roles,
            unresolved_ctxt: self.unresolved_ctxt,
            render_flags,
            saw_creation_anchor: false,
            observations: Vec::new(),
            view_id,
            next_call_order: 0,
        };
        if let Some(body) = &function.body {
            observer.collect_statements(&body.stmts, None);
        }
        if observer.saw_creation_anchor
            || has_unclassified_element_anchor(&observer.observations, self.function_index)
            || has_unclassified_defer_anchor(&observer.observations, self.function_index)
            || has_unclassified_repeater_anchor(&observer.observations, self.function_index)
        {
            self.observations.extend(observer.observations);
        }
        function.visit_children_with(self);
    }
}

struct TemplateCallObserver<'a> {
    roles: &'a IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
    render_flags: BindingKey,
    saw_creation_anchor: bool,
    observations: Vec<TemplateCallObservation>,
    view_id: usize,
    next_call_order: usize,
}

impl TemplateCallObserver<'_> {
    fn collect_statements(&mut self, statements: &[Stmt], phase: Option<u8>) {
        for statement in statements {
            self.collect_statement(statement, phase);
        }
    }

    fn collect_statement(&mut self, statement: &Stmt, phase: Option<u8>) {
        match statement {
            Stmt::Block(block) => self.collect_statements(&block.stmts, phase),
            Stmt::If(if_statement) => {
                let branch_phase = self.collect_if_test(if_statement.test.as_ref(), phase);
                self.collect_statement(if_statement.cons.as_ref(), branch_phase);
                if let Some(alternate) = &if_statement.alt {
                    self.collect_statement(alternate.as_ref(), phase);
                }
            }
            Stmt::Expr(expression) => self.collect_expression(expression.expr.as_ref(), phase),
            Stmt::Decl(swc_core::ecma::ast::Decl::Var(declaration)) => {
                for declarator in &declaration.decls {
                    if let Some(initializer) = &declarator.init {
                        self.collect_initializer(initializer.as_ref(), phase);
                    }
                }
            }
            _ => {}
        }
    }

    fn collect_if_test(&mut self, test: &Expr, phase: Option<u8>) -> Option<u8> {
        let test = strip_parentheses(test);
        let Expr::Seq(sequence) = test else {
            return render_flag_mask(test, &self.render_flags).or(phase);
        };
        let Some((condition, effects)) = sequence.exprs.split_last() else {
            return phase;
        };
        for effect in effects {
            self.collect_expression(effect.as_ref(), phase);
        }
        render_flag_mask(strip_parentheses(condition), &self.render_flags).or(phase)
    }

    fn collect_expression(&mut self, expression: &Expr, phase: Option<u8>) {
        match expression {
            Expr::Paren(paren) => self.collect_expression(paren.expr.as_ref(), phase),
            Expr::Seq(sequence) => {
                for expression in &sequence.exprs {
                    self.collect_expression(expression.as_ref(), phase);
                }
            }
            Expr::Bin(binary) if binary.op == BinaryOp::LogicalAnd => {
                let branch_phase =
                    render_flag_mask(binary.left.as_ref(), &self.render_flags).or(phase);
                self.collect_expression(binary.right.as_ref(), branch_phase);
            }
            Expr::Assign(assignment) => {
                self.collect_initializer(assignment.right.as_ref(), phase);
            }
            Expr::Call(call) => self.collect_call(call, phase, TemplateCallUsage::Effect),
            _ => {}
        }
    }

    fn collect_initializer(&mut self, expression: &Expr, phase: Option<u8>) {
        match expression {
            Expr::Paren(parenthesized) => {
                self.collect_initializer(parenthesized.expr.as_ref(), phase)
            }
            Expr::Call(call) => self.collect_call(call, phase, TemplateCallUsage::Initializer),
            _ => self.collect_expression(expression, phase),
        }
    }

    fn collect_call(&mut self, call: &CallExpr, phase: Option<u8>, usage: TemplateCallUsage) {
        let Some(phase @ (1 | 2)) = phase else {
            return;
        };
        for argument in &call.args {
            self.collect_expression(argument.expr.as_ref(), Some(phase));
        }
        let call_order = self.next_call_order;
        self.next_call_order += 1;
        let Some((root, argument_lists)) = call_chain(call) else {
            return;
        };
        if self
            .roles
            .instruction_for_expr(root, self.unresolved_ctxt)
            .is_some_and(|instruction| {
                phase == 1
                    && matches!(
                        instruction,
                        IvyInstruction::ElementStart
                            | IvyInstruction::ElementEnd
                            | IvyInstruction::Element
                    )
            })
        {
            self.saw_creation_anchor = true;
            return;
        }
        if self
            .roles
            .instruction_for_expr(root, self.unresolved_ctxt)
            .is_some()
        {
            return;
        }
        let Some(identity) = symbol_identity(root, self.unresolved_ctxt) else {
            return;
        };
        self.observations
            .extend(argument_lists.into_iter().map(|arguments| {
                TemplateCallObservation {
                    identity: identity.clone(),
                    phase,
                    arguments: arguments
                        .iter()
                        .map(|argument| argument.expr.clone())
                        .collect(),
                    usage,
                    view_id: self.view_id,
                    call_order,
                }
            }));
    }
}

fn function_param_binding(function: &Function, index: usize) -> Option<BindingKey> {
    let Pat::Ident(binding) = &function.params.get(index)?.pat else {
        return None;
    };
    Some(binding_key(&binding.id))
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
    let (Expr::Ident(identifier), Expr::Lit(Lit::Num(mask))) =
        (binary.left.as_ref(), binary.right.as_ref())
    else {
        return None;
    };
    (binding_key(identifier) == *render_flags && (mask.value == 1.0 || mask.value == 2.0))
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

fn has_unclassified_element_anchor(
    observations: &[TemplateCallObservation],
    function_index: &RuntimeFunctionIndex<'_>,
) -> bool {
    let mut grouped: HashMap<&SymbolIdentity, Vec<&TemplateCallObservation>> = HashMap::new();
    for observation in observations {
        grouped
            .entry(&observation.identity)
            .or_default()
            .push(observation);
    }

    let has_start = grouped.iter().any(|(identity, observations)| {
        function_index
            .unique(identity)
            .is_some_and(|definition| is_specialized_element_start_shape(definition, observations))
    });
    let has_end = grouped.iter().any(|(identity, observations)| {
        function_index
            .unique(identity)
            .is_some_and(|definition| is_specialized_element_end_shape(definition, observations))
    });
    has_start && has_end
}

fn has_unclassified_defer_anchor(
    observations: &[TemplateCallObservation],
    function_index: &RuntimeFunctionIndex<'_>,
) -> bool {
    let has_template = observations.iter().any(|observation| {
        observation.phase == 1 && is_embedded_template_arguments(&observation.arguments)
    });
    has_template
        && observations.iter().any(|observation| {
            observation.phase == 1
                && function_index
                    .unique(&observation.identity)
                    .is_some_and(|definition| {
                        (6..=10).contains(&definition.params.len())
                            && contains_string_literal(&definition.body, "NgDefer")
                    })
        })
}

fn has_unclassified_repeater_anchor(
    observations: &[TemplateCallObservation],
    function_index: &RuntimeFunctionIndex<'_>,
) -> bool {
    observations.iter().any(|observation| {
        let Some(definition) = function_index.unique(&observation.identity) else {
            return false;
        };
        observation.phase == 1
            && is_repeater_create_arguments(&observation.arguments, definition.unresolved_ctxt)
            && is_repeater_create_definition(definition)
    })
}

fn infer_specialized_element_pair(
    function_index: &RuntimeFunctionIndex<'_>,
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
) -> Vec<(SymbolIdentity, &'static str)> {
    let mut starts_by_view: HashMap<usize, HashSet<SymbolIdentity>> = HashMap::new();
    let mut ends_by_view: HashMap<usize, HashSet<SymbolIdentity>> = HashMap::new();
    for (identity, calls) in observations {
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        if is_specialized_element_start_shape(definition, calls) {
            for call in calls {
                starts_by_view
                    .entry(call.view_id)
                    .or_default()
                    .insert(definition.identity.clone());
            }
        }
        if is_specialized_element_end_shape(definition, calls) {
            for call in calls {
                ends_by_view
                    .entry(call.view_id)
                    .or_default()
                    .insert(definition.identity.clone());
            }
        }
    }

    let mut proven_starts = HashSet::new();
    let mut proven_ends = HashSet::new();
    for (view_id, starts) in starts_by_view {
        let Some(ends) = ends_by_view.get(&view_id) else {
            continue;
        };
        let (Some(start), Some(end)) =
            (single_identity(starts.iter()), single_identity(ends.iter()))
        else {
            continue;
        };
        if start != end {
            proven_starts.insert(start.clone());
            proven_ends.insert(end.clone());
        }
    }
    proven_starts
        .into_iter()
        .map(|identity| (identity, "ɵɵelementStart"))
        .chain(
            proven_ends
                .into_iter()
                .map(|identity| (identity, "ɵɵelementEnd")),
        )
        .collect()
}

fn single_identity<'a>(
    mut identities: impl Iterator<Item = &'a SymbolIdentity>,
) -> Option<&'a SymbolIdentity> {
    let identity = identities.next()?;
    identities.next().is_none().then_some(identity)
}

fn infer_embedded_template_continuation_family(
    function_index: &RuntimeFunctionIndex<'_>,
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
) -> Vec<(SymbolIdentity, &'static str)> {
    let mut inferred = Vec::new();
    for (identity, calls_in_templates) in observations {
        if !calls_in_templates.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && is_embedded_template_arguments(&observation.arguments)
        }) {
            continue;
        }

        let Some(wrapper) = function_index.unique(identity) else {
            continue;
        };
        let Some(wrapper_parameters) = plain_parameter_bindings(wrapper) else {
            continue;
        };
        if !matches!(wrapper_parameters.len(), 4..=8) {
            continue;
        }
        let Some(continuation_identity) = exact_returned_identity(wrapper) else {
            continue;
        };
        if continuation_identity == wrapper.identity {
            continue;
        }
        let Some(continuation) = function_index.unique(&continuation_identity) else {
            continue;
        };
        let Some(continuation_parameters) = plain_parameter_bindings(continuation) else {
            continue;
        };
        if continuation_parameters.len() != 8
            || !returns_identity(continuation, &continuation.identity)
        {
            continue;
        }

        let forwarding_targets = direct_calls(continuation)
            .into_iter()
            .filter(|call| {
                call.arguments.len() >= continuation_parameters.len()
                    && forwards_parameter_dependencies_in_order(call, &continuation_parameters)
            })
            .map(|call| call.callee)
            .collect::<HashSet<_>>();
        if forwarding_targets.is_empty()
            || !direct_calls(wrapper).iter().any(|call| {
                forwarding_targets.contains(&call.callee)
                    && forwards_parameter_dependencies_in_order(call, &wrapper_parameters)
            })
        {
            continue;
        }

        inferred.push((wrapper.identity.clone(), "ɵɵtemplate"));
        inferred.push((continuation.identity.clone(), "ɵɵtemplate"));
    }
    inferred
}

fn infer_defer_role_family(
    function_index: &RuntimeFunctionIndex<'_>,
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
) -> Vec<(SymbolIdentity, &'static str)> {
    let mut template_slots_by_view = HashMap::<usize, HashSet<usize>>::new();
    for calls in observations.values() {
        for call in calls {
            if is_embedded_template_arguments(&call.arguments) {
                if let Some(index) = nonnegative_integer_value(call.arguments[0].as_ref()) {
                    template_slots_by_view
                        .entry(call.view_id)
                        .or_default()
                        .insert(index);
                }
            }
        }
    }

    let mut inferred = Vec::new();
    let mut defer_views = HashSet::new();
    let mut ordinary_trigger_positions = HashSet::new();
    for (identity, calls) in observations {
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        if !(6..=10).contains(&definition.params.len())
            || !contains_string_literal(&definition.body, "NgDefer")
            || !calls.iter().all(|call| {
                call.usage == TemplateCallUsage::Effect
                    && call.phase == 1
                    && template_slots_by_view
                        .get(&call.view_id)
                        .is_some_and(|slots| {
                            is_defer_arguments(&call.arguments, slots, definition.unresolved_ctxt)
                        })
            })
        {
            continue;
        }
        defer_views.extend(calls.iter().map(|call| call.view_id));
        ordinary_trigger_positions.extend(
            calls
                .iter()
                .map(|call| (call.view_id, call.call_order.saturating_add(1))),
        );
        inferred.push((identity.clone(), "ɵɵdefer"));
    }

    if inferred.is_empty() {
        return inferred;
    }
    for (identity, calls) in observations {
        if inferred
            .iter()
            .any(|(defer_identity, _)| defer_identity == identity)
        {
            continue;
        }
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        let Some(parameters) = plain_parameter_bindings(definition) else {
            continue;
        };
        let [parameter] = parameters.as_slice() else {
            continue;
        };
        if contains_timeout_parameter_object(definition, parameter)
            && has_ordinary_idle_scheduler_proof(definition)
            && calls.iter().all(|call| {
                defer_views.contains(&call.view_id)
                    && ordinary_trigger_positions.contains(&(call.view_id, call.call_order))
                    && call.usage == TemplateCallUsage::Effect
                    && call.phase == 1
                    && matches!(call.arguments.len(), 0 | 1)
                    && call
                        .arguments
                        .first()
                        .is_none_or(|argument| is_nonnegative_integer(argument.as_ref()))
            })
        {
            inferred.push((identity.clone(), "ɵɵdeferOnIdle"));
        }
    }
    inferred
}

fn is_defer_arguments(
    arguments: &[Box<Expr>],
    template_slots: &HashSet<usize>,
    unresolved_ctxt: SyntaxContext,
) -> bool {
    if !(3..=10).contains(&arguments.len()) {
        return false;
    }
    let Some(defer_index) = nonnegative_integer_value(arguments[0].as_ref()) else {
        return false;
    };
    let Some(primary_index) = nonnegative_integer_value(arguments[1].as_ref()) else {
        return false;
    };
    if defer_index == primary_index || !template_slots.contains(&primary_index) {
        return false;
    }
    if !is_nullish_or_callable(arguments[2].as_ref(), unresolved_ctxt) {
        return false;
    }

    let mut referenced = HashSet::from([primary_index]);
    for argument in arguments.iter().take(6).skip(3) {
        if is_nullish(argument.as_ref(), unresolved_ctxt) {
            continue;
        }
        let Some(index) = nonnegative_integer_value(argument.as_ref()) else {
            return false;
        };
        if !template_slots.contains(&index) || !referenced.insert(index) {
            return false;
        }
    }
    arguments.get(6).is_none_or(|argument| {
        is_nullish(argument.as_ref(), unresolved_ctxt) || is_nonnegative_integer(argument.as_ref())
    }) && arguments.get(7).is_none_or(|argument| {
        is_nullish(argument.as_ref(), unresolved_ctxt) || is_nonnegative_integer(argument.as_ref())
    }) && arguments
        .get(8)
        .is_none_or(|argument| is_nullish_or_callable(argument.as_ref(), unresolved_ctxt))
        && arguments.get(9).is_none_or(|argument| {
            is_nullish(argument.as_ref(), unresolved_ctxt)
                || is_nonnegative_integer(argument.as_ref())
        })
}

fn nonnegative_integer_value(expression: &Expr) -> Option<usize> {
    let Expr::Lit(Lit::Num(number)) = strip_parentheses(expression) else {
        return None;
    };
    (number.value >= 0.0 && number.value.fract() == 0.0).then_some(number.value as usize)
}

fn is_nullish(expression: &Expr, unresolved_ctxt: SyntaxContext) -> bool {
    match strip_parentheses(expression) {
        Expr::Lit(Lit::Null(_)) => true,
        Expr::Ident(identifier) => {
            identifier.sym == "undefined" && identifier.ctxt == unresolved_ctxt
        }
        Expr::Unary(unary) if unary.op == UnaryOp::Void => true,
        _ => false,
    }
}

fn is_nullish_or_callable(expression: &Expr, unresolved_ctxt: SyntaxContext) -> bool {
    is_nullish(expression, unresolved_ctxt)
        || matches!(
            strip_parentheses(expression),
            Expr::Ident(_) | Expr::Member(_) | Expr::Fn(_) | Expr::Arrow(_)
        )
}

fn contains_string_literal(block: &BlockStmt, expected: &str) -> bool {
    struct Finder<'a> {
        expected: &'a str,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_expr(&mut self, expression: &Expr) {
            if matches!(
                expression,
                Expr::Lit(Lit::Str(string)) if string.value == self.expected
            ) {
                self.found = true;
                return;
            }
            expression.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut finder = Finder {
        expected,
        found: false,
    };
    block.visit_with(&mut finder);
    finder.found
}

fn contains_timeout_parameter_object(function: &RuntimeFunction, parameter: &BindingKey) -> bool {
    struct Finder<'a> {
        parameter: &'a BindingKey,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_prop(&mut self, property: &Prop) {
            let matches = match property {
                Prop::Shorthand(identifier) => {
                    identifier.sym == "timeout" && binding_key(identifier) == *self.parameter
                }
                Prop::KeyValue(key_value) if prop_name_is(&key_value.key, "timeout") => matches!(
                    strip_parentheses(key_value.value.as_ref()),
                    Expr::Ident(identifier) if binding_key(identifier) == *self.parameter
                ),
                _ => false,
            };
            if matches {
                self.found = true;
                return;
            }
            property.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut finder = Finder {
        parameter,
        found: false,
    };
    function.body.visit_with(&mut finder);
    finder.found
}

fn has_ordinary_idle_scheduler_proof(function: &RuntimeFunction) -> bool {
    struct Finder {
        found: bool,
    }

    impl Visit for Finder {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if self.found {
                return;
            }
            let has_ordinary_mode = call.args.len() >= 3
                && call.args.first().is_some_and(|argument| {
                    matches!(
                        strip_parentheses(argument.expr.as_ref()),
                        Expr::Lit(Lit::Num(number)) if number.value == 0.0
                    )
                });
            let has_named_scheduler = match &call.callee {
                Callee::Expr(callee) => match strip_parentheses(callee.as_ref()) {
                    Expr::Ident(identifier) => matches!(
                        identifier.sym.as_ref(),
                        "scheduleIdle" | "scheduleDelayedTrigger"
                    ),
                    Expr::Member(member) => {
                        member_prop_name(&member.prop).is_some_and(|property| {
                            matches!(property.as_ref(), "scheduleIdle" | "scheduleDelayedTrigger")
                        })
                    }
                    _ => false,
                },
                _ => false,
            };
            if has_ordinary_mode || has_named_scheduler {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut finder = Finder { found: false };
    function.body.visit_with(&mut finder);
    finder.found
}

fn prop_name_is(name: &PropName, expected: &str) -> bool {
    match name {
        PropName::Ident(identifier) => identifier.sym == expected,
        PropName::Str(string) => string.value == expected,
        _ => false,
    }
}

fn infer_repeater_role_family(
    function_index: &RuntimeFunctionIndex<'_>,
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
) -> Vec<(SymbolIdentity, &'static str)> {
    let mut inferred = Vec::new();
    let mut repeater_views = HashSet::new();
    let mut track_candidates = HashSet::new();
    for (identity, calls) in observations {
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        if !is_repeater_create_definition(definition)
            || !calls.iter().all(|call| {
                call.usage == TemplateCallUsage::Effect
                    && call.phase == 1
                    && is_repeater_create_arguments(&call.arguments, definition.unresolved_ctxt)
            })
        {
            continue;
        }
        repeater_views.extend(calls.iter().map(|call| call.view_id));
        track_candidates.extend(calls.iter().filter_map(|call| {
            symbol_identity(call.arguments.get(6)?.as_ref(), definition.unresolved_ctxt)
        }));
        inferred.push((identity.clone(), "ɵɵrepeaterCreate"));
    }
    if inferred.is_empty() {
        return inferred;
    }

    let mut updates_by_view = HashMap::<usize, HashSet<SymbolIdentity>>::new();
    for (identity, calls) in observations {
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        if definition.params.len() == 1
            && contains_try_finally(&definition.body)
            && contains_member_property(&definition.body, "selectedIndex")
            && direct_calls(definition).len() >= 5
            && calls.iter().all(|call| {
                call.usage == TemplateCallUsage::Effect
                    && call.phase == 2
                    && call.arguments.len() == 1
            })
        {
            for view_id in calls
                .iter()
                .map(|call| call.view_id)
                .filter(|view_id| repeater_views.contains(view_id))
            {
                updates_by_view
                    .entry(view_id)
                    .or_default()
                    .insert(identity.clone());
            }
        }
    }
    let mut proven_updates = HashSet::new();
    for candidates in updates_by_view.values() {
        if let Some(candidate) = single_identity(candidates.iter()) {
            proven_updates.insert(candidate.clone());
        }
    }
    inferred.extend(
        proven_updates
            .into_iter()
            .map(|identity| (identity, "ɵɵrepeater")),
    );

    for identity in track_candidates {
        let Some(definition) = function_index.unique(&identity) else {
            continue;
        };
        let Some(parameters) = plain_parameter_bindings(definition) else {
            continue;
        };
        let role = if parameters.len() == 1
            && exact_returned_identity(definition)
                == Some(SymbolIdentity::LocalBinding(parameters[0].clone()))
        {
            Some("ɵɵrepeaterTrackByIndex")
        } else if parameters.len() == 2
            && exact_returned_identity(definition)
                == Some(SymbolIdentity::LocalBinding(parameters[1].clone()))
        {
            Some("ɵɵrepeaterTrackByIdentity")
        } else {
            None
        };
        if let Some(role) = role {
            inferred.push((identity, role));
        }
    }
    inferred
}

fn is_repeater_create_definition(definition: &RuntimeFunction) -> bool {
    definition.params.len() == 13 && contains_string_literal(&definition.body, "NgControlFlow")
}

fn is_repeater_create_arguments(arguments: &[Box<Expr>], unresolved_ctxt: SyntaxContext) -> bool {
    matches!(arguments.len(), 7..=13)
        && arguments
            .first()
            .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
        && arguments
            .get(1)
            .is_some_and(|argument| is_callable(argument.as_ref()))
        && arguments
            .get(2)
            .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
        && arguments
            .get(3)
            .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
        && arguments.get(4).is_some_and(|argument| {
            is_nullish(argument.as_ref(), unresolved_ctxt) || is_string_literal(argument.as_ref())
        })
        && arguments.get(5).is_some_and(|argument| {
            is_nullish(argument.as_ref(), unresolved_ctxt)
                || is_nonnegative_integer(argument.as_ref())
        })
        && arguments
            .get(6)
            .is_some_and(|argument| is_callable(argument.as_ref()))
        && arguments
            .get(7)
            .is_none_or(|argument| is_boolean_literal(argument.as_ref()))
        && arguments
            .get(8)
            .is_none_or(|argument| is_nullish_or_callable(argument.as_ref(), unresolved_ctxt))
        && arguments.get(9).is_none_or(|argument| {
            is_nullish(argument.as_ref(), unresolved_ctxt)
                || is_nonnegative_integer(argument.as_ref())
        })
        && arguments.get(10).is_none_or(|argument| {
            is_nullish(argument.as_ref(), unresolved_ctxt)
                || is_nonnegative_integer(argument.as_ref())
        })
        && arguments.get(11).is_none_or(|argument| {
            is_nullish(argument.as_ref(), unresolved_ctxt) || is_string_literal(argument.as_ref())
        })
        && arguments.get(12).is_none_or(|argument| {
            is_nullish(argument.as_ref(), unresolved_ctxt)
                || is_nonnegative_integer(argument.as_ref())
        })
}

fn is_callable(expression: &Expr) -> bool {
    matches!(
        strip_parentheses(expression),
        Expr::Ident(_) | Expr::Member(_) | Expr::Fn(_) | Expr::Arrow(_)
    )
}

fn is_boolean_literal(expression: &Expr) -> bool {
    match strip_parentheses(expression) {
        Expr::Lit(Lit::Bool(_)) => true,
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => matches!(
            strip_parentheses(unary.arg.as_ref()),
            Expr::Lit(Lit::Num(number)) if number.value == 0.0 || number.value == 1.0
        ),
        _ => false,
    }
}

fn contains_try_finally(block: &BlockStmt) -> bool {
    struct Finder {
        found: bool,
    }

    impl Visit for Finder {
        fn visit_try_stmt(&mut self, statement: &swc_core::ecma::ast::TryStmt) {
            if statement.finalizer.is_some() {
                self.found = true;
                return;
            }
            statement.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut finder = Finder { found: false };
    block.visit_with(&mut finder);
    finder.found
}

fn contains_member_property(block: &BlockStmt, expected: &str) -> bool {
    struct Finder<'a> {
        expected: &'a str,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
            if member_prop_name(&member.prop).as_deref() == Some(self.expected) {
                self.found = true;
                return;
            }
            member.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut finder = Finder {
        expected,
        found: false,
    };
    block.visit_with(&mut finder);
    finder.found
}

fn infer_view_state_role_family(
    functions: &[RuntimeFunction],
    modules: &[PreparedAngularModule],
) -> Vec<(SymbolIdentity, &'static str)> {
    let mut restores_by_state: HashMap<SymbolIdentity, Vec<&RuntimeFunction>> = HashMap::new();
    let mut resets_by_state: HashMap<SymbolIdentity, Vec<&RuntimeFunction>> = HashMap::new();
    for function in functions {
        let Some(parameters) = plain_parameter_bindings(function) else {
            continue;
        };
        let [parameter] = parameters.as_slice() else {
            continue;
        };
        if returns_parameter_index(function, parameter, 8) {
            if let Some(state) = single_assigned_member(function, AssignedValue::Binding(parameter))
            {
                restores_by_state.entry(state).or_default().push(function);
            }
        }
        if exact_returned_identity(function)
            == Some(SymbolIdentity::LocalBinding(parameter.clone()))
        {
            if let Some(state) = single_assigned_member(function, AssignedValue::Null) {
                resets_by_state.entry(state).or_default().push(function);
            }
        }
    }

    let mut inferred = Vec::new();
    for (state, restores) in restores_by_state {
        let Some(resets) = resets_by_state.get(&state) else {
            continue;
        };
        let ([restore], [reset]) = (restores.as_slice(), resets.as_slice()) else {
            continue;
        };
        if restore.identity == reset.identity {
            continue;
        }
        inferred.push((restore.identity.clone(), "ɵɵrestoreView"));
        inferred.push((reset.identity.clone(), "ɵɵresetView"));

        let getters = functions
            .iter()
            .filter(|function| {
                function.params.is_empty()
                    && exact_returned_identity(function)
                        .is_some_and(|returned| same_member_object(&returned, &state))
                    && uses_capture_restore_flow(modules, &function.identity, &restore.identity)
            })
            .collect::<Vec<_>>();
        if let [getter] = getters.as_slice() {
            inferred.push((getter.identity.clone(), "ɵɵgetCurrentView"));
        }
    }
    inferred
}

fn same_member_object(left: &SymbolIdentity, right: &SymbolIdentity) -> bool {
    match (left, right) {
        (
            SymbolIdentity::LocalMember {
                object: left_object,
                ..
            },
            SymbolIdentity::LocalMember {
                object: right_object,
                ..
            },
        ) => left_object == right_object,
        (
            SymbolIdentity::GlobalMember {
                object: left_object,
                ..
            },
            SymbolIdentity::GlobalMember {
                object: right_object,
                ..
            },
        ) => left_object == right_object,
        _ => false,
    }
}

fn uses_capture_restore_flow(
    modules: &[PreparedAngularModule],
    getter: &SymbolIdentity,
    restore: &SymbolIdentity,
) -> bool {
    struct Collector<'a> {
        getter: &'a SymbolIdentity,
        restore: &'a SymbolIdentity,
        unresolved_ctxt: SyntaxContext,
        captures: HashSet<BindingKey>,
        restored: HashSet<BindingKey>,
    }

    impl Visit for Collector<'_> {
        fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
            if let (Pat::Ident(binding), Some(Expr::Call(call))) =
                (&declarator.name, declarator.init.as_deref())
            {
                if call_chain(call).is_some_and(|(root, argument_lists)| {
                    argument_lists.len() == 1
                        && argument_lists[0].is_empty()
                        && symbol_identity(root, self.unresolved_ctxt).as_ref() == Some(self.getter)
                }) {
                    self.captures.insert(binding_key(&binding.id));
                }
            }
            declarator.visit_children_with(self);
        }

        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Some((root, argument_lists)) = call_chain(call) {
                if symbol_identity(root, self.unresolved_ctxt).as_ref() == Some(self.restore)
                    && argument_lists.len() == 1
                    && argument_lists[0].len() == 1
                {
                    if let Expr::Ident(saved_view) =
                        strip_parentheses(argument_lists[0][0].expr.as_ref())
                    {
                        self.restored.insert(binding_key(saved_view));
                    }
                }
            }
            call.visit_children_with(self);
        }
    }

    modules.iter().any(|prepared| {
        let mut collector = Collector {
            getter,
            restore,
            unresolved_ctxt: prepared.unresolved_ctxt,
            captures: HashSet::new(),
            restored: HashSet::new(),
        };
        prepared.module.visit_with(&mut collector);
        collector
            .captures
            .iter()
            .any(|capture| collector.restored.contains(capture))
    })
}

#[derive(Clone, Copy)]
enum AssignedValue<'a> {
    Binding(&'a BindingKey),
    Null,
}

fn single_assigned_member(
    function: &RuntimeFunction,
    value: AssignedValue<'_>,
) -> Option<SymbolIdentity> {
    struct Collector<'a> {
        value: AssignedValue<'a>,
        unresolved_ctxt: SyntaxContext,
        targets: HashSet<SymbolIdentity>,
    }

    impl Visit for Collector<'_> {
        fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
            if assignment.op != swc_core::ecma::ast::AssignOp::Assign {
                assignment.visit_children_with(self);
                return;
            }
            let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assignment.left else {
                assignment.visit_children_with(self);
                return;
            };
            let matches_value = match self.value {
                AssignedValue::Binding(binding) => matches!(
                    strip_parentheses(assignment.right.as_ref()),
                    Expr::Ident(identifier) if binding_key(identifier) == *binding
                ),
                AssignedValue::Null => matches!(
                    strip_parentheses(assignment.right.as_ref()),
                    Expr::Lit(Lit::Null(_))
                ),
            };
            if matches_value {
                if let Some(identity) =
                    symbol_identity(&Expr::Member(member.clone()), self.unresolved_ctxt)
                {
                    self.targets.insert(identity);
                }
            }
            assignment.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut collector = Collector {
        value,
        unresolved_ctxt: function.unresolved_ctxt,
        targets: HashSet::new(),
    };
    function.body.visit_with(&mut collector);
    let mut targets = collector.targets.into_iter();
    let target = targets.next()?;
    targets.next().is_none().then_some(target)
}

fn returns_parameter_index(
    function: &RuntimeFunction,
    parameter: &BindingKey,
    expected_index: u64,
) -> bool {
    let mut returns = ReturnExpressionCollector::default();
    function.body.visit_with(&mut returns);
    returns.expressions.iter().any(|expression| {
        let expression = match strip_parentheses(expression.as_ref()) {
            Expr::Seq(sequence) => sequence.exprs.last().map(|expression| expression.as_ref()),
            expression => Some(expression),
        };
        let Some(Expr::Member(member)) = expression.map(strip_parentheses) else {
            return false;
        };
        let Expr::Ident(object) = strip_parentheses(member.obj.as_ref()) else {
            return false;
        };
        binding_key(object) == *parameter
            && computed_member_index(&member.prop) == Some(expected_index)
    })
}

fn exact_returned_identity(function: &RuntimeFunction) -> Option<SymbolIdentity> {
    let mut returns = ReturnExpressionCollector::default();
    function.body.visit_with(&mut returns);
    let mut identity = None;
    for expression in returns.expressions {
        let expression = match strip_parentheses(expression.as_ref()) {
            Expr::Seq(sequence) => sequence.exprs.last()?.as_ref(),
            expression => expression,
        };
        let current = symbol_identity(expression, function.unresolved_ctxt)?;
        if identity
            .as_ref()
            .is_some_and(|existing| existing != &current)
        {
            return None;
        }
        identity = Some(current);
    }
    identity
}

fn is_specialized_element_start_shape(
    definition: &RuntimeFunction,
    observations: &[impl std::borrow::Borrow<TemplateCallObservation>],
) -> bool {
    plain_parameter_bindings(definition).is_some_and(|parameters| parameters.len() == 4)
        && (returns_identity(definition, &definition.identity)
            || returns_identity_through_tracing_wrapper(definition, &definition.identity))
        && observations.iter().all(|observation| {
            let observation = observation.borrow();
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && matches!(observation.arguments.len(), 2..=4)
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
                && observation
                    .arguments
                    .get(1)
                    .is_some_and(|argument| is_string_literal(argument.as_ref()))
        })
}

fn returns_identity_through_tracing_wrapper(
    function: &RuntimeFunction,
    identity: &SymbolIdentity,
) -> bool {
    let mut returns = ReturnExpressionCollector::default();
    function.body.visit_with(&mut returns);
    let has_direct_identity_return = returns.expressions.iter().any(|expression| {
        expression_returns_identity(expression.as_ref(), identity, function.unresolved_ctxt)
    });
    !returns.expressions.is_empty()
        && returns.expressions.iter().all(|expression| {
            expression_returns_identity_through_tracing(
                expression.as_ref(),
                identity,
                function.unresolved_ctxt,
                has_direct_identity_return,
            )
        })
        && !block_can_fall_through(&function.body)
}

fn expression_returns_identity_through_tracing(
    expression: &Expr,
    identity: &SymbolIdentity,
    unresolved_ctxt: SyntaxContext,
    allow_minified_wrapper: bool,
) -> bool {
    if expression_returns_identity(expression, identity, unresolved_ctxt) {
        return true;
    }
    match strip_parentheses(expression) {
        Expr::Cond(conditional) => {
            let consequent = expression_returns_identity_through_tracing(
                conditional.cons.as_ref(),
                identity,
                unresolved_ctxt,
                true,
            );
            let alternate = expression_returns_identity_through_tracing(
                conditional.alt.as_ref(),
                identity,
                unresolved_ctxt,
                true,
            );
            consequent && alternate
        }
        Expr::Call(call) if is_tracing_wrapper_call(call, allow_minified_wrapper) => {
            call.args.last().is_some_and(|argument| {
                argument.spread.is_none()
                    && callback_returns_identity(argument.expr.as_ref(), identity, unresolved_ctxt)
            })
        }
        _ => false,
    }
}

fn is_tracing_wrapper_call(call: &CallExpr, allow_minified_wrapper: bool) -> bool {
    is_named_component_create_wrapper(call)
        || (allow_minified_wrapper
            && call.args.len() >= 2
            && matches!(
                &call.callee,
                Callee::Expr(callee)
                    if matches!(strip_parentheses(callee.as_ref()), Expr::Member(_))
            ))
}

fn is_named_component_create_wrapper(call: &CallExpr) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    matches!(
        strip_parentheses(callee.as_ref()),
        Expr::Member(member)
            if member_prop_name(&member.prop)
                .is_some_and(|property| property.as_ref() == "componentCreate")
    )
}

fn callback_returns_identity(
    expression: &Expr,
    identity: &SymbolIdentity,
    unresolved_ctxt: SyntaxContext,
) -> bool {
    match strip_parentheses(expression) {
        Expr::Arrow(arrow) if arrow.params.is_empty() => match arrow.body.as_ref() {
            BlockStmtOrExpr::Expr(expression) => {
                expression_returns_identity(expression.as_ref(), identity, unresolved_ctxt)
            }
            BlockStmtOrExpr::BlockStmt(block) => {
                let function = RuntimeFunction {
                    identity: identity.clone(),
                    params: Vec::new(),
                    body: block.clone(),
                    unresolved_ctxt,
                };
                returns_identity(&function, identity)
            }
        },
        Expr::Fn(function) if function.function.params.is_empty() => {
            let Some(body) = &function.function.body else {
                return false;
            };
            let function = RuntimeFunction {
                identity: identity.clone(),
                params: Vec::new(),
                body: body.clone(),
                unresolved_ctxt,
            };
            returns_identity(&function, identity)
        }
        _ => false,
    }
}

fn is_specialized_element_end_shape(
    definition: &RuntimeFunction,
    observations: &[impl std::borrow::Borrow<TemplateCallObservation>],
) -> bool {
    definition.params.is_empty()
        && returns_identity(definition, &definition.identity)
        && observations.iter().all(|observation| {
            let observation = observation.borrow();
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && observation.arguments.is_empty()
        })
}

fn is_text_shape(definition: &RuntimeFunction, observations: &[TemplateCallObservation]) -> bool {
    definition.params.len() == 2
        && is_empty_string_default(&definition.params[1])
        && observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && matches!(observation.arguments.len(), 1 | 2)
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
                && observation
                    .arguments
                    .get(1)
                    .is_none_or(|argument| is_string_literal(argument.as_ref()))
        })
}

fn is_listener_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    definition.params.len() == 3
        && definition
            .params
            .iter()
            .all(|parameter| matches!(parameter, Pat::Ident(_)))
        && returns_identity(definition, &definition.identity)
        && observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && matches!(observation.arguments.len(), 2 | 3)
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_string_literal(argument.as_ref()))
                && observation.arguments.get(1).is_some_and(|argument| {
                    matches!(argument.as_ref(), Expr::Fn(_) | Expr::Arrow(_))
                })
        })
}

fn is_advance_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    definition.params.len() == 1
        && is_numeric_default(&definition.params[0], 1.0)
        && observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 2
                && matches!(observation.arguments.len(), 0 | 1)
                && observation
                    .arguments
                    .first()
                    .is_none_or(|argument| is_nonnegative_integer(argument.as_ref()))
        })
}

fn is_property_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    let Some(parameters) = plain_parameter_bindings(definition) else {
        return false;
    };
    if parameters.len() != 3
        || !returns_identity(definition, &definition.identity)
        || !observations.iter().all(|observation| {
            matches!(
                observation.usage,
                TemplateCallUsage::Effect | TemplateCallUsage::Initializer
            ) && observation.phase == 2
                && matches!(observation.arguments.len(), 2 | 3)
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_string_literal(argument.as_ref()))
        })
    {
        return false;
    }

    let calls = direct_calls(definition);
    calls.len() >= 4
        && (calls.iter().any(|call| {
            call.arguments.len() >= 6 && forwards_parameters_in_order(call, &parameters)
        }) || calls.iter().any(|call| {
            is_member_call_named(call, "setProperty")
                && forwards_parameter_dependencies_in_order(call, &parameters[..2])
        }))
}

fn is_embedded_template_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    definition.params.len() == 8
        && {
            let direct_calls = direct_calls(definition);
            direct_calls.len() >= 3
                || (!direct_calls.is_empty() && returns_identity(definition, &definition.identity))
        }
        && observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && is_embedded_template_arguments(&observation.arguments)
        })
}

fn is_embedded_template_arguments(arguments: &[Box<Expr>]) -> bool {
    matches!(arguments.len(), 4..=8)
        && arguments
            .first()
            .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
        && arguments.get(1).is_some_and(|argument| {
            matches!(
                argument.as_ref(),
                Expr::Ident(_) | Expr::Fn(_) | Expr::Arrow(_)
            )
        })
        && arguments
            .get(2)
            .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
        && arguments
            .get(3)
            .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
        && arguments.get(4).is_none_or(|argument| {
            is_string_literal(argument.as_ref())
                || matches!(argument.as_ref(), Expr::Lit(Lit::Null(_)))
        })
        && arguments.get(5).is_none_or(|argument| {
            is_nonnegative_integer(argument.as_ref())
                || matches!(argument.as_ref(), Expr::Lit(Lit::Null(_)))
        })
}

fn is_conditional_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    plain_parameter_bindings(definition).is_some_and(|parameters| matches!(parameters.len(), 1 | 2))
        && direct_calls(definition).len() >= 6
        && contains_negative_one(&definition.body)
        && observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 2
                && matches!(observation.arguments.len(), 1 | 2)
                && observation.arguments.first().is_some_and(|argument| {
                    is_template_selection(argument.as_ref()) || is_template_index(argument.as_ref())
                })
        })
}

fn is_next_context_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    let [parameter] = definition.params.as_slice() else {
        return false;
    };
    if !is_numeric_default(parameter, 1.0)
        || !observations.iter().all(|observation| {
            matches!(
                observation.usage,
                TemplateCallUsage::Effect | TemplateCallUsage::Initializer
            ) && observation.phase == 2
                && matches!(observation.arguments.len(), 0 | 1)
                && observation
                    .arguments
                    .first()
                    .is_none_or(|argument| is_nonnegative_integer(argument.as_ref()))
        })
    {
        return false;
    }
    let Pat::Assign(assignment) = parameter else {
        return false;
    };
    let Pat::Ident(parameter) = assignment.left.as_ref() else {
        return false;
    };
    let calls = direct_calls(definition);
    let parameter = binding_key(&parameter.id);
    if let [call] = calls.as_slice() {
        return call.arguments.as_slice().first().is_some_and(|argument| {
            matches!(
                argument.as_ref(),
                Expr::Ident(identifier) if binding_key(identifier) == parameter
            )
        });
    }
    calls.is_empty()
        && decrements_binding(&definition.body, &parameter)
        && contains_computed_member_index(&definition.body, 14)
        && returns_computed_member_index(definition, 8)
}

fn is_projection_def_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    plain_parameter_bindings(definition).is_some_and(|parameters| parameters.len() == 1)
        && direct_calls(definition).len() >= 4
        && observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && matches!(observation.arguments.len(), 0 | 1)
                && observation.arguments.first().is_none_or(|argument| {
                    matches!(argument.as_ref(), Expr::Ident(_) | Expr::Array(_))
                })
        })
}

fn is_projection_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    definition.params.len() == 6
        && is_numeric_default(&definition.params[1], 0.0)
        && direct_calls(definition).len() >= 6
        && observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && matches!(observation.arguments.len(), 1..=6)
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
                && observation
                    .arguments
                    .get(1)
                    .is_none_or(|argument| is_nonnegative_integer(argument.as_ref()))
        })
}

fn is_reference_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    let Some(parameters) = plain_parameter_bindings(definition) else {
        return false;
    };
    let [slot] = parameters.as_slice() else {
        return false;
    };
    (direct_calls(definition).len() >= 2
        || (loads_parameter_from_offset_member(definition, slot, 27)
            && exact_returned_identity(definition)
                == Some(SymbolIdentity::LocalBinding(slot.clone()))
            && contains_throw_statement(&definition.body)))
        && observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Initializer
                && observation.phase == 2
                && observation.arguments.len() == 1
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
        })
}

fn loads_parameter_from_offset_member(
    function: &RuntimeFunction,
    parameter: &BindingKey,
    offset: u64,
) -> bool {
    struct Finder<'a> {
        parameter: &'a BindingKey,
        offset: u64,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
            if assignment.op == swc_core::ecma::ast::AssignOp::Assign
                && matches!(
                    &assignment.left,
                    AssignTarget::Simple(SimpleAssignTarget::Ident(identifier))
                        if binding_key(&identifier.id) == *self.parameter
                )
                && matches!(
                    strip_parentheses(assignment.right.as_ref()),
                    Expr::Member(member)
                        if member_uses_parameter_offset(member, self.parameter, self.offset)
                )
            {
                self.found = true;
                return;
            }
            assignment.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut finder = Finder {
        parameter,
        offset,
        found: false,
    };
    function.body.visit_with(&mut finder);
    finder.found
}

fn contains_throw_statement(block: &BlockStmt) -> bool {
    struct Finder {
        found: bool,
    }

    impl Visit for Finder {
        fn visit_throw_stmt(&mut self, _statement: &swc_core::ecma::ast::ThrowStmt) {
            self.found = true;
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut finder = Finder { found: false };
    block.visit_with(&mut finder);
    finder.found
}

fn member_uses_parameter_offset(
    member: &swc_core::ecma::ast::MemberExpr,
    parameter: &BindingKey,
    offset: u64,
) -> bool {
    if !matches!(strip_parentheses(member.obj.as_ref()), Expr::Member(_)) {
        return false;
    }
    let MemberProp::Computed(computed) = &member.prop else {
        return false;
    };
    let Expr::Bin(binary) = strip_parentheses(computed.expr.as_ref()) else {
        return false;
    };
    if binary.op != BinaryOp::Add {
        return false;
    }
    let operands_match = |left: &Expr, right: &Expr| {
        computed_member_offset(left) == Some(offset)
            && matches!(
                strip_parentheses(right),
                Expr::Ident(identifier) if binding_key(identifier) == *parameter
            )
    };
    operands_match(binary.left.as_ref(), binary.right.as_ref())
        || operands_match(binary.right.as_ref(), binary.left.as_ref())
}

fn computed_member_offset(expression: &Expr) -> Option<u64> {
    let Expr::Lit(Lit::Num(number)) = strip_parentheses(expression) else {
        return None;
    };
    (number.value >= 0.0 && number.value.fract() == 0.0).then_some(number.value as u64)
}

fn is_pipe_shape(definition: &RuntimeFunction, observations: &[TemplateCallObservation]) -> bool {
    plain_parameter_bindings(definition).is_some_and(|parameters| parameters.len() == 2)
        && direct_calls(definition).len() >= 5
        && observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && observation.arguments.len() == 2
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
                && observation
                    .arguments
                    .get(1)
                    .is_some_and(|argument| is_string_literal(argument.as_ref()))
        })
}

fn pipe_binding_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> Option<&'static str> {
    let parameters = plain_parameter_bindings(definition)?;
    if !(3..=6).contains(&parameters.len())
        || direct_calls(definition).len() < 4
        || !observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 2
                && observation.arguments.len() == parameters.len()
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
                && observation
                    .arguments
                    .get(1)
                    .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
        })
    {
        return None;
    }
    Some(match parameters.len() {
        3 if observations
            .iter()
            .all(|observation| matches!(observation.arguments[2].as_ref(), Expr::Array(_))) =>
        {
            "ɵɵpipeBindV"
        }
        3 => "ɵɵpipeBind1",
        4 => "ɵɵpipeBind2",
        5 => "ɵɵpipeBind3",
        6 => "ɵɵpipeBind4",
        _ => return None,
    })
}

fn is_template_selection(expression: &Expr) -> bool {
    let expression = strip_parentheses(expression);
    let Expr::Cond(conditional) = expression else {
        return false;
    };
    is_template_index(conditional.cons.as_ref())
        && match strip_parentheses(conditional.alt.as_ref()) {
            Expr::Cond(_) => is_template_selection(conditional.alt.as_ref()),
            alternate => is_template_index(alternate),
        }
}

fn is_template_index(expression: &Expr) -> bool {
    is_nonnegative_integer(strip_parentheses(expression))
        || matches!(
            strip_parentheses(expression),
            Expr::Unary(unary)
                if unary.op == UnaryOp::Minus
                    && matches!(
                        strip_parentheses(unary.arg.as_ref()),
                        Expr::Lit(Lit::Num(number)) if number.value == 1.0
                    )
        )
}

fn contains_negative_one(block: &BlockStmt) -> bool {
    struct Finder {
        found: bool,
    }

    impl Visit for Finder {
        fn visit_expr(&mut self, expression: &Expr) {
            if is_template_index(expression)
                && matches!(
                    strip_parentheses(expression),
                    Expr::Unary(unary) if unary.op == UnaryOp::Minus
                )
            {
                self.found = true;
                return;
            }
            expression.visit_children_with(self);
        }
    }

    let mut finder = Finder { found: false };
    block.visit_with(&mut finder);
    finder.found
}

fn infer_text_interpolation_family(
    function_index: &RuntimeFunctionIndex<'_>,
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
) -> Vec<(SymbolIdentity, &'static str)> {
    let mut inferred = Vec::new();
    for (identity, calls_in_templates) in observations {
        if !calls_in_templates.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 2
                && observation.arguments.len() == 1
        }) {
            continue;
        }
        let Some(wrapper) = function_index.unique(identity) else {
            continue;
        };
        let Some(parameters) = plain_parameter_bindings(wrapper) else {
            continue;
        };
        if parameters.len() != 1 || !returns_identity(wrapper, &wrapper.identity) {
            continue;
        }
        let wrapper_calls = direct_calls(wrapper);
        let [target_call] = wrapper_calls.as_slice() else {
            continue;
        };
        if target_call.callee == *identity
            || !matches!(target_call.arguments.len(), 2 | 3)
            || !target_call
                .arguments
                .first()
                .is_some_and(|argument| is_empty_string_literal(argument.as_ref()))
            || !target_call.arguments.get(1).is_some_and(|argument| {
                matches!(
                    argument.as_ref(),
                    Expr::Ident(identifier) if binding_key(identifier) == parameters[0]
                )
            })
        {
            continue;
        }
        let Some(target) = function_index.unique(&target_call.callee) else {
            continue;
        };
        let Some(target_parameters) = plain_parameter_bindings(target) else {
            continue;
        };
        let target_calls = direct_calls(target);
        if target_parameters.len() != 3
            || !returns_identity(target, &target.identity)
            || target_calls.len() < 2
            || !target_calls.iter().any(|call| {
                call.arguments.len() >= target_parameters.len()
                    && forwards_parameters_in_order(call, &target_parameters)
            })
        {
            continue;
        }
        inferred.push((identity.clone(), "ɵɵtextInterpolate"));
        inferred.push((target.identity.clone(), "ɵɵtextInterpolate1"));
    }
    inferred
}

fn is_empty_string_default(pattern: &Pat) -> bool {
    let Pat::Assign(assignment) = pattern else {
        return false;
    };
    matches!(
        assignment.right.as_ref(),
        Expr::Lit(Lit::Str(string)) if string.value.is_empty()
    )
}

fn is_numeric_default(pattern: &Pat, expected: f64) -> bool {
    let Pat::Assign(assignment) = pattern else {
        return false;
    };
    matches!(
        assignment.right.as_ref(),
        Expr::Lit(Lit::Num(number)) if number.value == expected
    )
}

fn is_nonnegative_integer(expression: &Expr) -> bool {
    matches!(
        expression,
        Expr::Lit(Lit::Num(number))
            if number.value >= 0.0 && number.value.fract() == 0.0
    )
}

fn computed_member_index(property: &MemberProp) -> Option<u64> {
    let MemberProp::Computed(computed) = property else {
        return None;
    };
    let Expr::Lit(Lit::Num(number)) = strip_parentheses(computed.expr.as_ref()) else {
        return None;
    };
    (number.value >= 0.0 && number.value.fract() == 0.0).then_some(number.value as u64)
}

fn contains_computed_member_index(block: &BlockStmt, expected: u64) -> bool {
    struct Finder {
        expected: u64,
        found: bool,
    }

    impl Visit for Finder {
        fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
            if computed_member_index(&member.prop) == Some(self.expected) {
                self.found = true;
                return;
            }
            member.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut finder = Finder {
        expected,
        found: false,
    };
    block.visit_with(&mut finder);
    finder.found
}

fn returns_computed_member_index(function: &RuntimeFunction, expected: u64) -> bool {
    let mut returns = ReturnExpressionCollector::default();
    function.body.visit_with(&mut returns);
    returns.expressions.iter().any(|expression| {
        let expression = match strip_parentheses(expression.as_ref()) {
            Expr::Seq(sequence) => sequence.exprs.last().map(|expression| expression.as_ref()),
            expression => Some(expression),
        };
        matches!(
            expression.map(strip_parentheses),
            Some(Expr::Member(member)) if computed_member_index(&member.prop) == Some(expected)
        )
    })
}

fn decrements_binding(block: &BlockStmt, binding: &BindingKey) -> bool {
    struct Finder<'a> {
        binding: &'a BindingKey,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
            if update.op == swc_core::ecma::ast::UpdateOp::MinusMinus
                && matches!(
                    strip_parentheses(update.arg.as_ref()),
                    Expr::Ident(identifier) if binding_key(identifier) == *self.binding
                )
            {
                self.found = true;
                return;
            }
            update.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut finder = Finder {
        binding,
        found: false,
    };
    block.visit_with(&mut finder);
    finder.found
}

fn is_string_literal(expression: &Expr) -> bool {
    matches!(expression, Expr::Lit(Lit::Str(_)))
        || matches!(
            expression,
            Expr::Tpl(template) if template.exprs.is_empty() && template.quasis.len() == 1
        )
}

fn is_empty_string_literal(expression: &Expr) -> bool {
    match expression {
        Expr::Lit(Lit::Str(string)) => string.value.is_empty(),
        Expr::Tpl(template) if template.exprs.is_empty() && template.quasis.len() == 1 => template
            .quasis
            .first()
            .is_some_and(|quasi| quasi.raw.is_empty()),
        _ => false,
    }
}

impl RuntimeFunctionCollector {
    fn record_definition(&mut self, identity: SymbolIdentity) {
        *self.definition_counts.entry(identity).or_default() += 1;
    }

    fn record_assignment_definition(&mut self, identity: SymbolIdentity, position: u32) {
        self.record_definition(identity.clone());
        self.assignment_definitions
            .entry(identity)
            .or_default()
            .push((self.module_index, position));
    }

    fn record_function(&mut self, identity: SymbolIdentity, function: &Function) {
        let Some(body) = function.body.as_ref() else {
            return;
        };
        self.functions.push(RuntimeFunction {
            identity,
            params: function
                .params
                .iter()
                .map(|param| param.pat.clone())
                .collect(),
            body: body.clone(),
            unresolved_ctxt: self.unresolved_ctxt,
        });
    }

    fn record_arrow(&mut self, identity: SymbolIdentity, arrow: &ArrowExpr) {
        let body = match arrow.body.as_ref() {
            BlockStmtOrExpr::BlockStmt(body) => body.clone(),
            BlockStmtOrExpr::Expr(expression) => BlockStmt {
                span: DUMMY_SP,
                ctxt: SyntaxContext::empty(),
                stmts: vec![Stmt::Return(ReturnStmt {
                    span: DUMMY_SP,
                    arg: Some(expression.clone()),
                })],
            },
        };
        self.functions.push(RuntimeFunction {
            identity,
            params: arrow.params.clone(),
            body,
            unresolved_ctxt: self.unresolved_ctxt,
        });
    }

    fn record_value_definition(
        &mut self,
        target: &Expr,
        value: &Expr,
        assignment_position: Option<u32>,
    ) {
        let Some(identity) = symbol_identity(target, self.unresolved_ctxt) else {
            return;
        };
        if let Some(position) = assignment_position {
            self.record_assignment_definition(identity.clone(), position);
        } else {
            self.record_definition(identity.clone());
        }
        self.record_function_value(identity, value);
    }

    fn record_function_value(&mut self, identity: SymbolIdentity, value: &Expr) {
        match value {
            Expr::Fn(function) => self.record_function(identity, function.function.as_ref()),
            Expr::Arrow(arrow) => self.record_arrow(identity, arrow),
            Expr::Paren(paren) => self.record_function_value(identity, paren.expr.as_ref()),
            _ => {}
        }
    }

    fn record_pattern_definition(&mut self, pattern: &Pat) {
        match pattern {
            Pat::Ident(binding) => {
                self.record_definition(SymbolIdentity::LocalBinding(binding_key(&binding.id)));
            }
            Pat::Array(array) => {
                for element in array.elems.iter().flatten() {
                    self.record_pattern_definition(element);
                }
            }
            Pat::Object(object) => {
                for property in &object.props {
                    match property {
                        ObjectPatProp::KeyValue(key_value) => {
                            self.record_pattern_definition(key_value.value.as_ref());
                        }
                        ObjectPatProp::Assign(assign) => {
                            self.record_definition(SymbolIdentity::LocalBinding(binding_key(
                                &assign.key.id,
                            )));
                        }
                        ObjectPatProp::Rest(rest) => {
                            self.record_pattern_definition(rest.arg.as_ref());
                        }
                    }
                }
            }
            Pat::Assign(assign) => self.record_pattern_definition(assign.left.as_ref()),
            Pat::Rest(rest) => self.record_pattern_definition(rest.arg.as_ref()),
            Pat::Expr(expression) => {
                if let Some(identity) = symbol_identity(expression, self.unresolved_ctxt) {
                    self.record_definition(identity);
                }
            }
            Pat::Invalid(_) => {}
        }
    }

    fn invalidate_expression(&mut self, expression: &Expr) {
        if let Some(identity) = symbol_identity(expression, self.unresolved_ctxt) {
            self.invalid_values.insert(identity);
        }
    }

    fn invalidate_pattern(&mut self, pattern: &Pat) {
        match pattern {
            Pat::Ident(binding) => {
                self.invalid_values
                    .insert(SymbolIdentity::LocalBinding(binding_key(&binding.id)));
            }
            Pat::Array(array) => {
                for element in array.elems.iter().flatten() {
                    self.invalidate_pattern(element);
                }
            }
            Pat::Object(object) => {
                for property in &object.props {
                    match property {
                        ObjectPatProp::KeyValue(key_value) => {
                            self.invalidate_pattern(key_value.value.as_ref());
                        }
                        ObjectPatProp::Assign(assign) => {
                            self.invalid_values
                                .insert(SymbolIdentity::LocalBinding(binding_key(&assign.key.id)));
                        }
                        ObjectPatProp::Rest(rest) => {
                            self.invalidate_pattern(rest.arg.as_ref());
                        }
                    }
                }
            }
            Pat::Assign(assign) => self.invalidate_pattern(assign.left.as_ref()),
            Pat::Rest(rest) => self.invalidate_pattern(rest.arg.as_ref()),
            Pat::Expr(expression) => self.invalidate_expression(expression.as_ref()),
            Pat::Invalid(_) => {}
        }
    }

    fn invalidate_assignment_target(&mut self, target: &AssignTarget) {
        match target {
            AssignTarget::Simple(simple) => match simple {
                SimpleAssignTarget::Ident(binding) => {
                    self.invalidate_expression(&Expr::Ident(binding.id.clone()));
                }
                SimpleAssignTarget::Member(member) => {
                    self.invalidate_expression(&Expr::Member(member.clone()));
                }
                SimpleAssignTarget::Paren(paren) => {
                    self.invalidate_expression(paren.expr.as_ref());
                }
                SimpleAssignTarget::TsAs(ts_as) => {
                    self.invalidate_expression(ts_as.expr.as_ref());
                }
                SimpleAssignTarget::TsSatisfies(ts_satisfies) => {
                    self.invalidate_expression(ts_satisfies.expr.as_ref());
                }
                SimpleAssignTarget::TsNonNull(ts_non_null) => {
                    self.invalidate_expression(ts_non_null.expr.as_ref());
                }
                SimpleAssignTarget::TsTypeAssertion(ts_assertion) => {
                    self.invalidate_expression(ts_assertion.expr.as_ref());
                }
                SimpleAssignTarget::TsInstantiation(ts_instantiation) => {
                    self.invalidate_expression(ts_instantiation.expr.as_ref());
                }
                _ => {}
            },
            AssignTarget::Pat(pattern) => match pattern {
                swc_core::ecma::ast::AssignTargetPat::Array(array) => {
                    for element in array.elems.iter().flatten() {
                        self.invalidate_pattern(element);
                    }
                }
                swc_core::ecma::ast::AssignTargetPat::Object(object) => {
                    self.invalidate_pattern(&Pat::Object(object.clone()));
                }
                swc_core::ecma::ast::AssignTargetPat::Invalid(_) => {}
            },
        }
    }
}

impl Visit for RuntimeFunctionCollector {
    fn visit_fn_decl(&mut self, declaration: &FnDecl) {
        let identity = SymbolIdentity::LocalBinding(binding_key(&declaration.ident));
        self.record_definition(identity.clone());
        self.record_function(identity, declaration.function.as_ref());
        for parameter in &declaration.function.params {
            self.record_pattern_definition(&parameter.pat);
        }
        declaration.function.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let Some(value) = declarator.init.as_deref() {
            if let Pat::Ident(binding) = &declarator.name {
                self.record_value_definition(&Expr::Ident(binding.id.clone()), value, None);
            } else {
                self.record_pattern_definition(&declarator.name);
            }
        }
        declarator.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        if assignment.op == AssignOp::Assign {
            if let Some(target) = assignment_target_expression(&assignment.left) {
                self.record_value_definition(
                    &target,
                    assignment.right.as_ref(),
                    Some(assignment.span.lo.0),
                );
            } else {
                self.invalidate_assignment_target(&assignment.left);
            }
        } else {
            self.invalidate_assignment_target(&assignment.left);
        }
        assignment.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        self.invalidate_expression(update.arg.as_ref());
        update.visit_children_with(self);
    }

    fn visit_for_in_stmt(&mut self, statement: &ForInStmt) {
        if let ForHead::Pat(pattern) = &statement.left {
            self.invalidate_pattern(pattern);
        }
        statement.visit_children_with(self);
    }

    fn visit_for_of_stmt(&mut self, statement: &ForOfStmt) {
        if let ForHead::Pat(pattern) = &statement.left {
            self.invalidate_pattern(pattern);
        }
        statement.visit_children_with(self);
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        if unary.op == UnaryOp::Delete {
            self.invalidate_expression(unary.arg.as_ref());
        }
        unary.visit_children_with(self);
    }

    fn visit_import_decl(&mut self, import: &ImportDecl) {
        for specifier in &import.specifiers {
            let local = match specifier {
                ImportSpecifier::Named(named) => &named.local,
                ImportSpecifier::Default(default) => &default.local,
                ImportSpecifier::Namespace(namespace) => &namespace.local,
            };
            self.record_definition(SymbolIdentity::LocalBinding(binding_key(local)));
        }
    }

    fn visit_class_decl(&mut self, declaration: &ClassDecl) {
        self.record_definition(SymbolIdentity::LocalBinding(binding_key(
            &declaration.ident,
        )));
        declaration.class.visit_children_with(self);
    }

    fn visit_catch_clause(&mut self, clause: &CatchClause) {
        if let Some(parameter) = &clause.param {
            self.record_pattern_definition(parameter);
        }
        clause.visit_children_with(self);
    }

    fn visit_function(&mut self, function: &Function) {
        for parameter in &function.params {
            self.record_pattern_definition(&parameter.pat);
        }
        function.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        for parameter in &arrow.params {
            self.record_pattern_definition(parameter);
        }
        arrow.visit_children_with(self);
    }
}

fn assignment_target_expression(target: &AssignTarget) -> Option<Expr> {
    match target {
        AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
            Some(Expr::Ident(binding.id.clone()))
        }
        AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
            Some(Expr::Member(member.clone()))
        }
        AssignTarget::Simple(SimpleAssignTarget::Paren(paren)) => Some(paren.expr.as_ref().clone()),
        _ => None,
    }
}

fn is_define_component_shape(function: &RuntimeFunction) -> bool {
    let [Pat::Ident(parameter)] = function.params.as_slice() else {
        return false;
    };
    let parameter = binding_key(&parameter.id);

    let mut returns = ReturnExpressionCollector::default();
    function.body.visit_with(&mut returns);
    returns.expressions.iter().any(|expression| {
        let mut evidence = ReturnedDescriptorBuilder {
            parameter: &parameter,
            unresolved_ctxt: function.unresolved_ctxt,
            matched: false,
        };
        expression.visit_with(&mut evidence);
        evidence.matched
    })
}

#[derive(Default)]
struct ReturnExpressionCollector {
    expressions: Vec<Box<Expr>>,
}

impl Visit for ReturnExpressionCollector {
    fn visit_return_stmt(&mut self, statement: &ReturnStmt) {
        if let Some(expression) = &statement.arg {
            self.expressions.push(expression.clone());
        }
    }

    fn visit_function(&mut self, _function: &Function) {}

    fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
}

struct ReturnedDescriptorBuilder<'a> {
    parameter: &'a BindingKey,
    unresolved_ctxt: SyntaxContext,
    matched: bool,
}

impl Visit for ReturnedDescriptorBuilder<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if self.matched {
            return;
        }
        for argument in &call.args {
            let Expr::Arrow(arrow) = argument.expr.as_ref() else {
                continue;
            };
            let mut evidence = DescriptorBuilderEvidence {
                parameter: self.parameter,
                unresolved_ctxt: self.unresolved_ctxt,
                parameter_fields: HashSet::new(),
                has_object_assign: false,
            };
            arrow.visit_with(&mut evidence);
            let has_fields = |names: &[&str]| {
                names.iter().all(|name| {
                    evidence
                        .parameter_fields
                        .iter()
                        .any(|field| field.as_ref() == *name)
                })
            };
            if (evidence.has_object_assign && has_fields(&["template", "dependencies", "styles"]))
                || has_fields(&[
                    "decls",
                    "vars",
                    "template",
                    "consts",
                    "dependencies",
                    "styles",
                ])
            {
                self.matched = true;
                return;
            }
        }
        call.visit_children_with(self);
    }
}

struct DescriptorBuilderEvidence<'a> {
    parameter: &'a BindingKey,
    unresolved_ctxt: SyntaxContext,
    parameter_fields: HashSet<Atom>,
    has_object_assign: bool,
}

impl Visit for DescriptorBuilderEvidence<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if call.args.len() >= 3 && is_unresolved_object_assign(&call.callee, self.unresolved_ctxt) {
            self.has_object_assign = true;
        }
        call.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
        if let Expr::Ident(object) = member.obj.as_ref() {
            if binding_key(object) == *self.parameter {
                if let Some(property) = member_prop_name(&member.prop) {
                    self.parameter_fields.insert(property);
                }
            }
        }
        member.visit_children_with(self);
    }
}

fn is_unresolved_object_assign(callee: &Callee, unresolved_ctxt: SyntaxContext) -> bool {
    let Callee::Expr(callee) = callee else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    let Expr::Ident(object) = member.obj.as_ref() else {
        return false;
    };
    object.ctxt == unresolved_ctxt
        && object.sym.as_ref() == "Object"
        && member_prop_name(&member.prop).is_some_and(|property| property.as_ref() == "assign")
}

fn infer_element_family(functions: &[RuntimeFunction]) -> Vec<(SymbolIdentity, &'static str)> {
    let mut by_identity: HashMap<&SymbolIdentity, Vec<&RuntimeFunction>> = HashMap::new();
    for function in functions {
        by_identity
            .entry(&function.identity)
            .or_default()
            .push(function);
    }

    let mut inferred = Vec::new();
    for wrapper in functions {
        let Some(parameters) = plain_parameter_bindings(wrapper) else {
            continue;
        };
        if parameters.len() != 4 || !returns_identity(wrapper, &wrapper.identity) {
            continue;
        }
        let calls = direct_calls(wrapper);
        if calls.len() != 2 {
            continue;
        }
        let start = &calls[0];
        let end = &calls[1];
        if !forwards_parameters(start, &parameters)
            || !end.arguments.is_empty()
            || start.callee == end.callee
            || start.callee == wrapper.identity
            || end.callee == wrapper.identity
        {
            continue;
        }
        if !has_unique_self_returning_arity(&by_identity, &start.callee, 4, true)
            || !has_unique_self_returning_arity(&by_identity, &end.callee, 0, false)
        {
            continue;
        }
        inferred.push((wrapper.identity.clone(), "ɵɵelement"));
        inferred.push((start.callee.clone(), "ɵɵelementStart"));
        inferred.push((end.callee.clone(), "ɵɵelementEnd"));
    }
    inferred
}

fn plain_parameter_bindings(function: &RuntimeFunction) -> Option<Vec<BindingKey>> {
    function
        .params
        .iter()
        .map(|parameter| {
            let Pat::Ident(binding) = parameter else {
                return None;
            };
            Some(binding_key(&binding.id))
        })
        .collect()
}

fn returns_identity(function: &RuntimeFunction, identity: &SymbolIdentity) -> bool {
    let mut returns = ReturnExpressionCollector::default();
    function.body.visit_with(&mut returns);
    !returns.expressions.is_empty()
        && returns.expressions.iter().all(|expression| {
            expression_returns_identity(expression.as_ref(), identity, function.unresolved_ctxt)
        })
        && !block_can_fall_through(&function.body)
}

fn expression_returns_identity(
    expression: &Expr,
    identity: &SymbolIdentity,
    unresolved_ctxt: SyntaxContext,
) -> bool {
    match strip_parentheses(expression) {
        Expr::Seq(sequence) => sequence.exprs.last().is_some_and(|expression| {
            expression_returns_identity(expression.as_ref(), identity, unresolved_ctxt)
        }),
        Expr::Cond(conditional) => {
            expression_returns_identity(conditional.cons.as_ref(), identity, unresolved_ctxt)
                && expression_returns_identity(conditional.alt.as_ref(), identity, unresolved_ctxt)
        }
        expression => symbol_identity(expression, unresolved_ctxt).as_ref() == Some(identity),
    }
}

fn block_can_fall_through(block: &BlockStmt) -> bool {
    let mut reachable = true;
    for statement in &block.stmts {
        if !reachable {
            break;
        }
        reachable = statement_can_fall_through(statement);
    }
    reachable
}

fn statement_can_fall_through(statement: &Stmt) -> bool {
    match statement {
        Stmt::Return(_) | Stmt::Throw(_) => false,
        Stmt::Block(block) => block_can_fall_through(block),
        Stmt::If(if_statement) => {
            let consequent = statement_can_fall_through(if_statement.cons.as_ref());
            let Some(alternate) = if_statement.alt.as_deref() else {
                return true;
            };
            consequent || statement_can_fall_through(alternate)
        }
        Stmt::Try(try_statement) => {
            if let Some(finalizer) = &try_statement.finalizer {
                if !block_can_fall_through(finalizer) {
                    return false;
                }
            }
            block_can_fall_through(&try_statement.block)
                || try_statement
                    .handler
                    .as_ref()
                    .is_some_and(|handler| block_can_fall_through(&handler.body))
        }
        _ => true,
    }
}

struct DirectCall {
    callee: SymbolIdentity,
    arguments: Vec<Box<Expr>>,
}

fn is_member_call_named(call: &DirectCall, expected: &str) -> bool {
    matches!(
        &call.callee,
        SymbolIdentity::LocalMember { property, .. }
            | SymbolIdentity::GlobalMember { property, .. }
            if property == expected
    )
}

fn direct_calls(function: &RuntimeFunction) -> Vec<DirectCall> {
    let mut collector = DirectCallCollector {
        unresolved_ctxt: function.unresolved_ctxt,
        calls: Vec::new(),
    };
    function.body.visit_with(&mut collector);
    collector.calls
}

struct DirectCallCollector {
    unresolved_ctxt: SyntaxContext,
    calls: Vec<DirectCall>,
}

impl Visit for DirectCallCollector {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        if let Some(identity) = symbol_identity(callee.as_ref(), self.unresolved_ctxt) {
            self.calls.push(DirectCall {
                callee: identity,
                arguments: call
                    .args
                    .iter()
                    .map(|argument| argument.expr.clone())
                    .collect(),
            });
        }
    }

    fn visit_function(&mut self, _function: &Function) {}

    fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
}

fn forwards_parameters(call: &DirectCall, parameters: &[BindingKey]) -> bool {
    call.arguments.len() == parameters.len()
        && call
            .arguments
            .iter()
            .zip(parameters)
            .all(|(argument, parameter)| {
                matches!(
                    argument.as_ref(),
                    Expr::Ident(identifier) if binding_key(identifier) == *parameter
                )
            })
}

fn forwards_parameters_in_order(call: &DirectCall, parameters: &[BindingKey]) -> bool {
    let mut next_parameter = 0;
    for argument in &call.arguments {
        if next_parameter == parameters.len() {
            break;
        }
        if matches!(
            argument.as_ref(),
            Expr::Ident(identifier)
                if binding_key(identifier) == parameters[next_parameter]
        ) {
            next_parameter += 1;
        }
    }
    next_parameter == parameters.len()
}

fn forwards_parameter_dependencies_in_order(call: &DirectCall, parameters: &[BindingKey]) -> bool {
    struct BindingFinder<'a> {
        binding: &'a BindingKey,
        found: bool,
    }

    impl Visit for BindingFinder<'_> {
        fn visit_expr(&mut self, expression: &Expr) {
            if matches!(
                expression,
                Expr::Ident(identifier) if binding_key(identifier) == *self.binding
            ) {
                self.found = true;
                return;
            }
            expression.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut next_parameter = 0;
    for argument in &call.arguments {
        if next_parameter == parameters.len() {
            break;
        }
        let mut finder = BindingFinder {
            binding: &parameters[next_parameter],
            found: false,
        };
        argument.visit_with(&mut finder);
        if finder.found {
            next_parameter += 1;
        }
    }
    next_parameter == parameters.len()
}

fn has_unique_self_returning_arity(
    functions: &HashMap<&SymbolIdentity, Vec<&RuntimeFunction>>,
    identity: &SymbolIdentity,
    arity: usize,
    allow_tracing_wrapper: bool,
) -> bool {
    let Some(candidates) = functions.get(identity) else {
        return false;
    };
    let mut matching = candidates.iter().filter(|candidate| {
        candidate.params.len() == arity
            && (returns_identity(candidate, identity)
                || (allow_tracing_wrapper
                    && returns_identity_through_tracing_wrapper(candidate, identity)))
    });
    matching.next().is_some() && matching.next().is_none()
}
