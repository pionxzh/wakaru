use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignTarget, BinaryOp, BlockStmt, BlockStmtOrExpr, CallExpr, Callee,
    Expr, ExprOrSpread, FnDecl, Function, Lit, Pat, ReturnStmt, SimpleAssignTarget, Stmt, UnaryOp,
    VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::{symbol_identity, IvyInstruction, IvyRoleTable, SymbolIdentity};
use crate::angular_recovery::syntax::{binding_key, member_prop_name, BindingKey};
use crate::angular_recovery::PreparedAngularModule;

pub(super) fn infer_ivy_roles(
    modules: &[PreparedAngularModule],
) -> Vec<(SymbolIdentity, &'static str)> {
    let functions = collect_runtime_functions(modules);

    let mut inferred = functions
        .iter()
        .filter(|function| is_define_component_shape(function))
        .map(|function| (function.identity.clone(), "ɵɵdefineComponent"))
        .collect::<Vec<_>>();
    inferred.extend(infer_element_family(&functions));
    inferred
}

fn collect_runtime_functions(modules: &[PreparedAngularModule]) -> Vec<RuntimeFunction> {
    let mut functions = Vec::new();
    for prepared in modules {
        let mut collector = RuntimeFunctionCollector {
            unresolved_ctxt: prepared.unresolved_ctxt,
            functions: Vec::new(),
        };
        prepared.module.visit_with(&mut collector);
        functions.extend(collector.functions);
    }
    functions
}

pub(super) fn infer_template_roles(
    modules: &[PreparedAngularModule],
    roles: &IvyRoleTable,
) -> Vec<(SymbolIdentity, &'static str)> {
    let functions = collect_runtime_functions(modules);
    let mut observations = Vec::new();
    for prepared in modules {
        let mut collector = TemplateFunctionCollector {
            roles,
            functions: &functions,
            unresolved_ctxt: prepared.unresolved_ctxt,
            observations: Vec::new(),
        };
        prepared.module.visit_with(&mut collector);
        observations.extend(collector.observations);
    }

    let mut by_identity: HashMap<SymbolIdentity, Vec<TemplateCallObservation>> = HashMap::new();
    for observation in observations {
        by_identity
            .entry(observation.identity.clone())
            .or_default()
            .push(observation);
    }

    let mut inferred = infer_specialized_element_pair(&functions, &by_identity, roles);
    inferred.extend(infer_text_interpolation_family(
        &functions,
        &by_identity,
        roles,
    ));
    for (identity, observations) in &by_identity {
        let mut definitions = functions
            .iter()
            .filter(|function| roles.symbols_equivalent(&function.identity, identity));
        let Some(definition) = definitions.next() else {
            continue;
        };
        if definitions.next().is_some() {
            continue;
        }

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
        if let [name] = matches.as_slice() {
            inferred.push((definition.identity.clone(), *name));
        }
    }
    inferred
}

#[derive(Clone)]
struct RuntimeFunction {
    identity: SymbolIdentity,
    params: Vec<Pat>,
    body: BlockStmt,
    unresolved_ctxt: SyntaxContext,
}

struct RuntimeFunctionCollector {
    unresolved_ctxt: SyntaxContext,
    functions: Vec<RuntimeFunction>,
}

struct TemplateCallObservation {
    identity: SymbolIdentity,
    phase: u8,
    arguments: Vec<Box<Expr>>,
    usage: TemplateCallUsage,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TemplateCallUsage {
    Effect,
    Initializer,
}

struct TemplateFunctionCollector<'a> {
    roles: &'a IvyRoleTable,
    functions: &'a [RuntimeFunction],
    unresolved_ctxt: SyntaxContext,
    observations: Vec<TemplateCallObservation>,
}

impl Visit for TemplateFunctionCollector<'_> {
    fn visit_function(&mut self, function: &Function) {
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
        };
        if let Some(body) = &function.body {
            observer.collect_statements(&body.stmts, None);
        }
        if observer.saw_creation_anchor
            || has_unclassified_element_anchor(&observer.observations, self.functions, self.roles)
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
    functions: &[RuntimeFunction],
    roles: &IvyRoleTable,
) -> bool {
    let mut grouped: HashMap<&SymbolIdentity, Vec<&TemplateCallObservation>> = HashMap::new();
    for observation in observations {
        grouped
            .entry(&observation.identity)
            .or_default()
            .push(observation);
    }

    let has_start = grouped.iter().any(|(identity, observations)| {
        unique_runtime_function_equivalent(functions, identity, roles)
            .is_some_and(|definition| is_specialized_element_start_shape(definition, observations))
    });
    let has_end = grouped.iter().any(|(identity, observations)| {
        unique_runtime_function_equivalent(functions, identity, roles)
            .is_some_and(|definition| is_specialized_element_end_shape(definition, observations))
    });
    has_start && has_end
}

fn infer_specialized_element_pair(
    functions: &[RuntimeFunction],
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
    roles: &IvyRoleTable,
) -> Vec<(SymbolIdentity, &'static str)> {
    let mut starts = HashSet::new();
    let mut ends = HashSet::new();
    for (identity, calls) in observations {
        let Some(definition) = unique_runtime_function_equivalent(functions, identity, roles)
        else {
            continue;
        };
        if is_specialized_element_start_shape(definition, calls) {
            starts.insert(definition.identity.clone());
        }
        if is_specialized_element_end_shape(definition, calls) {
            ends.insert(definition.identity.clone());
        }
    }

    match (
        starts.into_iter().collect::<Vec<_>>().as_slice(),
        ends.into_iter().collect::<Vec<_>>().as_slice(),
    ) {
        ([start], [end]) if start != end => vec![
            (start.clone(), "ɵɵelementStart"),
            (end.clone(), "ɵɵelementEnd"),
        ],
        _ => Vec::new(),
    }
}

fn is_specialized_element_start_shape(
    definition: &RuntimeFunction,
    observations: &[impl std::borrow::Borrow<TemplateCallObservation>],
) -> bool {
    plain_parameter_bindings(definition).is_some_and(|parameters| parameters.len() == 4)
        && returns_identity(definition, &definition.identity)
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
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 2
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
        && calls.iter().any(|call| {
            call.arguments.len() >= 6 && forwards_parameters_in_order(call, &parameters)
        })
}

fn is_embedded_template_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    definition.params.len() == 8
        && direct_calls(definition).len() >= 3
        && observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && matches!(observation.arguments.len(), 4..=8)
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
                && observation.arguments.get(1).is_some_and(|argument| {
                    matches!(
                        argument.as_ref(),
                        Expr::Ident(_) | Expr::Fn(_) | Expr::Arrow(_)
                    )
                })
                && observation
                    .arguments
                    .get(2)
                    .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
                && observation
                    .arguments
                    .get(3)
                    .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
                && observation.arguments.get(4).is_none_or(|argument| {
                    is_string_literal(argument.as_ref())
                        || matches!(argument.as_ref(), Expr::Lit(Lit::Null(_)))
                })
                && observation.arguments.get(5).is_none_or(|argument| {
                    is_nonnegative_integer(argument.as_ref())
                        || matches!(argument.as_ref(), Expr::Lit(Lit::Null(_)))
                })
        })
}

fn is_conditional_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    plain_parameter_bindings(definition).is_some_and(|parameters| parameters.len() == 2)
        && direct_calls(definition).len() >= 6
        && contains_negative_one(&definition.body)
        && observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 2
                && matches!(observation.arguments.len(), 1 | 2)
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_template_selection(argument.as_ref()))
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
            observation.usage == TemplateCallUsage::Initializer
                && observation.phase == 2
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
    let [call] = calls.as_slice() else {
        return false;
    };
    call.arguments.as_slice().first().is_some_and(|argument| {
        matches!(
            argument.as_ref(),
            Expr::Ident(identifier) if binding_key(identifier) == binding_key(&parameter.id)
        )
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
    functions: &[RuntimeFunction],
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
    roles: &IvyRoleTable,
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
        let Some(wrapper) = unique_runtime_function_equivalent(functions, identity, roles) else {
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
        let Some(target) =
            unique_runtime_function_equivalent(functions, &target_call.callee, roles)
        else {
            continue;
        };
        let Some(target_parameters) = plain_parameter_bindings(target) else {
            continue;
        };
        if target_parameters.len() != 3
            || !returns_identity(target, &target.identity)
            || direct_calls(target).len() < 3
        {
            continue;
        }
        inferred.push((identity.clone(), "ɵɵtextInterpolate"));
        inferred.push((target.identity.clone(), "ɵɵtextInterpolate1"));
    }
    inferred
}

fn unique_runtime_function_equivalent<'a>(
    functions: &'a [RuntimeFunction],
    identity: &SymbolIdentity,
    roles: &IvyRoleTable,
) -> Option<&'a RuntimeFunction> {
    let mut candidates = functions
        .iter()
        .filter(|function| roles.symbols_equivalent(&function.identity, identity));
    let candidate = candidates.next()?;
    candidates.next().is_none().then_some(candidate)
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
    fn record_function(&mut self, target: &Expr, function: &Function) {
        let Some(identity) = symbol_identity(target, self.unresolved_ctxt) else {
            return;
        };
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

    fn record_arrow(&mut self, target: &Expr, arrow: &ArrowExpr) {
        let Some(identity) = symbol_identity(target, self.unresolved_ctxt) else {
            return;
        };
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

    fn record_expression(&mut self, target: &Expr, value: &Expr) {
        match value {
            Expr::Fn(function) => self.record_function(target, function.function.as_ref()),
            Expr::Arrow(arrow) => self.record_arrow(target, arrow),
            Expr::Paren(paren) => self.record_expression(target, paren.expr.as_ref()),
            _ => {}
        }
    }
}

impl Visit for RuntimeFunctionCollector {
    fn visit_fn_decl(&mut self, declaration: &FnDecl) {
        self.record_function(
            &Expr::Ident(declaration.ident.clone()),
            declaration.function.as_ref(),
        );
        declaration.function.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Pat::Ident(binding), Some(value)) = (&declarator.name, declarator.init.as_deref()) {
            self.record_expression(&Expr::Ident(binding.id.clone()), value);
        }
        declarator.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        if let Some(target) = assignment_target_expression(&assignment.left) {
            self.record_expression(&target, assignment.right.as_ref());
        }
        assignment.visit_children_with(self);
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
        if !has_unique_self_returning_arity(&by_identity, &start.callee, 4)
            || !has_unique_self_returning_arity(&by_identity, &end.callee, 0)
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
    returns.expressions.iter().any(|expression| {
        let mut finder = IdentityFinder {
            wanted: identity,
            unresolved_ctxt: function.unresolved_ctxt,
            found: false,
        };
        expression.visit_with(&mut finder);
        finder.found
    })
}

struct IdentityFinder<'a> {
    wanted: &'a SymbolIdentity,
    unresolved_ctxt: SyntaxContext,
    found: bool,
}

impl Visit for IdentityFinder<'_> {
    fn visit_expr(&mut self, expression: &Expr) {
        if symbol_identity(expression, self.unresolved_ctxt).as_ref() == Some(self.wanted) {
            self.found = true;
            return;
        }
        expression.visit_children_with(self);
    }

    fn visit_function(&mut self, _function: &Function) {}

    fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
}

struct DirectCall {
    callee: SymbolIdentity,
    arguments: Vec<Box<Expr>>,
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

fn has_unique_self_returning_arity(
    functions: &HashMap<&SymbolIdentity, Vec<&RuntimeFunction>>,
    identity: &SymbolIdentity,
    arity: usize,
) -> bool {
    let Some(candidates) = functions.get(identity) else {
        return false;
    };
    let mut matching = candidates.iter().filter(|candidate| {
        candidate.params.len() == arity && returns_identity(candidate, identity)
    });
    matching.next().is_some() && matching.next().is_none()
}
