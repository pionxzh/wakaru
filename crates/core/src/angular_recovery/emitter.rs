use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::{anyhow, Result};
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, EqIgnoreSpan, SourceMap, Spanned, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, BindingIdent, BlockStmtOrExpr, CallExpr, Class, ClassDecl,
    ClassMember, ClassMethod, Decl, Expr, ExprOrSpread, Ident, IdentName, KeyValueProp, Lit,
    MemberExpr, MemberProp, MethodKind, Module, ModuleDecl, ModuleItem, ObjectLit, Pat, Prop,
    PropName, PropOrSpread, ReturnStmt, SimpleAssignTarget, Stmt, VarDecl, VarDeclKind,
    VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::artifact::{expression_references, ArtifactSupportPlan};
use super::roles::{AngularClassApi, AngularQueryOwner, IvyInstruction, IvyRoleTable};
use super::syntax::{binding_key, member_prop_name, prop_name, BindingKey};
use super::template::{RecoveredListenerMethod, RecoveredTemplate};
use super::ComponentQueryMetadata;
use crate::rules::rename_utils::{rename_bindings, BindingRename};

pub(super) struct ComponentEmitInput<'a> {
    pub(super) name: &'a str,
    pub(super) selector: &'a str,
    pub(super) styles: &'a [String],
    pub(super) class: &'a Class,
    pub(super) template: &'a RecoveredTemplate,
    pub(super) support: &'a ArtifactSupportPlan,
    pub(super) dependencies: &'a [String],
    pub(super) angular_imports: &'a [AngularClassApi],
}

pub(super) struct ModuleComponentEmitInput<'a> {
    pub(super) name: &'a str,
    pub(super) selector: &'a str,
    pub(super) styles: &'a [String],
    pub(super) class: &'a Class,
    pub(super) template_source: &'a str,
    pub(super) listener_methods: &'a [RecoveredListenerMethod],
    pub(super) dependencies: &'a [String],
    pub(super) angular_imports: &'a [AngularClassApi],
}

pub(super) fn emit_component_source(
    input: ComponentEmitInput<'_>,
    cm: Lrc<SourceMap>,
) -> Result<String> {
    let support_source = print_support_source(input.support, &[], cm.clone())?;

    let mut source = angular_core_import(input.angular_imports);
    source.push('\n');
    if let Some(support_source) = support_source {
        source.push_str(&support_source);
        source.push('\n');
    }
    append_unresolved_symbols(&mut source, input.support);
    source.push('\n');
    source.push_str(&component_source_fragment(
        ModuleComponentEmitInput {
            name: input.name,
            selector: input.selector,
            styles: input.styles,
            class: input.class,
            template_source: &input.template.source,
            listener_methods: &input.template.listener_methods,
            dependencies: input.dependencies,
            angular_imports: input.angular_imports,
        },
        &[],
        cm,
    )?);
    Ok(source)
}

pub(super) fn emit_angular_module_source(
    components: &[ModuleComponentEmitInput<'_>],
    support: &ArtifactSupportPlan,
    renames: &[BindingRename],
    cm: Lrc<SourceMap>,
) -> Result<String> {
    let angular_imports = components
        .iter()
        .flat_map(|component| component.angular_imports.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut source = angular_core_import(&angular_imports);
    source.push('\n');
    if let Some(support_source) = print_support_source(support, renames, cm.clone())? {
        source.push_str(&support_source);
        source.push('\n');
    }
    append_unresolved_symbols(&mut source, support);
    for component in components {
        source.push('\n');
        source.push_str(&component_source_fragment(
            ModuleComponentEmitInput {
                name: component.name,
                selector: component.selector,
                styles: component.styles,
                class: component.class,
                template_source: component.template_source,
                listener_methods: component.listener_methods,
                dependencies: component.dependencies,
                angular_imports: component.angular_imports,
            },
            renames,
            cm.clone(),
        )?);
        source.push('\n');
    }
    Ok(source.trim_end().to_string())
}

fn angular_core_import(imports: &[AngularClassApi]) -> String {
    let mut names = BTreeSet::from(["Component"]);
    names.extend(imports.iter().map(|api| api.canonical_export_name()));
    format!(
        "import {{ {} }} from \"@angular/core\";",
        names.into_iter().collect::<Vec<_>>().join(", ")
    )
}

fn print_support_source(
    support: &ArtifactSupportPlan,
    renames: &[BindingRename],
    cm: Lrc<SourceMap>,
) -> Result<Option<String>> {
    let items = support.module_items();
    if items.is_empty() {
        return Ok(None);
    }
    let mut module = Module {
        span: DUMMY_SP,
        body: items,
        shebang: None,
    };
    if !renames.is_empty() {
        rename_bindings(&mut module, renames);
    }
    print_module(&module, cm).map(Some)
}

fn append_unresolved_symbols(source: &mut String, support: &ArtifactSupportPlan) {
    let unresolved = support.unresolved_symbols();
    if unresolved.is_empty() {
        return;
    }
    source.push_str("\n// Unresolved artifact-local symbols: ");
    source.push_str(&unresolved.join(", "));
    source.push('\n');
}

fn component_source_fragment(
    input: ModuleComponentEmitInput<'_>,
    renames: &[BindingRename],
    cm: Lrc<SourceMap>,
) -> Result<String> {
    let mut class = Box::new(input.class.clone());
    let template_source = materialize_listener_methods(
        class.as_mut(),
        input.template_source,
        input.listener_methods,
    );
    if !renames.is_empty() {
        rename_bindings(class.as_mut(), renames);
    }
    let class_source = print_component_class(input.name, class, cm)?;
    let mut metadata = String::new();
    metadata.push_str("@Component({\n");
    metadata.push_str("  selector: ");
    metadata.push_str(&quoted_js_string(input.selector));
    metadata.push_str(",\n");
    metadata.push_str("  template: `\n");
    metadata.push_str(&indent_template_literal(&template_source, 4));
    metadata.push_str("\n  `,\n");
    if !input.styles.is_empty() {
        metadata.push_str("  styles: [\n");
        for style in input.styles {
            metadata.push_str("    `\n");
            metadata.push_str(&indent_template_literal(&recover_scoped_styles(style), 6));
            metadata.push_str("\n    `,\n");
        }
        metadata.push_str("  ],\n");
    }
    if !input.dependencies.is_empty() {
        metadata.push_str("  imports: [");
        metadata.push_str(&input.dependencies.join(", "));
        metadata.push_str("],\n");
    }
    metadata.push_str("})\n");

    metadata.push_str(&class_source);
    Ok(metadata)
}

fn materialize_listener_methods(
    class: &mut Class,
    template_source: &str,
    methods: &[RecoveredListenerMethod],
) -> String {
    if methods.is_empty() {
        return template_source.to_string();
    }

    // Listener methods carry evidence-context bindings so module-level Closure
    // renames can still reach their helper references. Attaching them here,
    // after support planning, avoids mixing those bindings into readable-class
    // root discovery.
    let mut occupied_names = IdentifierNameCollector::default();
    class.visit_with(&mut occupied_names);
    occupied_names
        .names
        .extend(class.body.iter().filter_map(|member| {
            let name = match member {
                ClassMember::Method(method) => prop_name(&method.key),
                ClassMember::ClassProp(property) => prop_name(&property.key),
                ClassMember::AutoAccessor(accessor) => {
                    let swc_core::ecma::ast::Key::Public(key) = &accessor.key else {
                        return None;
                    };
                    prop_name(key)
                }
                _ => None,
            };
            name.map(Atom::from)
        }));
    let mut template_source = template_source.to_string();
    for method in methods {
        let mut name = method.preferred_name.clone();
        let mut suffix = 2usize;
        while occupied_names.names.contains(&Atom::from(name.as_str())) {
            name = format!("{}{}", method.preferred_name, suffix);
            suffix += 1;
        }
        occupied_names.names.insert(Atom::from(name.as_str()));
        template_source = template_source.replace(&method.placeholder, &name);
        class.body.push(ClassMember::Method(ClassMethod {
            span: DUMMY_SP,
            key: PropName::Ident(IdentName::new(Atom::from(name.as_str()), DUMMY_SP)),
            function: Box::new(method.function.clone()),
            kind: MethodKind::Method,
            is_static: false,
            accessibility: None,
            is_abstract: false,
            is_optional: false,
            is_override: false,
        }));
    }
    template_source
}

pub(super) fn clean_component_class(
    class: &Class,
    definition_field: Option<&Atom>,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
) -> Box<Class> {
    let mut class = Box::new(class.clone());
    class.decorators.clear();
    class.body.retain(|member| match member {
        ClassMember::ClassProp(property) if property.is_static => {
            let canonical_ivy_field =
                prop_name(&property.key).is_some_and(|name| name.starts_with('ɵ'));
            let assigned_component_field = definition_field
                .is_some_and(|field| prop_name(&property.key).as_deref() == Some(field.as_ref()));
            let component_initializer = property.value.as_deref().is_some_and(|value| {
                let Expr::Call(call) = value else {
                    return false;
                };
                roles.instruction_for_callee(&call.callee, unresolved_ctxt)
                    == Some(IvyInstruction::DefineComponent)
            });
            !canonical_ivy_field && !assigned_component_field && !component_initializer
        }
        _ => true,
    });
    class
}

pub(super) fn recover_component_class_apis(
    class: &Class,
    evidence_class: &Class,
    queries: &[ComponentQueryMetadata],
    roles: &IvyRoleTable,
    evidence_unresolved_ctxt: SyntaxContext,
    readable_unresolved_ctxt: SyntaxContext,
) -> (Box<Class>, Vec<AngularClassApi>, HashSet<BindingKey>) {
    let mut occupied_names = IdentifierNameCollector::default();
    class.visit_with(&mut occupied_names);

    let mut class = Box::new(class.clone());
    let mut rewriter = ClassApiRewriter {
        roles,
        unresolved_ctxt: readable_unresolved_ctxt,
        occupied_names: &occupied_names.names,
        imports: BTreeSet::new(),
    };
    let plans = query_rewrite_plans(evidence_class, queries, roles, evidence_unresolved_ctxt);
    let query_references = rewriter.rewrite_query_initializers(&mut class, &plans);
    class.visit_mut_with(&mut rewriter);
    (
        class,
        rewriter.imports.into_iter().collect(),
        query_references,
    )
}

#[derive(Clone)]
struct QueryRewritePlan {
    api: AngularClassApi,
    required: bool,
    arguments: Vec<ExprOrSpread>,
    reuse_initializer_arguments: bool,
}

fn query_rewrite_plans(
    evidence_class: &Class,
    queries: &[ComponentQueryMetadata],
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
) -> HashMap<Atom, QueryRewritePlan> {
    let metadata = queries
        .iter()
        .map(|query| (query.field.clone(), query))
        .collect::<HashMap<_, _>>();
    let mut plans = HashMap::new();
    let mut ambiguous = HashSet::new();

    let mut record = |field: Atom, initializer: &CallExpr| {
        let Some(query) = metadata.get(&field) else {
            return;
        };
        if initializer.type_args.is_some() {
            return;
        }
        let Some(query_initializer) =
            roles.query_initializer_for_call(initializer, unresolved_ctxt)
        else {
            return;
        };
        if query_initializer
            .owner
            .is_some_and(|owner| owner != query.owner)
            || (query_initializer.multiple && query_initializer.required)
        {
            return;
        }
        let api = match (query.owner, query_initializer.multiple) {
            (AngularQueryOwner::View, false) => AngularClassApi::ViewChild,
            (AngularQueryOwner::View, true) => AngularClassApi::ViewChildren,
            (AngularQueryOwner::Content, false) => AngularClassApi::ContentChild,
            (AngularQueryOwner::Content, true) => AngularClassApi::ContentChildren,
        };
        let arguments = query_source_arguments(query, query_initializer.multiple);
        let plan = QueryRewritePlan {
            api,
            required: query_initializer.required,
            reuse_initializer_arguments: query_arguments_match(&arguments, &initializer.args),
            arguments,
        };
        if plans.insert(field.clone(), plan).is_some() {
            plans.remove(&field);
            ambiguous.insert(field);
        }
    };

    for member in &evidence_class.body {
        match member {
            ClassMember::ClassProp(property) if !property.is_static => {
                let Some(field) = prop_name(&property.key).map(Atom::from) else {
                    continue;
                };
                let Some(Expr::Call(initializer)) = property.value.as_deref() else {
                    continue;
                };
                record(field, initializer);
            }
            ClassMember::Constructor(constructor) => {
                let Some(body) = &constructor.body else {
                    continue;
                };
                for statement in &body.stmts {
                    if let Some((field, initializer)) = constructor_query_initializer(statement) {
                        record(field, initializer);
                    }
                }
            }
            _ => {}
        }
    }
    plans.retain(|field, _| !ambiguous.contains(field));
    plans
}

fn query_arguments_match(metadata: &[ExprOrSpread], initializer: &[ExprOrSpread]) -> bool {
    metadata.len() == initializer.len()
        && metadata
            .iter()
            .zip(initializer)
            .all(|(metadata, initializer)| {
                metadata.spread == initializer.spread
                    && metadata
                        .expr
                        .as_ref()
                        .eq_ignore_span(initializer.expr.as_ref())
            })
}

fn constructor_query_initializer(statement: &Stmt) -> Option<(Atom, &CallExpr)> {
    let Stmt::Expr(statement) = statement else {
        return None;
    };
    let Expr::Assign(assignment) = statement.expr.as_ref() else {
        return None;
    };
    if assignment.op != swc_core::ecma::ast::AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assignment.left else {
        return None;
    };
    if !matches!(target.obj.as_ref(), Expr::This(_)) {
        return None;
    }
    let field = member_prop_name(&target.prop)?;
    let Expr::Call(initializer) = assignment.right.as_ref() else {
        return None;
    };
    Some((field, initializer))
}

fn constructor_query_initializer_mut(statement: &mut Stmt) -> Option<(Atom, &mut CallExpr)> {
    let Stmt::Expr(statement) = statement else {
        return None;
    };
    let Expr::Assign(assignment) = statement.expr.as_mut() else {
        return None;
    };
    if assignment.op != swc_core::ecma::ast::AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assignment.left else {
        return None;
    };
    if !matches!(target.obj.as_ref(), Expr::This(_)) {
        return None;
    }
    let field = member_prop_name(&target.prop)?;
    let Expr::Call(initializer) = assignment.right.as_mut() else {
        return None;
    };
    Some((field, initializer))
}

fn query_source_arguments(query: &ComponentQueryMetadata, multiple: bool) -> Vec<ExprOrSpread> {
    let mut arguments = vec![ExprOrSpread {
        spread: None,
        expr: query.locator.clone(),
    }];
    let mut options = Vec::new();
    if query.owner == AngularQueryOwner::Content {
        let default_descendants = !multiple;
        if query.descendants != default_descendants {
            options.push(query_option(
                "descendants",
                Box::new(Expr::Lit(Lit::Bool(swc_core::ecma::ast::Bool {
                    span: DUMMY_SP,
                    value: query.descendants,
                }))),
            ));
        }
    }
    if let Some(read) = &query.read {
        options.push(query_option("read", read.clone()));
    }
    if !options.is_empty() {
        arguments.push(ExprOrSpread {
            spread: None,
            expr: Box::new(Expr::Object(ObjectLit {
                span: DUMMY_SP,
                props: options,
            })),
        });
    }
    arguments
}

fn query_option(name: &str, value: Box<Expr>) -> PropOrSpread {
    PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
        key: PropName::Ident(IdentName::new(Atom::from(name), DUMMY_SP)),
        value,
    })))
}

#[derive(Default)]
struct IdentifierNameCollector {
    names: HashSet<Atom>,
}

impl Visit for IdentifierNameCollector {
    fn visit_ident(&mut self, identifier: &Ident) {
        self.names.insert(identifier.sym.clone());
    }
}

struct ClassApiRewriter<'a> {
    roles: &'a IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
    occupied_names: &'a HashSet<Atom>,
    imports: BTreeSet<AngularClassApi>,
}

impl ClassApiRewriter<'_> {
    fn rewrite_query_initializers(
        &mut self,
        class: &mut Class,
        plans: &HashMap<Atom, QueryRewritePlan>,
    ) -> HashSet<BindingKey> {
        let mut field_counts = HashMap::<Atom, usize>::new();
        for member in &class.body {
            match member {
                ClassMember::ClassProp(property) if !property.is_static => {
                    if let Some(field) = prop_name(&property.key).map(Atom::from) {
                        *field_counts.entry(field).or_default() += 1;
                    }
                }
                ClassMember::Constructor(constructor) => {
                    let Some(body) = &constructor.body else {
                        continue;
                    };
                    for statement in &body.stmts {
                        if let Some((field, _)) = constructor_query_initializer(statement) {
                            *field_counts.entry(field).or_default() += 1;
                        }
                    }
                }
                _ => {}
            }
        }

        let mut references = HashSet::new();
        for member in &mut class.body {
            match member {
                ClassMember::ClassProp(property) if !property.is_static => {
                    let Some(field) = prop_name(&property.key).map(Atom::from) else {
                        continue;
                    };
                    if field_counts.get(&field) != Some(&1) {
                        continue;
                    }
                    let Some(plan) = plans.get(&field) else {
                        continue;
                    };
                    let Some(Expr::Call(call)) = property.value.as_deref_mut() else {
                        continue;
                    };
                    self.rewrite_query_call(call, plan, &mut references);
                }
                ClassMember::Constructor(constructor) => {
                    let Some(body) = &mut constructor.body else {
                        continue;
                    };
                    for statement in &mut body.stmts {
                        let Some((field, call)) = constructor_query_initializer_mut(statement)
                        else {
                            continue;
                        };
                        if field_counts.get(&field) != Some(&1) {
                            continue;
                        }
                        let Some(plan) = plans.get(&field) else {
                            continue;
                        };
                        self.rewrite_query_call(call, plan, &mut references);
                    }
                }
                _ => {}
            }
        }
        references
    }

    fn rewrite_query_call(
        &mut self,
        call: &mut CallExpr,
        plan: &QueryRewritePlan,
        references: &mut HashSet<BindingKey>,
    ) {
        if call.type_args.is_some() {
            return;
        }
        let Some(identifier) = self.imported_identifier(plan.api, call.span) else {
            return;
        };
        let callee = if plan.required {
            Expr::Member(MemberExpr {
                span: call.span,
                obj: Box::new(Expr::Ident(identifier)),
                prop: MemberProp::Ident(IdentName::new(Atom::from("required"), call.span)),
            })
        } else {
            Expr::Ident(identifier)
        };
        let readable_arguments = (plan.reuse_initializer_arguments
            && call.args.len() == plan.arguments.len())
        .then(|| call.args.clone());
        call.callee = swc_core::ecma::ast::Callee::Expr(Box::new(callee));
        if let Some(readable_arguments) = readable_arguments {
            call.args = readable_arguments;
        } else {
            call.args = plan.arguments.clone();
            for argument in &call.args {
                references.extend(expression_references(argument.expr.as_ref()));
            }
        }
    }
}

impl VisitMut for ClassApiRewriter<'_> {
    fn visit_mut_expr(&mut self, expression: &mut Expr) {
        expression.visit_mut_children_with(self);
        let Expr::New(created) = expression else {
            return;
        };
        if self
            .roles
            .class_api_for_expr(created.callee.as_ref(), self.unresolved_ctxt)
            != Some(AngularClassApi::Output)
            || created
                .args
                .as_ref()
                .is_some_and(|arguments| !arguments.is_empty())
            || created.type_args.is_some()
        {
            return;
        }
        let Some(identifier) =
            self.imported_identifier(AngularClassApi::Output, created.callee.span())
        else {
            return;
        };
        *expression = Expr::Call(CallExpr {
            span: created.span,
            ctxt: created.ctxt,
            callee: swc_core::ecma::ast::Callee::Expr(Box::new(Expr::Ident(identifier))),
            args: Vec::new(),
            type_args: None,
        });
    }

    fn visit_mut_call_expr(&mut self, call: &mut swc_core::ecma::ast::CallExpr) {
        call.visit_mut_children_with(self);
        let specialized_arguments = self
            .roles
            .specialized_class_api_arguments_for_callee(&call.callee, self.unresolved_ctxt);
        if let Some(api) = self
            .roles
            .class_api_for_callee(&call.callee, self.unresolved_ctxt)
        {
            if api.is_query() {
                return;
            }
            let swc_core::ecma::ast::Callee::Expr(callee) = &mut call.callee else {
                return;
            };
            if let Some(identifier) = self.imported_identifier(api, callee.span()) {
                **callee = Expr::Ident(identifier);
                if call.args.is_empty() {
                    if let Some(arguments) = specialized_arguments {
                        call.args = arguments
                            .into_iter()
                            .map(|expr| ExprOrSpread { spread: None, expr })
                            .collect();
                    }
                }
            }
            return;
        }

        let swc_core::ecma::ast::Callee::Expr(callee) = &mut call.callee else {
            return;
        };
        let Expr::Member(member) = callee.as_mut() else {
            return;
        };
        if member_prop_name(&member.prop).as_deref() != Some("required") {
            return;
        }
        let Some(api @ (AngularClassApi::Input | AngularClassApi::Model)) = self
            .roles
            .class_api_for_expr(member.obj.as_ref(), self.unresolved_ctxt)
        else {
            return;
        };
        if let Some(identifier) = self.imported_identifier(api, member.obj.span()) {
            *member.obj = Expr::Ident(identifier);
        }
    }
}

impl ClassApiRewriter<'_> {
    fn imported_identifier(
        &mut self,
        api: AngularClassApi,
        span: swc_core::common::Span,
    ) -> Option<Ident> {
        let name = Atom::from(api.canonical_export_name());
        if self.occupied_names.contains(&name) {
            return None;
        }
        self.imports.insert(api);
        Some(Ident::new(name, span, SyntaxContext::empty()))
    }
}

fn print_component_class(name: &str, class: Box<Class>, cm: Lrc<SourceMap>) -> Result<String> {
    let module = Module {
        span: DUMMY_SP,
        body: vec![ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(
            swc_core::ecma::ast::ExportDecl {
                span: DUMMY_SP,
                decl: Decl::Class(ClassDecl {
                    ident: Ident::new(Atom::from(name), DUMMY_SP, SyntaxContext::empty()),
                    declare: false,
                    class,
                }),
            },
        ))],
        shebang: None,
    };
    print_module(&module, cm)
}

pub(super) fn handler_expression(
    expression: &Expr,
    component_contexts: &HashSet<BindingKey>,
    local_references: &HashMap<BindingKey, String>,
    cm: Lrc<SourceMap>,
) -> Result<String> {
    let mut expression = expression.clone();
    restore_event_parameter(&mut expression);
    match &expression {
        Expr::Fn(function) => {
            let body = function.function.body.as_ref();
            if let Some(expression) = body.and_then(single_return_expression) {
                print_template_expression(expression, component_contexts, local_references, cm)
            } else if let Some(expressions) = body.and_then(handler_effect_expressions) {
                print_handler_effects(&expressions, component_contexts, local_references, cm)
            } else {
                print_expression(&expression, cm)
            }
        }
        Expr::Arrow(arrow) => match arrow.body.as_ref() {
            BlockStmtOrExpr::Expr(expression) => {
                print_template_expression(expression, component_contexts, local_references, cm)
            }
            BlockStmtOrExpr::BlockStmt(block) => {
                if let Some(expression) = single_return_expression(block) {
                    print_template_expression(expression, component_contexts, local_references, cm)
                } else if let Some(expressions) = handler_effect_expressions(block) {
                    print_handler_effects(&expressions, component_contexts, local_references, cm)
                } else {
                    print_expression(&expression, cm)
                }
            }
        },
        _ => print_template_expression(&expression, component_contexts, local_references, cm),
    }
}

fn restore_event_parameter(expression: &mut Expr) {
    let parameter = match expression {
        Expr::Fn(function) => function.function.params.first().map(|param| &param.pat),
        Expr::Arrow(arrow) => arrow.params.first(),
        _ => None,
    };
    let Some(Pat::Ident(parameter)) = parameter else {
        return;
    };
    let old = binding_key(&parameter.id);
    if old.0.as_ref() == "$event" {
        return;
    }
    rename_bindings(
        expression,
        &[BindingRename {
            old,
            new: Atom::from("$event"),
        }],
    );
}

fn single_return_expression(block: &swc_core::ecma::ast::BlockStmt) -> Option<&Expr> {
    let [Stmt::Return(ReturnStmt {
        arg: Some(expression),
        ..
    })] = block.stmts.as_slice()
    else {
        return None;
    };
    Some(expression.as_ref())
}

fn handler_effect_expressions(block: &swc_core::ecma::ast::BlockStmt) -> Option<Vec<&Expr>> {
    let mut expressions = Vec::new();
    for (index, statement) in block.stmts.iter().enumerate() {
        match statement {
            Stmt::Expr(expression) => expressions.push(expression.expr.as_ref()),
            Stmt::Return(ReturnStmt { arg, .. })
                if block.stmts[index + 1..]
                    .iter()
                    .all(|statement| matches!(statement, Stmt::Empty(_))) =>
            {
                if let Some(expression) = arg {
                    expressions.push(expression.as_ref());
                }
            }
            Stmt::Empty(_) => {}
            _ => return None,
        }
    }
    (!expressions.is_empty()).then_some(expressions)
}

fn print_handler_effects(
    expressions: &[&Expr],
    component_contexts: &HashSet<BindingKey>,
    local_references: &HashMap<BindingKey, String>,
    cm: Lrc<SourceMap>,
) -> Result<String> {
    expressions
        .iter()
        .map(|expression| {
            print_template_expression(expression, component_contexts, local_references, cm.clone())
        })
        .collect::<Result<Vec<_>>>()
        .map(|expressions| expressions.join("; "))
}

pub(super) fn print_template_expression(
    expression: &Expr,
    component_contexts: &HashSet<BindingKey>,
    local_references: &HashMap<BindingKey, String>,
    cm: Lrc<SourceMap>,
) -> Result<String> {
    print_template_expression_with_aliases(
        expression,
        component_contexts,
        local_references,
        &HashMap::new(),
        &HashMap::new(),
        cm,
    )
}

pub(super) fn print_template_expression_with_aliases(
    expression: &Expr,
    component_contexts: &HashSet<BindingKey>,
    local_references: &HashMap<BindingKey, String>,
    expression_aliases: &HashMap<BindingKey, Box<Expr>>,
    local_contexts: &HashMap<BindingKey, HashMap<String, String>>,
    cm: Lrc<SourceMap>,
) -> Result<String> {
    let mut expression = expression.clone();
    if !expression_aliases.is_empty() {
        expression.visit_mut_with(&mut TemplateExpressionAliasResolver {
            aliases: expression_aliases,
            active: HashSet::new(),
        });
    }
    if !component_contexts.is_empty() || !local_references.is_empty() || !local_contexts.is_empty()
    {
        expression.visit_mut_with(&mut TemplateBindingCleaner {
            contexts: component_contexts,
            local_references,
            local_contexts,
        });
    }
    print_expression(&expression, cm)
}

pub(super) struct TemplateExpressionAliasResolver<'a> {
    pub(super) aliases: &'a HashMap<BindingKey, Box<Expr>>,
    pub(super) active: HashSet<BindingKey>,
}

impl VisitMut for TemplateExpressionAliasResolver<'_> {
    fn visit_mut_expr(&mut self, expression: &mut Expr) {
        let Expr::Ident(identifier) = expression else {
            expression.visit_mut_children_with(self);
            return;
        };
        let key = binding_key(identifier);
        let Some(alias) = self.aliases.get(&key) else {
            return;
        };
        if !self.active.insert(key.clone()) {
            return;
        }
        let mut alias = alias.as_ref().clone();
        alias.visit_mut_with(self);
        self.active.remove(&key);
        *expression = alias;
    }
}

fn print_expression(expression: &Expr, cm: Lrc<SourceMap>) -> Result<String> {
    let module = Module {
        span: DUMMY_SP,
        body: vec![ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
            span: DUMMY_SP,
            ctxt: SyntaxContext::empty(),
            kind: VarDeclKind::Const,
            declare: false,
            decls: vec![VarDeclarator {
                span: DUMMY_SP,
                name: Pat::Ident(BindingIdent {
                    id: Ident::new("__wakaru_ivy_expr".into(), DUMMY_SP, SyntaxContext::empty()),
                    type_ann: None,
                }),
                init: Some(Box::new(expression.clone())),
                definite: false,
            }],
        }))))],
        shebang: None,
    };
    let source = print_module(&module, cm)?;
    Ok(source
        .trim()
        .strip_prefix("const __wakaru_ivy_expr = ")
        .unwrap_or(source.trim())
        .trim_end_matches(';')
        .trim()
        .to_string())
}

fn print_module(module: &Module, cm: Lrc<SourceMap>) -> Result<String> {
    let mut output = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: Config::default().with_minify(false),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm, "\n", &mut output, None),
        };
        emitter
            .emit_module(module)
            .map_err(|error| anyhow!("failed to print Angular artifact: {error:?}"))?;
    }
    String::from_utf8(output)
        .map(|source| source.trim().to_string())
        .map_err(|error| anyhow!("Angular artifact is not UTF-8: {error}"))
}

struct TemplateBindingCleaner<'a> {
    contexts: &'a HashSet<BindingKey>,
    local_references: &'a HashMap<BindingKey, String>,
    local_contexts: &'a HashMap<BindingKey, HashMap<String, String>>,
}

impl VisitMut for TemplateBindingCleaner<'_> {
    fn visit_mut_assign_expr(&mut self, assignment: &mut AssignExpr) {
        assignment.right.visit_mut_with(self);
        let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assignment.left else {
            assignment.left.visit_mut_children_with(self);
            return;
        };
        let Expr::Ident(object) = member.obj.as_ref() else {
            assignment.left.visit_mut_children_with(self);
            return;
        };
        let swc_core::ecma::ast::MemberProp::Ident(property) = &member.prop else {
            assignment.left.visit_mut_children_with(self);
            return;
        };
        let Some(recovered_property) = self.recovered_context_property(object, property) else {
            assignment.left.visit_mut_children_with(self);
            return;
        };
        assignment.left = AssignTarget::Simple(SimpleAssignTarget::Ident(BindingIdent {
            id: Ident::new(
                recovered_property.into(),
                property.span,
                SyntaxContext::empty(),
            ),
            type_ann: None,
        }));
    }

    fn visit_mut_expr(&mut self, expression: &mut Expr) {
        expression.visit_mut_children_with(self);
        if let Expr::Ident(identifier) = expression {
            if let Some(name) = self.local_references.get(&binding_key(identifier)) {
                *identifier = Ident::new(
                    Atom::from(name.as_str()),
                    identifier.span,
                    SyntaxContext::empty(),
                );
            }
            return;
        }
        let Expr::Member(member) = expression else {
            return;
        };
        let Expr::Ident(object) = member.obj.as_ref() else {
            return;
        };
        let swc_core::ecma::ast::MemberProp::Ident(property) = &member.prop else {
            return;
        };
        let recovered_property = self.recovered_context_property(object, property);
        let Some(recovered_property) = recovered_property else {
            return;
        };
        *expression = Expr::Ident(Ident::new(
            recovered_property.into(),
            property.span,
            SyntaxContext::empty(),
        ));
    }
}

impl TemplateBindingCleaner<'_> {
    fn recovered_context_property(&self, object: &Ident, property: &IdentName) -> Option<String> {
        let object_key = binding_key(object);
        if self.contexts.contains(&object_key) {
            Some(property.sym.to_string())
        } else {
            self.local_contexts
                .get(&object_key)
                .and_then(|properties| properties.get(property.sym.as_ref()))
                .cloned()
        }
    }
}

fn recover_scoped_styles(style: &str) -> String {
    style
        .replace("[_nghost-%COMP%]", ":host")
        .replace("[_ngcontent-%COMP%]", "")
        .trim()
        .to_string()
}

fn indent_template_literal(value: &str, spaces: usize) -> String {
    let indent = " ".repeat(spaces);
    value
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
        .lines()
        .map(|line| format!("{indent}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn quoted_js_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::indent_template_literal;

    #[test]
    fn escapes_backslashes_before_template_interpolation_markers() {
        assert_eq!(
            indent_template_literal(r"\${globalThis.injected = true}`", 0),
            r"\\\${globalThis.injected = true}\`"
        );
    }
}
