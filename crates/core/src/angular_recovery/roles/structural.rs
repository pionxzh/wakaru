use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Span, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayLit, ArrowExpr, AssignExpr, AssignOp, AssignTarget, BinaryOp, BlockStmt, BlockStmtOrExpr,
    CallExpr, Callee, CatchClause, Class, ClassDecl, ClassMember, Expr, ExprOrSpread, FnDecl,
    ForHead, ForInStmt, ForOfStmt, Function, ImportDecl, ImportSpecifier, Lit, MemberProp,
    ObjectLit, ObjectPatProp, Pat, Prop, PropName, PropOrSpread, ReturnStmt, SimpleAssignTarget,
    Stmt, ThrowStmt, UnaryExpr, UnaryOp, UpdateExpr, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::{
    symbol_identity, IvyInstruction, IvyRoleTable, QueryInitializerRole, SymbolIdentity,
    REFERENCE_CANDIDATE_NAME,
};
use crate::angular_recovery::syntax::{
    binding_key, member_prop_name, prop_name, render_flag_mask, wtf8_to_string, BindingKey,
};
use crate::angular_recovery::PreparedAngularModule;

pub(super) struct StructuralRoleEvidence {
    functions: Vec<RuntimeFunction>,
    classes: Vec<RuntimeClass>,
    definition_counts: HashMap<SymbolIdentity, usize>,
    invalid_values: HashSet<SymbolIdentity>,
    assignment_definitions: HashMap<SymbolIdentity, Vec<(usize, u32)>>,
    value_aliases: Vec<(SymbolIdentity, SymbolIdentity)>,
    integer_constants: HashMap<BindingKey, u64>,
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
        inferred.extend(infer_element_container_family(&self.functions));
        inferred.extend(
            self.functions
                .iter()
                .filter(|function| is_specialized_i18n_postprocess_shape(function))
                .map(|function| (function.identity.clone(), "ɵɵi18nPostprocess")),
        );
        inferred
    }

    pub(super) fn infer_listener_target_roles(&self) -> Vec<(SymbolIdentity, &'static str)> {
        self.functions
            .iter()
            .filter_map(|function| {
                let name =
                    if returns_parameter_member_path(function, &["ownerDocument", "defaultView"]) {
                        "ɵɵresolveWindow"
                    } else if returns_parameter_member_path(function, &["ownerDocument", "body"]) {
                        "ɵɵresolveBody"
                    } else if returns_parameter_member_path(function, &["ownerDocument"]) {
                        "ɵɵresolveDocument"
                    } else {
                        return None;
                    };
                Some((function.identity.clone(), name))
            })
            .collect()
    }

    fn propagate_class_api_aliases(
        &self,
        inferred: Vec<(SymbolIdentity, &'static str)>,
    ) -> Vec<(SymbolIdentity, &'static str)> {
        let mut names = inferred.into_iter().collect::<HashMap<_, _>>();
        let specialized_signals = self
            .specialized_signal_api_calls()
            .into_iter()
            .map(|(identity, _)| identity)
            .collect::<HashSet<_>>();
        let mut adjacency = HashMap::<SymbolIdentity, Vec<SymbolIdentity>>::new();
        for (left, right) in &self.value_aliases {
            adjacency
                .entry(left.clone())
                .or_default()
                .push(right.clone());
            adjacency
                .entry(right.clone())
                .or_default()
                .push(left.clone());
        }

        let mut visited = HashSet::new();
        for start in adjacency.keys() {
            if visited.contains(start) {
                continue;
            }
            let mut stack = vec![start.clone()];
            let mut component = Vec::new();
            while let Some(identity) = stack.pop() {
                if !visited.insert(identity.clone()) {
                    continue;
                }
                if let Some(neighbors) = adjacency.get(&identity) {
                    stack.extend(neighbors.iter().cloned());
                }
                component.push(identity);
            }
            let component_names = component
                .iter()
                .filter_map(|identity| names.get(identity).copied())
                .collect::<HashSet<_>>();
            let mut component_names = component_names.into_iter();
            let Some(name) = component_names.next() else {
                continue;
            };
            if component_names.next().is_some() {
                continue;
            }
            if component
                .iter()
                .filter(|identity| specialized_signals.contains(*identity))
                .take(2)
                .count()
                > 1
            {
                continue;
            }
            for identity in component {
                names.entry(identity).or_insert(name);
            }
        }
        names.into_iter().collect()
    }

    pub(super) fn infer_class_api_roles(&self) -> Vec<(SymbolIdentity, &'static str)> {
        let mut inferred = Vec::new();

        let mut signal_apis = self
            .functions
            .iter()
            .filter(|function| is_signal_api_shape(function))
            .map(|function| function.identity.clone())
            .collect::<HashSet<_>>();
        signal_apis.extend(
            self.specialized_signal_api_calls()
                .into_iter()
                .map(|(identity, _)| identity),
        );
        inferred.extend(
            signal_apis
                .iter()
                .cloned()
                .map(|identity| (identity, "signal")),
        );

        let computed_factories = self
            .functions
            .iter()
            .filter(|function| is_computed_factory_shape(function))
            .map(|function| function.identity.clone())
            .collect::<HashSet<_>>();
        inferred.extend(
            self.functions
                .iter()
                .filter(|function| is_computed_api_shape(function, &computed_factories))
                .map(|function| (function.identity.clone(), "computed")),
        );

        let inject_flags_helpers = self
            .functions
            .iter()
            .filter(|function| is_inject_options_flags_shape(function))
            .map(|function| function.identity.clone())
            .collect::<HashSet<_>>();
        inferred.extend(
            self.functions
                .iter()
                .filter(|function| is_inject_api_shape(function, &inject_flags_helpers))
                .map(|function| (function.identity.clone(), "inject")),
        );

        let mut input_apis = self
            .functions
            .iter()
            .filter(|function| is_input_signal_factory_shape(function))
            .map(|function| function.identity.clone())
            .collect::<HashSet<_>>();
        loop {
            let wrappers = self
                .functions
                .iter()
                .filter(|function| {
                    !input_apis.contains(&function.identity)
                        && is_forwarding_api_wrapper(function, &input_apis)
                })
                .map(|function| function.identity.clone())
                .collect::<Vec<_>>();
            if wrappers.is_empty() {
                break;
            }
            input_apis.extend(wrappers);
        }
        inferred.extend(input_apis.into_iter().map(|identity| (identity, "input")));

        let mut model_apis = self
            .functions
            .iter()
            .filter(|function| is_model_signal_factory_shape(function))
            .map(|function| function.identity.clone())
            .collect::<HashSet<_>>();
        loop {
            let wrappers = self
                .functions
                .iter()
                .filter(|function| {
                    !model_apis.contains(&function.identity)
                        && is_forwarding_api_wrapper(function, &model_apis)
                })
                .map(|function| function.identity.clone())
                .collect::<Vec<_>>();
            if wrappers.is_empty() {
                break;
            }
            model_apis.extend(wrappers);
        }
        inferred.extend(model_apis.into_iter().map(|identity| (identity, "model")));

        let output_classes = self
            .classes
            .iter()
            .filter(|class| is_output_ref_class_shape(&class.class))
            .map(|class| class.identity.clone())
            .collect::<HashSet<_>>();
        inferred.extend(
            output_classes
                .iter()
                .cloned()
                .map(|identity| (identity, "output")),
        );
        inferred.extend(
            self.functions
                .iter()
                .filter(|function| is_output_api_shape(function, &output_classes))
                .map(|function| (function.identity.clone(), "output")),
        );

        self.propagate_class_api_aliases(inferred)
    }

    pub(super) fn specialized_class_api_call_arguments(
        &self,
    ) -> Vec<(SymbolIdentity, Vec<Box<Expr>>)> {
        let direct = self.specialized_signal_api_calls();
        let mut arguments = direct.iter().cloned().collect::<HashMap<_, _>>();
        let mut adjacency = HashMap::<SymbolIdentity, Vec<SymbolIdentity>>::new();
        for (left, right) in &self.value_aliases {
            adjacency
                .entry(left.clone())
                .or_default()
                .push(right.clone());
            adjacency
                .entry(right.clone())
                .or_default()
                .push(left.clone());
        }

        let direct_identities = direct
            .iter()
            .map(|(identity, _)| identity)
            .collect::<HashSet<_>>();
        let mut visited = HashSet::new();
        for start in adjacency.keys() {
            if visited.contains(start) {
                continue;
            }
            let mut stack = vec![start.clone()];
            let mut component = Vec::new();
            while let Some(identity) = stack.pop() {
                if !visited.insert(identity.clone()) {
                    continue;
                }
                if let Some(neighbors) = adjacency.get(&identity) {
                    stack.extend(neighbors.iter().cloned());
                }
                component.push(identity);
            }
            let sources = component
                .iter()
                .filter(|identity| direct_identities.contains(*identity))
                .collect::<Vec<_>>();
            let [source] = sources.as_slice() else {
                continue;
            };
            let Some(source_arguments) = arguments.get(*source).cloned() else {
                continue;
            };
            for identity in component {
                arguments
                    .entry(identity)
                    .or_insert_with(|| source_arguments.clone());
            }
        }
        arguments.into_iter().collect()
    }

    fn specialized_signal_api_calls(&self) -> Vec<(SymbolIdentity, Vec<Box<Expr>>)> {
        let factories = self
            .functions
            .iter()
            .filter_map(|function| {
                specialized_signal_factory_argument(function)
                    .map(|argument| (function.identity.clone(), argument))
            })
            .collect::<HashMap<_, _>>();

        self.functions
            .iter()
            .filter_map(|function| {
                specialized_signal_api_arguments(function, &factories)
                    .map(|arguments| (function.identity.clone(), arguments))
            })
            .collect()
    }

    pub(super) fn infer_query_initializer_roles(
        &self,
    ) -> Vec<(SymbolIdentity, QueryInitializerRole)> {
        let mut roles = self
            .functions
            .iter()
            .filter(|function| is_query_signal_factory_shape(function))
            .map(|function| {
                (
                    function.identity.clone(),
                    QueryInitializerRole::DynamicFactory,
                )
            })
            .collect::<HashMap<_, _>>();

        loop {
            let inferred = self
                .functions
                .iter()
                .filter(|function| !roles.contains_key(&function.identity))
                .filter_map(|function| {
                    query_initializer_wrapper_role(function, &roles)
                        .map(|role| (function.identity.clone(), role))
                })
                .collect::<Vec<_>>();
            if inferred.is_empty() {
                break;
            }
            roles.extend(inferred);
        }

        let mut adjacency = HashMap::<SymbolIdentity, Vec<SymbolIdentity>>::new();
        for (left, right) in &self.value_aliases {
            adjacency
                .entry(left.clone())
                .or_default()
                .push(right.clone());
            adjacency
                .entry(right.clone())
                .or_default()
                .push(left.clone());
        }
        let mut visited = HashSet::new();
        for start in adjacency.keys() {
            if visited.contains(start) {
                continue;
            }
            let mut stack = vec![start.clone()];
            let mut component = Vec::new();
            while let Some(identity) = stack.pop() {
                if !visited.insert(identity.clone()) {
                    continue;
                }
                if let Some(neighbors) = adjacency.get(&identity) {
                    stack.extend(neighbors.iter().cloned());
                }
                component.push(identity);
            }
            let component_roles = component
                .iter()
                .filter_map(|identity| roles.get(identity).copied())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let [role] = component_roles.as_slice() else {
                continue;
            };
            let role = *role;
            for identity in component {
                roles.entry(identity).or_insert(role);
            }
        }

        roles.into_iter().collect()
    }

    pub(super) fn infer_template_roles(
        &self,
        modules: &[PreparedAngularModule],
        roles: &IvyRoleTable,
    ) -> Vec<(SymbolIdentity, &'static str)> {
        let function_index = RuntimeFunctionIndex::new(&self.functions, roles);
        let mut observations = Vec::new();
        let mut creation_null_assignments = Vec::new();
        let mut creation_false_assignments = Vec::new();
        let mut next_view_id = 0;
        for prepared in modules {
            let mut collector = TemplateFunctionCollector {
                roles,
                function_index: &function_index,
                unresolved_ctxt: prepared.unresolved_ctxt,
                observations: Vec::new(),
                creation_null_assignments: Vec::new(),
                creation_false_assignments: Vec::new(),
                next_view_id,
            };
            prepared.module.visit_with(&mut collector);
            next_view_id = collector.next_view_id;
            observations.extend(collector.observations);
            creation_null_assignments.extend(collector.creation_null_assignments);
            creation_false_assignments.extend(collector.creation_false_assignments);
        }

        let mut by_identity: HashMap<SymbolIdentity, Vec<TemplateCallObservation>> = HashMap::new();
        for observation in observations {
            by_identity
                .entry(observation.identity.clone())
                .or_default()
                .push(observation);
        }

        let mut inferred = infer_specialized_element_pair(&function_index, &by_identity);
        inferred.extend(infer_specialized_element_container_pair(
            &function_index,
            &by_identity,
        ));
        inferred.extend(infer_namespace_family(
            &function_index,
            &by_identity,
            &creation_null_assignments,
        ));
        inferred.extend(infer_aria_property_family(&function_index, &by_identity));
        inferred.extend(infer_styling_map_family(
            &self.functions,
            &function_index,
            &by_identity,
        ));
        inferred.extend(infer_styling_property_family(&function_index, &by_identity));
        inferred.extend(infer_i18n_role_family(
            &function_index,
            &by_identity,
            &creation_false_assignments,
        ));
        let two_way_roles = infer_two_way_role_family(&function_index, &by_identity);
        let animation_roles = infer_animation_role_family(&function_index, &by_identity);
        let specialized_identities = two_way_roles
            .iter()
            .chain(&animation_roles)
            .map(|(identity, _)| identity.clone())
            .collect::<HashSet<_>>();
        inferred.extend(two_way_roles);
        inferred.extend(animation_roles);
        inferred.extend(infer_text_interpolation_family(
            &function_index,
            &by_identity,
        ));
        inferred.extend(infer_expression_interpolation_family(
            &function_index,
            &by_identity,
        ));
        inferred.extend(infer_embedded_template_continuation_family(
            &function_index,
            &by_identity,
        ));
        inferred.extend(infer_defer_role_family(&function_index, &by_identity));
        inferred.extend(infer_repeater_role_family(&function_index, &by_identity));
        inferred.extend(infer_projection_role_family(&function_index, &by_identity));
        inferred.extend(infer_view_state_role_family(
            &self.functions,
            modules,
            &self.integer_constants,
            roles,
        ));
        inferred.extend(infer_pure_function_family(&function_index, &by_identity));
        for (identity, observations) in &by_identity {
            if specialized_identities.contains(identity) {
                continue;
            }
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
            if is_property_interpolate_shape(definition, observations) {
                matches.push("ɵɵpropertyInterpolate");
            }
            if is_property_shape(definition, observations) {
                matches.push("ɵɵproperty");
            }
            if is_attribute_shape(definition, observations) {
                matches.push("ɵɵattribute");
            }
            if is_embedded_template_shape(definition, observations) {
                matches.push("ɵɵtemplate");
            }
            if is_conditional_shape(definition, observations) {
                matches.push("ɵɵconditional");
            }
            if is_next_context_shape(definition, observations, &self.integer_constants) {
                matches.push("ɵɵnextContext");
            }
            if is_reference_shape(definition, observations) {
                matches.push("ɵɵreference");
            } else if is_reference_candidate_shape(
                definition,
                observations,
                &self.integer_constants,
                &function_index,
            ) {
                inferred.push((definition.identity.clone(), REFERENCE_CANDIDATE_NAME));
            }
            if is_declare_let_shape(definition, observations) {
                matches.push("ɵɵdeclareLet");
            }
            if is_store_let_shape(definition, observations) {
                matches.push("ɵɵstoreLet");
            }
            if is_read_context_let_shape(definition, observations) {
                matches.push("ɵɵreadContextLet");
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

    pub(super) fn inferred_namespace_state_targets(
        &self,
        roles: &IvyRoleTable,
    ) -> HashSet<SymbolIdentity> {
        self.functions
            .iter()
            .filter(|function| {
                roles
                    .ivy_names
                    .get(&function.identity)
                    .and_then(|name| IvyInstruction::from_export_name(name))
                    .is_some_and(|instruction| {
                        matches!(
                            instruction,
                            IvyInstruction::NamespaceHtml
                                | IvyInstruction::NamespaceSvg
                                | IvyInstruction::NamespaceMathMl
                        )
                    })
            })
            .filter_map(|function| namespace_assignment(function).map(|(target, _)| target))
            .collect()
    }

    pub(super) fn inferred_i18n_state_targets(
        &self,
        roles: &IvyRoleTable,
    ) -> HashSet<SymbolIdentity> {
        self.functions
            .iter()
            .filter(|function| {
                roles
                    .ivy_names
                    .get(&function.identity)
                    .and_then(|name| IvyInstruction::from_export_name(name))
                    == Some(IvyInstruction::I18nStart)
            })
            .filter_map(optimized_i18n_start_target)
            .collect()
    }
}

fn is_signal_api_shape(function: &RuntimeFunction) -> bool {
    let Some(initial) = runtime_parameter_binding(function, 0) else {
        return false;
    };
    let Some(options) = runtime_parameter_binding(function, 1) else {
        return false;
    };
    let Some(returned) = single_returned_binding(&function.body) else {
        return false;
    };

    let mut destructured = None;
    for statement in &function.body.stmts {
        let Stmt::Decl(swc_core::ecma::ast::Decl::Var(declaration)) = statement else {
            continue;
        };
        for declarator in &declaration.decls {
            let Pat::Array(array) = &declarator.name else {
                continue;
            };
            let [Some(Pat::Ident(read)), Some(Pat::Ident(set)), Some(Pat::Ident(update))] =
                array.elems.as_slice()
            else {
                continue;
            };
            if array.optional || array.type_ann.is_some() {
                continue;
            }
            let Some(Expr::Call(call)) = declarator.init.as_deref().map(strip_parentheses) else {
                continue;
            };
            if call.args.len() != 2
                || call.args.iter().any(|argument| argument.spread.is_some())
                || !expression_is_binding(call.args[0].expr.as_ref(), &initial)
                || !expression_has_member_on_binding(call.args[1].expr.as_ref(), &options, "equal")
            {
                continue;
            }
            destructured = Some((
                binding_key(&read.id),
                binding_key(&set.id),
                binding_key(&update.id),
            ));
        }
    }
    let Some((read, set, update)) = destructured else {
        return false;
    };
    if returned != read {
        return false;
    }

    let assignments = top_level_assignments(&function.body);
    member_assignment_from_binding(&assignments, &read, "set", &set)
        && member_assignment_from_binding(&assignments, &read, "update", &update)
        && member_assignment_binds_receiver(&assignments, &read)
}

fn specialized_signal_factory_argument(function: &RuntimeFunction) -> Option<Box<Expr>> {
    if !function.params.is_empty() {
        return None;
    }
    let mut returns = ReturnExpressionCollector::default();
    function.body.visit_with(&mut returns);
    let [returned] = returns.expressions.as_slice() else {
        return None;
    };
    let returned = match strip_parentheses(returned.as_ref()) {
        Expr::Seq(sequence) => sequence.exprs.last()?.as_ref(),
        returned => returned,
    };
    let Expr::Array(returned) = strip_parentheses(returned) else {
        return None;
    };
    let [Some(read), Some(set), Some(update)] = returned.elems.as_slice() else {
        return None;
    };
    if read.spread.is_some()
        || set.spread.is_some()
        || update.spread.is_some()
        || !matches!(
            strip_parentheses(set.expr.as_ref()),
            Expr::Arrow(_) | Expr::Fn(_)
        )
        || !matches!(
            strip_parentheses(update.expr.as_ref()),
            Expr::Arrow(_) | Expr::Fn(_)
        )
    {
        return None;
    }
    let Expr::Ident(read) = strip_parentheses(read.expr.as_ref()) else {
        return None;
    };
    let read = binding_key(read);
    let assignments = top_level_assignments(&function.body);
    let candidates = object_create_state_bindings(&function.body)
        .into_iter()
        .filter_map(|state| {
            let initial = member_assignment_value(&assignments, &state, "value")?;
            if !is_portable_specialized_argument(initial.as_ref())
                || !computed_state_is_attached(&assignments, &read, &state)
                || !local_function_value(&function.body, &read).is_some_and(|read| {
                    read.params.is_empty() && local_function_returns_member(&read, &state, "value")
                })
            {
                return None;
            }
            Some(initial)
        })
        .collect::<Vec<_>>();
    let [initial] = candidates.as_slice() else {
        return None;
    };
    Some(initial.clone())
}

fn specialized_signal_api_arguments(
    function: &RuntimeFunction,
    factories: &HashMap<SymbolIdentity, Box<Expr>>,
) -> Option<Vec<Box<Expr>>> {
    if !function.params.is_empty() {
        return None;
    }
    let returned = single_returned_binding(&function.body)?;
    let assignments = top_level_assignments(&function.body);
    let mut candidates = Vec::new();

    for statement in &function.body.stmts {
        let Stmt::Decl(swc_core::ecma::ast::Decl::Var(declaration)) = statement else {
            continue;
        };
        for declarator in &declaration.decls {
            let Pat::Array(array) = &declarator.name else {
                continue;
            };
            let [Some(Pat::Ident(read)), Some(Pat::Ident(set)), Some(Pat::Ident(update))] =
                array.elems.as_slice()
            else {
                continue;
            };
            let Some(Expr::Call(call)) = declarator.init.as_deref().map(strip_parentheses) else {
                continue;
            };
            if !call.args.is_empty() {
                continue;
            }
            let Some(factory) = call_callee_identity(call, function.unresolved_ctxt) else {
                continue;
            };
            let Some(initial) = factories.get(&factory) else {
                continue;
            };
            let (read, set, update) = (
                binding_key(&read.id),
                binding_key(&set.id),
                binding_key(&update.id),
            );
            if returned == read
                && member_assignment_from_binding(&assignments, &read, "set", &set)
                && member_assignment_from_binding(&assignments, &read, "update", &update)
                && member_assignment_binds_receiver(&assignments, &read)
            {
                candidates.push(initial.clone());
            }
        }
    }

    let [initial] = candidates.as_slice() else {
        return None;
    };
    Some(vec![initial.clone()])
}

fn is_portable_specialized_argument(expression: &Expr) -> bool {
    match strip_parentheses(expression) {
        Expr::Lit(Lit::Null(_) | Lit::Bool(_) | Lit::Num(_) | Lit::Str(_) | Lit::BigInt(_)) => true,
        Expr::Unary(unary) if matches!(unary.op, UnaryOp::Plus | UnaryOp::Minus) => {
            matches!(
                strip_parentheses(unary.arg.as_ref()),
                Expr::Lit(Lit::Num(_))
            )
        }
        _ => false,
    }
}

fn is_computed_factory_shape(function: &RuntimeFunction) -> bool {
    let Some(computation) = runtime_parameter_binding(function, 0) else {
        return false;
    };
    let Some(equal) = runtime_parameter_binding(function, 1) else {
        return false;
    };
    let Some(returned) = single_returned_binding(&function.body) else {
        return false;
    };
    let assignments = top_level_assignments(&function.body);

    object_create_state_bindings(&function.body)
        .into_iter()
        .any(|state| {
            member_assignment_from_any_property(&assignments, &state, &computation)
                && member_assignment_from_binding(&assignments, &state, "equal", &equal)
                && computed_state_is_attached(&assignments, &returned, &state)
                && local_function_value(&function.body, &returned).is_some_and(|read| {
                    read.params.is_empty()
                        && local_function_returns_member(&read, &state, "value")
                        && local_function_contains_member(&read, &state, "error")
                })
        })
}

fn is_computed_api_shape(
    function: &RuntimeFunction,
    computed_factories: &HashSet<SymbolIdentity>,
) -> bool {
    if is_specialized_computed_api_shape(function) {
        return true;
    }

    let Some(computation) = runtime_parameter_binding(function, 0) else {
        return false;
    };
    let Some(options) = runtime_parameter_binding(function, 1) else {
        return false;
    };
    let Some(call) = single_returned_call(&function.body) else {
        return false;
    };
    let Some(identity) = call_callee_identity(call, function.unresolved_ctxt) else {
        return false;
    };
    computed_factories.contains(&identity)
        && call.args.len() == 2
        && call.args.iter().all(|argument| argument.spread.is_none())
        && expression_is_binding(call.args[0].expr.as_ref(), &computation)
        && expression_has_member_on_binding(call.args[1].expr.as_ref(), &options, "equal")
}

fn is_specialized_computed_api_shape(function: &RuntimeFunction) -> bool {
    let Some(computation) = runtime_parameter_binding(function, 0) else {
        return false;
    };
    if function.params.len() != 1 {
        return false;
    }
    let Some(returned) = single_returned_binding(&function.body) else {
        return false;
    };
    let assignments = top_level_assignments(&function.body);

    object_create_state_bindings(&function.body)
        .into_iter()
        .any(|state| {
            member_assignment_from_any_property(&assignments, &state, &computation)
                && computed_state_is_attached(&assignments, &returned, &state)
                && local_function_value(&function.body, &returned).is_some_and(|read| {
                    read.params.is_empty()
                        && local_function_returns_member(&read, &state, "value")
                        && local_function_contains_member(&read, &state, "error")
                })
        })
}

fn is_inject_options_flags_shape(function: &RuntimeFunction) -> bool {
    let Some(options) = runtime_parameter_binding(function, 0) else {
        return false;
    };
    if function.params.len() != 1 {
        return false;
    }

    let mut evidence = InjectFlagsEvidence {
        options: &options,
        properties: HashSet::new(),
        typeof_options: false,
        type_strings: HashSet::new(),
        bitwise_or: false,
        closure_undefined_check: false,
    };
    function.body.visit_with(&mut evidence);
    let mut returns = ReturnExpressionCollector::default();
    function.body.visit_with(&mut returns);

    evidence.typeof_options
        && (evidence.type_strings.contains("undefined") || evidence.closure_undefined_check)
        && evidence.type_strings.contains("number")
        && evidence.bitwise_or
        && ["optional", "host", "self"]
            .iter()
            .all(|property| evidence.properties.contains(*property))
        && returns
            .expressions
            .iter()
            .any(|expression| expression_contains_binding(expression.as_ref(), &options))
        && !returns.expressions.is_empty()
}

fn is_inject_api_shape(
    function: &RuntimeFunction,
    flags_helpers: &HashSet<SymbolIdentity>,
) -> bool {
    let Some(token) = runtime_parameter_binding(function, 0) else {
        return false;
    };
    let Some(options) = runtime_parameter_binding(function, 1) else {
        return false;
    };
    let Some(call) = single_returned_call(&function.body) else {
        return false;
    };
    if call.args.len() != 2
        || call.args.iter().any(|argument| argument.spread.is_some())
        || !expression_is_binding(call.args[0].expr.as_ref(), &token)
    {
        return false;
    }
    let Expr::Call(flags_call) = strip_parentheses(call.args[1].expr.as_ref()) else {
        return false;
    };
    flags_call.args.len() == 1
        && flags_call.args[0].spread.is_none()
        && expression_is_binding(flags_call.args[0].expr.as_ref(), &options)
        && call_callee_identity(flags_call, function.unresolved_ctxt)
            .is_some_and(|identity| flags_helpers.contains(&identity))
}

fn is_input_signal_factory_shape(function: &RuntimeFunction) -> bool {
    let Some(initial) = runtime_parameter_binding(function, 0) else {
        return false;
    };
    let Some(options) = runtime_parameter_binding(function, 1) else {
        return false;
    };
    let Some(returned) = single_returned_binding(&function.body) else {
        return false;
    };
    let assignments = top_level_assignments(&function.body);

    object_create_state_bindings(&function.body)
        .into_iter()
        .any(|state| {
            member_assignment_from_binding(&assignments, &state, "value", &initial)
                && assignments.iter().any(|assignment| {
                    member_assignment_target(&assignment.left)
                        .is_some_and(|(object, _)| object == state)
                        && expression_has_member_on_binding(
                            assignment.right.as_ref(),
                            &options,
                            "transform",
                        )
                })
                && computed_state_is_attached(&assignments, &returned, &state)
                && local_function_value(&function.body, &returned).is_some_and(|read| {
                    read.params.is_empty()
                        && local_function_returns_member(&read, &state, "value")
                        && local_function_contains_number(&read, -950.0)
                })
        })
}

fn is_model_signal_factory_shape(function: &RuntimeFunction) -> bool {
    let Some(initial) = runtime_parameter_binding(function, 0) else {
        return false;
    };
    if function.params.len() != 1 {
        return false;
    }
    let Some(returned) = single_returned_binding(&function.body) else {
        return false;
    };
    let assignments = top_level_assignments(&function.body);

    object_create_state_bindings(&function.body)
        .into_iter()
        .any(|state| {
            member_assignment_from_binding(&assignments, &state, "value", &initial)
                && computed_state_is_attached(&assignments, &returned, &state)
                && ["set", "update", "subscribe"]
                    .iter()
                    .all(|property| member_assignment_exists(&assignments, &returned, property))
                && member_assignment_binds_receiver(&assignments, &returned)
                && local_function_value(&function.body, &returned).is_some_and(|read| {
                    read.params.is_empty()
                        && local_function_returns_member(&read, &state, "value")
                        && local_function_contains_number(&read, 952.0)
                })
        })
}

fn is_output_api_shape(
    function: &RuntimeFunction,
    output_classes: &HashSet<SymbolIdentity>,
) -> bool {
    if !function.params.is_empty() {
        return false;
    }
    let mut returns = ReturnExpressionCollector::default();
    function.body.visit_with(&mut returns);
    let [expression] = returns.expressions.as_slice() else {
        return false;
    };
    let Expr::New(created) = strip_parentheses(expression.as_ref()) else {
        return false;
    };
    if created
        .args
        .as_ref()
        .is_some_and(|arguments| !arguments.is_empty())
    {
        return false;
    }
    symbol_identity(created.callee.as_ref(), function.unresolved_ctxt)
        .is_some_and(|identity| output_classes.contains(&identity))
}

fn is_query_signal_factory_shape(function: &RuntimeFunction) -> bool {
    if !(2..=3).contains(&function.params.len())
        || single_returned_binding(&function.body).is_none()
    {
        return false;
    }
    let Some(first_only) = runtime_parameter_binding(function, 0) else {
        return false;
    };
    let Some(required) = runtime_parameter_binding(function, 1) else {
        return false;
    };

    struct Evidence<'a> {
        first_only: &'a BindingKey,
        required: &'a BindingKey,
        reads_first_only: bool,
        reads_required: bool,
        required_error: bool,
        reads_first_result: bool,
    }

    impl Visit for Evidence<'_> {
        fn visit_expr(&mut self, expression: &Expr) {
            if numeric_expression_value(expression) == Some(-951.0) {
                self.required_error = true;
            }
            expression.visit_children_with(self);
        }

        fn visit_ident(&mut self, identifier: &swc_core::ecma::ast::Ident) {
            let binding = binding_key(identifier);
            self.reads_first_only |= binding == *self.first_only;
            self.reads_required |= binding == *self.required;
        }

        fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
            if member_prop_name(&member.prop).as_deref() == Some("first") {
                self.reads_first_result = true;
            }
            member.visit_children_with(self);
        }
    }

    let mut evidence = Evidence {
        first_only: &first_only,
        required: &required,
        reads_first_only: false,
        reads_required: false,
        required_error: false,
        reads_first_result: false,
    };
    function.body.visit_with(&mut evidence);
    evidence.reads_first_only
        && evidence.reads_required
        && evidence.required_error
        && evidence.reads_first_result
}

fn query_initializer_wrapper_role(
    function: &RuntimeFunction,
    known_roles: &HashMap<SymbolIdentity, QueryInitializerRole>,
) -> Option<QueryInitializerRole> {
    let call = single_returned_call(&function.body)?;
    let target = call_callee_identity(call, function.unresolved_ctxt)?;
    if target == function.identity {
        return None;
    }
    match known_roles.get(&target)? {
        QueryInitializerRole::DynamicFactory => {
            let [first_only, required, ..] = call.args.as_slice() else {
                return None;
            };
            if first_only.spread.is_some() || required.spread.is_some() {
                return None;
            }
            let first_only = static_boolean_value(first_only.expr.as_ref())?;
            let required = static_boolean_value(required.expr.as_ref())?;
            (!(!first_only && required)).then_some(QueryInitializerRole::Fixed {
                multiple: !first_only,
                required,
            })
        }
        role @ QueryInitializerRole::Fixed { .. } => Some(*role),
    }
}

fn static_boolean_value(expression: &Expr) -> Option<bool> {
    match strip_parentheses(expression) {
        Expr::Lit(Lit::Bool(boolean)) => Some(boolean.value),
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => {
            match strip_parentheses(unary.arg.as_ref()) {
                Expr::Lit(Lit::Num(number)) => Some(number.value == 0.0),
                _ => None,
            }
        }
        _ => None,
    }
}

fn is_output_ref_class_shape(class: &Class) -> bool {
    let methods = class
        .body
        .iter()
        .filter_map(|member| {
            let ClassMember::Method(method) = member else {
                return None;
            };
            Some((prop_name(&method.key)?, method.function.as_ref()))
        })
        .collect::<Vec<_>>();
    let unique_method = |name: &str| {
        let candidates = methods
            .iter()
            .filter(|(method_name, _)| method_name == name)
            .map(|(_, function)| *function)
            .collect::<Vec<_>>();
        let [function] = candidates.as_slice() else {
            return None;
        };
        Some(*function)
    };
    let Some(subscribe) = unique_method("subscribe") else {
        return false;
    };
    let Some(subscribe_body) = &subscribe.body else {
        return false;
    };
    let emit_candidates = methods
        .iter()
        .filter(|(name, function)| {
            name != "subscribe"
                && function.params.len() == 1
                && function.body.as_ref().is_some_and(|body| {
                    block_contains_number(body, 953.0) || block_contains_string(body, "NG0953")
                })
        })
        .map(|(_, function)| *function)
        .collect::<Vec<_>>();
    let [_emit] = emit_candidates.as_slice() else {
        return false;
    };

    subscribe.params.len() == 1
        && block_contains_number(subscribe_body, 953.0)
        && contains_object_property(subscribe_body, "unsubscribe")
}

fn is_forwarding_api_wrapper(
    function: &RuntimeFunction,
    targets: &HashSet<SymbolIdentity>,
) -> bool {
    let Some(call) = single_returned_call(&function.body) else {
        return false;
    };
    let Some(target) = call_callee_identity(call, function.unresolved_ctxt) else {
        return false;
    };
    targets.contains(&target)
        && call.args.len() == function.params.len()
        && call
            .args
            .iter()
            .zip(&function.params)
            .all(|(argument, parameter)| {
                argument.spread.is_none()
                    && pat_binding(parameter).is_some_and(|binding| {
                        expression_is_binding(argument.expr.as_ref(), &binding)
                    })
            })
}

fn runtime_parameter_binding(function: &RuntimeFunction, index: usize) -> Option<BindingKey> {
    pat_binding(function.params.get(index)?)
}

fn pat_binding(pattern: &Pat) -> Option<BindingKey> {
    let Pat::Ident(binding) = pattern else {
        return None;
    };
    Some(binding_key(&binding.id))
}

fn single_returned_binding(body: &BlockStmt) -> Option<BindingKey> {
    let mut returns = ReturnExpressionCollector::default();
    body.visit_with(&mut returns);
    let [expression] = returns.expressions.as_slice() else {
        return None;
    };
    let expression = match strip_parentheses(expression.as_ref()) {
        Expr::Seq(sequence) => sequence.exprs.last()?.as_ref(),
        expression => expression,
    };
    let Expr::Ident(identifier) = strip_parentheses(expression) else {
        return None;
    };
    Some(binding_key(identifier))
}

fn single_returned_call(body: &BlockStmt) -> Option<&CallExpr> {
    let mut expression = None;
    for statement in &body.stmts {
        match statement {
            Stmt::Return(ReturnStmt {
                arg: Some(returned),
                ..
            }) if expression.is_none() => expression = Some(returned.as_ref()),
            Stmt::Return(_) => return None,
            Stmt::Empty(_) => {}
            _ => {}
        }
    }
    let Expr::Call(call) = strip_parentheses(expression?) else {
        return None;
    };
    Some(call)
}

fn call_callee_identity(call: &CallExpr, unresolved_ctxt: SyntaxContext) -> Option<SymbolIdentity> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    symbol_identity(callee.as_ref(), unresolved_ctxt)
}

fn expression_is_binding(expression: &Expr, expected: &BindingKey) -> bool {
    matches!(
        strip_parentheses(expression),
        Expr::Ident(identifier) if binding_key(identifier) == *expected
    )
}

fn expression_contains_binding(expression: &Expr, expected: &BindingKey) -> bool {
    struct Finder<'a> {
        expected: &'a BindingKey,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_ident(&mut self, identifier: &swc_core::ecma::ast::Ident) {
            if binding_key(identifier) == *self.expected {
                self.found = true;
            }
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut finder = Finder {
        expected,
        found: false,
    };
    expression.visit_with(&mut finder);
    finder.found
}

fn block_contains_binding(block: &BlockStmt, expected: &BindingKey) -> bool {
    struct Finder<'a> {
        expected: &'a BindingKey,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_ident(&mut self, identifier: &swc_core::ecma::ast::Ident) {
            if binding_key(identifier) == *self.expected {
                self.found = true;
            }
        }
    }

    let mut finder = Finder {
        expected,
        found: false,
    };
    block.visit_with(&mut finder);
    finder.found
}

fn returns_parameter_member_path(function: &RuntimeFunction, path: &[&str]) -> bool {
    let Some(parameters) = plain_parameter_bindings(function) else {
        return false;
    };
    let [parameter] = parameters.as_slice() else {
        return false;
    };
    let [Stmt::Return(ReturnStmt {
        arg: Some(expression),
        ..
    })] = function.body.stmts.as_slice()
    else {
        return false;
    };
    expression_is_parameter_member_path(expression.as_ref(), parameter, path)
}

fn expression_is_parameter_member_path(
    expression: &Expr,
    parameter: &BindingKey,
    path: &[&str],
) -> bool {
    let expression = strip_parentheses(expression);
    let Some((property, parent_path)) = path.split_last() else {
        return expression_is_binding(expression, parameter);
    };
    let Expr::Member(member) = expression else {
        return false;
    };
    member_prop_name(&member.prop).as_deref() == Some(*property)
        && expression_is_parameter_member_path(member.obj.as_ref(), parameter, parent_path)
}

fn expression_has_member_on_binding(
    expression: &Expr,
    object: &BindingKey,
    property: &str,
) -> bool {
    struct Finder<'a> {
        object: &'a BindingKey,
        property: &'a str,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
            if self.found {
                return;
            }
            if member_prop_name(&member.prop).as_deref() == Some(self.property)
                && matches!(
                    strip_parentheses(member.obj.as_ref()),
                    Expr::Ident(identifier) if binding_key(identifier) == *self.object
                )
            {
                self.found = true;
                return;
            }
            member.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut finder = Finder {
        object,
        property,
        found: false,
    };
    expression.visit_with(&mut finder);
    finder.found
}

fn object_create_state_bindings(body: &BlockStmt) -> Vec<BindingKey> {
    let mut bindings = Vec::new();
    for statement in &body.stmts {
        let Stmt::Decl(swc_core::ecma::ast::Decl::Var(declaration)) = statement else {
            continue;
        };
        for declarator in &declaration.decls {
            let Pat::Ident(binding) = &declarator.name else {
                continue;
            };
            let Some(Expr::Call(call)) = declarator.init.as_deref().map(strip_parentheses) else {
                continue;
            };
            let Callee::Expr(callee) = &call.callee else {
                continue;
            };
            let Expr::Member(member) = strip_parentheses(callee.as_ref()) else {
                continue;
            };
            if member_prop_name(&member.prop).as_deref() != Some("create")
                || !matches!(
                    strip_parentheses(member.obj.as_ref()),
                    Expr::Ident(identifier) if identifier.sym.as_ref() == "Object"
                )
            {
                continue;
            }
            bindings.push(binding_key(&binding.id));
        }
    }
    bindings
}

fn top_level_assignments(body: &BlockStmt) -> Vec<AssignExpr> {
    #[derive(Default)]
    struct Collector {
        assignments: Vec<AssignExpr>,
    }

    impl Visit for Collector {
        fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
            self.assignments.push(assignment.clone());
            assignment.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut collector = Collector::default();
    body.visit_with(&mut collector);
    collector.assignments
}

fn member_assignment_target(target: &AssignTarget) -> Option<(BindingKey, Atom)> {
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = target else {
        return None;
    };
    let Expr::Ident(object) = strip_parentheses(member.obj.as_ref()) else {
        return None;
    };
    Some((binding_key(object), member_prop_name(&member.prop)?))
}

fn member_assignment_object(target: &AssignTarget) -> Option<BindingKey> {
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = target else {
        return None;
    };
    let Expr::Ident(object) = strip_parentheses(member.obj.as_ref()) else {
        return None;
    };
    Some(binding_key(object))
}

fn member_assignment_from_binding(
    assignments: &[AssignExpr],
    object: &BindingKey,
    property: &str,
    value: &BindingKey,
) -> bool {
    assignments.iter().any(|assignment| {
        member_assignment_target(&assignment.left).is_some_and(
            |(assigned_object, assigned_property)| {
                assigned_object == *object && assigned_property.as_ref() == property
            },
        ) && expression_is_binding(assignment.right.as_ref(), value)
    })
}

fn member_assignment_from_any_property(
    assignments: &[AssignExpr],
    object: &BindingKey,
    value: &BindingKey,
) -> bool {
    assignments.iter().any(|assignment| {
        member_assignment_target(&assignment.left)
            .is_some_and(|(assigned_object, _)| assigned_object == *object)
            && expression_is_binding(assignment.right.as_ref(), value)
    })
}

fn member_assignment_exists(
    assignments: &[AssignExpr],
    object: &BindingKey,
    property: &str,
) -> bool {
    assignments.iter().any(|assignment| {
        member_assignment_target(&assignment.left).is_some_and(
            |(assigned_object, assigned_property)| {
                assigned_object == *object && assigned_property.as_ref() == property
            },
        )
    })
}

fn member_assignment_value(
    assignments: &[AssignExpr],
    object: &BindingKey,
    property: &str,
) -> Option<Box<Expr>> {
    let values = assignments
        .iter()
        .filter(|assignment| {
            member_assignment_target(&assignment.left).is_some_and(
                |(assigned_object, assigned_property)| {
                    assigned_object == *object && assigned_property.as_ref() == property
                },
            )
        })
        .map(|assignment| assignment.right.clone())
        .collect::<Vec<_>>();
    let [value] = values.as_slice() else {
        return None;
    };
    Some(value.clone())
}

fn member_assignment_binds_receiver(assignments: &[AssignExpr], object: &BindingKey) -> bool {
    assignments.iter().any(|assignment| {
        if !member_assignment_object(&assignment.left)
            .is_some_and(|assigned_object| assigned_object == *object)
        {
            return false;
        }
        let Expr::Call(call) = strip_parentheses(assignment.right.as_ref()) else {
            return false;
        };
        let Callee::Expr(callee) = &call.callee else {
            return false;
        };
        let Expr::Member(member) = strip_parentheses(callee.as_ref()) else {
            return false;
        };
        member_prop_name(&member.prop).as_deref() == Some("bind")
            && matches!(
                call.args.as_slice(),
                [argument]
                    if argument.spread.is_none()
                        && expression_is_binding(argument.expr.as_ref(), object)
            )
    })
}

fn computed_state_is_attached(
    assignments: &[AssignExpr],
    callable: &BindingKey,
    state: &BindingKey,
) -> bool {
    assignments.iter().any(|assignment| {
        member_assignment_object(&assignment.left).is_some_and(|object| object == *callable)
            && expression_is_binding(assignment.right.as_ref(), state)
    })
}

#[derive(Clone)]
struct LocalFunctionValue {
    params: Vec<Pat>,
    body: BlockStmt,
}

fn local_function_value(body: &BlockStmt, binding: &BindingKey) -> Option<LocalFunctionValue> {
    struct Finder<'a> {
        binding: &'a BindingKey,
        values: Vec<LocalFunctionValue>,
    }

    impl Finder<'_> {
        fn record_function(&mut self, function: &Function) {
            let Some(body) = &function.body else {
                return;
            };
            self.values.push(LocalFunctionValue {
                params: function
                    .params
                    .iter()
                    .map(|parameter| parameter.pat.clone())
                    .collect(),
                body: body.clone(),
            });
        }

        fn record_arrow(&mut self, arrow: &ArrowExpr) {
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
            self.values.push(LocalFunctionValue {
                params: arrow.params.clone(),
                body,
            });
        }
    }

    impl Visit for Finder<'_> {
        fn visit_fn_decl(&mut self, declaration: &FnDecl) {
            if binding_key(&declaration.ident) == *self.binding {
                self.record_function(declaration.function.as_ref());
            }
        }

        fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
            let Pat::Ident(identifier) = &declarator.name else {
                return;
            };
            if binding_key(&identifier.id) != *self.binding {
                return;
            }
            match declarator.init.as_deref().map(strip_parentheses) {
                Some(Expr::Fn(function)) => self.record_function(function.function.as_ref()),
                Some(Expr::Arrow(arrow)) => self.record_arrow(arrow),
                _ => {}
            }
        }

        fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
            let AssignTarget::Simple(SimpleAssignTarget::Ident(target)) = &assignment.left else {
                return;
            };
            if binding_key(&target.id) != *self.binding {
                return;
            }
            match strip_parentheses(assignment.right.as_ref()) {
                Expr::Fn(function) => self.record_function(function.function.as_ref()),
                Expr::Arrow(arrow) => self.record_arrow(arrow),
                _ => {}
            }
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut finder = Finder {
        binding,
        values: Vec::new(),
    };
    body.visit_with(&mut finder);
    let [value] = finder.values.as_slice() else {
        return None;
    };
    Some(value.clone())
}

fn local_function_returns_member(
    function: &LocalFunctionValue,
    object: &BindingKey,
    property: &str,
) -> bool {
    let mut returns = ReturnExpressionCollector::default();
    function.body.visit_with(&mut returns);
    returns
        .expressions
        .iter()
        .any(|expression| expression_has_member_on_binding(expression.as_ref(), object, property))
}

fn local_function_contains_member(
    function: &LocalFunctionValue,
    object: &BindingKey,
    property: &str,
) -> bool {
    expression_or_block_has_member(&function.body, object, property)
}

fn expression_or_block_has_member(body: &BlockStmt, object: &BindingKey, property: &str) -> bool {
    struct Finder<'a> {
        object: &'a BindingKey,
        property: &'a str,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
            if member_prop_name(&member.prop).as_deref() == Some(self.property)
                && matches!(
                    strip_parentheses(member.obj.as_ref()),
                    Expr::Ident(identifier) if binding_key(identifier) == *self.object
                )
            {
                self.found = true;
                return;
            }
            member.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut finder = Finder {
        object,
        property,
        found: false,
    };
    body.visit_with(&mut finder);
    finder.found
}

fn local_function_contains_number(function: &LocalFunctionValue, expected: f64) -> bool {
    block_contains_number(&function.body, expected)
}

fn block_contains_number(body: &BlockStmt, expected: f64) -> bool {
    struct Finder {
        expected: f64,
        found: bool,
    }

    impl Visit for Finder {
        fn visit_expr(&mut self, expression: &Expr) {
            if numeric_expression_value(expression) == Some(self.expected) {
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
    body.visit_with(&mut finder);
    finder.found
}

fn contains_object_property(body: &BlockStmt, expected: &str) -> bool {
    struct Finder<'a> {
        expected: &'a str,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_prop(&mut self, property: &Prop) {
            let name = match property {
                Prop::Shorthand(identifier) => Some(identifier.sym.to_string()),
                Prop::KeyValue(property) => prop_name(&property.key),
                Prop::Assign(property) => Some(property.key.sym.to_string()),
                Prop::Getter(property) => prop_name(&property.key),
                Prop::Setter(property) => prop_name(&property.key),
                Prop::Method(property) => prop_name(&property.key),
            };
            if name.as_deref() == Some(self.expected) {
                self.found = true;
                return;
            }
            property.visit_children_with(self);
        }

        fn visit_function(&mut self, function: &Function) {
            function.visit_children_with(self);
        }

        fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
            arrow.visit_children_with(self);
        }
    }

    let mut finder = Finder {
        expected,
        found: false,
    };
    body.visit_with(&mut finder);
    finder.found
}

fn block_contains_string(body: &BlockStmt, expected: &str) -> bool {
    struct Finder<'a> {
        expected: &'a str,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_str(&mut self, string: &swc_core::ecma::ast::Str) {
            if string.value.as_bytes() == self.expected.as_bytes() {
                self.found = true;
            }
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut finder = Finder {
        expected,
        found: false,
    };
    body.visit_with(&mut finder);
    finder.found
}

fn numeric_expression_value(expression: &Expr) -> Option<f64> {
    match strip_parentheses(expression) {
        Expr::Lit(Lit::Num(number)) => Some(number.value),
        Expr::Unary(unary) if unary.op == UnaryOp::Minus => {
            numeric_expression_value(unary.arg.as_ref()).map(|value| -value)
        }
        _ => None,
    }
}

struct InjectFlagsEvidence<'a> {
    options: &'a BindingKey,
    properties: HashSet<String>,
    typeof_options: bool,
    type_strings: HashSet<String>,
    bitwise_or: bool,
    closure_undefined_check: bool,
}

impl Visit for InjectFlagsEvidence<'_> {
    fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
        if matches!(
            strip_parentheses(member.obj.as_ref()),
            Expr::Ident(identifier) if binding_key(identifier) == *self.options
        ) {
            if let Some(property) = member_prop_name(&member.prop) {
                self.properties.insert(property.to_string());
            }
        }
        member.visit_children_with(self);
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        if unary.op == UnaryOp::TypeOf && expression_is_binding(unary.arg.as_ref(), self.options) {
            self.typeof_options = true;
        }
        unary.visit_children_with(self);
    }

    fn visit_str(&mut self, string: &swc_core::ecma::ast::Str) {
        self.type_strings
            .insert(String::from_utf8_lossy(string.value.as_bytes()).to_string());
    }

    fn visit_bin_expr(&mut self, binary: &swc_core::ecma::ast::BinExpr) {
        if binary.op == BinaryOp::BitOr {
            self.bitwise_or = true;
        }
        if binary.op == BinaryOp::Gt
            && matches!(
                strip_parentheses(binary.left.as_ref()),
                Expr::Unary(unary)
                    if unary.op == UnaryOp::TypeOf
                        && expression_is_binding(unary.arg.as_ref(), self.options)
            )
            && matches!(
                strip_parentheses(binary.right.as_ref()),
                Expr::Lit(Lit::Str(string))
                    if string.value.as_bytes() == b"u"
            )
        {
            self.closure_undefined_check = true;
        }
        binary.visit_children_with(self);
    }

    fn visit_function(&mut self, _function: &Function) {}

    fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
}

fn collect_runtime_functions(modules: &[PreparedAngularModule]) -> StructuralRoleEvidence {
    let mut functions = Vec::new();
    let mut classes = Vec::new();
    let mut definition_counts = HashMap::<SymbolIdentity, usize>::new();
    let mut invalid_values = HashSet::new();
    let mut assignment_definitions = HashMap::<SymbolIdentity, Vec<(usize, u32)>>::new();
    let mut value_aliases = Vec::new();
    let mut integer_candidates = HashMap::new();
    for (module_index, prepared) in modules.iter().enumerate() {
        let mut collector = RuntimeFunctionCollector {
            module_index,
            unresolved_ctxt: prepared.unresolved_ctxt,
            functions: Vec::new(),
            classes: Vec::new(),
            definition_counts: HashMap::new(),
            invalid_values: HashSet::new(),
            assignment_definitions: HashMap::new(),
            value_aliases: Vec::new(),
            integer_candidates: HashMap::new(),
        };
        prepared.module.visit_with(&mut collector);
        functions.extend(collector.functions);
        classes.extend(collector.classes);
        for (identity, count) in collector.definition_counts {
            *definition_counts.entry(identity).or_default() += count;
        }
        invalid_values.extend(collector.invalid_values);
        value_aliases.extend(collector.value_aliases);
        integer_candidates.extend(collector.integer_candidates);
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
    classes.retain(|class| stable_values.contains(&class.identity));
    value_aliases.retain(|(left, _)| {
        definition_counts.get(left) == Some(&1)
            && !invalid_values.contains(left)
            && is_supported_value_alias_identity(left)
    });
    let integer_constants = integer_candidates
        .into_iter()
        .filter(|(binding, _)| {
            stable_values.contains(&SymbolIdentity::LocalBinding(binding.clone()))
        })
        .collect();

    StructuralRoleEvidence {
        functions,
        classes,
        definition_counts,
        invalid_values,
        assignment_definitions,
        value_aliases,
        integer_constants,
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

fn is_supported_value_alias_identity(identity: &SymbolIdentity) -> bool {
    match identity {
        SymbolIdentity::LocalBinding(_) | SymbolIdentity::GlobalBinding(_) => true,
        SymbolIdentity::GlobalMember { object, .. } => {
            object.as_ref() == "globalThis" || object.starts_with("globalThis.")
        }
        SymbolIdentity::LocalMember { .. } => false,
    }
}

fn value_alias_identity(value: &Expr, unresolved_ctxt: SyntaxContext) -> Option<SymbolIdentity> {
    let value = match strip_parentheses(value) {
        Expr::Seq(sequence) => strip_parentheses(sequence.exprs.last()?.as_ref()),
        value => value,
    };
    let identity = symbol_identity(value, unresolved_ctxt)?;
    is_supported_value_alias_identity(&identity).then_some(identity)
}

#[derive(Clone)]
struct RuntimeFunction {
    identity: SymbolIdentity,
    params: Vec<Pat>,
    body: BlockStmt,
    unresolved_ctxt: SyntaxContext,
}

#[derive(Clone)]
struct RuntimeClass {
    identity: SymbolIdentity,
    class: Box<Class>,
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
    classes: Vec<RuntimeClass>,
    definition_counts: HashMap<SymbolIdentity, usize>,
    invalid_values: HashSet<SymbolIdentity>,
    assignment_definitions: HashMap<SymbolIdentity, Vec<(usize, u32)>>,
    value_aliases: Vec<(SymbolIdentity, SymbolIdentity)>,
    integer_candidates: HashMap<BindingKey, u64>,
}

struct TemplateCallObservation {
    identity: SymbolIdentity,
    phase: u8,
    arguments: Vec<Box<Expr>>,
    usage: TemplateCallUsage,
    view_id: usize,
    call_order: usize,
    unresolved_ctxt: SyntaxContext,
}

struct CreationNullAssignmentObservation {
    target: SymbolIdentity,
    view_id: usize,
    operation_order: usize,
}

struct CreationFalseAssignmentObservation {
    target: SymbolIdentity,
    view_id: usize,
    operation_order: usize,
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
    creation_null_assignments: Vec<CreationNullAssignmentObservation>,
    creation_false_assignments: Vec<CreationFalseAssignmentObservation>,
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
            creation_null_assignments: Vec::new(),
            creation_false_assignments: Vec::new(),
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
            self.creation_null_assignments
                .extend(observer.creation_null_assignments);
            self.creation_false_assignments
                .extend(observer.creation_false_assignments);
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
    creation_null_assignments: Vec<CreationNullAssignmentObservation>,
    creation_false_assignments: Vec<CreationFalseAssignmentObservation>,
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
                if phase == Some(1) {
                    if let Some((target, NamespaceAssignmentValue::Html)) =
                        namespace_assignment_expression(assignment, self.unresolved_ctxt)
                    {
                        let operation_order = self.next_call_order;
                        self.next_call_order += 1;
                        self.creation_null_assignments
                            .push(CreationNullAssignmentObservation {
                                target,
                                view_id: self.view_id,
                                operation_order,
                            });
                    }
                    if let Some(target) =
                        boolean_member_assignment_target(assignment, false, self.unresolved_ctxt)
                    {
                        let operation_order = self.next_call_order;
                        self.next_call_order += 1;
                        self.creation_false_assignments
                            .push(CreationFalseAssignmentObservation {
                                target,
                                view_id: self.view_id,
                                operation_order,
                            });
                    }
                }
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
            .is_some_and(|instruction| phase == 1 && is_creation_anchor(instruction))
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
                    unresolved_ctxt: self.unresolved_ctxt,
                }
            }));
    }
}

fn is_creation_anchor(instruction: IvyInstruction) -> bool {
    matches!(
        instruction,
        IvyInstruction::ElementStart
            | IvyInstruction::ElementEnd
            | IvyInstruction::Element
            | IvyInstruction::ElementContainerStart
            | IvyInstruction::ElementContainerEnd
            | IvyInstruction::ElementContainer
            | IvyInstruction::Text
            | IvyInstruction::Template
            | IvyInstruction::Defer
            | IvyInstruction::ProjectionDef
            | IvyInstruction::Projection
            | IvyInstruction::Pipe
            | IvyInstruction::I18n
            | IvyInstruction::I18nStart
            | IvyInstruction::I18nEnd
    )
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

fn infer_specialized_element_container_pair(
    function_index: &RuntimeFunctionIndex<'_>,
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
) -> Vec<(SymbolIdentity, &'static str)> {
    let starts = observations
        .iter()
        .filter_map(|(identity, calls)| {
            let definition = function_index.unique(identity)?;
            is_specialized_element_container_start_shape(definition, calls)
                .then_some((identity, calls))
        })
        .collect::<Vec<_>>();
    let ends = observations
        .iter()
        .filter_map(|(identity, calls)| {
            if function_index.roles.ivy_names.contains_key(identity) {
                return None;
            }
            let definition = function_index.unique(identity)?;
            (definition.params.is_empty()
                && !is_specialized_element_end_shape(definition, calls)
                && direct_calls(definition).len() <= 3
                && calls.iter().all(|observation| {
                    observation.usage == TemplateCallUsage::Effect
                        && observation.phase == 1
                        && observation.arguments.is_empty()
                }))
            .then_some((identity, calls))
        })
        .collect::<Vec<_>>();

    let mut inferred = Vec::new();
    for (start, start_calls) in starts {
        let mut paired_end = None;
        let mut valid = true;
        for start_call in start_calls {
            let nearest = ends
                .iter()
                .filter_map(|(identity, end_calls)| {
                    end_calls
                        .iter()
                        .filter(|end_call| {
                            end_call.view_id == start_call.view_id
                                && end_call.call_order > start_call.call_order
                        })
                        .min_by_key(|end_call| end_call.call_order)
                        .map(|end_call| (*identity, end_call.call_order))
                })
                .min_by_key(|(_, call_order)| *call_order)
                .map(|(identity, _)| identity);
            let Some(nearest) = nearest else {
                valid = false;
                break;
            };
            if paired_end.is_some_and(|existing| existing != nearest) {
                valid = false;
                break;
            }
            paired_end = Some(nearest);
        }
        if valid {
            if let Some(end) = paired_end {
                inferred.push((start.clone(), "ɵɵelementContainerStart"));
                inferred.push((end.clone(), "ɵɵelementContainerEnd"));
            }
        }
    }
    inferred
}

fn is_specialized_element_container_start_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    plain_parameter_bindings(definition).is_some_and(|parameters| parameters.len() == 3)
        && returns_identity(definition, &definition.identity)
        && contains_string_literal(&definition.body, "ng-container")
        && observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && matches!(observation.arguments.len(), 1..=3)
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
        })
}

#[derive(Clone, Copy)]
enum NamespaceAssignmentValue {
    Html,
    Svg,
    MathMl,
}

#[derive(Default)]
struct NamespaceCandidates {
    html: Vec<SymbolIdentity>,
    svg: Vec<SymbolIdentity>,
    math_ml: Vec<SymbolIdentity>,
}

fn infer_namespace_family(
    function_index: &RuntimeFunctionIndex<'_>,
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
    creation_null_assignments: &[CreationNullAssignmentObservation],
) -> Vec<(SymbolIdentity, &'static str)> {
    let mut candidates = HashMap::<SymbolIdentity, NamespaceCandidates>::new();
    for (identity, calls) in observations {
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        if !definition.params.is_empty()
            || !direct_calls(definition).is_empty()
            || !calls.iter().all(|observation| {
                observation.usage == TemplateCallUsage::Effect
                    && observation.phase == 1
                    && observation.arguments.is_empty()
            })
        {
            continue;
        }
        let Some((target, value)) = namespace_assignment(definition) else {
            continue;
        };
        let family = candidates.entry(target).or_default();
        match value {
            NamespaceAssignmentValue::Html => family.html.push(identity.clone()),
            NamespaceAssignmentValue::Svg => family.svg.push(identity.clone()),
            NamespaceAssignmentValue::MathMl => family.math_ml.push(identity.clone()),
        }
    }

    let mut inferred = Vec::new();
    for (target, family) in candidates {
        if family.html.len() > 1 || family.svg.len() > 1 || family.math_ml.len() > 1 {
            continue;
        }
        let variants = usize::from(!family.html.is_empty())
            + usize::from(!family.svg.is_empty())
            + usize::from(!family.math_ml.is_empty());
        let specialized_svg = variants == 1
            && family.svg.as_slice().first().is_some_and(|svg| {
                observations.get(svg).is_some_and(|calls| {
                    calls.iter().any(|call| {
                        creation_null_assignments.iter().any(|assignment| {
                            assignment.target == target
                                && assignment.view_id == call.view_id
                                && assignment.operation_order > call.call_order
                        })
                    })
                })
            });
        if variants < 2 && !specialized_svg {
            continue;
        }
        if let [html] = family.html.as_slice() {
            inferred.push((html.clone(), "ɵɵnamespaceHTML"));
        }
        if let [svg] = family.svg.as_slice() {
            inferred.push((svg.clone(), "ɵɵnamespaceSVG"));
        }
        if let [math_ml] = family.math_ml.as_slice() {
            inferred.push((math_ml.clone(), "ɵɵnamespaceMathML"));
        }
    }
    inferred
}

fn namespace_assignment(
    function: &RuntimeFunction,
) -> Option<(SymbolIdentity, NamespaceAssignmentValue)> {
    struct Collector {
        unresolved_ctxt: SyntaxContext,
        assignments: Vec<(SymbolIdentity, NamespaceAssignmentValue)>,
    }

    impl Visit for Collector {
        fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
            if let Some(assignment) =
                namespace_assignment_expression(assignment, self.unresolved_ctxt)
            {
                self.assignments.push(assignment);
            }
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut collector = Collector {
        unresolved_ctxt: function.unresolved_ctxt,
        assignments: Vec::new(),
    };
    function.body.visit_with(&mut collector);
    let [assignment] = collector.assignments.as_slice() else {
        return None;
    };
    Some(assignment.clone())
}

fn namespace_assignment_expression(
    assignment: &AssignExpr,
    unresolved_ctxt: SyntaxContext,
) -> Option<(SymbolIdentity, NamespaceAssignmentValue)> {
    if assignment.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assignment.left else {
        return None;
    };
    let value = match strip_parentheses(assignment.right.as_ref()) {
        Expr::Lit(Lit::Null(_)) => NamespaceAssignmentValue::Html,
        Expr::Lit(Lit::Str(string)) if string.value == "svg" => NamespaceAssignmentValue::Svg,
        Expr::Lit(Lit::Str(string)) if matches!(string.value.as_str(), Some("math" | "mathml")) => {
            NamespaceAssignmentValue::MathMl
        }
        _ => return None,
    };
    let target = symbol_identity(&Expr::Member(member.clone()), unresolved_ctxt)?;
    Some((target, value))
}

fn infer_styling_property_family(
    function_index: &RuntimeFunctionIndex<'_>,
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
) -> Vec<(SymbolIdentity, &'static str)> {
    let mut styles_by_helper = HashMap::<SymbolIdentity, Vec<&SymbolIdentity>>::new();
    let mut classes_by_helper = HashMap::<SymbolIdentity, Vec<&SymbolIdentity>>::new();

    for (identity, calls_in_templates) in observations {
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        let Some(parameters) = plain_parameter_bindings(definition) else {
            continue;
        };
        if !returns_identity(definition, &definition.identity) {
            continue;
        }
        let direct = direct_calls(definition);
        let [call] = direct.as_slice() else {
            continue;
        };
        if call.arguments.len() != 4 {
            continue;
        }
        if parameters.len() == 3
            && calls_in_templates.iter().all(|observation| {
                observation.usage == TemplateCallUsage::Effect
                    && observation.phase == 2
                    && matches!(observation.arguments.len(), 2 | 3)
                    && observation
                        .arguments
                        .first()
                        .is_some_and(|argument| is_string_literal(argument.as_ref()))
            })
            && forwards_parameters_in_order(call, &parameters)
            && is_boolean_value(call.arguments[3].as_ref(), false)
        {
            styles_by_helper
                .entry(call.callee.clone())
                .or_default()
                .push(identity);
        }
        if parameters.len() == 2
            && calls_in_templates.iter().all(|observation| {
                observation.usage == TemplateCallUsage::Effect
                    && observation.phase == 2
                    && observation.arguments.len() == 2
                    && observation
                        .arguments
                        .first()
                        .is_some_and(|argument| is_string_literal(argument.as_ref()))
            })
            && forwards_parameters_in_order(call, &parameters)
            && is_nullish(call.arguments[2].as_ref(), definition.unresolved_ctxt)
            && is_boolean_value(call.arguments[3].as_ref(), true)
        {
            classes_by_helper
                .entry(call.callee.clone())
                .or_default()
                .push(identity);
        }
    }

    let mut inferred = Vec::new();
    for (helper, styles) in styles_by_helper {
        let Some(classes) = classes_by_helper.get(&helper) else {
            continue;
        };
        let ([style], [class]) = (styles.as_slice(), classes.as_slice()) else {
            continue;
        };
        inferred.push(((*style).clone(), "ɵɵstyleProp"));
        inferred.push(((*class).clone(), "ɵɵclassProp"));
    }
    inferred
}

fn infer_styling_map_family(
    functions: &[RuntimeFunction],
    function_index: &RuntimeFunctionIndex<'_>,
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
) -> Vec<(SymbolIdentity, &'static str)> {
    let mut styles_by_helper = HashMap::<SymbolIdentity, Vec<(&SymbolIdentity, bool)>>::new();
    let mut classes_by_helper = HashMap::<SymbolIdentity, Vec<(&SymbolIdentity, bool)>>::new();
    let mut seen = HashSet::new();

    for function in functions {
        let identity = &function.identity;
        if !seen.insert(identity) {
            continue;
        }
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        let Some(parameters) = plain_parameter_bindings(definition) else {
            continue;
        };
        let [value] = parameters.as_slice() else {
            continue;
        };
        let observed = observations
            .get(identity)
            .is_some_and(|calls_in_templates| {
                !calls_in_templates.is_empty()
                    && calls_in_templates.iter().all(|observation| {
                        observation.usage == TemplateCallUsage::Effect
                            && observation.phase == 2
                            && observation.arguments.len() == 1
                    })
            });
        let direct = direct_calls(definition);
        let [call] = direct.as_slice() else {
            continue;
        };
        if call.callee == definition.identity
            || call.arguments.len() != 4
            || !expression_is_binding(call.arguments[2].as_ref(), value)
            || symbol_identity(call.arguments[0].as_ref(), definition.unresolved_ctxt).is_none()
            || symbol_identity(call.arguments[1].as_ref(), definition.unresolved_ctxt).is_none()
        {
            continue;
        }
        if is_boolean_value(call.arguments[3].as_ref(), false) {
            styles_by_helper
                .entry(call.callee.clone())
                .or_default()
                .push((identity, observed));
        }
        if is_boolean_value(call.arguments[3].as_ref(), true) {
            classes_by_helper
                .entry(call.callee.clone())
                .or_default()
                .push((identity, observed));
        }
    }

    let mut inferred = Vec::new();
    for (helper, styles) in styles_by_helper {
        let Some(classes) = classes_by_helper.get(&helper) else {
            continue;
        };
        let ([(style, style_observed)], [(class, class_observed)]) =
            (styles.as_slice(), classes.as_slice())
        else {
            continue;
        };
        if *style_observed {
            inferred.push(((*style).clone(), "ɵɵstyleMap"));
        }
        if *class_observed {
            inferred.push(((*class).clone(), "ɵɵclassMap"));
        }
    }
    inferred
}

fn infer_i18n_role_family(
    function_index: &RuntimeFunctionIndex<'_>,
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
    creation_false_assignments: &[CreationFalseAssignmentObservation],
) -> Vec<(SymbolIdentity, &'static str)> {
    let mut inferred = Vec::new();
    for (identity, calls_in_templates) in observations {
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        let Some(parameters) = plain_parameter_bindings(definition) else {
            continue;
        };
        if parameters.len() != 3
            || !calls_in_templates.iter().all(|observation| {
                observation.usage == TemplateCallUsage::Effect
                    && observation.phase == 1
                    && matches!(observation.arguments.len(), 2 | 3)
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
            continue;
        }
        let direct = direct_calls(definition);
        let [start, end] = direct.as_slice() else {
            continue;
        };
        if forwards_parameters(start, &parameters)
            && end.arguments.is_empty()
            && start.callee != end.callee
            && start.callee != definition.identity
            && end.callee != definition.identity
        {
            inferred.push((identity.clone(), "ɵɵi18n"));
            inferred.push((start.callee.clone(), "ɵɵi18nStart"));
            inferred.push((end.callee.clone(), "ɵɵi18nEnd"));
        }
    }

    let mut calls_by_definition = HashMap::<SymbolIdentity, Vec<&TemplateCallObservation>>::new();
    for (identity, calls) in observations {
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        calls_by_definition
            .entry(definition.identity.clone())
            .or_default()
            .extend(calls);
    }

    let mut starts_by_target = HashMap::<SymbolIdentity, HashSet<SymbolIdentity>>::new();
    for (identity, calls) in &calls_by_definition {
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        let Some(target) = optimized_i18n_start_target(definition) else {
            continue;
        };
        if !optimized_i18n_creation_calls(calls) {
            continue;
        }
        starts_by_target
            .entry(target)
            .or_default()
            .insert(definition.identity.clone());
    }

    for (target, starts) in &starts_by_target {
        let Some(start) = unique_identity(starts) else {
            continue;
        };
        let Some(calls) = calls_by_definition.get(start) else {
            continue;
        };
        if calls.iter().any(|call| {
            creation_false_assignments.iter().any(|assignment| {
                assignment.target == *target
                    && assignment.view_id == call.view_id
                    && assignment.operation_order > call.call_order
            })
        }) {
            inferred.push((start.clone(), "ɵɵi18nStart"));
        }
    }

    for (identity, calls) in &calls_by_definition {
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        let Some(parameters) = plain_parameter_bindings(definition) else {
            continue;
        };
        if !matches!(parameters.len(), 2 | 3) || !optimized_i18n_creation_calls(calls) {
            continue;
        }
        let Some(end_assignment) = unique_boolean_member_assignment(definition, false) else {
            continue;
        };
        let direct = direct_calls(definition);
        let [start_call] = direct.as_slice() else {
            continue;
        };
        if start_call.callee == definition.identity
            || !forwards_parameters(start_call, &parameters)
            || start_call.span.lo >= end_assignment.span.lo
        {
            continue;
        }
        let Some(start_definition) = function_index.unique(&start_call.callee) else {
            continue;
        };
        if optimized_i18n_start_target(start_definition).as_ref() != Some(&end_assignment.target) {
            continue;
        }
        inferred.push((definition.identity.clone(), "ɵɵi18n"));
        inferred.push((start_definition.identity.clone(), "ɵɵi18nStart"));
    }

    let apply_candidates = observations
        .iter()
        .filter_map(|(identity, calls)| {
            let definition = function_index.unique(identity)?;
            (plain_parameter_bindings(definition).is_some_and(|parameters| parameters.len() == 1)
                && contains_try_finally(&definition.body)
                && !returns_identity(definition, &definition.identity)
                && calls.iter().all(|observation| {
                    observation.usage == TemplateCallUsage::Effect
                        && observation.phase == 2
                        && observation.arguments.len() == 1
                        && is_nonnegative_integer(observation.arguments[0].as_ref())
                }))
            .then_some((identity, calls))
        })
        .collect::<Vec<_>>();
    let exp_candidates = observations
        .iter()
        .filter_map(|(identity, calls)| {
            let definition = function_index.unique(identity)?;
            (plain_parameter_bindings(definition).is_some_and(|parameters| parameters.len() == 1)
                && returns_identity(definition, &definition.identity)
                && calls.iter().all(|observation| {
                    observation.usage == TemplateCallUsage::Effect
                        && observation.phase == 2
                        && observation.arguments.len() == 1
                }))
            .then_some((identity, calls))
        })
        .collect::<Vec<_>>();

    for (apply, apply_calls) in apply_candidates {
        let matching_expressions = exp_candidates
            .iter()
            .filter(|(_, expression_calls)| {
                apply_calls.iter().all(|apply_call| {
                    expression_calls.iter().any(|expression_call| {
                        expression_call.view_id == apply_call.view_id
                            && expression_call.call_order < apply_call.call_order
                    })
                })
            })
            .collect::<Vec<_>>();
        let [matching_expression] = matching_expressions.as_slice() else {
            continue;
        };
        inferred.push(((*matching_expression.0).clone(), "ɵɵi18nExp"));
        inferred.push((apply.clone(), "ɵɵi18nApply"));
    }

    inferred
}

fn optimized_i18n_creation_calls(calls: &[&TemplateCallObservation]) -> bool {
    !calls.is_empty()
        && calls.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && matches!(observation.arguments.len(), 2 | 3)
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
                && observation
                    .arguments
                    .get(1)
                    .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
        })
}

fn optimized_i18n_start_target(definition: &RuntimeFunction) -> Option<SymbolIdentity> {
    let parameters = plain_parameter_bindings(definition)?;
    if !matches!(parameters.len(), 2 | 3)
        || direct_calls(definition).len() < 5
        || !block_contains_loop(&definition.body)
        || !block_contains_binding(&definition.body, &parameters[1])
        || !reassigns_binding_with_literal_offset(&definition.body, &parameters[0], 27)
    {
        return None;
    }
    unique_boolean_member_assignment(definition, true).map(|assignment| assignment.target)
}

fn reassigns_binding_with_literal_offset(
    body: &BlockStmt,
    binding: &BindingKey,
    offset: isize,
) -> bool {
    struct Finder<'a> {
        binding: &'a BindingKey,
        offset: isize,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
            if assignment.op == AssignOp::Assign
                && matches!(
                    &assignment.left,
                    AssignTarget::Simple(SimpleAssignTarget::Ident(target))
                        if binding_key(&target.id) == *self.binding
                )
                && expression_is_binding_plus_literal(
                    assignment.right.as_ref(),
                    self.binding,
                    self.offset,
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
        binding,
        offset,
        found: false,
    };
    body.visit_with(&mut finder);
    finder.found
}

fn expression_is_binding_plus_literal(
    expression: &Expr,
    binding: &BindingKey,
    offset: isize,
) -> bool {
    let Expr::Bin(binary) = strip_parentheses(expression) else {
        return false;
    };
    if binary.op != BinaryOp::Add {
        return false;
    }
    let operands_match = |binding_expression: &Expr, offset_expression: &Expr| {
        expression_is_binding(binding_expression, binding)
            && numeric_expression_value(offset_expression) == Some(offset as f64)
    };
    operands_match(binary.left.as_ref(), binary.right.as_ref())
        || operands_match(binary.right.as_ref(), binary.left.as_ref())
}

struct BooleanMemberAssignment {
    target: SymbolIdentity,
    span: Span,
}

fn unique_boolean_member_assignment(
    definition: &RuntimeFunction,
    expected: bool,
) -> Option<BooleanMemberAssignment> {
    struct Collector {
        unresolved_ctxt: SyntaxContext,
        expected: bool,
        assignments: Vec<BooleanMemberAssignment>,
    }

    impl Visit for Collector {
        fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
            if let Some(target) =
                boolean_member_assignment_target(assignment, self.expected, self.unresolved_ctxt)
            {
                self.assignments.push(BooleanMemberAssignment {
                    target,
                    span: assignment.span,
                });
            }
            assignment.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut collector = Collector {
        unresolved_ctxt: definition.unresolved_ctxt,
        expected,
        assignments: Vec::new(),
    };
    definition.body.visit_with(&mut collector);
    let [assignment] = collector.assignments.as_slice() else {
        return None;
    };
    Some(BooleanMemberAssignment {
        target: assignment.target.clone(),
        span: assignment.span,
    })
}

pub(super) fn boolean_member_assignment_target(
    assignment: &AssignExpr,
    expected: bool,
    unresolved_ctxt: SyntaxContext,
) -> Option<SymbolIdentity> {
    if assignment.op != AssignOp::Assign || !is_boolean_value(assignment.right.as_ref(), expected) {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assignment.left else {
        return None;
    };
    symbol_identity(&Expr::Member(member.clone()), unresolved_ctxt)
}

fn infer_two_way_role_family(
    function_index: &RuntimeFunctionIndex<'_>,
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
) -> Vec<(SymbolIdentity, &'static str)> {
    let listener_candidates = observations
        .iter()
        .filter(|(identity, calls)| {
            function_index
                .unique(identity)
                .is_some_and(|definition| is_two_way_listener_shape(definition, calls))
        })
        .collect::<Vec<_>>();
    let property_candidates = observations
        .iter()
        .filter(|(identity, calls)| {
            function_index
                .unique(identity)
                .is_some_and(|definition| is_two_way_property_shape(definition, calls))
        })
        .collect::<Vec<_>>();

    let pairs = listener_candidates
        .iter()
        .flat_map(|(listener, listener_calls)| {
            property_candidates
                .iter()
                .filter(move |(_, property_calls)| {
                    two_way_observations_pair(listener_calls, property_calls)
                })
                .map(move |(property, _)| ((*listener).clone(), (*property).clone()))
        })
        .collect::<Vec<_>>();
    let mut listener_pair_counts = HashMap::<SymbolIdentity, usize>::new();
    let mut property_pair_counts = HashMap::<SymbolIdentity, usize>::new();
    for (listener, property) in &pairs {
        *listener_pair_counts.entry(listener.clone()).or_default() += 1;
        *property_pair_counts.entry(property.clone()).or_default() += 1;
    }

    let mut inferred = Vec::new();
    let mut inferred_binding_sets = HashSet::new();
    for (listener, property) in pairs {
        if listener_pair_counts.get(&listener) != Some(&1)
            || property_pair_counts.get(&property) != Some(&1)
        {
            continue;
        }
        inferred.push((listener.clone(), "ɵɵtwoWayListener"));
        inferred.push((property, "ɵɵtwoWayProperty"));
        let Some(listener_calls) = observations.get(&listener) else {
            continue;
        };
        if let Some(binding_set) = infer_two_way_binding_set(function_index, listener_calls)
            .filter(|identity| inferred_binding_sets.insert(identity.clone()))
        {
            inferred.push((binding_set, "ɵɵtwoWayBindingSet"));
        }
    }
    inferred
}

fn is_two_way_listener_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    plain_parameter_bindings(definition).is_some_and(|parameters| parameters.len() == 2)
        && returns_identity(definition, &definition.identity)
        && observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && observation.arguments.len() == 2
                && string_literal_value(observation.arguments[0].as_ref()).is_some_and(|event| {
                    event
                        .strip_suffix("Change")
                        .is_some_and(|property| !property.is_empty())
                })
                && matches!(
                    strip_parentheses(observation.arguments[1].as_ref()),
                    Expr::Fn(_) | Expr::Arrow(_)
                )
        })
}

fn is_two_way_property_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    is_property_shape(definition, observations)
        && contains_member_property(&definition.body, "set")
        && contains_string_literal(&definition.body, "function")
}

fn two_way_observations_pair(
    listener_calls: &[TemplateCallObservation],
    property_calls: &[TemplateCallObservation],
) -> bool {
    listener_calls.iter().all(|listener| {
        property_calls.iter().any(|property| {
            listener.view_id == property.view_id && two_way_names_pair(listener, property)
        })
    }) && property_calls.iter().all(|property| {
        listener_calls.iter().any(|listener| {
            listener.view_id == property.view_id && two_way_names_pair(listener, property)
        })
    })
}

fn two_way_names_pair(
    listener: &TemplateCallObservation,
    property: &TemplateCallObservation,
) -> bool {
    let Some(event) = listener
        .arguments
        .first()
        .and_then(|argument| string_literal_value(argument.as_ref()))
    else {
        return false;
    };
    let Some(property) = property
        .arguments
        .first()
        .and_then(|argument| string_literal_value(argument.as_ref()))
    else {
        return false;
    };
    event.strip_suffix("Change") == Some(property)
}

fn infer_two_way_binding_set(
    function_index: &RuntimeFunctionIndex<'_>,
    listener_calls: &[TemplateCallObservation],
) -> Option<SymbolIdentity> {
    let mut common_candidates = None::<HashSet<SymbolIdentity>>;
    for observation in listener_calls {
        let handler = observation.arguments.get(1)?;
        let candidates =
            nested_runtime_call_identities(handler.as_ref(), observation.unresolved_ctxt)
                .into_iter()
                .filter(|identity| {
                    function_index
                        .unique(identity)
                        .is_some_and(is_two_way_binding_set_definition)
                })
                .collect::<HashSet<_>>();
        if candidates.is_empty() {
            return None;
        }
        common_candidates = Some(match common_candidates {
            None => candidates,
            Some(common) => common.intersection(&candidates).cloned().collect(),
        });
    }
    single_identity(common_candidates?.iter()).cloned()
}

fn is_two_way_binding_set_definition(definition: &RuntimeFunction) -> bool {
    let Some(parameters) = plain_parameter_bindings(definition) else {
        return false;
    };
    let [_, value] = parameters.as_slice() else {
        return false;
    };
    !returns_identity(definition, &definition.identity)
        && exact_returned_identity(definition).is_some()
        && contains_member_property(&definition.body, "set")
        && contains_string_literal(&definition.body, "function")
        && direct_calls(definition).iter().any(|call| {
            is_member_call_named(call, "set")
                && matches!(
                    call.arguments.first().map(Box::as_ref),
                    Some(Expr::Ident(identifier)) if binding_key(identifier) == *value
                )
        })
}

fn nested_runtime_call_identities(
    expression: &Expr,
    unresolved_ctxt: SyntaxContext,
) -> HashSet<SymbolIdentity> {
    struct Collector {
        unresolved_ctxt: SyntaxContext,
        identities: HashSet<SymbolIdentity>,
    }

    impl Visit for Collector {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if let Some(identity) = symbol_identity(callee.as_ref(), self.unresolved_ctxt) {
                    self.identities.insert(identity);
                }
            }
            call.visit_children_with(self);
        }
    }

    let mut collector = Collector {
        unresolved_ctxt,
        identities: HashSet::new(),
    };
    expression.visit_with(&mut collector);
    collector.identities
}

fn infer_animation_role_family(
    function_index: &RuntimeFunctionIndex<'_>,
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
) -> Vec<(SymbolIdentity, &'static str)> {
    let mut inferred = Vec::new();
    for (identity, calls) in observations {
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        let Some(parameters) = plain_parameter_bindings(definition) else {
            continue;
        };
        let [value] = parameters.as_slice() else {
            continue;
        };
        if !returns_identity(definition, &definition.identity)
            || !calls.iter().all(|observation| {
                observation.usage == TemplateCallUsage::Effect
                    && observation.phase == 1
                    && observation.arguments.len() == 1
            })
        {
            continue;
        }
        let family = match (
            contains_string_literal(&definition.body, "NgAnimateEnter"),
            contains_string_literal(&definition.body, "NgAnimateLeave"),
        ) {
            (true, false) => "enter",
            (false, true) => "leave",
            _ => continue,
        };
        let is_listener = parameter_called_via_call_member(definition, value)
            || parameter_forwarded_to_call_member(definition, value, function_index);
        let arguments_match = calls.iter().all(|observation| {
            let argument = observation.arguments[0].as_ref();
            if is_listener {
                matches!(strip_parentheses(argument), Expr::Fn(_) | Expr::Arrow(_))
            } else {
                is_animation_binding_argument(argument)
            }
        });
        if !arguments_match {
            continue;
        }
        let role = match (family, is_listener) {
            ("enter", false) => "ɵɵanimateEnter",
            ("enter", true) => "ɵɵanimateEnterListener",
            ("leave", false) => "ɵɵanimateLeave",
            ("leave", true) => "ɵɵanimateLeaveListener",
            _ => unreachable!("animation family is constrained above"),
        };
        inferred.push((identity.clone(), role));
    }
    inferred
}

fn is_animation_binding_argument(expression: &Expr) -> bool {
    match strip_parentheses(expression) {
        Expr::Lit(Lit::Str(_)) => true,
        Expr::Fn(function) => function.function.params.is_empty(),
        Expr::Arrow(arrow) => arrow.params.is_empty(),
        _ => false,
    }
}

fn parameter_called_via_call_member(definition: &RuntimeFunction, parameter: &BindingKey) -> bool {
    struct Finder<'a> {
        parameter: &'a BindingKey,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if self.found {
                return;
            }
            let Callee::Expr(callee) = &call.callee else {
                call.visit_children_with(self);
                return;
            };
            if let Expr::Member(member) = strip_parentheses(callee.as_ref()) {
                if member_prop_name(&member.prop).as_deref() == Some("call")
                    && matches!(
                        strip_parentheses(member.obj.as_ref()),
                        Expr::Ident(identifier) if binding_key(identifier) == *self.parameter
                    )
                {
                    self.found = true;
                    return;
                }
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = Finder {
        parameter,
        found: false,
    };
    definition.body.visit_with(&mut finder);
    finder.found
}

fn parameter_forwarded_to_call_member(
    definition: &RuntimeFunction,
    parameter: &BindingKey,
    function_index: &RuntimeFunctionIndex<'_>,
) -> bool {
    struct ForwardingCollector<'a> {
        parameter: &'a BindingKey,
        unresolved_ctxt: SyntaxContext,
        targets: Vec<(SymbolIdentity, usize)>,
    }

    impl Visit for ForwardingCollector<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if let Some(identity) = symbol_identity(callee.as_ref(), self.unresolved_ctxt) {
                    for (index, argument) in call.args.iter().enumerate() {
                        if matches!(
                            strip_parentheses(argument.expr.as_ref()),
                            Expr::Ident(identifier)
                                if binding_key(identifier) == *self.parameter
                        ) {
                            self.targets.push((identity.clone(), index));
                        }
                    }
                }
            }
            call.visit_children_with(self);
        }
    }

    let mut collector = ForwardingCollector {
        parameter,
        unresolved_ctxt: definition.unresolved_ctxt,
        targets: Vec::new(),
    };
    definition.body.visit_with(&mut collector);
    collector.targets.into_iter().any(|(identity, index)| {
        let Some(target) = function_index.unique(&identity) else {
            return false;
        };
        let Some(parameters) = plain_parameter_bindings(target) else {
            return false;
        };
        parameters
            .get(index)
            .is_some_and(|parameter| parameter_called_via_call_member(target, parameter))
    })
}

fn string_literal_value(expression: &Expr) -> Option<&str> {
    let Expr::Lit(Lit::Str(string)) = strip_parentheses(expression) else {
        return None;
    };
    string.value.as_str()
}

fn is_boolean_value(expression: &Expr, expected: bool) -> bool {
    match strip_parentheses(expression) {
        Expr::Lit(Lit::Bool(value)) => value.value == expected,
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => matches!(
            strip_parentheses(unary.arg.as_ref()),
            Expr::Lit(Lit::Num(number))
                if (number.value == 0.0) == expected
        ),
        _ => false,
    }
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
    integer_constants: &HashMap<BindingKey, u64>,
    roles: &IvyRoleTable,
) -> Vec<(SymbolIdentity, &'static str)> {
    let mut restores_by_state: HashMap<ValuePath, Vec<&RuntimeFunction>> = HashMap::new();
    let mut resets_by_state: HashMap<ValuePath, Vec<&RuntimeFunction>> = HashMap::new();
    for function in functions {
        let Some(parameters) = plain_parameter_bindings(function) else {
            continue;
        };
        let [parameter] = parameters.as_slice() else {
            continue;
        };
        if returns_parameter_index(function, parameter, 8, integer_constants) {
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
                    && returns_current_view_from_state_object(function, functions, &state, roles)
                    && uses_capture_restore_flow(
                        modules,
                        &function.identity,
                        &restore.identity,
                        roles,
                    )
            })
            .collect::<Vec<_>>();
        if let [getter] = getters.as_slice() {
            inferred.push((getter.identity.clone(), "ɵɵgetCurrentView"));
        }
    }
    inferred
}

fn returns_current_view_from_state_object(
    function: &RuntimeFunction,
    functions: &[RuntimeFunction],
    state: &ValuePath,
    roles: &IvyRoleTable,
) -> bool {
    if exact_returned_value_path(function)
        .is_some_and(|returned| same_value_path_object(&returned, state))
    {
        return true;
    }
    let Some(callee) = exact_zero_argument_returned_call_callee(function) else {
        return false;
    };
    let mut targets = functions
        .iter()
        .filter(|candidate| roles.identities_equivalent(&candidate.identity, &callee));
    let Some(target) = targets.next() else {
        return false;
    };
    targets.next().is_none()
        && target.params.is_empty()
        && exact_returned_value_path(target)
            .is_some_and(|returned| same_value_path_object(&returned, state))
}

fn exact_zero_argument_returned_call_callee(function: &RuntimeFunction) -> Option<SymbolIdentity> {
    let expression = single_top_level_return_expression(&function.body)?;
    let Expr::Call(call) = strip_parentheses(expression) else {
        return None;
    };
    if !call.args.is_empty() {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    symbol_identity(callee.as_ref(), function.unresolved_ctxt)
}

fn same_value_path_object(left: &ValuePath, right: &ValuePath) -> bool {
    let (Some((_, left_object)), Some((_, right_object))) =
        (left.properties.split_last(), right.properties.split_last())
    else {
        return false;
    };
    left.root == right.root && left_object == right_object
}

fn exact_returned_value_path(function: &RuntimeFunction) -> Option<ValuePath> {
    let mut returns = ReturnExpressionCollector::default();
    function.body.visit_with(&mut returns);
    let mut path = None;
    for expression in returns.expressions {
        let expression = match strip_parentheses(expression.as_ref()) {
            Expr::Seq(sequence) => sequence.exprs.last()?.as_ref(),
            expression => expression,
        };
        let current = value_path(expression)?;
        if path.as_ref().is_some_and(|existing| existing != &current) {
            return None;
        }
        path = Some(current);
    }
    path
}

fn uses_capture_restore_flow(
    modules: &[PreparedAngularModule],
    getter: &SymbolIdentity,
    restore: &SymbolIdentity,
    roles: &IvyRoleTable,
) -> bool {
    struct Collector<'a> {
        getter: &'a SymbolIdentity,
        restore: &'a SymbolIdentity,
        roles: &'a IvyRoleTable,
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
                        && symbol_identity(root, self.unresolved_ctxt).is_some_and(|identity| {
                            self.roles.identities_equivalent(&identity, self.getter)
                        })
                }) {
                    self.captures.insert(binding_key(&binding.id));
                }
            }
            declarator.visit_children_with(self);
        }

        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Some((root, argument_lists)) = call_chain(call) {
                if symbol_identity(root, self.unresolved_ctxt).is_some_and(|identity| {
                    self.roles.identities_equivalent(&identity, self.restore)
                }) && argument_lists.len() == 1
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
            roles,
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
) -> Option<ValuePath> {
    struct Collector<'a> {
        value: AssignedValue<'a>,
        targets: HashSet<ValuePath>,
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
                if let Some(path) = value_path(&Expr::Member(member.clone())) {
                    self.targets.insert(path);
                }
            }
            assignment.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut collector = Collector {
        value,
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
    integer_constants: &HashMap<BindingKey, u64>,
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
            && computed_slot(&member.prop, integer_constants) == Some(expected_index)
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
    let Some(parameters) = plain_parameter_bindings(definition) else {
        return false;
    };
    if !matches!(parameters.len(), 3 | 4)
        || !returns_identity(definition, &definition.identity)
        || observations.is_empty()
        || !observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && matches!(observation.arguments.len(), 2..=4)
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_string_literal(argument.as_ref()))
                && observation.arguments.get(1).is_some_and(|argument| {
                    matches!(argument.as_ref(), Expr::Fn(_) | Expr::Arrow(_))
                })
        })
    {
        return false;
    }
    if parameters.len() == 3 {
        return true;
    }

    !block_contains_binding(&definition.body, &parameters[2])
        && direct_calls(definition).iter().any(|call| {
            call.callee != definition.identity
                && call.arguments.len() == 7
                && expression_is_binding(call.arguments[4].as_ref(), &parameters[0])
                && expression_is_binding(call.arguments[5].as_ref(), &parameters[1])
                && expression_is_binding(call.arguments[6].as_ref(), &parameters[3])
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

fn infer_aria_property_family(
    function_index: &RuntimeFunctionIndex<'_>,
    observations_by_identity: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
) -> Vec<(SymbolIdentity, &'static str)> {
    let mut attribute_write_targets = HashSet::new();
    for (identity, observations) in observations_by_identity {
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        if !is_attribute_shape(definition, observations) {
            continue;
        }
        let Some(parameters) = plain_parameter_bindings(definition) else {
            continue;
        };
        for call in direct_calls(definition) {
            if forwards_parameter_dependencies_in_order(&call, &parameters[..2]) {
                attribute_write_targets.insert(call.callee);
            }
        }
    }
    if attribute_write_targets.is_empty() {
        return Vec::new();
    }

    observations_by_identity
        .iter()
        .filter_map(|(identity, observations)| {
            let definition = function_index.unique(identity)?;
            let parameters = plain_parameter_bindings(definition)?;
            if parameters.len() != 2
                || !returns_identity(definition, &definition.identity)
                || !observations.iter().all(|observation| {
                    matches!(
                        observation.usage,
                        TemplateCallUsage::Effect | TemplateCallUsage::Initializer
                    ) && observation.phase == 2
                        && observation.arguments.len() == 2
                        && observation
                            .arguments
                            .first()
                            .and_then(|argument| string_literal_value(argument.as_ref()))
                            .is_some_and(|name| name.starts_with("aria-"))
                })
            {
                return None;
            }

            let forwarded_targets = direct_calls(definition)
                .into_iter()
                .filter(|call| {
                    forwards_parameter_dependencies_in_order(call, parameters.as_slice())
                })
                .map(|call| call.callee)
                .collect::<HashSet<_>>();
            let has_attribute_fallback = forwarded_targets
                .iter()
                .any(|target| attribute_write_targets.contains(target));
            let has_distinct_input_path = forwarded_targets
                .iter()
                .any(|target| !attribute_write_targets.contains(target));
            (has_attribute_fallback && has_distinct_input_path)
                .then(|| (identity.clone(), "ɵɵariaProperty"))
        })
        .collect()
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
    calls.len() >= 3
        && (calls.iter().any(|call| {
            call.arguments.len() >= 6 && forwards_parameters_in_order(call, &parameters)
        }) || calls.iter().any(|call| {
            is_member_call_named(call, "setProperty")
                && forwards_parameter_dependencies_in_order(call, &parameters[..2])
        }))
}

fn is_property_interpolate_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    let Some(parameters) = plain_parameter_bindings(definition) else {
        return false;
    };
    if parameters.len() != 3
        || !returns_identity(definition, &definition.identity)
        || observations.is_empty()
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
    let [call] = calls.as_slice() else {
        return false;
    };
    call.callee != definition.identity
        && call.arguments.len() == 5
        && expression_is_binding(call.arguments[0].as_ref(), &parameters[0])
        && is_empty_string_literal(call.arguments[1].as_ref())
        && expression_is_binding(call.arguments[2].as_ref(), &parameters[1])
        && is_empty_string_literal(call.arguments[3].as_ref())
        && expression_is_binding(call.arguments[4].as_ref(), &parameters[2])
}

fn is_attribute_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    plain_parameter_bindings(definition).is_some_and(|parameters| parameters.len() == 4)
        && returns_identity(definition, &definition.identity)
        && direct_calls(definition).len() >= 4
        && observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 2
                && matches!(observation.arguments.len(), 2..=4)
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_string_literal(argument.as_ref()))
        })
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
    integer_constants: &HashMap<BindingKey, u64>,
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
    let nested_iife = calls.is_empty()
        && is_nested_iife_next_context_shape(definition, &parameter, integer_constants);
    if let [call] = calls.as_slice() {
        return call.arguments.as_slice().first().is_some_and(|argument| {
            matches!(
                argument.as_ref(),
                Expr::Ident(identifier) if binding_key(identifier) == parameter
            )
        });
    }
    calls.is_empty()
        && ((decrements_binding(&definition.body, &parameter)
            && contains_computed_member_index(&definition.body, 14)
            && returns_computed_member_index(definition, 8))
            || nested_iife)
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct ValuePath {
    root: BindingKey,
    properties: Vec<Atom>,
}

fn is_nested_iife_next_context_shape(
    definition: &RuntimeFunction,
    outer_depth: &BindingKey,
    integer_constants: &HashMap<BindingKey, u64>,
) -> bool {
    let Some(read_call) = single_returned_call(&definition.body) else {
        return false;
    };
    let Some((read_function, read_parameters)) = direct_iife_with_plain_parameters(read_call)
    else {
        return false;
    };
    let [read_depth] = read_parameters.as_slice() else {
        return false;
    };
    if read_call.args.len() != 1
        || read_call.args[0].spread.is_some()
        || !expression_is_binding(read_call.args[0].expr.as_ref(), outer_depth)
    {
        return false;
    }
    let Some(read_body) = &read_function.body else {
        return false;
    };
    let Some(returned) = single_top_level_return_expression(read_body) else {
        return false;
    };
    let Expr::Member(context_member) = strip_parentheses(returned) else {
        return false;
    };
    let Some(context_slot) = computed_slot(&context_member.prop, integer_constants) else {
        return false;
    };
    if context_slot != 8 {
        return false;
    }
    let Expr::Assign(state_assignment) = strip_parentheses(context_member.obj.as_ref()) else {
        return false;
    };
    if state_assignment.op != AssignOp::Assign {
        return false;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(state_member)) = &state_assignment.left
    else {
        return false;
    };
    let Some(state) = value_path(&Expr::Member(state_member.clone())) else {
        return false;
    };
    let Expr::Call(walk_call) = strip_parentheses(state_assignment.right.as_ref()) else {
        return false;
    };
    let Some((walk_function, walk_parameters)) = direct_iife_with_plain_parameters(walk_call)
    else {
        return false;
    };
    let [walk_depth, walk_view] = walk_parameters.as_slice() else {
        return false;
    };
    if walk_call.args.len() != 2
        || walk_call
            .args
            .iter()
            .any(|argument| argument.spread.is_some())
        || !expression_is_binding(walk_call.args[0].expr.as_ref(), read_depth)
        || value_path(walk_call.args[1].expr.as_ref()) != Some(state)
    {
        return false;
    }
    let Some(walk_body) = &walk_function.body else {
        return false;
    };
    if single_returned_binding(walk_body).as_ref() != Some(walk_view) {
        return false;
    }
    let Some(parent_slot) =
        loop_traversal_slot(walk_body, walk_depth, walk_view, integer_constants)
    else {
        return false;
    };
    parent_slot == 14
}

fn direct_iife_with_plain_parameters(call: &CallExpr) -> Option<(&Function, Vec<BindingKey>)> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Fn(function) = strip_parentheses(callee.as_ref()) else {
        return None;
    };
    if function.function.is_async || function.function.is_generator {
        return None;
    }
    let parameters = function
        .function
        .params
        .iter()
        .map(|parameter| pat_binding(&parameter.pat))
        .collect::<Option<Vec<_>>>()?;
    Some((&function.function, parameters))
}

fn single_top_level_return_expression(body: &BlockStmt) -> Option<&Expr> {
    let mut returned = None;
    for statement in &body.stmts {
        match statement {
            Stmt::Return(ReturnStmt {
                arg: Some(expression),
                ..
            }) if returned.is_none() => returned = Some(expression.as_ref()),
            Stmt::Empty(_) => {}
            _ => return None,
        }
    }
    returned
}

fn computed_slot(
    property: &MemberProp,
    integer_constants: &HashMap<BindingKey, u64>,
) -> Option<u64> {
    if let Some(index) = computed_member_index(property) {
        return Some(index);
    }
    let MemberProp::Computed(computed) = property else {
        return None;
    };
    let Expr::Ident(identifier) = strip_parentheses(computed.expr.as_ref()) else {
        return None;
    };
    integer_constants.get(&binding_key(identifier)).copied()
}

fn value_path(expression: &Expr) -> Option<ValuePath> {
    match strip_parentheses(expression) {
        Expr::Ident(identifier) => Some(ValuePath {
            root: binding_key(identifier),
            properties: Vec::new(),
        }),
        Expr::Member(member) => {
            let mut path = value_path(member.obj.as_ref())?;
            path.properties.push(member_prop_name(&member.prop)?);
            Some(path)
        }
        _ => None,
    }
}

fn loop_traversal_slot(
    body: &BlockStmt,
    depth: &BindingKey,
    view: &BindingKey,
    integer_constants: &HashMap<BindingKey, u64>,
) -> Option<u64> {
    struct LoopFinder<'a> {
        depth: &'a BindingKey,
        view: &'a BindingKey,
        integer_constants: &'a HashMap<BindingKey, u64>,
        slots: Vec<u64>,
    }

    impl LoopFinder<'_> {
        fn inspect_loop(&mut self, test: &Expr, body: &Stmt, update: Option<&Expr>) {
            if !is_positive_depth_test(test, self.depth) {
                return;
            }
            let mut collector = LoopBodyTraversalCollector {
                depth: self.depth,
                view: self.view,
                integer_constants: self.integer_constants,
                decrements_depth: false,
                slots: Vec::new(),
            };
            body.visit_with(&mut collector);
            if let Some(update) = update {
                update.visit_with(&mut collector);
            }
            let mut slots = collector.slots.into_iter();
            let Some(slot) = slots.next() else {
                return;
            };
            if collector.decrements_depth && slots.all(|candidate| candidate == slot) {
                self.slots.push(slot);
            }
        }
    }

    impl Visit for LoopFinder<'_> {
        fn visit_for_stmt(&mut self, statement: &swc_core::ecma::ast::ForStmt) {
            if let Some(test) = statement.test.as_deref() {
                self.inspect_loop(test, statement.body.as_ref(), statement.update.as_deref());
            }
        }

        fn visit_while_stmt(&mut self, statement: &swc_core::ecma::ast::WhileStmt) {
            self.inspect_loop(statement.test.as_ref(), statement.body.as_ref(), None);
        }

        fn visit_do_while_stmt(&mut self, statement: &swc_core::ecma::ast::DoWhileStmt) {
            self.inspect_loop(statement.test.as_ref(), statement.body.as_ref(), None);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    struct LoopBodyTraversalCollector<'a> {
        depth: &'a BindingKey,
        view: &'a BindingKey,
        integer_constants: &'a HashMap<BindingKey, u64>,
        decrements_depth: bool,
        slots: Vec<u64>,
    }

    impl Visit for LoopBodyTraversalCollector<'_> {
        fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
            if assignment.op == AssignOp::Assign {
                if let AssignTarget::Simple(SimpleAssignTarget::Ident(target)) = &assignment.left {
                    if binding_key(&target.id) == *self.view {
                        if let Expr::Member(member) = strip_parentheses(assignment.right.as_ref()) {
                            if matches!(
                                strip_parentheses(member.obj.as_ref()),
                                Expr::Ident(object) if binding_key(object) == *self.view
                            ) {
                                if let Some(slot) =
                                    computed_slot(&member.prop, self.integer_constants)
                                {
                                    self.slots.push(slot);
                                }
                            }
                        }
                    }
                }
            }
            assignment.visit_children_with(self);
        }

        fn visit_update_expr(&mut self, update: &UpdateExpr) {
            if update.op == swc_core::ecma::ast::UpdateOp::MinusMinus
                && expression_is_binding(update.arg.as_ref(), self.depth)
            {
                self.decrements_depth = true;
            }
            update.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut finder = LoopFinder {
        depth,
        view,
        integer_constants,
        slots: Vec::new(),
    };
    body.visit_with(&mut finder);
    let mut slots = finder.slots.into_iter();
    let slot = slots.next()?;
    slots.all(|candidate| candidate == slot).then_some(slot)
}

fn is_positive_depth_test(expression: &Expr, depth: &BindingKey) -> bool {
    let Expr::Bin(binary) = strip_parentheses(expression) else {
        return false;
    };
    match binary.op {
        BinaryOp::Gt => {
            expression_is_binding(binary.left.as_ref(), depth)
                && nonnegative_integer_value(binary.right.as_ref()) == Some(0)
        }
        BinaryOp::Lt => {
            nonnegative_integer_value(binary.left.as_ref()) == Some(0)
                && expression_is_binding(binary.right.as_ref(), depth)
        }
        _ => false,
    }
}

fn infer_projection_role_family(
    function_index: &RuntimeFunctionIndex<'_>,
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
) -> Vec<(SymbolIdentity, &'static str)> {
    let mut observations_by_definition =
        HashMap::<SymbolIdentity, Vec<&TemplateCallObservation>>::new();
    for (identity, calls) in observations {
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        observations_by_definition
            .entry(definition.identity.clone())
            .or_default()
            .extend(calls);
    }

    let mut definitions_by_property = HashMap::<Atom, HashSet<SymbolIdentity>>::new();
    let mut projections_by_property = HashMap::<Atom, HashSet<SymbolIdentity>>::new();
    for (identity, calls) in observations_by_definition {
        let Some(definition) = function_index.unique(&identity) else {
            continue;
        };
        if let Some(properties) = projection_definition_properties(definition, &calls) {
            for property in properties {
                definitions_by_property
                    .entry(property)
                    .or_default()
                    .insert(identity.clone());
            }
        }
        if let Some(property) = projection_selector_property(definition, &calls) {
            projections_by_property
                .entry(property)
                .or_default()
                .insert(identity);
        }
    }

    let mut inferred = Vec::new();
    for (property, definitions) in definitions_by_property {
        let Some(projections) = projections_by_property.get(&property) else {
            continue;
        };
        let (Some(definition), Some(projection)) =
            (unique_identity(&definitions), unique_identity(projections))
        else {
            continue;
        };
        if definition == projection {
            continue;
        }
        inferred.push((definition.clone(), "ɵɵprojectionDef"));
        inferred.push((projection.clone(), "ɵɵprojection"));
    }
    inferred
}

fn unique_identity(identities: &HashSet<SymbolIdentity>) -> Option<&SymbolIdentity> {
    let mut identities = identities.iter();
    let identity = identities.next()?;
    identities.next().is_none().then_some(identity)
}

fn projection_definition_properties(
    definition: &RuntimeFunction,
    observations: &[&TemplateCallObservation],
) -> Option<HashSet<Atom>> {
    let parameters = plain_parameter_bindings(definition)?;
    let [selectors] = parameters.as_slice() else {
        return None;
    };
    if direct_calls(definition).len() < 3
        || !block_contains_loop(&definition.body)
        || !block_contains_binding(&definition.body, selectors)
        || observations.is_empty()
        || !observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && matches!(observation.arguments.len(), 0 | 1)
                && observation.arguments.first().is_none_or(|argument| {
                    matches!(argument.as_ref(), Expr::Ident(_) | Expr::Array(_))
                })
        })
    {
        return None;
    }
    let properties = assigned_member_properties(&definition.body);
    (properties.len() >= 2).then_some(properties)
}

fn projection_selector_property(
    definition: &RuntimeFunction,
    observations: &[&TemplateCallObservation],
) -> Option<Atom> {
    if !matches!(definition.params.len(), 2 | 3 | 6)
        || !is_numeric_default(&definition.params[1], 0.0)
        || direct_calls(definition).len() < 5
        || observations.is_empty()
        || !observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && (1..=definition.params.len()).contains(&observation.arguments.len())
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
                && observation
                    .arguments
                    .get(1)
                    .is_none_or(|argument| is_nonnegative_integer(argument.as_ref()))
        })
    {
        return None;
    }
    let selector = parameter_binding_with_default(&definition.params[1])?;
    unique_atom(&member_properties_assigned_from_binding(
        &definition.body,
        &selector,
    ))
    .cloned()
}

fn unique_atom(atoms: &HashSet<Atom>) -> Option<&Atom> {
    let mut atoms = atoms.iter();
    let atom = atoms.next()?;
    atoms.next().is_none().then_some(atom)
}

fn assigned_member_properties(block: &BlockStmt) -> HashSet<Atom> {
    struct Collector {
        properties: HashSet<Atom>,
    }

    impl Visit for Collector {
        fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
            if assignment.op == AssignOp::Assign {
                if let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assignment.left {
                    if let Some(property) = member_prop_name(&member.prop) {
                        self.properties.insert(property);
                    }
                }
            }
            assignment.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut collector = Collector {
        properties: HashSet::new(),
    };
    block.visit_with(&mut collector);
    collector.properties
}

fn member_properties_assigned_from_binding(
    block: &BlockStmt,
    binding: &BindingKey,
) -> HashSet<Atom> {
    struct Collector<'a> {
        binding: &'a BindingKey,
        properties: HashSet<Atom>,
    }

    impl Visit for Collector<'_> {
        fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
            if assignment.op == AssignOp::Assign
                && expression_is_binding(assignment.right.as_ref(), self.binding)
            {
                if let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assignment.left {
                    if let Some(property) = member_prop_name(&member.prop) {
                        self.properties.insert(property);
                    }
                }
            }
            assignment.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut collector = Collector {
        binding,
        properties: HashSet::new(),
    };
    block.visit_with(&mut collector);
    collector.properties
}

fn block_contains_loop(block: &BlockStmt) -> bool {
    struct Finder {
        found: bool,
    }

    impl Visit for Finder {
        fn visit_for_stmt(&mut self, _statement: &swc_core::ecma::ast::ForStmt) {
            self.found = true;
        }

        fn visit_while_stmt(&mut self, _statement: &swc_core::ecma::ast::WhileStmt) {
            self.found = true;
        }

        fn visit_do_while_stmt(&mut self, _statement: &swc_core::ecma::ast::DoWhileStmt) {
            self.found = true;
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut finder = Finder { found: false };
    block.visit_with(&mut finder);
    finder.found
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
    ((direct_calls(definition).len() >= 2 && !contains_throw_statement(&definition.body))
        || (loads_parameter_from_offset_member(definition, slot, 27)
            && exact_returned_identity(definition)
                == Some(SymbolIdentity::LocalBinding(slot.clone()))
            && !contains_throw_statement(&definition.body)))
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

fn is_declare_let_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    plain_parameter_bindings(definition).is_some_and(|parameters| parameters.len() == 1)
        && contains_string_literal(&definition.body, "NgLet")
        && direct_calls(definition).len() >= 4
        && observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect
                && observation.phase == 1
                && observation.arguments.len() == 1
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
        })
}

fn is_store_let_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    let Some(parameters) = plain_parameter_bindings(definition) else {
        return false;
    };
    let [value] = parameters.as_slice() else {
        return false;
    };
    direct_calls(definition).iter().any(|call| {
        call.arguments.len() >= 3 && forwards_parameters_in_order(call, std::slice::from_ref(value))
    }) && exact_returned_identity(definition) == Some(SymbolIdentity::LocalBinding(value.clone()))
        && !contains_throw_statement(&definition.body)
        && observations.iter().all(|observation| {
            matches!(
                observation.usage,
                TemplateCallUsage::Effect | TemplateCallUsage::Initializer
            ) && observation.phase == 2
                && observation.arguments.len() == 1
        })
}

fn is_read_context_let_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
) -> bool {
    let Some(parameters) = plain_parameter_bindings(definition) else {
        return false;
    };
    let [slot] = parameters.as_slice() else {
        return false;
    };
    contains_throw_statement(&definition.body)
        && (loads_parameter_from_offset_member(definition, slot, 27)
            || direct_calls(definition).len() >= 2)
        && exact_returned_identity(definition) == Some(SymbolIdentity::LocalBinding(slot.clone()))
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

fn is_reference_candidate_shape(
    definition: &RuntimeFunction,
    observations: &[TemplateCallObservation],
    integer_constants: &HashMap<BindingKey, u64>,
    function_index: &RuntimeFunctionIndex<'_>,
) -> bool {
    let Some(parameters) = plain_parameter_bindings(definition) else {
        return false;
    };
    let [slot] = parameters.as_slice() else {
        return false;
    };
    !observations.is_empty()
        && observations.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Initializer
                && observation.phase == 2
                && observation.arguments.len() == 1
                && observation
                    .arguments
                    .first()
                    .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
        })
        && ((direct_calls(definition).is_empty()
            && returns_parameter_offset_member(definition, slot, 27))
            || returns_wrapped_context_slot(definition, slot, integer_constants, function_index))
}

fn returns_wrapped_context_slot(
    definition: &RuntimeFunction,
    slot: &BindingKey,
    integer_constants: &HashMap<BindingKey, u64>,
    function_index: &RuntimeFunctionIndex<'_>,
) -> bool {
    let Some(expression) = single_top_level_return_expression(&definition.body) else {
        return false;
    };
    let Expr::Call(call) = strip_parentheses(expression) else {
        return false;
    };
    let [view, index] = call.args.as_slice() else {
        return false;
    };
    if view.spread.is_some()
        || index.spread.is_some()
        || zero_argument_iife_returned_value_path(view.expr.as_ref()).is_none()
        || !matches!(
            parameter_offset(index.expr.as_ref(), slot, integer_constants,),
            Some(25 | 27)
        )
    {
        return false;
    }
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Some(callee) = symbol_identity(callee.as_ref(), definition.unresolved_ctxt) else {
        return false;
    };
    function_index
        .unique(&callee)
        .is_some_and(is_exact_index_loader)
}

fn zero_argument_iife_returned_value_path(expression: &Expr) -> Option<ValuePath> {
    let Expr::Call(call) = strip_parentheses(expression) else {
        return None;
    };
    if !call.args.is_empty() {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Fn(function) = strip_parentheses(callee.as_ref()) else {
        return None;
    };
    if !function.function.params.is_empty()
        || function.function.is_async
        || function.function.is_generator
    {
        return None;
    }
    let body = function.function.body.as_ref()?;
    let path = value_path(single_top_level_return_expression(body)?)?;
    (!path.properties.is_empty()).then_some(path)
}

fn is_exact_index_loader(function: &RuntimeFunction) -> bool {
    let Some(parameters) = plain_parameter_bindings(function) else {
        return false;
    };
    let [view, index] = parameters.as_slice() else {
        return false;
    };
    let Some(expression) = single_top_level_return_expression(&function.body) else {
        return false;
    };
    let Expr::Member(member) = strip_parentheses(expression) else {
        return false;
    };
    let MemberProp::Computed(property) = &member.prop else {
        return false;
    };
    expression_is_binding(member.obj.as_ref(), view)
        && expression_is_binding(property.expr.as_ref(), index)
}

fn parameter_offset(
    expression: &Expr,
    parameter: &BindingKey,
    integer_constants: &HashMap<BindingKey, u64>,
) -> Option<u64> {
    let Expr::Bin(binary) = strip_parentheses(expression) else {
        return None;
    };
    if binary.op != BinaryOp::Add {
        return None;
    }
    if expression_is_binding(binary.left.as_ref(), parameter) {
        return stable_integer_value(binary.right.as_ref(), integer_constants);
    }
    if expression_is_binding(binary.right.as_ref(), parameter) {
        return stable_integer_value(binary.left.as_ref(), integer_constants);
    }
    None
}

fn stable_integer_value(
    expression: &Expr,
    integer_constants: &HashMap<BindingKey, u64>,
) -> Option<u64> {
    if let Some(value) = nonnegative_integer_value(expression) {
        return u64::try_from(value).ok();
    }
    let Expr::Ident(identifier) = strip_parentheses(expression) else {
        return None;
    };
    integer_constants.get(&binding_key(identifier)).copied()
}

fn returns_parameter_offset_member(
    function: &RuntimeFunction,
    parameter: &BindingKey,
    offset: u64,
) -> bool {
    let mut returns = ReturnExpressionCollector::default();
    function.body.visit_with(&mut returns);
    !returns.expressions.is_empty()
        && returns.expressions.iter().all(|expression| {
            let expression = match strip_parentheses(expression.as_ref()) {
                Expr::Seq(sequence) => sequence.exprs.last().map(Box::as_ref),
                expression => Some(expression),
            };
            matches!(
                expression,
                Some(Expr::Member(member))
                    if member_uses_parameter_offset(member, parameter, offset)
            )
        })
        && !block_can_fall_through(&function.body)
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
        || direct_calls(definition).len() < 3
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

fn infer_pure_function_family(
    function_index: &RuntimeFunctionIndex<'_>,
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
) -> Vec<(SymbolIdentity, &'static str)> {
    struct Candidate {
        identity: SymbolIdentity,
        value_count: usize,
        direct_callees: HashSet<SymbolIdentity>,
    }

    let mut candidates = Vec::new();
    for (identity, calls) in observations {
        let Some(first) = calls.first() else {
            continue;
        };
        let argument_count = first.arguments.len();
        if !(2..=10).contains(&argument_count)
            || !calls.iter().all(|observation| {
                observation.phase == 2
                    && observation.usage == TemplateCallUsage::Effect
                    && observation.arguments.len() == argument_count
                    && observation
                        .arguments
                        .first()
                        .is_some_and(|argument| is_nonnegative_integer(argument.as_ref()))
                    && observation.arguments.get(1).is_some_and(|argument| {
                        matches!(
                            strip_parentheses(argument.as_ref()),
                            Expr::Ident(_) | Expr::Member(_) | Expr::Fn(_) | Expr::Arrow(_)
                        )
                    })
            })
        {
            continue;
        }
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        let Some(parameters) = plain_parameter_bindings(definition) else {
            continue;
        };
        if !(parameters.len() == argument_count || (argument_count == 6 && parameters.len() == 7)) {
            continue;
        }
        let direct_callees = all_call_callees(definition);
        let mut returns = ReturnExpressionCollector::default();
        definition.body.visit_with(&mut returns);
        if direct_callees.len() < 3 || returns.expressions.is_empty() {
            continue;
        }
        candidates.push(Candidate {
            identity: identity.clone(),
            value_count: argument_count - 2,
            direct_callees,
        });
    }

    candidates
        .iter()
        .filter(|candidate| {
            candidates.iter().any(|other| {
                candidate.identity != other.identity
                    && candidate.value_count != other.value_count
                    && candidate
                        .direct_callees
                        .intersection(&other.direct_callees)
                        .take(2)
                        .count()
                        >= 2
            })
        })
        .filter_map(|candidate| {
            pure_function_name(candidate.value_count).map(|name| (candidate.identity.clone(), name))
        })
        .collect()
}

fn all_call_callees(function: &RuntimeFunction) -> HashSet<SymbolIdentity> {
    struct Collector {
        unresolved_ctxt: SyntaxContext,
        callees: HashSet<SymbolIdentity>,
    }

    impl Visit for Collector {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if let Some(identity) = symbol_identity(callee.as_ref(), self.unresolved_ctxt) {
                    self.callees.insert(identity);
                }
            }
            call.visit_children_with(self);
        }

        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
    }

    let mut collector = Collector {
        unresolved_ctxt: function.unresolved_ctxt,
        callees: HashSet::new(),
    };
    function.body.visit_with(&mut collector);
    collector.callees
}

fn pure_function_name(value_count: usize) -> Option<&'static str> {
    Some(match value_count {
        0 => "ɵɵpureFunction0",
        1 => "ɵɵpureFunction1",
        2 => "ɵɵpureFunction2",
        3 => "ɵɵpureFunction3",
        4 => "ɵɵpureFunction4",
        5 => "ɵɵpureFunction5",
        6 => "ɵɵpureFunction6",
        7 => "ɵɵpureFunction7",
        8 => "ɵɵpureFunction8",
        _ => return None,
    })
}

fn is_template_selection(expression: &Expr) -> bool {
    let expression = strip_parentheses(expression);
    let Expr::Cond(conditional) = expression else {
        return false;
    };
    [conditional.cons.as_ref(), conditional.alt.as_ref()]
        .into_iter()
        .all(|branch| is_template_index(branch) || is_template_selection(branch))
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
    let mut inferred = text_interpolation_pairs(function_index, observations)
        .into_iter()
        .flat_map(|pair| {
            [
                (pair.text, "ɵɵtextInterpolate"),
                (pair.text_one, "ɵɵtextInterpolate1"),
            ]
        })
        .collect::<Vec<_>>();

    for (identity, calls_in_templates) in observations {
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };
        let parameters = definition
            .params
            .iter()
            .map(parameter_binding_with_default)
            .collect::<Option<Vec<_>>>();
        let Some(parameters) = parameters else {
            continue;
        };
        let Some(name) = text_interpolation_name(parameters.len()) else {
            continue;
        };
        if parameters.len() < 3
            || !calls_in_templates.iter().all(|observation| {
                observation.usage == TemplateCallUsage::Effect
                    && observation.phase == 2
                    && matches!(
                        observation.arguments.len(),
                        count if count == parameters.len() || count + 1 == parameters.len()
                    )
            })
            || !returns_identity(definition, identity)
            || !contains_member_property(&definition.body, "nodeValue")
        {
            continue;
        }
        let has_interpolation_helper = direct_calls(definition).iter().any(|call| {
            call.callee != *identity
                && call.arguments.len() == parameters.len() + 1
                && forwards_parameters_in_order(call, &parameters)
                && function_index
                    .unique(&call.callee)
                    .is_some_and(|helper| helper.params.len() == call.arguments.len())
        });
        if has_interpolation_helper {
            inferred.push((identity.clone(), name));
        }
    }

    inferred
}

fn text_interpolation_name(parameter_count: usize) -> Option<&'static str> {
    Some(match parameter_count {
        1 => "ɵɵtextInterpolate",
        3 => "ɵɵtextInterpolate1",
        5 => "ɵɵtextInterpolate2",
        7 => "ɵɵtextInterpolate3",
        9 => "ɵɵtextInterpolate4",
        11 => "ɵɵtextInterpolate5",
        13 => "ɵɵtextInterpolate6",
        15 => "ɵɵtextInterpolate7",
        17 => "ɵɵtextInterpolate8",
        _ => return None,
    })
}

struct TextInterpolationPair {
    text: SymbolIdentity,
    text_one: SymbolIdentity,
    helper: SymbolIdentity,
}

fn text_interpolation_pairs(
    function_index: &RuntimeFunctionIndex<'_>,
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
) -> Vec<TextInterpolationPair> {
    let mut pairs = Vec::new();
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
        {
            continue;
        }
        let Some(helper) = target_calls.iter().find(|call| {
            call.arguments.len() >= target_parameters.len()
                && forwards_parameters_in_order(call, &target_parameters)
        }) else {
            continue;
        };
        pairs.push(TextInterpolationPair {
            text: identity.clone(),
            text_one: target.identity.clone(),
            helper: helper.callee.clone(),
        });
    }
    pairs
}

fn infer_expression_interpolation_family(
    function_index: &RuntimeFunctionIndex<'_>,
    observations: &HashMap<SymbolIdentity, Vec<TemplateCallObservation>>,
) -> Vec<(SymbolIdentity, &'static str)> {
    let pairs = text_interpolation_pairs(function_index, observations);
    if pairs.is_empty() {
        return Vec::new();
    }
    let helpers = pairs
        .iter()
        .map(|pair| pair.helper.clone())
        .collect::<HashSet<_>>();
    let helper_callees = helpers
        .iter()
        .filter_map(|helper| {
            function_index.unique(helper).map(|definition| {
                direct_calls(definition)
                    .into_iter()
                    .map(|call| call.callee)
                    .collect::<HashSet<_>>()
            })
        })
        .collect::<Vec<_>>();

    let mut inferred = Vec::new();
    for (identity, calls_in_templates) in observations {
        if !calls_in_templates.iter().all(|observation| {
            observation.usage == TemplateCallUsage::Effect && observation.phase == 2
        }) {
            continue;
        }
        let Some(definition) = function_index.unique(identity) else {
            continue;
        };

        if calls_in_templates
            .iter()
            .all(|observation| observation.arguments.len() == 1)
        {
            let Some(parameters) = plain_parameter_bindings(definition) else {
                continue;
            };
            if parameters.len() != 1 {
                continue;
            }
            let direct_callees = direct_calls(definition)
                .into_iter()
                .map(|call| call.callee)
                .collect::<HashSet<_>>();
            let mut returns = ReturnExpressionCollector::default();
            definition.body.visit_with(&mut returns);
            if direct_callees.len() == 2
                && matches!(
                    returns.expressions.as_slice(),
                    [expression] if matches!(strip_parentheses(expression.as_ref()), Expr::Cond(_))
                )
                && helper_callees
                    .iter()
                    .any(|helper| direct_callees.intersection(helper).take(2).count() == 2)
            {
                inferred.push((identity.clone(), "ɵɵinterpolate"));
            }
            continue;
        }

        if !calls_in_templates
            .iter()
            .all(|observation| matches!(observation.arguments.len(), 2 | 3))
            || definition.params.len() != 3
            || !is_empty_string_default(&definition.params[2])
        {
            continue;
        }
        let parameters = definition
            .params
            .iter()
            .map(parameter_binding_with_default)
            .collect::<Option<Vec<_>>>();
        let Some(parameters) = parameters else {
            continue;
        };
        let calls = direct_calls(definition);
        let [call] = calls.as_slice() else {
            continue;
        };
        if helpers.contains(&call.callee)
            && call.arguments.len() == parameters.len() + 1
            && forwards_parameters_in_order(call, &parameters)
            && returned_call_callee(definition).as_ref() == Some(&call.callee)
        {
            inferred.push((identity.clone(), "ɵɵinterpolate1"));
        }
    }
    inferred
}

fn parameter_binding_with_default(parameter: &Pat) -> Option<BindingKey> {
    let parameter = match parameter {
        Pat::Ident(binding) => return Some(binding_key(&binding.id)),
        Pat::Assign(assignment) => assignment.left.as_ref(),
        _ => return None,
    };
    let Pat::Ident(binding) = parameter else {
        return None;
    };
    Some(binding_key(&binding.id))
}

fn returned_call_callee(function: &RuntimeFunction) -> Option<SymbolIdentity> {
    let mut returns = ReturnExpressionCollector::default();
    function.body.visit_with(&mut returns);
    let [expression] = returns.expressions.as_slice() else {
        return None;
    };
    let Expr::Call(call) = strip_parentheses(expression.as_ref()) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    symbol_identity(callee.as_ref(), function.unresolved_ctxt)
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

    fn record_class(&mut self, identity: SymbolIdentity, class: &Class) {
        self.classes.push(RuntimeClass {
            identity,
            class: Box::new(class.clone()),
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
        if let Some(alias) = value_alias_identity(value, self.unresolved_ctxt) {
            if alias != identity {
                self.value_aliases.push((identity.clone(), alias));
            }
        }
        self.record_function_value(identity, value);
    }

    fn record_function_value(&mut self, identity: SymbolIdentity, value: &Expr) {
        match value {
            Expr::Fn(function) => self.record_function(identity, function.function.as_ref()),
            Expr::Arrow(arrow) => self.record_arrow(identity, arrow),
            Expr::Class(class) => self.record_class(identity, class.class.as_ref()),
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
                if let Some(value) = nonnegative_integer_value(value) {
                    self.integer_candidates
                        .insert(binding_key(&binding.id), value as u64);
                }
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
        let identity = SymbolIdentity::LocalBinding(binding_key(&declaration.ident));
        self.record_definition(identity.clone());
        self.record_class(identity, declaration.class.as_ref());
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

fn is_specialized_i18n_postprocess_shape(function: &RuntimeFunction) -> bool {
    if !matches!(function.params.as_slice(), [Pat::Ident(_)]) {
        return false;
    }
    let mut shape = I18nPostprocessShape::default();
    function.body.visit_with(&mut shape);
    let mut returns = ReturnExpressionCollector::default();
    function.body.visit_with(&mut returns);
    shape.replace_with_callback
        && shape.split_pipe
        && shape.splice_one
        && shape.marker_match
        && shape.zero_template_stack
        && shape.throws_on_exhaustion
        && !returns.expressions.is_empty()
}

#[derive(Default)]
struct I18nPostprocessShape {
    replace_with_callback: bool,
    split_pipe: bool,
    splice_one: bool,
    marker_match: bool,
    zero_template_stack: bool,
    throws_on_exhaustion: bool,
}

impl Visit for I18nPostprocessShape {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Member(member) = strip_parentheses(callee.as_ref()) {
                match member_prop_name(&member.prop).as_deref() {
                    Some("replace") => {
                        self.replace_with_callback |= matches!(
                            call.args.as_slice(),
                            [pattern, callback]
                                if pattern.spread.is_none()
                                    && callback.spread.is_none()
                                    && matches!(
                                        strip_parentheses(callback.expr.as_ref()),
                                        Expr::Arrow(_) | Expr::Fn(_)
                                    )
                        );
                    }
                    Some("split") => {
                        self.split_pipe |= matches!(
                            call.args.as_slice(),
                            [delimiter]
                                if delimiter.spread.is_none()
                                    && matches!(
                                        strip_parentheses(delimiter.expr.as_ref()),
                                        Expr::Lit(Lit::Str(value)) if wtf8_to_string(&value.value) == "|"
                                    )
                        );
                    }
                    Some("splice") => {
                        self.splice_one |= matches!(
                            call.args.as_slice(),
                            [index, count]
                                if index.spread.is_none()
                                    && count.spread.is_none()
                                    && matches!(
                                        strip_parentheses(count.expr.as_ref()),
                                        Expr::Lit(Lit::Num(value)) if value.value == 1.0
                                    )
                        );
                    }
                    Some("match") => {
                        self.marker_match |=
                            matches!(call.args.as_slice(), [pattern] if pattern.spread.is_none());
                    }
                    _ => {}
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_array_lit(&mut self, array: &ArrayLit) {
        self.zero_template_stack |= matches!(
            array.elems.as_slice(),
            [Some(element)]
                if element.spread.is_none()
                    && matches!(
                        strip_parentheses(element.expr.as_ref()),
                        Expr::Lit(Lit::Num(value)) if value.value == 0.0
                    )
        );
        array.visit_children_with(self);
    }

    fn visit_throw_stmt(&mut self, statement: &ThrowStmt) {
        self.throws_on_exhaustion = true;
        statement.visit_children_with(self);
    }
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
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Arrow(arrow) = strip_parentheses(callee.as_ref()) {
                self.inspect_arrow(arrow);
            }
        }
        if self.matched {
            return;
        }
        for argument in &call.args {
            let Expr::Arrow(arrow) = argument.expr.as_ref() else {
                continue;
            };
            self.inspect_arrow(arrow);
            if self.matched {
                return;
            }
        }
        call.visit_children_with(self);
    }
}

impl ReturnedDescriptorBuilder<'_> {
    fn inspect_arrow(&mut self, arrow: &ArrowExpr) {
        if self.matched {
            return;
        }
        let mut evidence = DescriptorBuilderEvidence {
            parameter: self.parameter,
            unresolved_ctxt: self.unresolved_ctxt,
            parameter_fields: HashSet::new(),
            has_object_assign: false,
            has_minified_component_descriptor: false,
            has_ng_standalone_marker: false,
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
        self.matched = (evidence.has_object_assign
            && has_fields(&["template", "dependencies", "styles"]))
            || has_fields(&[
                "decls",
                "vars",
                "template",
                "consts",
                "dependencies",
                "styles",
            ])
            || (evidence.has_minified_component_descriptor && evidence.has_ng_standalone_marker);
    }
}

struct DescriptorBuilderEvidence<'a> {
    parameter: &'a BindingKey,
    unresolved_ctxt: SyntaxContext,
    parameter_fields: HashSet<Atom>,
    has_object_assign: bool,
    has_minified_component_descriptor: bool,
    has_ng_standalone_marker: bool,
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

    fn visit_object_lit(&mut self, object: &ObjectLit) {
        if is_minified_component_descriptor_object(object, self.parameter) {
            self.has_minified_component_descriptor = true;
        }
        object.visit_children_with(self);
    }

    fn visit_str(&mut self, string: &swc_core::ecma::ast::Str) {
        if string.value.as_bytes() == b"NgStandalone" {
            self.has_ng_standalone_marker = true;
        }
    }
}

fn is_minified_component_descriptor_object(object: &ObjectLit, parameter: &BindingKey) -> bool {
    // Closure may rename every component-only field on both sides of this object. A full Ivy
    // component builder still has a much larger descriptor and forwards substantially more
    // definition fields than directive builders or ordinary configuration normalizers.
    const MIN_DESCRIPTOR_PROPERTIES: usize = 16;
    const MIN_FORWARDED_FIELDS: usize = 8;

    let mut property_count = 0usize;
    let mut output_fields = HashSet::new();
    let mut parameter_fields = HashSet::new();
    let mut has_empty_id = false;

    for property in &object.props {
        let PropOrSpread::Prop(property) = property else {
            continue;
        };
        let Prop::KeyValue(property) = property.as_ref() else {
            continue;
        };
        property_count += 1;
        if let Some(name) = prop_name(&property.key) {
            if name == "id" && is_empty_string_literal(property.value.as_ref()) {
                has_empty_id = true;
            }
            output_fields.insert(name);
        }
        let mut collector = ParameterFieldCollector {
            parameter,
            fields: HashSet::new(),
        };
        property.value.visit_with(&mut collector);
        parameter_fields.extend(collector.fields);
    }

    property_count >= MIN_DESCRIPTOR_PROPERTIES
        && parameter_fields.len() >= MIN_FORWARDED_FIELDS
        && has_empty_id
        && ["dependencies", "data", "id"]
            .iter()
            .all(|name| output_fields.contains(*name))
}

struct ParameterFieldCollector<'a> {
    parameter: &'a BindingKey,
    fields: HashSet<Atom>,
}

impl Visit for ParameterFieldCollector<'_> {
    fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
        if let Expr::Ident(object) = member.obj.as_ref() {
            if binding_key(object) == *self.parameter {
                if let Some(property) = member_prop_name(&member.prop) {
                    self.fields.insert(property);
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

fn infer_element_container_family(
    functions: &[RuntimeFunction],
) -> Vec<(SymbolIdentity, &'static str)> {
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
        if parameters.len() != 3 || !returns_identity(wrapper, &wrapper.identity) {
            continue;
        }
        let calls = direct_calls(wrapper);
        let [start, end] = calls.as_slice() else {
            continue;
        };
        let Some([start_definition]) = by_identity.get(&start.callee).map(Vec::as_slice) else {
            continue;
        };
        if !forwards_parameters(start, &parameters)
            || !end.arguments.is_empty()
            || start.callee == end.callee
            || start.callee == wrapper.identity
            || end.callee == wrapper.identity
            || !contains_string_literal(&start_definition.body, "ng-container")
            || !has_unique_self_returning_arity(&by_identity, &start.callee, 3, true)
            || !has_unique_self_returning_arity(&by_identity, &end.callee, 0, false)
        {
            continue;
        }
        inferred.push((wrapper.identity.clone(), "ɵɵelementContainer"));
        inferred.push((start.callee.clone(), "ɵɵelementContainerStart"));
        inferred.push((end.callee.clone(), "ɵɵelementContainerEnd"));
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
    span: Span,
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
                span: call.span,
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
