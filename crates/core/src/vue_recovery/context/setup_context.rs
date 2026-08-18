use super::*;

pub(in crate::vue_recovery) fn collect_render_context(
    render: RenderSource<'_>,
    ctx: &mut VueRecoveryContext,
) {
    let Some(stmts) = render_stmts(render) else {
        return;
    };
    let mut slot_partition_bindings = HashSet::new();
    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        for decl in &var.decls {
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            match &decl.name {
                Pat::Ident(binding) => {
                    if let Some(component) = resolve_component_name(init, ctx) {
                        ctx.component_bindings
                            .insert(binding.id.sym.clone(), component);
                    }
                    if let Some(directive) = resolve_directive_name(init, ctx) {
                        ctx.directive_bindings
                            .insert(binding.id.sym.clone(), directive);
                    }
                    if is_slot_partition_expr(init, ctx) {
                        slot_partition_bindings.insert(binding.id.sym.clone());
                    }
                    if is_slot_partition_slots_alias(init, &slot_partition_bindings) {
                        ctx.slot_bindings.insert(binding.id.sym.clone());
                    }
                    if let Some(source) =
                        slot_partition_child_list_alias_source(init, &slot_partition_bindings)
                    {
                        insert_render_child_list_binding(ctx, binding.id.sym.clone(), source);
                    }
                    if let Some(slot) = render_slot_binding_expr(init, ctx) {
                        ctx.render_slot_bindings
                            .insert(binding.id.sym.clone(), slot);
                    }
                }
                Pat::Object(object)
                    if is_slot_partition_expr(init, ctx)
                        || is_slot_partition_alias(init, &slot_partition_bindings) =>
                {
                    collect_named_object_pat_bindings(object, "slots", &mut ctx.slot_bindings);
                    collect_slot_partition_child_list_bindings(object, ctx);
                }
                _ => {}
            }
        }
    }
}

#[derive(Clone, Copy)]
enum SetupRenderNode<'a> {
    Arrow(&'a ArrowExpr),
    Function(&'a FnDecl),
}

fn visit_setup_render<V: Visit>(render: SetupRenderNode<'_>, visitor: &mut V) {
    match render {
        SetupRenderNode::Arrow(render) => render.visit_with(visitor),
        SetupRenderNode::Function(render) => render.visit_with(visitor),
    }
}

// Vue's playground compiles `<script setup>` with `inlineTemplate` in development.
// That shape has no `__isScriptSetup` marker, so use its generated component name
// together with the setup props and context/cache render parameters. Their names are
// not stable because downstream minifiers may rename them.
fn is_compiled_inline_script_setup(render: RenderSource<'_>) -> bool {
    let RenderSource::SetupArrow {
        render,
        setup_props: Some(_),
        component_options: Some(options),
        ..
    } = render
    else {
        return false;
    };

    render.params.len() >= 2
        && options.props.iter().any(|prop| {
            matches!(
                prop,
                PropOrSpread::Prop(prop)
                    if match prop.as_ref() {
                        Prop::KeyValue(key_value) => {
                            prop_name(&key_value.key).as_deref() == Some("__name")
                        }
                        _ => false,
                    }
            )
        })
}

pub(in crate::vue_recovery) fn collect_setup_context(
    render: RenderSource<'_>,
    compiled_setup: Option<CompiledScriptSetup<'_>>,
    ctx: &mut VueRecoveryContext,
) -> Result<()> {
    let owned_setup_stmts: Vec<Stmt>;
    let preserve_inline_script_setup = is_compiled_inline_script_setup(render);
    let (render, setup_stmts, setup_slots, setup_expose, preserve_side_effects) = match render {
        RenderSource::SetupArrow {
            render,
            setup_stmts,
            setup_slots,
            ..
        } => (
            SetupRenderNode::Arrow(render),
            setup_stmts,
            setup_slots.map(|slots| slots.sym.clone()),
            None,
            preserve_inline_script_setup,
        ),
        RenderSource::Function { render, .. } => {
            let Some(setup) = compiled_setup else {
                return Ok(());
            };
            owned_setup_stmts = setup.setup_stmts.to_vec();
            (
                SetupRenderNode::Function(render),
                owned_setup_stmts.as_slice(),
                setup.setup_slots.map(|slots| slots.sym.clone()),
                setup
                    .setup_expose
                    .map(|expose| (expose.sym.clone(), expose.ctxt)),
                true,
            )
        }
    };
    if let Some(setup_slots) = setup_slots {
        ctx.slot_bindings.insert(setup_slots);
    }

    let setup_tuple_value_candidates = setup_tuple_value_candidates(setup_stmts);
    let setup_template_ref_refs =
        setup_render_template_ref_refs(render, setup_stmts, ctx, &setup_tuple_value_candidates);
    let setup_template_ref_aliases = setup_render_template_ref_aliases(render);
    let setup_template_ref_alias_sources = setup_template_ref_aliases
        .iter()
        .map(|(from, _, _)| from.clone())
        .collect::<HashSet<_>>();
    let setup_ref_object_alias_refs = setup_ref_object_alias_refs(setup_stmts);
    let setup_non_value_member_refs = setup_non_value_member_refs(setup_stmts);
    let setup_value_member_refs = setup_value_member_refs(render, setup_stmts);
    let setup_render_refs = render_ident_refs(render);
    let compiled_return_binding = preserve_side_effects
        .then(|| compiled_setup_return_binding(setup_stmts))
        .flatten();
    let compiled_return_values = compiled_return_binding
        .as_ref()
        .map(|binding| compiled_setup_return_values(setup_stmts, binding))
        .unwrap_or_default();
    let mut provider_ref_object_bindings = HashMap::new();
    let mut composable_ref_object_bindings = HashMap::new();
    let mut local_candidates = Vec::new();

    for (setup_order, stmt) in setup_stmts.iter().enumerate() {
        match stmt {
            Stmt::Decl(Decl::Fn(function)) => {
                local_candidates.push(SetupLocalCandidate {
                    bindings: vec![function.ident.sym.clone()],
                    stmt: stmt.clone(),
                    template_selectable: true,
                    setup_order,
                    always_emit: preserve_side_effects,
                    preserve_ref_values: preserve_side_effects,
                });
            }
            Stmt::Decl(Decl::Class(class)) => {
                local_candidates.push(SetupLocalCandidate {
                    bindings: vec![class.ident.sym.clone()],
                    stmt: stmt.clone(),
                    template_selectable: true,
                    setup_order,
                    always_emit: preserve_side_effects,
                    preserve_ref_values: preserve_side_effects,
                });
            }
            Stmt::Decl(Decl::Var(var)) => {
                let mut local_decls = Vec::new();
                let mut local_bindings = HashSet::new();

                for original_decl in &var.decls {
                    let rewritten_decl = match (&original_decl.name, original_decl.init.as_deref())
                    {
                        (Pat::Ident(binding), Some(init)) if is_undefined_placeholder(init) => {
                            compiled_return_values
                                .iter()
                                .find(|(name, _, _)| name == &binding.id.sym)
                                .map(|(_, init, _)| {
                                    let mut decl = original_decl.clone();
                                    decl.init = Some(init.clone());
                                    decl
                                })
                        }
                        _ => None,
                    };
                    let decl = rewritten_decl.as_ref().unwrap_or(original_decl);
                    if matches!(
                        (&decl.name, compiled_return_binding.as_ref()),
                        (Pat::Ident(binding), Some((name, ctxt)))
                            if binding.id.sym == *name && binding.id.ctxt == *ctxt
                    ) {
                        continue;
                    }
                    let consumed = match decl.init.as_deref() {
                        Some(init) => match &decl.name {
                            Pat::Ident(binding) => {
                                if record_compiled_setup_alias(
                                    &binding.id.sym,
                                    Some(binding.id.ctxt),
                                    init,
                                    ctx,
                                ) {
                                    true
                                } else if preserve_side_effects {
                                    if is_ref_like_value_expr(init, ctx) {
                                        ctx.bindings.refs.insert(binding.id.sym.clone());
                                    }
                                    false
                                } else if let Some(alias) = ident_expr(unwrap_paren_expr(init)) {
                                    ctx.bindings
                                        .aliases
                                        .insert(binding.id.sym.clone(), alias.sym.clone());
                                    ctx.bindings
                                        .alias_ctxts
                                        .insert(binding.id.sym.clone(), binding.id.ctxt);
                                    true
                                } else {
                                    if let Some(ref_props) =
                                        setup_ref_props(init, ctx, &provider_ref_object_bindings)
                                    {
                                        provider_ref_object_bindings
                                            .insert(binding.id.sym.clone(), ref_props);
                                    }
                                    if let Some(ref_props) = setup_composable_ref_props(
                                        init,
                                        ctx,
                                        &composable_ref_object_bindings,
                                    ) {
                                        composable_ref_object_bindings
                                            .insert(binding.id.sym.clone(), ref_props);
                                    }
                                    let is_ref_object = is_ref_object_expr(init, ctx);
                                    if is_ref_object {
                                        ctx.bindings.ref_objects.insert(binding.id.sym.clone());
                                    }
                                    let is_ref_object_alias_source = is_ref_object
                                        && setup_ref_object_alias_refs.contains(&binding.id.sym);
                                    if setup_value_member_refs
                                        .contains(&(binding.id.sym.clone(), binding.id.ctxt))
                                        && is_ref_member_extraction_expr(
                                            init,
                                            ctx,
                                            &provider_ref_object_bindings,
                                        )
                                    {
                                        ctx.bindings.refs.insert(binding.id.sym.clone());
                                        if is_composable_ref_member_extraction_expr(
                                            init,
                                            ctx,
                                            &composable_ref_object_bindings,
                                        ) {
                                            ctx.bindings
                                                .composable_refs
                                                .insert(binding.id.sym.clone());
                                        }
                                    }
                                    if let Some(value) = computed_value_expr(init, ctx)? {
                                        if setup_value_member_refs
                                            .contains(&(binding.id.sym.clone(), binding.id.ctxt))
                                        {
                                            collect_setup_value_template_tuple_refs(
                                                &value,
                                                &setup_tuple_value_candidates,
                                                ctx,
                                            );
                                        }
                                        ctx.bindings.values.insert(binding.id.sym.clone(), value);
                                        let mut local_var = var.as_ref().clone();
                                        local_var.decls = vec![decl.clone()];
                                        local_candidates.push(SetupLocalCandidate {
                                            bindings: vec![binding.id.sym.clone()],
                                            stmt: Stmt::Decl(Decl::Var(Box::new(local_var))),
                                            template_selectable: false,
                                            setup_order,
                                            always_emit: false,
                                            preserve_ref_values: preserve_side_effects,
                                        });
                                        true
                                    } else if let Some((value, import_refs)) =
                                        computed_script_setup_expr(init, ctx)?
                                    {
                                        ctx.setup_script_import_refs.extend(import_refs);
                                        ctx.setup_script_bindings.push(VueSetupScriptBinding {
                                            binding: binding.id.sym.clone(),
                                            value,
                                            setup_order,
                                        });
                                        ctx.bindings.refs.insert(binding.id.sym.clone());
                                        true
                                    } else if (!is_ref_object_alias_source
                                        || setup_template_ref_alias_sources
                                            .contains(&binding.id.sym))
                                        && !setup_non_value_member_refs
                                            .contains(&(binding.id.sym.clone(), binding.id.ctxt))
                                        && (setup_template_ref_alias_sources
                                            .contains(&binding.id.sym)
                                            || should_emit_ref_script_setup_expr(
                                                init,
                                                ctx,
                                                &binding.id,
                                                &setup_value_member_refs,
                                            ))
                                    {
                                        if let Some((expr, helper, known_ref)) =
                                            ref_script_setup_expr(init, ctx)?
                                        {
                                            ctx.setup_ref_script_bindings.push(
                                                VueSetupRefBinding {
                                                    binding: binding.id.sym.clone(),
                                                    expr,
                                                    helper,
                                                    known_ref,
                                                },
                                            );
                                        }
                                        ctx.bindings.refs.insert(binding.id.sym.clone());
                                        true
                                    } else {
                                        false
                                    }
                                }
                            }
                            Pat::Object(object) if is_setup_context_alias(init, ctx) => {
                                collect_named_object_pat_bindings(
                                    object,
                                    "slots",
                                    &mut ctx.slot_bindings,
                                );
                                false
                            }
                            Pat::Object(_) if is_setup_props_alias(init, ctx) => true,
                            Pat::Object(object)
                                if is_ref_object_expr(init, ctx)
                                    || is_ref_object_alias(init, ctx) =>
                            {
                                collect_object_pat_bindings(object, &mut ctx.bindings.refs);
                                false
                            }
                            Pat::Object(object) => {
                                if let Some(ref_props) =
                                    setup_ref_props(init, ctx, &provider_ref_object_bindings)
                                {
                                    collect_provider_object_pat_bindings(
                                        object,
                                        &ref_props,
                                        &mut ctx.bindings.refs,
                                    );
                                }
                                if let Some(ref_props) = setup_composable_ref_props(
                                    init,
                                    ctx,
                                    &composable_ref_object_bindings,
                                ) {
                                    collect_provider_object_pat_bindings(
                                        object,
                                        &ref_props,
                                        &mut ctx.bindings.composable_refs,
                                    );
                                }
                                false
                            }
                            _ => false,
                        },
                        None => false,
                    };

                    if consumed {
                        continue;
                    }
                    let mut decl_bindings = HashSet::new();
                    collect_pat_bindings(&decl.name, &mut decl_bindings);
                    if decl_bindings.is_empty() {
                        continue;
                    }
                    let has_template_ref = decl_bindings
                        .iter()
                        .any(|binding| setup_template_ref_refs.contains(binding));
                    let has_render_ref = decl_bindings
                        .iter()
                        .any(|binding| setup_render_refs.contains(binding));
                    let is_ref_object_local = decl.init.as_deref().is_some_and(|init| {
                        is_ref_object_expr(init, ctx) || is_ref_object_alias(init, ctx)
                    });
                    let is_imported_call_local = decl
                        .init
                        .as_deref()
                        .is_some_and(|init| is_script_import_call_expr(init, ctx));
                    let is_provider_ref_local = decl.init.as_deref().is_some_and(|init| {
                        setup_ref_props(init, ctx, &provider_ref_object_bindings).is_some()
                    });
                    let is_local_candidate = match &decl.name {
                        Pat::Ident(_) | Pat::Array(_) => true,
                        Pat::Object(_) => {
                            preserve_side_effects
                                || has_template_ref
                                || has_render_ref
                                || is_ref_object_local
                                || is_imported_call_local
                                || is_provider_ref_local
                        }
                        _ => false,
                    };
                    if !is_local_candidate {
                        continue;
                    }
                    if has_template_ref && matches!(decl.name, Pat::Object(_)) {
                        ctx.bindings
                            .template_refs
                            .extend(decl_bindings.iter().cloned());
                    } else {
                        ctx.bindings.template_refs.extend(
                            decl_bindings
                                .iter()
                                .filter(|binding| setup_template_ref_refs.contains(*binding))
                                .cloned(),
                        );
                    }

                    local_bindings.extend(decl_bindings);
                    local_decls.push(decl.clone());
                }

                if !local_decls.is_empty() {
                    let mut bindings = local_bindings.into_iter().collect::<Vec<_>>();
                    bindings.sort_by(|left, right| left.as_ref().cmp(right.as_ref()));
                    bindings.dedup();
                    let mut local_var = var.as_ref().clone();
                    local_var.decls = local_decls;
                    local_candidates.push(SetupLocalCandidate {
                        bindings,
                        stmt: Stmt::Decl(Decl::Var(Box::new(local_var))),
                        template_selectable: true,
                        setup_order,
                        always_emit: preserve_side_effects,
                        preserve_ref_values: preserve_side_effects,
                    });
                }
            }
            _ if preserve_side_effects
                && !matches!(stmt, Stmt::Empty(_) | Stmt::Return(_))
                && !is_compiled_setup_artifact_stmt(stmt, setup_expose.as_ref()) =>
            {
                local_candidates.push(SetupLocalCandidate {
                    bindings: Vec::new(),
                    stmt: stmt.clone(),
                    template_selectable: false,
                    setup_order,
                    always_emit: true,
                    preserve_ref_values: true,
                });
            }
            _ => {}
        }
    }

    let mut candidate_bindings = local_candidates
        .iter()
        .flat_map(|candidate| candidate.bindings.iter().cloned())
        .collect::<HashSet<_>>();
    for (binding, init, setup_order) in compiled_return_values {
        if record_compiled_setup_alias(&binding, None, init.as_ref(), ctx) {
            continue;
        }
        if candidate_bindings.contains(&binding)
            || ctx.top_level_binding_ctxts.contains_key(&binding)
        {
            continue;
        }
        if is_ref_like_value_expr(init.as_ref(), ctx) {
            ctx.bindings.refs.insert(binding.clone());
        }
        let stmt = Stmt::Decl(Decl::Var(Box::new(VarDecl {
            span: DUMMY_SP,
            ctxt: Default::default(),
            kind: VarDeclKind::Const,
            declare: false,
            decls: vec![VarDeclarator {
                span: DUMMY_SP,
                name: Pat::Ident(swc_core::ecma::ast::BindingIdent {
                    id: Ident::new(binding.clone(), DUMMY_SP, Default::default()),
                    type_ann: None,
                }),
                init: Some(init),
                definite: false,
            }],
        })));
        local_candidates.push(SetupLocalCandidate {
            bindings: vec![binding.clone()],
            stmt,
            template_selectable: true,
            setup_order,
            always_emit: true,
            preserve_ref_values: true,
        });
        candidate_bindings.insert(binding);
    }

    for (from, from_ctxt, to) in setup_template_ref_aliases {
        if ctx
            .setup_ref_script_bindings
            .iter()
            .any(|binding| binding.binding == from)
        {
            ctx.bindings.alias_ctxts.insert(from.clone(), from_ctxt);
            ctx.bindings.aliases.insert(from, to);
        }
    }
    refresh_setup_value_binding_sources(ctx)?;

    assign_setup_prop_bindings(ctx, &local_candidates);

    for candidate in local_candidates {
        let cleaned_stmt = if candidate.preserve_ref_values {
            clean_setup_stmt_preserving_ref_values(&candidate.stmt, ctx)
        } else {
            clean_setup_stmt(&candidate.stmt, ctx)
        };
        let source = print_clean_setup_stmt(&cleaned_stmt, ctx)?;
        if !source.is_empty() {
            let emitted_bindings = emitted_stmt_bindings(&source, ctx, &candidate.bindings);
            ctx.setup_local_bindings.push(VueSetupLocalBinding {
                bindings: candidate.bindings,
                emitted_bindings,
                refs: stmt_ident_refs(&cleaned_stmt),
                source,
                import_refs: stmt_import_refs(&cleaned_stmt, &ctx.script_imports),
                stmt: cleaned_stmt,
                module_scope: false,
                template_selectable: candidate.template_selectable,
                setup_order: candidate.setup_order,
                always_emit: candidate.always_emit,
                preserve_ref_values: candidate.preserve_ref_values,
            });
        }
    }

    Ok(())
}

fn setup_ref_object_alias_refs(stmts: &[Stmt]) -> HashSet<Atom> {
    let mut refs = HashSet::new();
    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        for decl in &var.decls {
            if !matches!(decl.name, Pat::Object(_)) {
                continue;
            }
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            if let Some(ident) = ident_expr(unwrap_paren_expr(init)) {
                refs.insert(ident.sym.clone());
            }
        }
    }
    refs
}

fn setup_non_value_member_refs(stmts: &[Stmt]) -> HashSet<(Atom, SyntaxContext)> {
    let mut collector = NonValueMemberRefCollector {
        refs: HashSet::new(),
    };
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
    collector.refs
}

fn setup_value_member_refs(
    render: SetupRenderNode<'_>,
    setup_stmts: &[Stmt],
) -> HashSet<(Atom, SyntaxContext)> {
    let mut collector = ValueMemberIdentRefCollector {
        refs: HashSet::new(),
    };
    for stmt in setup_stmts {
        stmt.visit_with(&mut collector);
    }
    visit_setup_render(render, &mut collector);
    collector.refs
}

/// Collects `(name, ctxt)` of every `<ident>.value` member base. Runs on the
/// pristine resolver-processed render/setup AST, so shadow safety comes from
/// `SyntaxContext` identity — a nested local reusing a setup binding's name
/// carries a different context and never matches the recorded binding. No
/// hand-rolled scope tracking is needed.
struct ValueMemberIdentRefCollector {
    refs: HashSet<(Atom, SyntaxContext)>,
}

impl Visit for ValueMemberIdentRefCollector {
    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "value") {
            if let Expr::Ident(object) = member.obj.as_ref() {
                self.refs.insert((object.sym.clone(), object.ctxt));
            }
        }
        member.visit_children_with(self);
    }
}

/// Collects `(name, ctxt)` of every non-`.value` member base
/// (`<ident>.<prop>`). Ctxt-keyed for the same reason as
/// [`ValueMemberIdentRefCollector`].
struct NonValueMemberRefCollector {
    refs: HashSet<(Atom, SyntaxContext)>,
}

impl Visit for NonValueMemberRefCollector {
    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "value") {
            if let Expr::Ident(object) = member.obj.as_ref() {
                self.refs.insert((object.sym.clone(), object.ctxt));
            }
        }
        member.visit_children_with(self);
    }
}

fn setup_render_template_ref_aliases(
    render: SetupRenderNode<'_>,
) -> Vec<(Atom, SyntaxContext, Atom)> {
    let mut collector = TemplateRefAliasCollector {
        aliases: Vec::new(),
    };
    visit_setup_render(render, &mut collector);
    collector.aliases
}

struct TemplateRefAliasCollector {
    aliases: Vec<(Atom, SyntaxContext, Atom)>,
}

impl Visit for TemplateRefAliasCollector {
    fn visit_object_lit(&mut self, object: &ObjectLit) {
        let mut ref_key = None;
        let mut ref_binding = None;

        for prop in &object.props {
            let PropOrSpread::Prop(prop) = prop else {
                continue;
            };
            let Prop::KeyValue(key_value) = prop.as_ref() else {
                continue;
            };
            match prop_name(&key_value.key).as_deref() {
                Some("ref_key") => {
                    ref_key = string_lit(key_value.value.as_ref())
                        .filter(|name| is_valid_identifier_name(name))
                        .map(Atom::from);
                }
                Some("ref") => {
                    if let Expr::Ident(ident) = unwrap_paren_expr(key_value.value.as_ref()) {
                        ref_binding = Some((ident.sym.clone(), ident.ctxt));
                    }
                }
                _ => {}
            }
        }

        if let (Some((from, from_ctxt)), Some(to)) = (ref_binding, ref_key) {
            self.aliases.push((from, from_ctxt, to));
        }

        object.visit_children_with(self);
    }
}

fn render_ident_refs(render: SetupRenderNode<'_>) -> HashSet<Atom> {
    let mut declared_collector = DeclaredBindingIdents {
        idents: HashSet::new(),
    };
    visit_setup_render(render, &mut declared_collector);
    let declared = declared_collector.idents;
    let mut collector = IdentRefCollector {
        declared: &declared,
        refs: HashSet::new(),
    };
    visit_setup_render(render, &mut collector);
    collector.refs
}

fn setup_tuple_value_candidates(setup_stmts: &[Stmt]) -> HashSet<(Atom, SyntaxContext)> {
    let mut tuple_value_candidates = HashSet::new();
    for stmt in setup_stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        for decl in &var.decls {
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            match &decl.name {
                Pat::Array(_) => {
                    collect_pat_binding_idents(&decl.name, &mut tuple_value_candidates)
                }
                Pat::Ident(binding) if is_tuple_element_expr(init) => {
                    tuple_value_candidates.insert((binding.id.sym.clone(), binding.id.ctxt));
                }
                _ => {}
            }
        }
    }
    tuple_value_candidates
}

fn setup_render_template_ref_refs(
    render: SetupRenderNode<'_>,
    setup_stmts: &[Stmt],
    ctx: &VueRecoveryContext,
    tuple_value_candidates: &HashSet<(Atom, SyntaxContext)>,
) -> HashSet<Atom> {
    let mut object_value_candidates = HashSet::new();
    let mut unref_candidates = HashSet::new();
    for stmt in setup_stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        for decl in &var.decls {
            let Some(_init) = decl.init.as_deref() else {
                continue;
            };
            if matches!(decl.name, Pat::Object(_)) {
                collect_pat_binding_idents(&decl.name, &mut object_value_candidates);
                collect_pat_binding_idents(&decl.name, &mut unref_candidates);
            }
        }
    }
    if tuple_value_candidates.is_empty()
        && (object_value_candidates.is_empty() || unref_candidates.is_empty())
    {
        return HashSet::new();
    }

    let mut collector = RenderTemplateRefCollector {
        tuple_value_candidates,
        object_value_candidates: &object_value_candidates,
        unref_candidates: &unref_candidates,
        ctx,
        tuple_value_refs: HashSet::new(),
        object_value_refs: HashSet::new(),
        unref_refs: HashSet::new(),
    };
    visit_setup_render(render, &mut collector);
    let mut refs = collector.tuple_value_refs;
    refs.extend(
        collector
            .object_value_refs
            .intersection(&collector.unref_refs)
            .cloned(),
    );
    refs
}

fn collect_setup_value_template_tuple_refs(
    value: &VueSetupValueBinding,
    tuple_value_candidates: &HashSet<(Atom, SyntaxContext)>,
    ctx: &mut VueRecoveryContext,
) {
    if tuple_value_candidates.is_empty() {
        return;
    }
    let Some(expr) = value.expr.as_ref() else {
        return;
    };
    for ref_ident in value_member_refs_in_expr(expr) {
        if tuple_value_candidates.contains(&ref_ident) {
            ctx.bindings.template_refs.insert(ref_ident.0);
        }
    }
}

fn value_member_refs_in_expr(expr: &Expr) -> HashSet<(Atom, SyntaxContext)> {
    let mut collector = ValueMemberIdentRefCollector {
        refs: HashSet::new(),
    };
    expr.visit_with(&mut collector);
    collector.refs
}

struct RenderTemplateRefCollector<'a> {
    tuple_value_candidates: &'a HashSet<(Atom, SyntaxContext)>,
    object_value_candidates: &'a HashSet<(Atom, SyntaxContext)>,
    unref_candidates: &'a HashSet<(Atom, SyntaxContext)>,
    ctx: &'a VueRecoveryContext,
    tuple_value_refs: HashSet<Atom>,
    object_value_refs: HashSet<Atom>,
    unref_refs: HashSet<Atom>,
}

impl RenderTemplateRefCollector<'_> {
    fn collect_value_member(&mut self, member: &MemberExpr) {
        if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "value") {
            return;
        }
        let Expr::Ident(object) = member.obj.as_ref() else {
            return;
        };
        // Candidate sets are keyed on the setup binding's `(name, ctxt)`, so a
        // nested local reusing the name carries a different context and never
        // matches — no scope stack required.
        let key = (object.sym.clone(), object.ctxt);
        if self.tuple_value_candidates.contains(&key) {
            self.tuple_value_refs.insert(object.sym.clone());
        }
        if self.object_value_candidates.contains(&key) {
            self.object_value_refs.insert(object.sym.clone());
        }
    }

    fn collect_unref_call(&mut self, call: &CallExpr) {
        if helper_name(&call.callee, self.ctx) != Some(VueHelper::Unref) {
            return;
        }
        let Some(arg) = call.args.first() else {
            return;
        };
        let Expr::Ident(object) = unwrap_paren_expr(arg.expr.as_ref()) else {
            return;
        };
        if self
            .unref_candidates
            .contains(&(object.sym.clone(), object.ctxt))
        {
            self.unref_refs.insert(object.sym.clone());
        }
    }
}

impl Visit for RenderTemplateRefCollector<'_> {
    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        if let Expr::Member(member) = update.arg.as_ref() {
            self.collect_value_member(member);
        }
        update.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        self.collect_value_member(member);
        member.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        self.collect_unref_call(call);
        call.visit_children_with(self);
    }

    fn visit_prop_name(&mut self, prop: &PropName) {
        if let PropName::Computed(computed) = prop {
            computed.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(computed) = prop {
            computed.visit_with(self);
        }
    }
}

fn is_tuple_element_expr(expr: &Expr) -> bool {
    let Expr::Member(member) = unwrap_paren_expr(expr) else {
        return false;
    };
    if !is_zero_member_prop(&member.prop) {
        return false;
    }
    matches!(unwrap_paren_expr(member.obj.as_ref()), Expr::Call(_))
}

fn is_zero_member_prop(prop: &MemberProp) -> bool {
    let MemberProp::Computed(computed) = prop else {
        return false;
    };
    matches!(unwrap_paren_expr(computed.expr.as_ref()), Expr::Lit(Lit::Num(number)) if number.value == 0.0)
}

fn assign_setup_prop_bindings(
    ctx: &mut VueRecoveryContext,
    local_candidates: &[SetupLocalCandidate],
) {
    ctx.bindings.props.clear();
    let prop_names = ctx
        .setup_component_options
        .as_ref()
        .or(ctx.component_options.as_ref())
        .map(component_prop_names)
        .unwrap_or_default();
    let valid_props = prop_names
        .into_iter()
        .filter(|name| is_valid_identifier_name(name))
        .map(Atom::from)
        .collect::<Vec<_>>();
    if valid_props.is_empty() {
        return;
    }

    let mut reserved = HashSet::new();
    reserved.extend(ctx.bindings.aliases.keys().cloned());
    reserved.extend(
        local_candidates
            .iter()
            .flat_map(|candidate| candidate.bindings.iter().cloned()),
    );
    reserved.extend(
        ctx.setup_script_bindings
            .iter()
            .map(|binding| binding.binding.clone()),
    );
    reserved.extend(
        ctx.setup_ref_script_bindings
            .iter()
            .map(|binding| binding.binding.clone()),
    );
    reserved.extend(ctx.setup_emit_aliases.iter().cloned());
    if let Some(binding) = &ctx.setup_emit_context {
        reserved.insert(binding.clone());
    }

    let mut used = reserved.clone();
    used.extend(valid_props.iter().cloned());
    for prop in valid_props {
        let binding = if reserved.contains(&prop) {
            unique_setup_prop_binding(&prop, &mut used)
        } else {
            used.insert(prop.clone());
            prop.clone()
        };
        ctx.bindings.props.insert(prop, binding);
    }
}

fn unique_setup_prop_binding(prop: &Atom, used: &mut HashSet<Atom>) -> Atom {
    let mut index = 1;
    loop {
        let candidate = Atom::from(format!("{}_{index}", prop.as_ref()));
        if used.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

pub(super) fn is_setup_props_alias(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    let Expr::Ident(ident) = unwrap_paren_expr(expr) else {
        return false;
    };
    ctx.setup_props_context
        .as_ref()
        .is_some_and(|setup_props| setup_props == &ident.sym)
        || ctx.setup_props_aliases.contains(&ident.sym)
}

pub(super) fn record_compiled_setup_alias(
    binding: &Atom,
    binding_ctxt: Option<SyntaxContext>,
    expr: &Expr,
    ctx: &mut VueRecoveryContext,
) -> bool {
    if is_setup_props_alias(expr, ctx) {
        ctx.setup_props_aliases.insert(binding.clone());
        if let Some(ctxt) = binding_ctxt {
            ctx.setup_props_alias_ctxts.insert(binding.clone(), ctxt);
        }
        return true;
    }
    if is_setup_emit_alias(expr, ctx) {
        ctx.setup_emit_aliases.insert(binding.clone());
        return true;
    }
    if is_setup_slot_alias(expr, ctx) {
        ctx.slot_bindings.insert(binding.clone());
        return true;
    }
    false
}

fn is_setup_emit_alias(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    match unwrap_paren_expr(expr) {
        Expr::Ident(ident) => {
            ctx.setup_emit_context
                .as_ref()
                .is_some_and(|setup_emit| setup_emit == &ident.sym)
                || ctx.setup_emit_aliases.contains(&ident.sym)
        }
        Expr::Member(member) if matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "emit") =>
        {
            matches!(
                member.obj.as_ref(),
                Expr::Ident(object)
                    if ctx
                        .setup_context
                        .as_ref()
                        .is_some_and(|setup_context| setup_context == &object.sym)
            )
        }
        _ => false,
    }
}

fn is_setup_context_alias(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    let Expr::Ident(ident) = unwrap_paren_expr(expr) else {
        return false;
    };
    ctx.setup_context
        .as_ref()
        .is_some_and(|setup_context| setup_context == &ident.sym)
}

fn is_setup_slot_alias(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    match unwrap_paren_expr(expr) {
        Expr::Ident(ident) => ctx.slot_bindings.contains(&ident.sym),
        Expr::Member(member) if is_setup_slots_member_prop(&member.prop) => {
            matches!(
                member.obj.as_ref(),
                Expr::Ident(object)
                    if ctx
                        .setup_context
                        .as_ref()
                        .is_some_and(|setup_context| setup_context == &object.sym)
            )
        }
        _ => false,
    }
}

fn is_slot_partition_expr(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return false;
    };
    call.args
        .first()
        .is_some_and(|arg| is_slot_source_expr(arg.expr.as_ref(), ctx))
}

fn is_slot_partition_slots_alias(expr: &Expr, slot_partition_bindings: &HashSet<Atom>) -> bool {
    let Expr::Member(member) = unwrap_paren_expr(expr) else {
        return false;
    };
    if !is_setup_slots_member_prop(&member.prop) {
        return false;
    }
    matches!(
        member.obj.as_ref(),
        Expr::Ident(object) if slot_partition_bindings.contains(&object.sym)
    )
}

fn slot_partition_child_list_alias_source(
    expr: &Expr,
    slot_partition_bindings: &HashSet<Atom>,
) -> Option<VueRenderChildListSource> {
    let Expr::Member(member) = unwrap_paren_expr(expr) else {
        return None;
    };
    if !member_prop_is_named(&member.prop, "slides") {
        return None;
    }
    matches!(
        member.obj.as_ref(),
        Expr::Ident(object) if slot_partition_bindings.contains(&object.sym)
    )
    .then_some(VueRenderChildListSource::SlotPartitionChildren)
}

fn render_slot_binding_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Option<VueRenderSlotBinding> {
    match unwrap_paren_expr(expr) {
        Expr::Call(call) => {
            if let Some(binding) = slot_call_binding(call, ctx) {
                return Some(binding);
            }
            if is_slot_call_wrapper(call, ctx) {
                return render_slot_binding_expr(call.args[0].expr.as_ref(), ctx);
            }
            None
        }
        Expr::Bin(bin) if bin.op == BinaryOp::LogicalAnd && is_slot_member_expr(&bin.left, ctx) => {
            render_slot_binding_expr(bin.right.as_ref(), ctx)
        }
        Expr::Seq(seq) => seq
            .exprs
            .last()
            .and_then(|expr| render_slot_binding_expr(expr.as_ref(), ctx)),
        Expr::Assign(assign) => render_slot_binding_expr(assign.right.as_ref(), ctx),
        _ => None,
    }
}

fn is_slot_call_wrapper(call: &CallExpr, ctx: &VueRecoveryContext) -> bool {
    call.args.len() == 1
        && call.args[0].spread.is_none()
        && (helper_name(&call.callee, ctx).is_some()
            || call_callee_ident(call).is_some_and(|callee| {
                (ctx.vue_helper_candidates.contains(&callee.sym) && ctx.resolves_to_import(callee))
                    || ctx.slot_result_normalizers.contains(&callee.sym)
            }))
}

fn is_slot_member_expr(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    let Expr::Member(member) = unwrap_paren_expr(expr) else {
        return false;
    };
    member_prop_name(&member.prop).is_some() && is_slot_source_expr(member.obj.as_ref(), ctx)
}

fn is_slot_partition_alias(expr: &Expr, slot_partition_bindings: &HashSet<Atom>) -> bool {
    let Expr::Ident(ident) = unwrap_paren_expr(expr) else {
        return false;
    };
    slot_partition_bindings.contains(&ident.sym)
}

fn is_slot_source_expr(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    match unwrap_paren_expr(expr) {
        Expr::Ident(ident) => {
            matches!(ident.sym.as_ref(), "$slots" | "slots")
                || ctx.slot_bindings.contains(&ident.sym)
        }
        Expr::Member(member) if is_slots_member_prop(&member.prop) => true,
        Expr::Member(member) if is_setup_slots_member_prop(&member.prop) => {
            matches!(
                member.obj.as_ref(),
                Expr::Ident(object)
                    if ctx
                        .setup_context
                        .as_ref()
                        .is_some_and(|setup_context| setup_context == &object.sym)
            )
        }
        _ => false,
    }
}

fn collect_slot_partition_child_list_bindings(object: &ObjectPat, ctx: &mut VueRecoveryContext) {
    let mut bindings = HashSet::new();
    collect_named_object_pat_bindings(object, "slides", &mut bindings);
    for binding in bindings {
        insert_render_child_list_binding(
            ctx,
            binding,
            VueRenderChildListSource::SlotPartitionChildren,
        );
    }
}

fn insert_render_child_list_binding(
    ctx: &mut VueRecoveryContext,
    binding: Atom,
    source: VueRenderChildListSource,
) {
    ctx.render_child_list_bindings
        .insert(binding, VueRenderChildListBinding { source });
}

pub(super) fn member_prop_is_named(prop: &MemberProp, name: &str) -> bool {
    member_prop_name(prop)
        .as_ref()
        .is_some_and(|prop| prop.as_ref() == name)
}

fn member_prop_name(prop: &MemberProp) -> Option<Atom> {
    match prop {
        MemberProp::Ident(ident) => Some(ident.sym.clone()),
        MemberProp::Computed(computed) => string_lit(computed.expr.as_ref()).map(Atom::from),
        MemberProp::PrivateName(_) => None,
    }
}

fn is_ref_like_value_expr(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return false;
    };
    match helper_name(&call.callee, ctx) {
        Some(VueHelper::Computed) => return true,
        Some(VueHelper::Other(name)) if is_ref_like_vue_helper(&name) => return true,
        _ => {}
    }
    call_callee_ident(call).is_some_and(|callee| {
        ctx.vue_helper_candidates.contains(&callee.sym) && ctx.resolves_to_import(callee)
    })
}

fn should_emit_ref_script_setup_expr(
    expr: &Expr,
    ctx: &VueRecoveryContext,
    binding: &Ident,
    value_member_refs: &HashSet<(Atom, SyntaxContext)>,
) -> bool {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return false;
    };
    match helper_name(&call.callee, ctx) {
        Some(VueHelper::Computed) => return true,
        Some(VueHelper::Other(name)) if is_ref_like_vue_helper(&name) => return true,
        _ => {}
    }
    call_callee_ident(call).is_some_and(|callee| {
        ctx.vue_helper_candidates.contains(&callee.sym) && ctx.resolves_to_import(callee)
    }) && value_member_refs.contains(&(binding.sym.clone(), binding.ctxt))
}

fn is_ref_like_vue_helper(name: &str) -> bool {
    matches!(
        name,
        "ref" | "shallowRef" | "customRef" | "toRef" | "useModel"
    )
}

fn ref_script_setup_expr(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<(String, String, bool)>> {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return Ok(None);
    };
    let Some(helper) = ref_script_setup_helper(call, ctx) else {
        return Ok(None);
    };
    let mut args = Vec::new();
    for arg in &call.args {
        let mut printed = clean_expr(&print_expr(arg.expr.as_ref(), ctx)?, ctx);
        if arg.spread.is_some() {
            printed = format!("...{printed}");
        }
        args.push(printed);
    }
    let known_ref = helper_name(&call.callee, ctx).is_some_and(
        |helper| matches!(helper, VueHelper::Other(name) if is_ref_like_vue_helper(&name)),
    );
    Ok(Some((
        format!("{helper}({})", args.join(", ")),
        helper,
        known_ref,
    )))
}

fn ref_script_setup_helper(call: &CallExpr, ctx: &VueRecoveryContext) -> Option<String> {
    match helper_name(&call.callee, ctx) {
        Some(VueHelper::Other(name)) if is_ref_like_vue_helper(&name) => Some(name),
        _ => call_callee_ident(call)
            .filter(|&callee| {
                ctx.vue_helper_candidates.contains(&callee.sym) && ctx.resolves_to_import(callee)
            })
            .map(|_| "ref".to_string()),
    }
}

pub(in crate::vue_recovery) fn is_ref_object_expr(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return false;
    };
    match helper_name(&call.callee, ctx) {
        Some(VueHelper::Other(name)) if is_ref_object_helper(&name) => return true,
        _ => {}
    }
    call_callee_ident(call).is_some_and(|callee| {
        ctx.vue_helper_candidates.contains(&callee.sym) && ctx.resolves_to_import(callee)
    })
}

pub(in crate::vue_recovery) fn is_ref_object_alias(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    let Expr::Ident(ident) = unwrap_paren_expr(expr) else {
        return false;
    };
    ctx.bindings.ref_objects.contains(&ident.sym)
}

fn is_ref_member_extraction_expr(
    expr: &Expr,
    ctx: &VueRecoveryContext,
    provider_ref_object_bindings: &HashMap<Atom, HashSet<Atom>>,
) -> bool {
    let Expr::Member(member) = unwrap_paren_expr(expr) else {
        return false;
    };
    if is_ref_object_expr(member.obj.as_ref(), ctx) || is_ref_object_alias(member.obj.as_ref(), ctx)
    {
        return true;
    }
    let Some(prop) = member_prop_name(&member.prop) else {
        return false;
    };
    setup_ref_props(member.obj.as_ref(), ctx, provider_ref_object_bindings)
        .is_some_and(|props| props.contains(&prop))
}

fn is_composable_ref_member_extraction_expr(
    expr: &Expr,
    ctx: &VueRecoveryContext,
    bindings: &HashMap<Atom, HashSet<Atom>>,
) -> bool {
    let Expr::Member(member) = unwrap_paren_expr(expr) else {
        return false;
    };
    let Some(prop) = member_prop_name(&member.prop) else {
        return false;
    };
    setup_composable_ref_props(member.obj.as_ref(), ctx, bindings)
        .is_some_and(|props| props.contains(&prop))
}

fn is_ref_object_helper(name: &str) -> bool {
    matches!(name, "toRefs" | "storeToRefs")
}

fn is_script_import_call_expr(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return false;
    };
    call_callee_ident(call).is_some_and(|callee| ctx.script_imports.contains_key(&callee.sym))
}

pub(super) fn provider_ref_props_from_init(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Option<HashSet<Atom>> {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return None;
    };

    call.args
        .iter()
        .filter_map(|arg| provider_ref_props_from_callback(arg.expr.as_ref(), ctx))
        .find(|ref_props| !ref_props.is_empty())
}

fn provider_ref_props_from_callback(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Option<HashSet<Atom>> {
    match unwrap_paren_expr(expr) {
        Expr::Arrow(arrow) => match arrow.body.as_ref() {
            ArrowFunctionBody::FunctionBody(block) => {
                provider_ref_props_from_stmts(block.stmts.as_slice(), ctx)
            }
            ArrowFunctionBody::Expr(expr) => {
                provider_ref_props_from_return_expr(expr.as_ref(), ctx)
            }
        },
        Expr::Fn(function) => function
            .function
            .body
            .as_ref()
            .and_then(|body| provider_ref_props_from_stmts(body.stmts.as_slice(), ctx)),
        _ => None,
    }
}

fn provider_ref_props_from_stmts(
    stmts: &[Stmt],
    ctx: &VueRecoveryContext,
) -> Option<HashSet<Atom>> {
    let refs = collect_provider_ref_bindings(stmts, ctx);
    let object = stmts.iter().rev().find_map(return_expr_from_stmt)?;
    provider_ref_props_from_return_expr_with_refs(object, &refs, ctx)
}

fn provider_ref_props_from_return_expr(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Option<HashSet<Atom>> {
    let refs = HashSet::new();
    provider_ref_props_from_return_expr_with_refs(expr, &refs, ctx)
}

fn provider_ref_props_from_return_expr_with_refs(
    expr: &Expr,
    refs: &HashSet<Atom>,
    ctx: &VueRecoveryContext,
) -> Option<HashSet<Atom>> {
    let Expr::Object(object) = unwrap_paren_expr(expr) else {
        return None;
    };
    let mut ref_props = HashSet::new();
    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            continue;
        };
        match prop.as_ref() {
            Prop::Shorthand(ident) if refs.contains(&ident.sym) => {
                ref_props.insert(ident.sym.clone());
            }
            Prop::KeyValue(key_value) => {
                let value = unwrap_paren_expr(key_value.value.as_ref());
                let is_ref_value = match value {
                    Expr::Ident(value) => refs.contains(&value.sym),
                    _ => is_ref_like_value_expr(value, ctx),
                };
                if !is_ref_value {
                    continue;
                }
                if let Some(name) = prop_name(&key_value.key) {
                    ref_props.insert(Atom::from(name));
                }
            }
            _ => {}
        }
    }
    (!ref_props.is_empty()).then_some(ref_props)
}

fn collect_provider_ref_bindings(stmts: &[Stmt], ctx: &VueRecoveryContext) -> HashSet<Atom> {
    let mut ref_bindings = HashSet::new();
    let mut ref_object_bindings = HashSet::new();

    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        for decl in &var.decls {
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            match &decl.name {
                Pat::Ident(binding) => {
                    if is_ref_object_expr(init, ctx) {
                        ref_object_bindings.insert(binding.id.sym.clone());
                    }
                    if is_ref_like_value_expr(init, ctx)
                        || ident_expr(unwrap_paren_expr(init))
                            .is_some_and(|ident| ref_bindings.contains(&ident.sym))
                    {
                        ref_bindings.insert(binding.id.sym.clone());
                    }
                }
                Pat::Object(object)
                    if is_ref_object_expr(init, ctx)
                        || is_provider_ref_object_alias(init, &ref_object_bindings) =>
                {
                    collect_object_pat_bindings(object, &mut ref_bindings);
                }
                _ => {}
            }
        }
    }

    ref_bindings
}

fn is_provider_ref_object_alias(expr: &Expr, ref_object_bindings: &HashSet<Atom>) -> bool {
    let Expr::Ident(ident) = unwrap_paren_expr(expr) else {
        return false;
    };
    ref_object_bindings.contains(&ident.sym)
}

fn setup_ref_props(
    expr: &Expr,
    ctx: &VueRecoveryContext,
    bindings: &HashMap<Atom, HashSet<Atom>>,
) -> Option<HashSet<Atom>> {
    provider_ref_props_from_expr(expr, ctx)
        .cloned()
        .or_else(|| direct_composable_ref_props(expr, ctx))
        .or_else(|| provider_ref_props_from_alias(expr, bindings).cloned())
}

fn setup_composable_ref_props(
    expr: &Expr,
    ctx: &VueRecoveryContext,
    bindings: &HashMap<Atom, HashSet<Atom>>,
) -> Option<HashSet<Atom>> {
    direct_composable_ref_props(expr, ctx).or_else(|| ref_props_from_alias(expr, bindings).cloned())
}

fn direct_composable_ref_props(expr: &Expr, ctx: &VueRecoveryContext) -> Option<HashSet<Atom>> {
    imported_composable_ref_props_from_expr(expr, ctx)
        .cloned()
        .or_else(|| imports::composable_ref_props_from_iife_call(expr))
}

fn imported_composable_ref_props_from_expr<'a>(
    expr: &Expr,
    ctx: &'a VueRecoveryContext,
) -> Option<&'a HashSet<Atom>> {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return None;
    };
    let callee = call_callee_ident(call)?;
    ctx.imported_composable_ref_props.get(&callee.sym)
}

fn provider_ref_props_from_expr<'a>(
    expr: &Expr,
    ctx: &'a VueRecoveryContext,
) -> Option<&'a HashSet<Atom>> {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = unwrap_paren_expr(callee.as_ref()) else {
        return None;
    };
    if !is_provider_ref_method(&member.prop) {
        return None;
    }
    let Expr::Ident(provider) = unwrap_paren_expr(member.obj.as_ref()) else {
        return None;
    };
    ctx.provider_ref_bindings.get(&provider.sym)
}

fn is_provider_ref_method(prop: &MemberProp) -> bool {
    matches!(prop, MemberProp::Ident(prop) if matches!(prop.sym.as_ref(), "provide" | "inject"))
}

fn provider_ref_props_from_alias<'a>(
    expr: &Expr,
    bindings: &'a HashMap<Atom, HashSet<Atom>>,
) -> Option<&'a HashSet<Atom>> {
    ref_props_from_alias(expr, bindings)
}

fn ref_props_from_alias<'a>(
    expr: &Expr,
    bindings: &'a HashMap<Atom, HashSet<Atom>>,
) -> Option<&'a HashSet<Atom>> {
    let Expr::Ident(ident) = unwrap_paren_expr(expr) else {
        return None;
    };
    bindings.get(&ident.sym)
}

fn collect_object_pat_bindings(object: &ObjectPat, bindings: &mut HashSet<Atom>) {
    for prop in &object.props {
        match prop {
            ObjectPatProp::KeyValue(key_value) => {
                collect_pat_bindings(key_value.value.as_ref(), bindings);
            }
            ObjectPatProp::Assign(assign) => {
                bindings.insert(assign.key.sym.clone());
            }
            ObjectPatProp::Rest(rest) => collect_pat_bindings(rest.arg.as_ref(), bindings),
        }
    }
}

fn collect_named_object_pat_bindings(object: &ObjectPat, name: &str, bindings: &mut HashSet<Atom>) {
    for prop in &object.props {
        match prop {
            ObjectPatProp::KeyValue(key_value)
                if prop_name(&key_value.key).as_deref() == Some(name) =>
            {
                collect_pat_bindings(key_value.value.as_ref(), bindings);
            }
            ObjectPatProp::Assign(assign) if assign.key.sym.as_ref() == name => {
                bindings.insert(assign.key.sym.clone());
            }
            _ => {}
        }
    }
}

fn collect_provider_object_pat_bindings(
    object: &ObjectPat,
    ref_props: &HashSet<Atom>,
    bindings: &mut HashSet<Atom>,
) {
    for prop in &object.props {
        match prop {
            ObjectPatProp::KeyValue(key_value) => {
                let Some(name) = prop_name(&key_value.key) else {
                    continue;
                };
                if ref_props.iter().any(|prop| prop.as_ref() == name.as_str()) {
                    collect_pat_bindings(key_value.value.as_ref(), bindings);
                }
            }
            ObjectPatProp::Assign(assign) => {
                if ref_props.contains(&assign.key.sym) {
                    bindings.insert(assign.key.sym.clone());
                }
            }
            ObjectPatProp::Rest(_) => {}
        }
    }
}

pub(super) fn collect_pat_bindings(pat: &Pat, bindings: &mut HashSet<Atom>) {
    match pat {
        Pat::Ident(binding) => {
            bindings.insert(binding.id.sym.clone());
        }
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_pat_bindings(elem, bindings);
            }
        }
        Pat::Rest(rest) => collect_pat_bindings(rest.arg.as_ref(), bindings),
        Pat::Object(object) => collect_object_pat_bindings(object, bindings),
        Pat::Assign(assign) => collect_pat_bindings(assign.left.as_ref(), bindings),
        Pat::Expr(_) | Pat::Invalid(_) => {}
    }
}
