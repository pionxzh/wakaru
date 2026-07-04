use std::collections::{HashMap, HashSet};

use anyhow::Result;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayLit, ArrowExpr, AssignExpr, AssignPat, AssignTarget, BinaryOp, BindingIdent, BlockStmt,
    BlockStmtOrExpr, CallExpr, Callee, ClassDecl, CondExpr, Decl, ExportSpecifier, Expr,
    ExprOrSpread, FnDecl, Function, Ident, IfStmt, ImportSpecifier, KeyValuePatProp, Lit,
    MemberExpr, MemberProp, Module, ModuleDecl, ModuleItem, ObjectLit, ObjectPat, ObjectPatProp,
    ParenExpr, Pat, Prop, PropName, PropOrSpread, ReturnStmt, SimpleAssignTarget, Stmt, UnaryOp,
    UpdateExpr, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::expressions::{clean_expr, clean_setup_stmt, print_clean_setup_stmt, print_expr};
use super::helpers::{helper_name, VueHelper};
use super::imports;
use super::locals::{
    VueSetupLocalBinding, VueSetupRefBinding, VueSetupScriptBinding, VueSetupValueBinding,
};
use super::script_imports::VueScriptImport;
use super::setup_bindings::component_prop_names;
use super::slots::slot_call_binding;
use super::syntax::{
    module_export_name, param_binding_ident, prop_name, string_lit, wtf8_to_string,
};
use super::{
    RenderSource, VueRecoveryContext, VueRenderChildListBinding, VueRenderChildListSource,
    VueRenderSlotBinding,
};
use crate::js_names::is_valid_identifier_name;

const MAX_INLINE_COMPUTED_TEMPLATE_BINDING_LEN: usize = 80;

struct SetupLocalCandidate {
    bindings: Vec<Atom>,
    stmt: Stmt,
    template_selectable: bool,
    setup_order: usize,
}

pub(super) fn collect_context(
    module: &Module,
    cm: Lrc<SourceMap>,
    component_bindings: HashMap<Atom, String>,
    imported_composable_ref_props: HashMap<Atom, HashSet<Atom>>,
) -> VueRecoveryContext {
    let default_exported_bindings = default_exported_bindings(module);
    let mut ctx = VueRecoveryContext {
        cm,
        component_bindings,
        imported_composable_ref_props,
        ..Default::default()
    };
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                let source = wtf8_to_string(&import.src.value);
                let imported_component = vue_component_name_from_source(&source);
                for specifier in &import.specifiers {
                    match specifier {
                        ImportSpecifier::Named(named) => {
                            if let Some(component) = &imported_component {
                                ctx.component_bindings
                                    .entry(named.local.sym.clone())
                                    .or_insert_with(|| component.clone());
                            }
                            let imported = named
                                .imported
                                .as_ref()
                                .map(module_export_name)
                                .unwrap_or_else(|| named.local.sym.to_string());
                            if source != "vue" {
                                ctx.script_imports.insert(
                                    named.local.sym.clone(),
                                    VueScriptImport::Named {
                                        source: source.clone(),
                                        imported: imported.clone(),
                                    },
                                );
                            }
                            if source == "pinia" && imported == "storeToRefs" {
                                ctx.vue_helpers
                                    .insert(named.local.sym.clone(), VueHelper::Other(imported));
                                continue;
                            }
                            if source != "vue" {
                                if is_vue_helper_candidate_source(&source) {
                                    ctx.vue_helper_candidates.insert(named.local.sym.clone());
                                }
                                continue;
                            }
                            ctx.vue_helpers.insert(
                                named.local.sym.clone(),
                                VueHelper::from_imported_name(imported),
                            );
                        }
                        ImportSpecifier::Default(default) => {
                            if source != "vue" {
                                ctx.script_imports.insert(
                                    default.local.sym.clone(),
                                    VueScriptImport::Default {
                                        source: source.clone(),
                                    },
                                );
                            }
                            if let Some(component) = &imported_component {
                                ctx.component_bindings
                                    .entry(default.local.sym.clone())
                                    .or_insert_with(|| component.clone());
                            }
                        }
                        ImportSpecifier::Namespace(namespace) => {
                            if source == "vue" || is_vue_helper_candidate_source(&source) {
                                ctx.vue_namespaces.insert(namespace.local.sym.clone());
                            }
                            if source != "vue" {
                                ctx.script_imports.insert(
                                    namespace.local.sym.clone(),
                                    VueScriptImport::Namespace {
                                        source: source.clone(),
                                    },
                                );
                            }
                            if let Some(component) = &imported_component {
                                ctx.component_bindings
                                    .entry(namespace.local.sym.clone())
                                    .or_insert_with(|| component.clone());
                            }
                        }
                    }
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                collect_var_decl_context(var, &mut ctx, &default_exported_bindings);
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function))) => {
                collect_fn_decl_context(function, &mut ctx);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => match &export.decl {
                Decl::Var(var) => {
                    collect_var_decl_context(var, &mut ctx, &default_exported_bindings);
                }
                Decl::Fn(function) => collect_fn_decl_context(function, &mut ctx),
                _ => {}
            },
            _ => {}
        }
    }
    ctx
}

fn collect_fn_decl_context(function: &FnDecl, ctx: &mut VueRecoveryContext) {
    if is_slot_result_normalizer_function(&function.function) {
        ctx.slot_result_normalizers
            .insert(function.ident.sym.clone());
    }
}

fn is_slot_result_normalizer_function(function: &Function) -> bool {
    let Some(param) = function.params.first() else {
        return false;
    };
    if function.params.len() != 1 {
        return false;
    }
    let Pat::Ident(param) = &param.pat else {
        return false;
    };
    let Some(body) = &function.body else {
        return false;
    };
    let [Stmt::If(if_stmt), Stmt::Return(final_return)] = body.stmts.as_slice() else {
        return false;
    };
    if !is_length_one_test(if_stmt.test.as_ref(), &param.id.sym) {
        return false;
    }
    if !if_stmt
        .cons
        .as_ref()
        .is_return_with(|expr| is_member_index_expr(expr, &param.id.sym, 0.0))
    {
        return false;
    }
    final_return
        .arg
        .as_deref()
        .is_some_and(|expr| is_ident_expr(expr, &param.id.sym))
}

trait ReturnStmtExt {
    fn is_return_with(&self, predicate: impl FnOnce(&Expr) -> bool) -> bool;
}

impl ReturnStmtExt for Stmt {
    fn is_return_with(&self, predicate: impl FnOnce(&Expr) -> bool) -> bool {
        match self {
            Stmt::Return(return_stmt) => return_stmt.arg.as_deref().is_some_and(predicate),
            Stmt::Block(block) => match block.stmts.as_slice() {
                [Stmt::Return(return_stmt)] => return_stmt.arg.as_deref().is_some_and(predicate),
                _ => false,
            },
            _ => false,
        }
    }
}

fn is_length_one_test(expr: &Expr, param: &Atom) -> bool {
    let Expr::Bin(bin) = unwrap_paren_expr(expr) else {
        return false;
    };
    if !matches!(bin.op, BinaryOp::EqEq | BinaryOp::EqEqEq) {
        return false;
    }
    (is_member_prop_expr(bin.left.as_ref(), param, "length")
        && is_number_lit(bin.right.as_ref(), 1.0))
        || (is_member_prop_expr(bin.right.as_ref(), param, "length")
            && is_number_lit(bin.left.as_ref(), 1.0))
}

fn is_member_prop_expr(expr: &Expr, object: &Atom, prop: &str) -> bool {
    let Expr::Member(member) = unwrap_paren_expr(expr) else {
        return false;
    };
    is_ident_expr(member.obj.as_ref(), object) && member_prop_is_named(&member.prop, prop)
}

fn is_member_index_expr(expr: &Expr, object: &Atom, index: f64) -> bool {
    let Expr::Member(member) = unwrap_paren_expr(expr) else {
        return false;
    };
    if !is_ident_expr(member.obj.as_ref(), object) {
        return false;
    }
    let MemberProp::Computed(computed) = &member.prop else {
        return false;
    };
    is_number_lit(computed.expr.as_ref(), index)
}

fn is_ident_expr(expr: &Expr, sym: &Atom) -> bool {
    matches!(unwrap_paren_expr(expr), Expr::Ident(ident) if &ident.sym == sym)
}

fn is_number_lit(expr: &Expr, value: f64) -> bool {
    matches!(unwrap_paren_expr(expr), Expr::Lit(Lit::Num(number)) if number.value == value)
}

fn default_exported_bindings(module: &Module) -> HashSet<Atom> {
    let mut bindings = HashSet::new();

    for item in &module.body {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::ExportDefaultExpr(export) => {
                if let Expr::Ident(ident) = export.expr.as_ref() {
                    bindings.insert(ident.sym.clone());
                }
            }
            ModuleDecl::ExportNamed(export) if export.src.is_none() => {
                for specifier in &export.specifiers {
                    let ExportSpecifier::Named(named) = specifier else {
                        continue;
                    };
                    let exported = named
                        .exported
                        .as_ref()
                        .map(module_export_name)
                        .unwrap_or_else(|| module_export_name(&named.orig));
                    if exported == "default" {
                        bindings.insert(Atom::from(module_export_name(&named.orig)));
                    }
                }
            }
            _ => {}
        }
    }

    bindings
}

pub(super) fn collect_script_local_context(
    module: &Module,
    ctx: &mut VueRecoveryContext,
) -> Result<()> {
    let reserved_bindings = script_local_reserved_bindings(module, ctx);
    let mut used_bindings = reserved_bindings.clone();
    used_bindings.extend(ctx.script_imports.keys().cloned());

    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(decl)) => {
                collect_script_local_decl(decl, ctx, &reserved_bindings, &mut used_bindings)?
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                collect_script_local_decl(
                    &export.decl,
                    ctx,
                    &reserved_bindings,
                    &mut used_bindings,
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn script_local_reserved_bindings(module: &Module, ctx: &VueRecoveryContext) -> HashSet<Atom> {
    let mut reserved = HashSet::new();
    reserved.extend(
        ctx.setup_script_bindings
            .iter()
            .map(|binding| binding.binding.clone()),
    );
    reserved.extend(
        ctx.setup_local_bindings
            .iter()
            .flat_map(|binding| binding.emitted_bindings.iter().cloned()),
    );
    reserved.extend(
        ctx.setup_ref_script_bindings
            .iter()
            .map(|binding| binding.binding.clone()),
    );
    reserved.extend(ctx.bindings.values.keys().cloned());

    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(decl)) => collect_decl_bindings(decl, &mut reserved),
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                collect_decl_bindings(&export.decl, &mut reserved);
            }
            _ => {}
        }
    }
    reserved
}

fn collect_decl_bindings(decl: &Decl, bindings: &mut HashSet<Atom>) {
    match decl {
        Decl::Fn(function) => {
            bindings.insert(function.ident.sym.clone());
        }
        Decl::Class(class) => {
            bindings.insert(class.ident.sym.clone());
        }
        Decl::Var(var) => {
            for declarator in &var.decls {
                collect_pat_bindings(&declarator.name, bindings);
            }
        }
        _ => {}
    }
}

fn emitted_stmt_bindings(source: &str, ctx: &VueRecoveryContext, fallback: &[Atom]) -> Vec<Atom> {
    let bindings = emitted_decl_bindings(source, ctx);
    if bindings.is_empty() {
        fallback.to_vec()
    } else {
        bindings
    }
}

fn emitted_decl_bindings(source: &str, ctx: &VueRecoveryContext) -> Vec<Atom> {
    let Ok(module) = super::parse_module(source, ctx.cm.clone()) else {
        return Vec::new();
    };

    let mut bindings = HashSet::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(decl)) => collect_decl_bindings(decl, &mut bindings),
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                collect_decl_bindings(&export.decl, &mut bindings);
            }
            _ => {}
        }
    }

    let mut bindings = bindings.into_iter().collect::<Vec<_>>();
    bindings.sort_by(|left, right| left.as_ref().cmp(right.as_ref()));
    bindings.dedup();
    bindings
}

fn collect_script_local_decl(
    decl: &Decl,
    ctx: &mut VueRecoveryContext,
    reserved_bindings: &HashSet<Atom>,
    used_bindings: &mut HashSet<Atom>,
) -> Result<()> {
    match decl {
        Decl::Fn(function) => push_script_local_binding(
            ctx,
            vec![function.ident.sym.clone()],
            Stmt::Decl(Decl::Fn(function.clone())),
            reserved_bindings,
            used_bindings,
        ),
        Decl::Class(class) => push_script_local_binding(
            ctx,
            vec![class.ident.sym.clone()],
            Stmt::Decl(Decl::Class(class.clone())),
            reserved_bindings,
            used_bindings,
        ),
        Decl::Var(var) => {
            for declarator in &var.decls {
                if declarator.init.as_deref().is_some_and(|init| {
                    component_name_from_init(init, &ctx.component_bindings).is_some()
                }) {
                    continue;
                }
                let mut bindings = HashSet::new();
                collect_pat_bindings(&declarator.name, &mut bindings);
                if bindings.is_empty() {
                    continue;
                }
                let mut single_var = var.as_ref().clone();
                single_var.decls = vec![declarator.clone()];
                let mut bindings = bindings.into_iter().collect::<Vec<_>>();
                bindings.sort_by(|left, right| left.as_ref().cmp(right.as_ref()));
                push_script_local_binding(
                    ctx,
                    bindings,
                    Stmt::Decl(Decl::Var(Box::new(single_var))),
                    reserved_bindings,
                    used_bindings,
                )?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn push_script_local_binding(
    ctx: &mut VueRecoveryContext,
    bindings: Vec<Atom>,
    mut stmt: Stmt,
    reserved_bindings: &HashSet<Atom>,
    used_bindings: &mut HashSet<Atom>,
) -> Result<()> {
    let cleaned_stmt = clean_setup_stmt(&stmt, ctx);
    let mut source = print_clean_setup_stmt(&cleaned_stmt, ctx)?;
    if is_transpiler_runtime_helper_source(&source) {
        return Ok(());
    }

    let import_aliases = colliding_import_aliases(&stmt, ctx, reserved_bindings, used_bindings);
    if !import_aliases.is_empty() {
        stmt.visit_mut_with(&mut ImportAliasRenamer::new(&import_aliases));
        let cleaned_stmt = clean_setup_stmt(&stmt, ctx);
        source = print_clean_setup_stmt(&cleaned_stmt, ctx)?;
    }

    if !source.is_empty() {
        let cleaned_stmt = clean_setup_stmt(&stmt, ctx);
        let emitted_bindings = emitted_stmt_bindings(&source, ctx, &bindings);
        ctx.script_local_bindings.push(VueSetupLocalBinding {
            bindings,
            emitted_bindings,
            refs: stmt_ident_refs(&cleaned_stmt),
            source,
            import_refs: stmt_import_refs(&cleaned_stmt, &ctx.script_imports),
            stmt: cleaned_stmt,
            module_scope: true,
            template_selectable: true,
            setup_order: 0,
        });
    }
    Ok(())
}

pub(super) fn render_local_declaration_with_aliases(
    ctx: &VueRecoveryContext,
    declaration: &VueSetupLocalBinding,
    aliases: &HashMap<Atom, Atom>,
    props_binding: Option<&str>,
) -> Result<VueSetupLocalBinding> {
    let mut stmt = declaration.stmt.clone();
    if declaration.module_scope && !aliases.is_empty() {
        rename_top_level_decl_bindings(&mut stmt, aliases);
        stmt.visit_mut_with(&mut ImportAliasRenamer::new(aliases));
    }

    let mut cleaned_stmt = clean_setup_stmt(&stmt, ctx);
    if !declaration.module_scope {
        if let Some(props_binding) = props_binding {
            rewrite_setup_props_refs(&mut cleaned_stmt, ctx, props_binding);
        }
    }
    let source = print_clean_setup_stmt(&cleaned_stmt, ctx)?;
    let bindings = if declaration.module_scope {
        declaration
            .bindings
            .iter()
            .map(|binding| {
                aliases
                    .get(binding)
                    .cloned()
                    .unwrap_or_else(|| binding.clone())
            })
            .collect()
    } else {
        declaration.bindings.clone()
    };
    let emitted_bindings = emitted_stmt_bindings(&source, ctx, &bindings);

    Ok(VueSetupLocalBinding {
        bindings,
        emitted_bindings,
        refs: stmt_ident_refs(&cleaned_stmt),
        source,
        import_refs: stmt_import_refs(&cleaned_stmt, &ctx.script_imports),
        stmt: cleaned_stmt,
        module_scope: declaration.module_scope,
        template_selectable: declaration.template_selectable,
        setup_order: declaration.setup_order,
    })
}

fn rewrite_setup_props_refs(stmt: &mut Stmt, ctx: &VueRecoveryContext, props_binding: &str) {
    let mut rewriter = SetupPropsRefRewriter::new(ctx, props_binding);
    if !rewriter.is_empty() {
        stmt.visit_mut_with(&mut rewriter);
    }
}

struct SetupPropsRefRewriter {
    sources: Vec<Atom>,
    replacement: Atom,
    shadow_depths: Vec<usize>,
}

impl SetupPropsRefRewriter {
    fn new(ctx: &VueRecoveryContext, props_binding: &str) -> Self {
        let mut sources = Vec::new();
        if let Some(binding) = &ctx.setup_props_context {
            sources.push(binding.clone());
        }
        sources.extend(ctx.setup_props_aliases.iter().cloned());
        sources.sort_by(|left, right| left.as_ref().cmp(right.as_ref()));
        sources.dedup();
        let shadow_depths = vec![0; sources.len()];
        Self {
            sources,
            replacement: Atom::from(props_binding.to_string()),
            shadow_depths,
        }
    }

    fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    fn active_source(&self, name: &Atom) -> bool {
        self.sources
            .iter()
            .zip(self.shadow_depths.iter())
            .any(|(source, shadow_depth)| source == name && *shadow_depth == 0)
    }

    fn shadowing_indices(&self, params: &[&Pat]) -> Vec<usize> {
        self.sources
            .iter()
            .enumerate()
            .filter_map(|(index, source)| {
                params
                    .iter()
                    .any(|pat| pat_binds_atom(pat, source))
                    .then_some(index)
            })
            .collect()
    }

    fn decl_shadowing_indices(&self, decl: &Decl) -> Vec<usize> {
        self.sources
            .iter()
            .enumerate()
            .filter_map(|(index, source)| decl_binds_atom(decl, source).then_some(index))
            .collect()
    }

    fn block_shadowing_indices(&self, block: &BlockStmt) -> Vec<usize> {
        let mut indices = block
            .stmts
            .iter()
            .filter_map(|stmt| match stmt {
                Stmt::Decl(decl) => Some(decl),
                _ => None,
            })
            .flat_map(|decl| self.decl_shadowing_indices(decl))
            .collect::<Vec<_>>();
        indices.sort_unstable();
        indices.dedup();
        indices
    }

    fn enter_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] += 1;
        }
    }

    fn exit_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] -= 1;
        }
    }
}

impl VisitMut for SetupPropsRefRewriter {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let replacement = match expr {
            Expr::Ident(ident) if self.active_source(&ident.sym) => Some(Ident::new(
                self.replacement.clone(),
                ident.span,
                Default::default(),
            )),
            _ => None,
        };
        if let Some(replacement) = replacement {
            *expr = Expr::Ident(replacement);
        }
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        let params = arrow.params.iter().collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        arrow.visit_mut_children_with(self);
        self.exit_shadowed(&shadowed);
    }

    fn visit_mut_function(&mut self, function: &mut Function) {
        let params = function
            .params
            .iter()
            .map(|param| &param.pat)
            .collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        function.visit_mut_children_with(self);
        self.exit_shadowed(&shadowed);
    }

    fn visit_mut_block_stmt(&mut self, block: &mut BlockStmt) {
        let shadowed = self.block_shadowing_indices(block);
        self.enter_shadowed(&shadowed);
        block.visit_mut_children_with(self);
        self.exit_shadowed(&shadowed);
    }
}

fn pat_binds_atom(pat: &Pat, binding: &Atom) -> bool {
    let mut bindings = HashSet::new();
    collect_pat_bindings(pat, &mut bindings);
    bindings.contains(binding)
}

fn decl_binds_atom(decl: &Decl, binding: &Atom) -> bool {
    let mut bindings = HashSet::new();
    collect_decl_bindings(decl, &mut bindings);
    bindings.contains(binding)
}

fn rename_top_level_decl_bindings(stmt: &mut Stmt, aliases: &HashMap<Atom, Atom>) {
    let Stmt::Decl(decl) = stmt else {
        return;
    };

    match decl {
        Decl::Fn(function) => rename_binding_ident(&mut function.ident, aliases),
        Decl::Class(class) => rename_binding_ident(&mut class.ident, aliases),
        Decl::Var(var) => {
            for declarator in &mut var.decls {
                rename_pat_bindings(&mut declarator.name, aliases);
            }
        }
        _ => {}
    }
}

fn rename_binding_ident(ident: &mut Ident, aliases: &HashMap<Atom, Atom>) {
    if let Some(alias) = aliases.get(&ident.sym) {
        ident.sym = alias.clone();
    }
}

fn rename_binding_binding_ident(binding: &mut BindingIdent, aliases: &HashMap<Atom, Atom>) {
    if let Some(alias) = aliases.get(&binding.id.sym) {
        binding.id.sym = alias.clone();
    }
}

fn rename_pat_bindings(pat: &mut Pat, aliases: &HashMap<Atom, Atom>) {
    match pat {
        Pat::Ident(binding) => rename_binding_binding_ident(binding, aliases),
        Pat::Array(array) => {
            for elem in array.elems.iter_mut().flatten() {
                rename_pat_bindings(elem, aliases);
            }
        }
        Pat::Object(object) => {
            for prop in &mut object.props {
                rename_object_pat_prop_bindings(prop, aliases);
            }
        }
        Pat::Rest(rest) => rename_pat_bindings(rest.arg.as_mut(), aliases),
        Pat::Assign(assign) => rename_pat_bindings(assign.left.as_mut(), aliases),
        Pat::Expr(_) | Pat::Invalid(_) => {}
    }
}

fn rename_object_pat_prop_bindings(prop: &mut ObjectPatProp, aliases: &HashMap<Atom, Atom>) {
    match prop {
        ObjectPatProp::KeyValue(key_value) => {
            rename_pat_bindings(key_value.value.as_mut(), aliases)
        }
        ObjectPatProp::Assign(assign) => {
            if let Some(alias) = aliases.get(&assign.key.id.sym) {
                let key = PropName::Ident(assign.key.id.clone().into());
                let mut binding = assign.key.clone();
                binding.id.sym = alias.clone();
                let value = if let Some(default) = assign.value.take() {
                    Pat::Assign(AssignPat {
                        span: binding.id.span,
                        left: Box::new(Pat::Ident(binding)),
                        right: default,
                    })
                } else {
                    Pat::Ident(binding)
                };
                *prop = ObjectPatProp::KeyValue(KeyValuePatProp {
                    key,
                    value: Box::new(value),
                });
            }
        }
        ObjectPatProp::Rest(rest) => rename_pat_bindings(rest.arg.as_mut(), aliases),
    }
}

fn is_transpiler_runtime_helper_source(source: &str) -> bool {
    source.contains("suspendedStart")
        && source.contains("_invoke")
        && (source.contains("@@iterator") || source.contains("__await"))
}

fn is_vue_helper_candidate_source(source: &str) -> bool {
    if source.contains("runtime-core") || source.contains("runtime-dom") {
        return true;
    }
    if is_vue_adjacent_package_source(source) {
        return false;
    }
    if is_bare_import_source(source) {
        return false;
    }
    source.contains("vue")
}

fn is_vue_adjacent_package_source(source: &str) -> bool {
    let source = source.to_ascii_lowercase();
    source.contains("vueuse") || source.contains("vue-router") || source.contains("vuex")
}

fn is_bare_import_source(source: &str) -> bool {
    !source.starts_with('.')
        && !source.starts_with('/')
        && !source.starts_with("file:")
        && !source.starts_with("http:")
        && !source.starts_with("https:")
}

fn colliding_import_aliases(
    stmt: &Stmt,
    ctx: &mut VueRecoveryContext,
    reserved_bindings: &HashSet<Atom>,
    used_bindings: &mut HashSet<Atom>,
) -> HashMap<Atom, Atom> {
    let import_refs = stmt_import_refs(stmt, &ctx.script_imports);
    let mut aliases = HashMap::new();
    for import_ref in import_refs {
        if !reserved_bindings.contains(&import_ref) {
            continue;
        }
        let Some(import) = ctx.script_imports.get(&import_ref).cloned() else {
            continue;
        };
        let alias = unique_script_import_alias(&import_ref, used_bindings);
        ctx.script_imports.insert(alias.clone(), import);
        aliases.insert(import_ref, alias);
    }
    aliases
}

fn unique_script_import_alias(binding: &Atom, used_bindings: &mut HashSet<Atom>) -> Atom {
    let mut index = 1;
    loop {
        let candidate = Atom::from(format!("{}_{index}", binding.as_ref()));
        if used_bindings.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

struct ScopeStack(Vec<HashSet<Atom>>);

impl ScopeStack {
    fn new() -> Self {
        Self(vec![HashSet::new()])
    }

    fn push_scope(&mut self) {
        self.0.push(HashSet::new());
    }

    fn pop_scope(&mut self) {
        self.0.pop();
    }

    fn depth(&self) -> usize {
        self.0.len()
    }

    fn declare(&mut self, sym: &Atom) {
        if let Some(scope) = self.0.last_mut() {
            scope.insert(sym.clone());
        }
    }

    fn declare_pat(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(binding) => self.declare(&binding.id.sym),
            Pat::Array(array) => {
                for elem in array.elems.iter().flatten() {
                    self.declare_pat(elem);
                }
            }
            Pat::Object(object) => {
                for prop in &object.props {
                    match prop {
                        ObjectPatProp::KeyValue(key_value) => self.declare_pat(&key_value.value),
                        ObjectPatProp::Assign(assign) => self.declare(&assign.key.sym),
                        ObjectPatProp::Rest(rest) => self.declare_pat(&rest.arg),
                    }
                }
            }
            Pat::Rest(rest) => self.declare_pat(&rest.arg),
            Pat::Assign(assign) => self.declare_pat(&assign.left),
            Pat::Expr(_) | Pat::Invalid(_) => {}
        }
    }

    fn is_shadowed(&self, sym: &Atom) -> bool {
        self.0.iter().rev().any(|scope| scope.contains(sym))
    }
}

struct ImportAliasRenamer<'a> {
    aliases: &'a HashMap<Atom, Atom>,
    scopes: ScopeStack,
}

impl<'a> ImportAliasRenamer<'a> {
    fn new(aliases: &'a HashMap<Atom, Atom>) -> Self {
        Self {
            aliases,
            scopes: ScopeStack::new(),
        }
    }
}

impl VisitMut for ImportAliasRenamer<'_> {
    fn visit_mut_ident(&mut self, ident: &mut Ident) {
        if !self.scopes.is_shadowed(&ident.sym) {
            if let Some(alias) = self.aliases.get(&ident.sym) {
                ident.sym = alias.clone();
            }
        }
    }

    fn visit_mut_binding_ident(&mut self, ident: &mut BindingIdent) {
        self.scopes.declare(&ident.id.sym);
    }

    fn visit_mut_prop_name(&mut self, prop: &mut PropName) {
        if let PropName::Computed(computed) = prop {
            computed.visit_mut_with(self);
        }
    }

    fn visit_mut_member_prop(&mut self, prop: &mut MemberProp) {
        if let MemberProp::Computed(computed) = prop {
            computed.visit_mut_with(self);
        }
    }

    fn visit_mut_var_declarator(&mut self, declarator: &mut VarDeclarator) {
        self.scopes.declare_pat(&declarator.name);
        if let Some(init) = &mut declarator.init {
            init.visit_mut_with(self);
        }
    }

    fn visit_mut_fn_decl(&mut self, function: &mut FnDecl) {
        self.scopes.declare(&function.ident.sym);
        self.visit_mut_function(&mut function.function);
    }

    fn visit_mut_function(&mut self, function: &mut Function) {
        self.scopes.push_scope();
        for param in &function.params {
            self.scopes.declare_pat(&param.pat);
        }
        if let Some(body) = &mut function.body {
            body.visit_mut_with(self);
        }
        self.scopes.pop_scope();
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        self.scopes.push_scope();
        for param in &arrow.params {
            self.scopes.declare_pat(param);
        }
        arrow.body.visit_mut_with(self);
        self.scopes.pop_scope();
    }

    fn visit_mut_class_decl(&mut self, class: &mut ClassDecl) {
        self.scopes.declare(&class.ident.sym);
        class.class.visit_mut_with(self);
    }
}

fn collect_var_decl_context(
    var: &VarDecl,
    ctx: &mut VueRecoveryContext,
    default_exported_bindings: &HashSet<Atom>,
) {
    if !matches!(var.kind, VarDeclKind::Const | VarDeclKind::Var) {
        return;
    }
    for decl in &var.decls {
        let Pat::Ident(binding) = &decl.name else {
            continue;
        };
        let Some(init) = decl.init.as_deref() else {
            continue;
        };
        if let Expr::Object(object) = init {
            ctx.object_bindings
                .insert(binding.id.sym.clone(), object.clone());
        }
        if is_vue_fragment_symbol_init(init) {
            ctx.vue_helpers
                .insert(binding.id.sym.clone(), VueHelper::Fragment);
        }
        if is_likely_vue_runtime_require_namespace(&binding.id.sym, init) {
            ctx.vue_namespaces.insert(binding.id.sym.clone());
        }
        if let Some(ref_props) = provider_ref_props_from_init(init, ctx) {
            ctx.provider_ref_bindings
                .insert(binding.id.sym.clone(), ref_props);
        }
        if let Some(component) = component_name_from_init(init, &ctx.component_bindings) {
            ctx.component_bindings
                .insert(binding.id.sym.clone(), component);
        }
        if binding.id.sym.as_ref() == "__sfc__"
            || default_exported_bindings.contains(&binding.id.sym)
        {
            if let Some(object) = component_options_from_init(init) {
                ctx.component_options = Some(object.clone());
            }
        }
    }
}

fn is_vue_fragment_symbol_init(expr: &Expr) -> bool {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    let Expr::Ident(object) = member.obj.as_ref() else {
        return false;
    };
    if object.sym.as_ref() != "Symbol" {
        return false;
    }
    let MemberProp::Ident(prop) = &member.prop else {
        return false;
    };
    if prop.sym.as_ref() != "for" {
        return false;
    }
    call.args
        .first()
        .and_then(|arg| string_lit(arg.expr.as_ref()))
        .as_deref()
        == Some("v-fgt")
}

fn is_likely_vue_runtime_require_namespace(binding: &Atom, expr: &Expr) -> bool {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return false;
    };
    if !call_callee_ident(call)
        .is_some_and(|callee| matches!(callee.sym.as_ref(), "require" | "__webpack_require__"))
    {
        return false;
    }

    if let Some(source) = call
        .args
        .first()
        .and_then(|arg| string_lit(arg.expr.as_ref()))
    {
        return source == "vue"
            || source.contains("@vue/runtime")
            || source.contains("vue/dist")
            || source.contains("vue.runtime");
    }

    let binding = binding.to_string().to_ascii_lowercase();
    (binding.contains("vue") && binding.contains("runtime"))
        || binding.contains("vue__webpack_imported_module")
}

pub(super) fn component_options_from_init(expr: &Expr) -> Option<&ObjectLit> {
    match unwrap_paren_expr(expr) {
        Expr::Object(object) => Some(object),
        Expr::Call(call) => {
            call.args
                .first()
                .and_then(|arg| match unwrap_paren_expr(arg.expr.as_ref()) {
                    Expr::Object(object) => Some(object),
                    _ => None,
                })
        }
        _ => None,
    }
}

pub(super) fn component_name_from_init(
    expr: &Expr,
    component_bindings: &HashMap<Atom, String>,
) -> Option<String> {
    match unwrap_paren_expr(expr) {
        Expr::Object(object) => component_name_from_options(object),
        Expr::Call(call) => call.args.first().and_then(|arg| match arg.expr.as_ref() {
            Expr::Object(object) => component_name_from_options(object),
            Expr::Ident(ident) => component_bindings.get(&ident.sym).cloned(),
            Expr::Call(_) | Expr::Paren(_) => {
                component_name_from_init(arg.expr.as_ref(), component_bindings)
            }
            _ => None,
        }),
        _ => None,
    }
}

pub(super) fn component_name_from_options(object: &ObjectLit) -> Option<String> {
    object.props.iter().find_map(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            return None;
        };
        matches!(
            prop_name(&key_value.key).as_deref(),
            Some("__name" | "name")
        )
        .then(|| string_lit(key_value.value.as_ref()))
        .flatten()
    })
}

fn vue_component_name_from_source(source: &str) -> Option<String> {
    let file = source
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(source)
        .trim_start_matches("./");
    if source.contains(".vue") {
        let name = file.split(".vue").next()?;
        return (!name.is_empty()).then(|| name.to_string());
    }

    let stem = file
        .strip_suffix(".mjs")
        .or_else(|| file.strip_suffix(".js"))?;
    let name = stem
        .split('-')
        .next()
        .unwrap_or(stem)
        .split('.')
        .next()
        .unwrap_or(stem);
    let starts_with_uppercase = name
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase());
    (starts_with_uppercase && !name.is_empty()).then(|| name.to_string())
}

pub(super) fn infer_render_helpers(render: RenderSource<'_>, ctx: &mut VueRecoveryContext) {
    if ctx.vue_helper_candidates.is_empty() && ctx.vue_helpers.is_empty() {
        return;
    }

    let mut inference = HelperInference {
        candidates: &ctx.vue_helper_candidates,
        known_helpers: &ctx.vue_helpers,
        inferred: HashMap::new(),
        prop_value_depth: 0,
        vnode_child_depth: 0,
    };
    match render {
        RenderSource::Function { render, .. } => {
            if let Some(body) = render.function.body.as_ref() {
                body.visit_with(&mut inference);
            }
        }
        RenderSource::SetupArrow { render, .. } => render.body.visit_with(&mut inference),
    }

    for (local, helper) in inference.inferred {
        ctx.vue_helpers.entry(local).or_insert(helper);
    }
}

struct HelperInference<'a> {
    candidates: &'a std::collections::HashSet<Atom>,
    known_helpers: &'a HashMap<Atom, VueHelper>,
    inferred: HashMap<Atom, VueHelper>,
    prop_value_depth: usize,
    vnode_child_depth: usize,
}

impl Visit for HelperInference<'_> {
    fn visit_if_stmt(&mut self, if_stmt: &IfStmt) {
        self.infer_condition_unref_expr(if_stmt.test.as_ref());
        if_stmt.visit_children_with(self);
    }

    fn visit_cond_expr(&mut self, cond: &CondExpr) {
        self.infer_condition_unref_expr(cond.test.as_ref());
        cond.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        self.infer_unref_expr(member.obj.as_ref());
        member.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if self.prop_value_depth > 0 {
            self.infer_render_prop_unref_call(call);
        }
        if self.vnode_child_depth > 0 {
            self.infer_vnode_child_helper_call(call);
        }

        if let Callee::Expr(callee) = &call.callee {
            self.infer_unref_expr(callee.as_ref());
        }

        if let Some((callee, fragment)) = self.fragment_block_call(call) {
            self.inferred
                .insert(callee.sym.clone(), VueHelper::CreateElementBlock);
            self.inferred
                .insert(fragment.sym.clone(), VueHelper::Fragment);
        }

        if let Some(callee) = self.with_directives_call(call) {
            self.inferred
                .entry(callee.sym.clone())
                .or_insert(VueHelper::WithDirectives);
        }

        if let Some(callee) = call_callee_ident(call) {
            if self.candidates.contains(&callee.sym) {
                if let Some(helper) = infer_call_helper(call) {
                    self.inferred.entry(callee.sym.clone()).or_insert(helper);
                }
            }
        }

        if let Some(VueHelper::CreateElementBlock | VueHelper::CreateElementVNode) =
            self.call_helper(call)
        {
            if let Some(fragment) = call
                .args
                .first()
                .and_then(|arg| ident_expr(arg.expr.as_ref()))
                .filter(|ident| self.candidates.contains(&ident.sym))
            {
                self.inferred
                    .entry(fragment.sym.clone())
                    .or_insert(VueHelper::Fragment);
            }
        }

        if matches!(
            self.call_helper(call),
            Some(VueHelper::CreateBlock | VueHelper::CreateVNode)
        ) {
            self.infer_builtin_component_arg(call);
        }

        if matches!(self.call_helper(call), Some(VueHelper::RenderList)) {
            if let Some(source) = call.args.first() {
                self.infer_render_list_source_unref(source.expr.as_ref());
            }
        }

        if matches!(
            self.call_helper(call),
            Some(
                VueHelper::CreateBlock
                    | VueHelper::CreateElementBlock
                    | VueHelper::CreateElementVNode
                    | VueHelper::CreateVNode
            )
        ) {
            self.infer_render_prop_unrefs(call);
            self.infer_render_child_helpers(call);
        }

        call.visit_children_with(self);
    }
}

impl HelperInference<'_> {
    fn call_helper(&self, call: &CallExpr) -> Option<VueHelper> {
        call_callee_ident(call).and_then(|callee| {
            self.inferred
                .get(&callee.sym)
                .or_else(|| self.known_helpers.get(&callee.sym))
                .cloned()
        })
    }

    fn fragment_block_call<'a>(
        &self,
        call: &'a CallExpr,
    ) -> Option<(
        &'a swc_core::ecma::ast::Ident,
        &'a swc_core::ecma::ast::Ident,
    )> {
        let callee = call_callee_ident(call)?;
        if !self.candidates.contains(&callee.sym) {
            return None;
        }
        if !is_fragment_patch_flag(call.args.get(3).map(|arg| arg.expr.as_ref())) {
            return None;
        }
        let fragment = call
            .args
            .first()
            .and_then(|arg| ident_expr(arg.expr.as_ref()))?;
        Some((callee, fragment))
    }

    fn with_directives_call<'a>(
        &self,
        call: &'a CallExpr,
    ) -> Option<&'a swc_core::ecma::ast::Ident> {
        let callee = call_callee_ident(call)?;
        if !is_with_directives_call(&call.args) {
            return None;
        }
        let base = call.args.first()?;
        self.is_likely_vnode_expr(base.expr.as_ref())
            .then_some(callee)
    }

    fn is_likely_vnode_expr(&self, expr: &Expr) -> bool {
        match unwrap_paren_expr(expr) {
            Expr::Seq(seq) => seq
                .exprs
                .last()
                .is_some_and(|expr| self.is_likely_vnode_expr(expr.as_ref())),
            Expr::Call(call) => self
                .call_helper(call)
                .or_else(|| infer_call_helper(call))
                .is_some_and(|helper| {
                    matches!(
                        helper,
                        VueHelper::CreateBlock
                            | VueHelper::CreateElementBlock
                            | VueHelper::CreateElementVNode
                            | VueHelper::CreateVNode
                    )
                }),
            _ => false,
        }
    }

    fn infer_unref_expr(&mut self, expr: &Expr) {
        let Expr::Call(call) = unwrap_paren_expr(expr) else {
            return;
        };
        if !is_display_string_call(&call.args) {
            return;
        }
        let Some(callee) = call_callee_ident(call) else {
            return;
        };
        if !self.candidates.contains(&callee.sym) {
            return;
        }
        self.inferred.insert(callee.sym.clone(), VueHelper::Unref);
    }

    fn infer_condition_unref_expr(&mut self, expr: &Expr) {
        match unwrap_paren_expr(expr) {
            Expr::Call(_) => self.infer_unref_expr(expr),
            Expr::Unary(unary) if unary.op == UnaryOp::Bang => {
                self.infer_condition_unref_expr(unary.arg.as_ref());
            }
            Expr::Bin(bin)
                if matches!(
                    bin.op,
                    BinaryOp::LogicalAnd
                        | BinaryOp::LogicalOr
                        | BinaryOp::EqEq
                        | BinaryOp::EqEqEq
                        | BinaryOp::NotEq
                        | BinaryOp::NotEqEq
                ) =>
            {
                self.infer_condition_unref_expr(bin.left.as_ref());
                self.infer_condition_unref_expr(bin.right.as_ref());
            }
            Expr::Cond(cond) => {
                self.infer_condition_unref_expr(cond.test.as_ref());
            }
            _ => {}
        }
    }

    fn infer_render_prop_unrefs(&mut self, call: &CallExpr) {
        let Some(props) = call.args.get(1).and_then(|arg| match arg.expr.as_ref() {
            Expr::Object(object) => Some(object),
            _ => None,
        }) else {
            return;
        };

        self.prop_value_depth += 1;
        for prop in &props.props {
            match prop {
                PropOrSpread::Prop(prop) => {
                    if let Prop::KeyValue(key_value) = prop.as_ref() {
                        key_value.value.visit_with(self);
                    }
                }
                PropOrSpread::Spread(spread) => {
                    spread.expr.visit_with(self);
                }
            }
        }
        self.prop_value_depth -= 1;
    }

    fn infer_render_prop_unref_call(&mut self, call: &CallExpr) {
        if !is_render_prop_unref_call(&call.args) {
            return;
        }
        let Some(callee) = call_callee_ident(call) else {
            return;
        };
        if !self.candidates.contains(&callee.sym) {
            return;
        }
        self.inferred.insert(callee.sym.clone(), VueHelper::Unref);
    }

    fn infer_render_child_helpers(&mut self, call: &CallExpr) {
        let Some(children) = call.args.get(2) else {
            return;
        };
        self.vnode_child_depth += 1;
        children.expr.visit_with(self);
        self.vnode_child_depth -= 1;
    }

    fn infer_vnode_child_helper_call(&mut self, call: &CallExpr) {
        if !is_static_text_vnode_call(&call.args) {
            return;
        }
        let Some(callee) = call_callee_ident(call) else {
            return;
        };
        if !self.candidates.contains(&callee.sym) {
            return;
        }
        self.inferred
            .entry(callee.sym.clone())
            .or_insert(VueHelper::CreateTextVNode);
    }

    fn infer_render_list_source_unref(&mut self, expr: &Expr) {
        let Expr::Call(call) = unwrap_paren_expr(expr) else {
            return;
        };
        self.infer_render_prop_unref_call(call);
    }

    fn infer_builtin_component_arg(&mut self, call: &CallExpr) {
        let Some(component) = call
            .args
            .first()
            .and_then(|arg| ident_expr(arg.expr.as_ref()))
            .filter(|ident| self.candidates.contains(&ident.sym))
        else {
            return;
        };
        let Some(props) = call.args.get(1).and_then(|arg| match arg.expr.as_ref() {
            Expr::Object(object) => Some(object),
            _ => None,
        }) else {
            return;
        };
        if is_transition_component_props(props) {
            self.inferred
                .entry(component.sym.clone())
                .or_insert(VueHelper::Other("Transition".to_string()));
        }
    }
}

fn is_render_prop_unref_call(args: &[ExprOrSpread]) -> bool {
    if args.len() != 1 {
        return false;
    }
    matches!(
        args.first().map(|arg| unwrap_paren_expr(arg.expr.as_ref())),
        Some(Expr::Ident(_) | Expr::Member(_) | Expr::OptChain(_))
    )
}

fn is_transition_component_props(object: &ObjectLit) -> bool {
    object.props.iter().any(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return false;
        };
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            return false;
        };
        matches!(
            prop_name(&key_value.key).as_deref(),
            Some(
                "onBeforeEnter"
                    | "onEnter"
                    | "onAfterEnter"
                    | "onEnterCancelled"
                    | "onBeforeLeave"
                    | "onLeave"
                    | "onAfterLeave"
                    | "onLeaveCancelled"
            )
        )
    })
}

fn is_fragment_patch_flag(expr: Option<&Expr>) -> bool {
    matches!(
        expr,
        Some(Expr::Lit(Lit::Num(number)))
            if matches!(number.value as i32, 64 | 128 | 256)
    )
}

pub(super) fn unwrap_paren_expr(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => unwrap_paren_expr(paren.expr.as_ref()),
        _ => expr,
    }
}

fn infer_call_helper(call: &CallExpr) -> Option<VueHelper> {
    if is_with_directives_call(&call.args) {
        return Some(VueHelper::WithDirectives);
    }
    if is_with_memo_call(&call.args) {
        return Some(VueHelper::WithMemo);
    }
    if is_create_slots_call(&call.args) {
        return Some(VueHelper::CreateSlots);
    }
    if is_render_slot_call(&call.args) {
        return Some(VueHelper::RenderSlot);
    }
    if is_render_list_call(&call.args) {
        return Some(VueHelper::RenderList);
    }
    if is_event_modifier_helper_call(&call.args) {
        return Some(VueHelper::WithModifiers);
    }
    if is_with_ctx_call(&call.args) {
        return Some(VueHelper::WithCtx);
    }
    if is_create_static_vnode_call(&call.args) {
        return Some(VueHelper::CreateStaticVNode);
    }
    if is_create_comment_vnode_call(&call.args) {
        return Some(VueHelper::CreateCommentVNode);
    }
    if is_create_text_vnode_call(&call.args) {
        return Some(VueHelper::CreateTextVNode);
    }
    if is_element_vnode_call(&call.args) {
        return Some(VueHelper::CreateElementBlock);
    }
    if is_component_vnode_call(&call.args) {
        return Some(VueHelper::CreateVNode);
    }
    if is_resolve_component_call(&call.args) {
        return Some(VueHelper::ResolveComponent);
    }
    if is_display_string_call(&call.args) {
        return Some(VueHelper::ToDisplayString);
    }
    if is_open_block_call(&call.args) {
        return Some(VueHelper::OpenBlock);
    }
    None
}

fn is_with_directives_call(args: &[ExprOrSpread]) -> bool {
    matches!(args.get(1).map(|arg| arg.expr.as_ref()), Some(Expr::Array(array)) if array.elems.iter().flatten().any(|elem| matches!(elem.expr.as_ref(), Expr::Array(_))))
}

fn is_with_memo_call(args: &[ExprOrSpread]) -> bool {
    args.len() >= 4
        && matches!(
            args.get(1).map(|arg| arg.expr.as_ref()),
            Some(Expr::Arrow(_))
        )
}

fn is_create_slots_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        args.first().map(|arg| arg.expr.as_ref()),
        Some(Expr::Object(_))
    ) && matches!(
        args.get(1).map(|arg| arg.expr.as_ref()),
        Some(Expr::Array(_))
    )
}

fn is_render_slot_call(args: &[ExprOrSpread]) -> bool {
    args.len() >= 2
        && args
            .first()
            .is_some_and(|arg| is_slots_source_expr(arg.expr.as_ref()))
}

fn is_slots_source_expr(expr: &Expr) -> bool {
    match unwrap_paren_expr(expr) {
        Expr::Ident(ident) => matches!(ident.sym.as_ref(), "$slots" | "slots"),
        Expr::Member(member) => is_slots_member_prop(&member.prop),
        _ => false,
    }
}

fn is_slots_member_prop(prop: &MemberProp) -> bool {
    match prop {
        MemberProp::Ident(ident) => ident.sym.as_ref() == "$slots",
        MemberProp::Computed(computed) => {
            string_lit(computed.expr.as_ref()).as_deref() == Some("$slots")
        }
        MemberProp::PrivateName(_) => false,
    }
}

fn is_setup_slots_member_prop(prop: &MemberProp) -> bool {
    match prop {
        MemberProp::Ident(ident) => matches!(ident.sym.as_ref(), "$slots" | "slots"),
        MemberProp::Computed(computed) => {
            matches!(
                string_lit(computed.expr.as_ref()).as_deref(),
                Some("$slots" | "slots")
            )
        }
        MemberProp::PrivateName(_) => false,
    }
}

fn is_render_list_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        args.get(1).map(|arg| arg.expr.as_ref()),
        Some(Expr::Arrow(_))
    )
}

fn is_event_modifier_helper_call(args: &[ExprOrSpread]) -> bool {
    if args.len() != 2 {
        return false;
    }

    let Some(modifiers) = args.get(1).and_then(|arg| match arg.expr.as_ref() {
        Expr::Array(array) => Some(array),
        _ => None,
    }) else {
        return false;
    };
    if modifiers
        .elems
        .iter()
        .flatten()
        .any(|elem| string_lit(elem.expr.as_ref()).is_none())
    {
        return false;
    }

    matches!(
        args.first().map(|arg| unwrap_paren_expr(arg.expr.as_ref())),
        Some(
            Expr::Ident(_)
                | Expr::Member(_)
                | Expr::Call(_)
                | Expr::Arrow(_)
                | Expr::Fn(_)
                | Expr::Bin(_)
                | Expr::Assign(_)
        )
    )
}

fn is_with_ctx_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        args.first().map(|arg| arg.expr.as_ref()),
        Some(Expr::Arrow(_))
    )
}

fn is_create_static_vnode_call(args: &[ExprOrSpread]) -> bool {
    args.first()
        .and_then(|arg| string_lit(arg.expr.as_ref()))
        .is_some_and(|value| value.contains('<'))
}

fn is_create_comment_vnode_call(args: &[ExprOrSpread]) -> bool {
    args.first()
        .is_some_and(|arg| string_lit(arg.expr.as_ref()).is_some())
        && matches!(
            args.get(1).map(|arg| arg.expr.as_ref()),
            Some(Expr::Lit(Lit::Bool(_)))
        )
}

fn is_create_text_vnode_call(args: &[ExprOrSpread]) -> bool {
    args.get(1)
        .is_some_and(|arg| is_numeric_expr(arg.expr.as_ref()))
}

fn is_static_text_vnode_call(args: &[ExprOrSpread]) -> bool {
    matches!(args.len(), 1 | 2)
        && args
            .first()
            .is_some_and(|arg| string_lit(arg.expr.as_ref()).is_some())
        && args
            .get(1)
            .is_none_or(|arg| is_numeric_expr(arg.expr.as_ref()))
}

fn is_numeric_expr(expr: &Expr) -> bool {
    match unwrap_paren_expr(expr) {
        Expr::Lit(Lit::Num(_)) => true,
        Expr::Unary(unary) if unary.op == UnaryOp::Minus => {
            matches!(
                unwrap_paren_expr(unary.arg.as_ref()),
                Expr::Lit(Lit::Num(_))
            )
        }
        _ => false,
    }
}

fn is_element_vnode_call(args: &[ExprOrSpread]) -> bool {
    args.len() >= 2
        && args
            .first()
            .and_then(|arg| string_lit(arg.expr.as_ref()))
            .is_some_and(|value| !value.contains('<'))
}

fn is_component_vnode_call(args: &[ExprOrSpread]) -> bool {
    args.len() >= 2
        && !matches!(
            args.first().map(|arg| arg.expr.as_ref()),
            Some(Expr::Lit(Lit::Str(_)) | Expr::Object(_))
        )
}

fn is_resolve_component_call(args: &[ExprOrSpread]) -> bool {
    args.len() == 1
        && args
            .first()
            .is_some_and(|arg| string_lit(arg.expr.as_ref()).is_some())
}

fn is_display_string_call(args: &[ExprOrSpread]) -> bool {
    args.len() == 1
        && args
            .first()
            .is_none_or(|arg| string_lit(arg.expr.as_ref()).is_none())
}

fn is_open_block_call(args: &[ExprOrSpread]) -> bool {
    args.is_empty()
        || matches!(
            args.first().map(|arg| arg.expr.as_ref()),
            Some(Expr::Lit(Lit::Bool(_)))
        )
}

pub(super) fn call_callee_ident(call: &CallExpr) -> Option<&swc_core::ecma::ast::Ident> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    ident_expr(callee.as_ref())
}

fn ident_expr(expr: &Expr) -> Option<&swc_core::ecma::ast::Ident> {
    match expr {
        Expr::Ident(ident) => Some(ident),
        _ => None,
    }
}

pub(super) fn collect_render_context(render: RenderSource<'_>, ctx: &mut VueRecoveryContext) {
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

pub(super) fn collect_setup_context(
    render: RenderSource<'_>,
    ctx: &mut VueRecoveryContext,
) -> Result<()> {
    let RenderSource::SetupArrow {
        render,
        setup_stmts,
        setup_slots,
        ..
    } = render
    else {
        return Ok(());
    };
    if let Some(setup_slots) = setup_slots {
        ctx.slot_bindings.insert(setup_slots.sym.clone());
    }

    let setup_tuple_value_candidates = setup_tuple_value_candidates(setup_stmts);
    let setup_template_ref_refs =
        setup_render_template_ref_refs(render, setup_stmts, ctx, &setup_tuple_value_candidates);
    let setup_template_ref_aliases = setup_render_template_ref_aliases(render);
    let setup_template_ref_alias_sources = setup_template_ref_aliases
        .iter()
        .map(|(from, _)| from.clone())
        .collect::<HashSet<_>>();
    let setup_ref_object_alias_refs = setup_ref_object_alias_refs(setup_stmts);
    let setup_non_value_member_refs = setup_non_value_member_refs(setup_stmts);
    let setup_value_member_refs = setup_value_member_refs(render, setup_stmts);
    let setup_render_refs = render_ident_refs(render);
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
                });
            }
            Stmt::Decl(Decl::Class(class)) => {
                local_candidates.push(SetupLocalCandidate {
                    bindings: vec![class.ident.sym.clone()],
                    stmt: stmt.clone(),
                    template_selectable: true,
                    setup_order,
                });
            }
            Stmt::Decl(Decl::Var(var)) => {
                let mut local_decls = Vec::new();
                let mut local_bindings = HashSet::new();

                for decl in &var.decls {
                    let consumed = match decl.init.as_deref() {
                        Some(init) => match &decl.name {
                            Pat::Ident(binding) => {
                                if is_setup_props_alias(init, ctx) {
                                    ctx.setup_props_aliases.insert(binding.id.sym.clone());
                                    true
                                } else if is_setup_emit_alias(init, ctx) {
                                    ctx.setup_emit_aliases.insert(binding.id.sym.clone());
                                    true
                                } else if is_setup_slot_alias(init, ctx) {
                                    ctx.slot_bindings.insert(binding.id.sym.clone());
                                    true
                                } else if let Some(alias) = ident_expr(unwrap_paren_expr(init)) {
                                    ctx.bindings
                                        .aliases
                                        .insert(binding.id.sym.clone(), alias.sym.clone());
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
                                    if setup_value_member_refs.contains(&binding.id.sym)
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
                                        if setup_value_member_refs.contains(&binding.id.sym) {
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
                                        && !setup_non_value_member_refs.contains(&binding.id.sym)
                                        && (setup_template_ref_alias_sources
                                            .contains(&binding.id.sym)
                                            || should_emit_ref_script_setup_expr(
                                                init,
                                                ctx,
                                                &binding.id.sym,
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
                            has_template_ref
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
                    });
                }
            }
            _ => {}
        }
    }

    for (from, to) in setup_template_ref_aliases {
        if ctx
            .setup_ref_script_bindings
            .iter()
            .any(|binding| binding.binding == from)
        {
            ctx.bindings.aliases.insert(from, to);
        }
    }
    refresh_setup_value_binding_sources(ctx)?;

    assign_setup_prop_bindings(ctx, &local_candidates);

    for candidate in local_candidates {
        let cleaned_stmt = clean_setup_stmt(&candidate.stmt, ctx);
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

fn setup_non_value_member_refs(stmts: &[Stmt]) -> HashSet<Atom> {
    let mut collector = NonValueMemberRefCollector {
        scopes: ScopeStack::new(),
        refs: HashSet::new(),
    };
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
    collector.refs
}

fn setup_value_member_refs(render: &ArrowExpr, setup_stmts: &[Stmt]) -> HashSet<Atom> {
    let mut collector = ValueMemberIdentRefCollector {
        scopes: ScopeStack::new(),
        refs: HashSet::new(),
    };
    for stmt in setup_stmts {
        stmt.visit_with(&mut collector);
    }
    render.visit_with(&mut collector);
    collector.refs
}

struct ValueMemberIdentRefCollector {
    scopes: ScopeStack,
    refs: HashSet<Atom>,
}

impl ValueMemberIdentRefCollector {
    fn declare_if_nested(&mut self, sym: &Atom) {
        if self.scopes.depth() > 1 {
            self.scopes.declare(sym);
        }
    }
}

impl Visit for ValueMemberIdentRefCollector {
    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "value") {
            if let Expr::Ident(object) = member.obj.as_ref() {
                if !self.scopes.is_shadowed(&object.sym) {
                    self.refs.insert(object.sym.clone());
                }
            }
        }
        member.visit_children_with(self);
    }

    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        self.declare_if_nested(&ident.id.sym);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if self.scopes.depth() > 1 {
            self.scopes.declare_pat(&declarator.name);
        }
        if let Some(init) = &declarator.init {
            init.visit_with(self);
        }
    }

    fn visit_fn_decl(&mut self, function: &FnDecl) {
        self.declare_if_nested(&function.ident.sym);
        self.scopes.push_scope();
        for param in &function.function.params {
            self.scopes.declare_pat(&param.pat);
        }
        function.function.visit_with(self);
        self.scopes.pop_scope();
    }

    fn visit_function(&mut self, function: &Function) {
        self.scopes.push_scope();
        for param in &function.params {
            self.scopes.declare_pat(&param.pat);
        }
        if let Some(body) = &function.body {
            body.visit_with(self);
        }
        self.scopes.pop_scope();
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.scopes.push_scope();
        for param in &arrow.params {
            self.scopes.declare_pat(param);
        }
        arrow.body.visit_with(self);
        self.scopes.pop_scope();
    }

    fn visit_class_decl(&mut self, class: &ClassDecl) {
        self.declare_if_nested(&class.ident.sym);
        class.class.visit_with(self);
    }
}

struct NonValueMemberRefCollector {
    scopes: ScopeStack,
    refs: HashSet<Atom>,
}

impl NonValueMemberRefCollector {
    fn declare_if_nested(&mut self, sym: &Atom) {
        if self.scopes.depth() > 1 {
            self.scopes.declare(sym);
        }
    }
}

impl Visit for NonValueMemberRefCollector {
    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "value") {
            if let Expr::Ident(object) = member.obj.as_ref() {
                if !self.scopes.is_shadowed(&object.sym) {
                    self.refs.insert(object.sym.clone());
                }
            }
        }
        member.visit_children_with(self);
    }

    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        self.declare_if_nested(&ident.id.sym);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if self.scopes.depth() > 1 {
            self.scopes.declare_pat(&declarator.name);
        }
        if let Some(init) = &declarator.init {
            init.visit_with(self);
        }
    }

    fn visit_fn_decl(&mut self, function: &FnDecl) {
        self.declare_if_nested(&function.ident.sym);
        self.visit_function(&function.function);
    }

    fn visit_function(&mut self, function: &Function) {
        self.scopes.push_scope();
        for param in &function.params {
            self.scopes.declare_pat(&param.pat);
        }
        if let Some(body) = &function.body {
            body.visit_with(self);
        }
        self.scopes.pop_scope();
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.scopes.push_scope();
        for param in &arrow.params {
            self.scopes.declare_pat(param);
        }
        arrow.body.visit_with(self);
        self.scopes.pop_scope();
    }

    fn visit_class_decl(&mut self, class: &ClassDecl) {
        self.declare_if_nested(&class.ident.sym);
        class.class.visit_with(self);
    }
}

fn setup_render_template_ref_aliases(render: &ArrowExpr) -> Vec<(Atom, Atom)> {
    let mut collector = TemplateRefAliasCollector {
        aliases: Vec::new(),
    };
    render.visit_with(&mut collector);
    collector.aliases
}

struct TemplateRefAliasCollector {
    aliases: Vec<(Atom, Atom)>,
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
                        ref_binding = Some(ident.sym.clone());
                    }
                }
                _ => {}
            }
        }

        if let (Some(from), Some(to)) = (ref_binding, ref_key) {
            self.aliases.push((from, to));
        }

        object.visit_children_with(self);
    }
}

fn render_ident_refs(render: &ArrowExpr) -> HashSet<Atom> {
    let mut collector = IdentRefCollector {
        scopes: ScopeStack::new(),
        refs: HashSet::new(),
    };
    render.visit_with(&mut collector);
    collector.refs
}

fn setup_tuple_value_candidates(setup_stmts: &[Stmt]) -> HashSet<Atom> {
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
                Pat::Array(_) => collect_pat_bindings(&decl.name, &mut tuple_value_candidates),
                Pat::Ident(binding) if is_tuple_element_expr(init) => {
                    tuple_value_candidates.insert(binding.id.sym.clone());
                }
                _ => {}
            }
        }
    }
    tuple_value_candidates
}

fn setup_render_template_ref_refs(
    render: &ArrowExpr,
    setup_stmts: &[Stmt],
    ctx: &VueRecoveryContext,
    tuple_value_candidates: &HashSet<Atom>,
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
                collect_pat_bindings(&decl.name, &mut object_value_candidates);
                collect_pat_bindings(&decl.name, &mut unref_candidates);
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
        scopes: ScopeStack::new(),
        tuple_value_refs: HashSet::new(),
        object_value_refs: HashSet::new(),
        unref_refs: HashSet::new(),
    };
    render.visit_with(&mut collector);
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
    tuple_value_candidates: &HashSet<Atom>,
    ctx: &mut VueRecoveryContext,
) {
    if tuple_value_candidates.is_empty() {
        return;
    }
    let Some(expr) = value.expr.as_ref() else {
        return;
    };
    for ref_name in value_member_refs_in_expr(expr) {
        if tuple_value_candidates.contains(&ref_name) {
            ctx.bindings.template_refs.insert(ref_name);
        }
    }
}

fn value_member_refs_in_expr(expr: &Expr) -> HashSet<Atom> {
    let mut collector = ValueMemberIdentRefCollector {
        scopes: ScopeStack::new(),
        refs: HashSet::new(),
    };
    expr.visit_with(&mut collector);
    collector.refs
}

struct RenderTemplateRefCollector<'a> {
    tuple_value_candidates: &'a HashSet<Atom>,
    object_value_candidates: &'a HashSet<Atom>,
    unref_candidates: &'a HashSet<Atom>,
    ctx: &'a VueRecoveryContext,
    scopes: ScopeStack,
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
        if self.scopes.is_shadowed(&object.sym) {
            return;
        }
        if self.tuple_value_candidates.contains(&object.sym) {
            self.tuple_value_refs.insert(object.sym.clone());
        }
        if self.object_value_candidates.contains(&object.sym) {
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
        if self.unref_candidates.contains(&object.sym) && !self.scopes.is_shadowed(&object.sym) {
            self.unref_refs.insert(object.sym.clone());
        }
    }
}

impl Visit for RenderTemplateRefCollector<'_> {
    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left {
            self.collect_value_member(member);
        }
        assign.visit_children_with(self);
    }

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

    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        self.scopes.declare(&ident.id.sym);
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

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let Some(init) = &declarator.init {
            init.visit_with(self);
        }
        self.scopes.declare_pat(&declarator.name);
    }

    fn visit_fn_decl(&mut self, function: &FnDecl) {
        self.scopes.declare(&function.ident.sym);
        self.visit_function(&function.function);
    }

    fn visit_function(&mut self, function: &Function) {
        self.scopes.push_scope();
        for param in &function.params {
            self.scopes.declare_pat(&param.pat);
        }
        if let Some(body) = &function.body {
            body.visit_with(self);
        }
        self.scopes.pop_scope();
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.scopes.push_scope();
        for param in &arrow.params {
            self.scopes.declare_pat(param);
        }
        arrow.body.visit_with(self);
        self.scopes.pop_scope();
    }

    fn visit_class_decl(&mut self, class: &ClassDecl) {
        self.scopes.declare(&class.ident.sym);
        class.class.visit_with(self);
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

fn is_setup_props_alias(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    let Expr::Ident(ident) = unwrap_paren_expr(expr) else {
        return false;
    };
    ctx.setup_props_context
        .as_ref()
        .is_some_and(|setup_props| setup_props == &ident.sym)
        || ctx.setup_props_aliases.contains(&ident.sym)
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
                ctx.vue_helper_candidates.contains(&callee.sym)
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

fn member_prop_is_named(prop: &MemberProp, name: &str) -> bool {
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
    call_callee_ident(call).is_some_and(|callee| ctx.vue_helper_candidates.contains(&callee.sym))
}

fn should_emit_ref_script_setup_expr(
    expr: &Expr,
    ctx: &VueRecoveryContext,
    binding: &Atom,
    value_member_refs: &HashSet<Atom>,
) -> bool {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return false;
    };
    match helper_name(&call.callee, ctx) {
        Some(VueHelper::Computed) => return true,
        Some(VueHelper::Other(name)) if is_ref_like_vue_helper(&name) => return true,
        _ => {}
    }
    call_callee_ident(call).is_some_and(|callee| ctx.vue_helper_candidates.contains(&callee.sym))
        && value_member_refs.contains(binding)
}

fn is_ref_like_vue_helper(name: &str) -> bool {
    matches!(name, "ref" | "shallowRef" | "customRef" | "toRef")
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
            .filter(|callee| ctx.vue_helper_candidates.contains(&callee.sym))
            .map(|_| "ref".to_string()),
    }
}

pub(super) fn is_ref_object_expr(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return false;
    };
    match helper_name(&call.callee, ctx) {
        Some(VueHelper::Other(name)) if is_ref_object_helper(&name) => return true,
        _ => {}
    }
    call_callee_ident(call).is_some_and(|callee| ctx.vue_helper_candidates.contains(&callee.sym))
}

pub(super) fn is_ref_object_alias(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
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

fn provider_ref_props_from_init(expr: &Expr, ctx: &VueRecoveryContext) -> Option<HashSet<Atom>> {
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
            BlockStmtOrExpr::BlockStmt(block) => {
                provider_ref_props_from_stmts(block.stmts.as_slice(), ctx)
            }
            BlockStmtOrExpr::Expr(expr) => provider_ref_props_from_return_expr(expr.as_ref(), ctx),
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

fn collect_pat_bindings(pat: &Pat, bindings: &mut HashSet<Atom>) {
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

pub(super) fn render_context_param(render: RenderSource<'_>) -> Option<Atom> {
    match render {
        RenderSource::Function { render, .. } => render
            .function
            .params
            .first()
            .and_then(param_binding_ident)
            .map(|ident| ident.sym.clone()),
        RenderSource::SetupArrow { render, .. } => {
            render.params.first().and_then(|param| match param {
                Pat::Ident(binding) => Some(binding.id.sym.clone()),
                _ => None,
            })
        }
    }
}

pub(super) fn setup_props_param(render: RenderSource<'_>) -> Option<Atom> {
    match render {
        RenderSource::SetupArrow {
            setup_props: Some(setup_props),
            ..
        } => Some(setup_props.sym.clone()),
        _ => None,
    }
}

pub(super) fn setup_context_param(render: RenderSource<'_>) -> Option<Atom> {
    match render {
        RenderSource::SetupArrow {
            setup_context: Some(setup_context),
            ..
        } => Some(setup_context.sym.clone()),
        _ => None,
    }
}

pub(super) fn setup_emit_param(render: RenderSource<'_>) -> Option<Atom> {
    match render {
        RenderSource::SetupArrow {
            setup_emit: Some(setup_emit),
            ..
        } => Some(setup_emit.sym.clone()),
        _ => None,
    }
}

fn render_stmts(render: RenderSource<'_>) -> Option<&[Stmt]> {
    match render {
        RenderSource::Function { render, .. } => render
            .function
            .body
            .as_ref()
            .map(|body| body.stmts.as_slice()),
        RenderSource::SetupArrow { render, .. } => match render.body.as_ref() {
            BlockStmtOrExpr::BlockStmt(block) => Some(block.stmts.as_slice()),
            BlockStmtOrExpr::Expr(_) => None,
        },
    }
}

fn refresh_setup_value_binding_sources(ctx: &mut VueRecoveryContext) -> Result<()> {
    let bindings = ctx.bindings.values.clone();
    for (binding, value) in bindings {
        let Some(expr) = value.expr else {
            continue;
        };
        let value = clean_expr(&print_expr(&expr, ctx)?, ctx);
        if let Some(binding) = ctx.bindings.values.get_mut(&binding) {
            binding.value = value;
        }
    }
    Ok(())
}

fn computed_value_expr(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<VueSetupValueBinding>> {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return Ok(None);
    };
    if !is_computed_call(call, ctx) {
        return Ok(None);
    }
    let Some(arg) = call.args.first() else {
        return Ok(None);
    };
    let Some(binding) = computed_getter_expr(arg.expr.as_ref(), ctx)? else {
        return Ok(None);
    };
    if should_inline_computed_template_binding(&binding) {
        Ok(Some(binding))
    } else {
        Ok(None)
    }
}

fn should_inline_computed_template_binding(binding: &VueSetupValueBinding) -> bool {
    !computed_value_contains_block_function(&binding.value)
        && !should_preserve_long_computed_template_binding(&binding.value)
}

fn computed_value_contains_block_function(value: &str) -> bool {
    value.contains("function") || value_contains_block_arrow(value)
}

fn should_preserve_long_computed_template_binding(value: &str) -> bool {
    let mut value = value.trim();
    while let Some(inner) = value.strip_prefix('(') {
        value = inner.trim_start();
    }
    // Keep class/style-friendly literal values inline; preserve long computed
    // expressions where a named binding is usually easier to read.
    value.len() > MAX_INLINE_COMPUTED_TEMPLATE_BINDING_LEN
        && !value.starts_with('[')
        && !value.starts_with('{')
}

fn value_contains_block_arrow(value: &str) -> bool {
    let mut cursor = 0;
    while let Some(relative_arrow) = value[cursor..].find("=>") {
        let arrow = cursor + relative_arrow + "=>".len();
        let rest = &value[arrow..];
        let body = rest.trim_start();
        if body.starts_with('{') {
            return true;
        }
        cursor = arrow;
    }
    false
}

fn computed_script_setup_expr(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<(String, HashSet<Atom>)>> {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return Ok(None);
    };
    let Some(arg) = call.args.first() else {
        return Ok(None);
    };
    if !is_computed_script_setup_call(call, arg.expr.as_ref(), ctx) {
        return Ok(None);
    }
    let getter = computed_script_setup_getter(arg.expr.as_ref(), ctx)?;
    let import_refs = script_import_refs(arg.expr.as_ref(), &ctx.script_imports);
    Ok(Some((format!("computed({getter})"), import_refs)))
}

fn computed_script_setup_getter(expr: &Expr, ctx: &VueRecoveryContext) -> Result<String> {
    let getter = clean_expr(&print_expr(expr, ctx)?, ctx);
    if arrow_returns_object_expr(expr) {
        Ok(wrap_arrow_object_return(&getter))
    } else {
        Ok(getter)
    }
}

fn arrow_returns_object_expr(expr: &Expr) -> bool {
    let Expr::Arrow(arrow) = unwrap_paren_expr(expr) else {
        return false;
    };
    matches!(
        arrow.body.as_ref(),
        BlockStmtOrExpr::Expr(expr) if matches!(unwrap_paren_expr(expr.as_ref()), Expr::Object(_))
    )
}

fn wrap_arrow_object_return(getter: &str) -> String {
    let Some(arrow_index) = getter.find("=>") else {
        return getter.to_string();
    };
    let body_start = arrow_index + "=>".len();
    let leading_ws = getter[body_start..]
        .chars()
        .take_while(|ch| ch.is_whitespace())
        .map(char::len_utf8)
        .sum::<usize>();
    let object_start = body_start + leading_ws;
    if !getter[object_start..].starts_with('{') {
        return getter.to_string();
    }

    let mut output = String::with_capacity(getter.len() + 2);
    output.push_str(&getter[..object_start]);
    output.push('(');
    output.push_str(&getter[object_start..]);
    output.push(')');
    output
}

fn is_computed_script_setup_call(call: &CallExpr, getter: &Expr, ctx: &VueRecoveryContext) -> bool {
    let is_getter = matches!(unwrap_paren_expr(getter), Expr::Arrow(_) | Expr::Fn(_));
    if !is_getter {
        return false;
    }
    helper_name(&call.callee, ctx) == Some(VueHelper::Computed)
        || call_callee_ident(call)
            .is_some_and(|callee| ctx.vue_helper_candidates.contains(&callee.sym))
}

fn script_import_refs(expr: &Expr, imports: &HashMap<Atom, VueScriptImport>) -> HashSet<Atom> {
    let mut collector = ScriptImportRefCollector {
        imports,
        scopes: ScopeStack::new(),
        refs: HashSet::new(),
    };
    expr.visit_with(&mut collector);
    collector.refs
}

fn stmt_import_refs(stmt: &Stmt, imports: &HashMap<Atom, VueScriptImport>) -> HashSet<Atom> {
    let mut collector = ScriptImportRefCollector {
        imports,
        scopes: ScopeStack::new(),
        refs: HashSet::new(),
    };
    stmt.visit_with(&mut collector);
    collector.refs
}

pub(super) fn stmt_ident_refs(stmt: &Stmt) -> HashSet<Atom> {
    let mut collector = IdentRefCollector {
        scopes: ScopeStack::new(),
        refs: HashSet::new(),
    };
    stmt.visit_with(&mut collector);
    collector.refs
}

fn expr_ident_refs(expr: &Expr) -> HashSet<Atom> {
    let mut collector = IdentRefCollector {
        scopes: ScopeStack::new(),
        refs: HashSet::new(),
    };
    expr.visit_with(&mut collector);
    collector.refs
}

struct IdentRefCollector {
    scopes: ScopeStack,
    refs: HashSet<Atom>,
}

impl Visit for IdentRefCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        if !self.scopes.is_shadowed(&ident.sym) {
            self.refs.insert(ident.sym.clone());
        }
    }

    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        self.scopes.declare(&ident.id.sym);
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

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        self.scopes.declare_pat(&declarator.name);
        if let Some(init) = &declarator.init {
            init.visit_with(self);
        }
    }

    fn visit_fn_decl(&mut self, function: &FnDecl) {
        self.scopes.declare(&function.ident.sym);
        self.visit_function(&function.function);
    }

    fn visit_function(&mut self, function: &Function) {
        self.scopes.push_scope();
        for param in &function.params {
            self.scopes.declare_pat(&param.pat);
        }
        if let Some(body) = &function.body {
            body.visit_with(self);
        }
        self.scopes.pop_scope();
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.scopes.push_scope();
        for param in &arrow.params {
            self.scopes.declare_pat(param);
        }
        arrow.body.visit_with(self);
        self.scopes.pop_scope();
    }

    fn visit_class_decl(&mut self, class: &ClassDecl) {
        self.scopes.declare(&class.ident.sym);
        class.class.visit_with(self);
    }
}

struct ScriptImportRefCollector<'a> {
    imports: &'a HashMap<Atom, VueScriptImport>,
    scopes: ScopeStack,
    refs: HashSet<Atom>,
}

impl Visit for ScriptImportRefCollector<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if self.imports.contains_key(&ident.sym) && !self.scopes.is_shadowed(&ident.sym) {
            self.refs.insert(ident.sym.clone());
        }
    }

    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        self.scopes.declare(&ident.id.sym);
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

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        self.scopes.declare_pat(&declarator.name);
        if let Some(init) = &declarator.init {
            init.visit_with(self);
        }
    }

    fn visit_fn_decl(&mut self, function: &FnDecl) {
        self.scopes.declare(&function.ident.sym);
        self.visit_function(&function.function);
    }

    fn visit_function(&mut self, function: &Function) {
        self.scopes.push_scope();
        for param in &function.params {
            self.scopes.declare_pat(&param.pat);
        }
        if let Some(body) = &function.body {
            body.visit_with(self);
        }
        self.scopes.pop_scope();
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.scopes.push_scope();
        for param in &arrow.params {
            self.scopes.declare_pat(param);
        }
        arrow.body.visit_with(self);
        self.scopes.pop_scope();
    }

    fn visit_class_decl(&mut self, class: &ClassDecl) {
        self.scopes.declare(&class.ident.sym);
        class.class.visit_with(self);
    }
}

fn is_computed_call(call: &CallExpr, ctx: &VueRecoveryContext) -> bool {
    if helper_name(&call.callee, ctx) == Some(VueHelper::Computed) {
        return true;
    }
    call_callee_ident(call).is_some_and(|callee| ctx.vue_helper_candidates.contains(&callee.sym))
}

fn computed_getter_expr(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<VueSetupValueBinding>> {
    let Expr::Arrow(arrow) = unwrap_paren_expr(expr) else {
        return Ok(None);
    };
    match arrow.body.as_ref() {
        BlockStmtOrExpr::Expr(expr) => Ok(Some(VueSetupValueBinding {
            value: clean_expr(&print_expr(expr.as_ref(), ctx)?, ctx),
            expr: Some(*expr.clone()),
        })),
        BlockStmtOrExpr::BlockStmt(block) => computed_block_value_expr(&block.stmts, ctx),
    }
}

fn computed_block_value_expr(
    stmts: &[Stmt],
    ctx: &VueRecoveryContext,
) -> Result<Option<VueSetupValueBinding>> {
    if let Some(expr) = computed_if_return_chain_expr(stmts, ctx)? {
        return Ok(Some(VueSetupValueBinding {
            value: expr,
            expr: None,
        }));
    }

    let Some((return_index, expr)) = computed_final_return_expr(stmts) else {
        return Ok(None);
    };
    let prior_stmts = &stmts[..return_index];
    if let Some(expr) = computed_array_push_expr(prior_stmts, expr, ctx)? {
        return Ok(Some(expr));
    }
    if !computed_prior_stmts_are_inlineable(prior_stmts, ctx) {
        return Ok(None);
    }
    let local_exprs = computed_block_local_exprs(prior_stmts);
    let mutated_locals = computed_mutated_local_bindings(prior_stmts, &local_exprs);
    if computed_local_ref_counts(expr, &mutated_locals)
        .values()
        .any(|count| *count > 0)
    {
        return Ok(None);
    }
    let expr = inline_computed_block_locals(expr, prior_stmts);
    let local_exprs = computed_block_local_exprs(prior_stmts);
    if computed_local_ref_counts(&expr, &local_exprs)
        .values()
        .any(|count| *count > 0)
    {
        return Ok(None);
    }
    let expr = inline_computed_setup_prop_aliases(&expr, &stmts[..return_index], ctx);
    Ok(Some(VueSetupValueBinding {
        value: clean_expr(&print_expr(&expr, ctx)?, ctx),
        expr: Some(expr),
    }))
}

fn computed_prior_stmts_are_inlineable(stmts: &[Stmt], ctx: &VueRecoveryContext) -> bool {
    stmts.iter().all(|stmt| match stmt {
        Stmt::Decl(Decl::Var(var)) => {
            if var.kind != VarDeclKind::Const || var.decls.is_empty() {
                return false;
            }
            var.decls.iter().all(|decl| {
                decl.init.is_some()
                    && (matches!(decl.name, Pat::Ident(_))
                        || matches!(decl.name, Pat::Object(_))
                            && decl
                                .init
                                .as_deref()
                                .is_some_and(|init| is_setup_props_alias(init, ctx)))
            })
        }
        _ => false,
    })
}

fn computed_array_push_expr(
    stmts: &[Stmt],
    return_expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<VueSetupValueBinding>> {
    let Expr::Ident(return_ident) = unwrap_paren_expr(return_expr) else {
        return Ok(None);
    };
    let Some((array_name, push_stmts)) = computed_array_builder_binding(stmts) else {
        return Ok(None);
    };
    if return_ident.sym != array_name {
        return Ok(None);
    }
    let Some(elems) = computed_array_push_elements(push_stmts, &array_name) else {
        return Ok(None);
    };
    let expr = Expr::Array(ArrayLit {
        span: DUMMY_SP,
        elems: elems.into_iter().map(Some).collect(),
    });
    let expr = inline_computed_setup_prop_aliases(&expr, stmts, ctx);

    Ok(Some(VueSetupValueBinding {
        value: clean_expr(&print_expr(&expr, ctx)?, ctx),
        expr: Some(expr),
    }))
}

fn computed_array_builder_binding(stmts: &[Stmt]) -> Option<(Atom, &[Stmt])> {
    let [first, rest @ ..] = stmts else {
        return None;
    };
    let Stmt::Decl(Decl::Var(var)) = first else {
        return None;
    };
    if var.kind != VarDeclKind::Const {
        return None;
    }
    let [decl] = var.decls.as_slice() else {
        return None;
    };
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let init = decl.init.as_deref()?;
    if !is_empty_array_expr(init) {
        return None;
    }
    Some((binding.id.sym.clone(), rest))
}

fn is_empty_array_expr(expr: &Expr) -> bool {
    match unwrap_paren_expr(expr) {
        Expr::Array(array) => array.elems.is_empty(),
        _ => false,
    }
}

fn computed_array_push_elements(stmts: &[Stmt], array_name: &Atom) -> Option<Vec<ExprOrSpread>> {
    let mut elems = Vec::new();
    for stmt in stmts {
        elems.extend(computed_array_push_stmt_elements(stmt, array_name)?);
    }
    Some(elems)
}

fn computed_array_push_stmt_elements(stmt: &Stmt, array_name: &Atom) -> Option<Vec<ExprOrSpread>> {
    if let Some(expr) = computed_array_push_arg(stmt, array_name) {
        return Some(vec![ExprOrSpread {
            spread: None,
            expr: Box::new(expr.clone()),
        }]);
    }

    let Stmt::If(if_stmt) = stmt else {
        return None;
    };
    Some(vec![ExprOrSpread {
        spread: Some(DUMMY_SP),
        expr: Box::new(Expr::Paren(ParenExpr {
            span: DUMMY_SP,
            expr: Box::new(Expr::Cond(CondExpr {
                span: DUMMY_SP,
                test: if_stmt.test.clone(),
                cons: Box::new(array_expr_from_push_branch(&if_stmt.cons, array_name)?),
                alt: Box::new(if_stmt.alt.as_deref().map_or_else(
                    || Some(empty_array_expr()),
                    |alt| array_expr_from_push_branch(alt, array_name),
                )?),
            })),
        })),
    }])
}

fn computed_array_push_arg<'a>(stmt: &'a Stmt, array_name: &Atom) -> Option<&'a Expr> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    let Expr::Call(call) = unwrap_paren_expr(expr_stmt.expr.as_ref()) else {
        return None;
    };
    if call.args.len() != 1 || call.args.first()?.spread.is_some() {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = unwrap_paren_expr(callee.as_ref()) else {
        return None;
    };
    if !matches!(member.obj.as_ref(), Expr::Ident(object) if object.sym == *array_name) {
        return None;
    }
    if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "push") {
        return None;
    }
    call.args.first().map(|arg| arg.expr.as_ref())
}

fn array_expr_from_push_branch(stmt: &Stmt, array_name: &Atom) -> Option<Expr> {
    let elems = match stmt {
        Stmt::Block(block) => computed_array_push_elements(&block.stmts, array_name)?,
        stmt => computed_array_push_stmt_elements(stmt, array_name)?,
    };
    Some(Expr::Array(ArrayLit {
        span: DUMMY_SP,
        elems: elems.into_iter().map(Some).collect(),
    }))
}

fn empty_array_expr() -> Expr {
    Expr::Array(ArrayLit {
        span: DUMMY_SP,
        elems: Vec::new(),
    })
}

fn computed_final_return_expr(stmts: &[Stmt]) -> Option<(usize, &Expr)> {
    stmts
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, stmt)| match stmt {
            Stmt::Return(ReturnStmt {
                arg: Some(expr), ..
            }) => Some((index, expr.as_ref())),
            _ => None,
        })
}

fn inline_computed_block_locals(expr: &Expr, stmts: &[Stmt]) -> Expr {
    let mut locals = computed_block_local_exprs(stmts);
    if locals.is_empty() {
        return expr.clone();
    }

    let mut expr = expr.clone();
    while !locals.is_empty() {
        let counts = computed_local_ref_counts(&expr, &locals);
        let inline_bindings = locals
            .iter()
            .filter(|(name, expr)| {
                counts.get(*name).copied().unwrap_or_default() == 1
                    && computed_local_ref_counts(expr, &locals)
                        .values()
                        .all(|count| *count == 0)
            })
            .map(|(name, expr)| (name.clone(), expr.clone()))
            .collect::<HashMap<_, _>>();
        if inline_bindings.is_empty() {
            break;
        }
        for name in inline_bindings.keys() {
            locals.remove(name);
        }
        expr.visit_mut_with(&mut ComputedLocalInliner::new(inline_bindings));
    }

    expr
}

fn inline_computed_setup_prop_aliases(
    expr: &Expr,
    stmts: &[Stmt],
    ctx: &VueRecoveryContext,
) -> Expr {
    let aliases = computed_setup_prop_alias_exprs(stmts, ctx);
    if aliases.is_empty() {
        return expr.clone();
    }

    inline_computed_alias_expr(expr, &aliases)
}

fn computed_setup_prop_alias_exprs(
    stmts: &[Stmt],
    ctx: &VueRecoveryContext,
) -> HashMap<Atom, Expr> {
    let mut aliases = HashMap::new();
    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        if var.kind != VarDeclKind::Const {
            continue;
        }
        for decl in &var.decls {
            let Pat::Object(object) = &decl.name else {
                continue;
            };
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            if !is_setup_props_alias(init, ctx) {
                continue;
            }
            collect_computed_setup_prop_aliases(object, &mut aliases);
        }
    }
    aliases
}

fn collect_computed_setup_prop_alias_var(
    var: &VarDecl,
    ctx: &VueRecoveryContext,
    aliases: &mut HashMap<Atom, Expr>,
) -> bool {
    if var.kind != VarDeclKind::Const || var.decls.is_empty() {
        return false;
    }

    let mut next_aliases = HashMap::new();
    for decl in &var.decls {
        let Pat::Object(object) = &decl.name else {
            return false;
        };
        let Some(init) = decl.init.as_deref() else {
            return false;
        };
        if !is_setup_props_alias(init, ctx) {
            return false;
        }
        if !collect_computed_setup_prop_aliases(object, &mut next_aliases) {
            return false;
        }
    }

    aliases.extend(next_aliases);
    true
}

fn collect_computed_setup_prop_aliases(
    object: &ObjectPat,
    aliases: &mut HashMap<Atom, Expr>,
) -> bool {
    let mut next_aliases = HashMap::new();
    for prop in &object.props {
        match prop {
            ObjectPatProp::KeyValue(key_value) => {
                let Some(name) =
                    prop_name(&key_value.key).filter(|name| is_valid_identifier_name(name))
                else {
                    return false;
                };
                let Some(binding) = ident_binding_from_pat(key_value.value.as_ref()) else {
                    return false;
                };
                next_aliases.insert(
                    binding.sym.clone(),
                    Expr::Ident(Ident::new(name.into(), DUMMY_SP, Default::default())),
                );
            }
            ObjectPatProp::Assign(assign) => {
                let name = assign.key.sym.as_ref();
                if !is_valid_identifier_name(name) {
                    return false;
                }
                next_aliases.insert(
                    assign.key.sym.clone(),
                    Expr::Ident(Ident::new(
                        assign.key.sym.clone(),
                        DUMMY_SP,
                        Default::default(),
                    )),
                );
            }
            ObjectPatProp::Rest(_) => return false,
        }
    }
    if next_aliases.is_empty() {
        return false;
    }

    aliases.extend(next_aliases);
    true
}

fn ident_binding_from_pat(pat: &Pat) -> Option<&Ident> {
    match pat {
        Pat::Ident(binding) => Some(&binding.id),
        Pat::Assign(assign) => ident_binding_from_pat(assign.left.as_ref()),
        _ => None,
    }
}

fn computed_block_local_exprs(stmts: &[Stmt]) -> HashMap<Atom, Expr> {
    let mut locals = HashMap::new();
    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        if var.kind != VarDeclKind::Const {
            continue;
        }
        for decl in &var.decls {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            locals.insert(binding.id.sym.clone(), init.clone());
        }
    }
    locals
}

fn computed_mutated_local_bindings(
    stmts: &[Stmt],
    locals: &HashMap<Atom, Expr>,
) -> HashMap<Atom, Expr> {
    if locals.is_empty() {
        return HashMap::new();
    }

    let mut detector = ComputedLocalMutationDetector::new(locals.keys().cloned().collect());
    for stmt in stmts {
        stmt.visit_with(&mut detector);
    }
    let mutated = detector.finish();
    locals
        .iter()
        .filter_map(|(name, expr)| {
            mutated
                .contains(name)
                .then_some((name.clone(), expr.clone()))
        })
        .collect()
}

fn computed_local_ref_counts(expr: &Expr, locals: &HashMap<Atom, Expr>) -> HashMap<Atom, usize> {
    let mut counter = ComputedLocalRefCounter::new(locals.keys().cloned().collect());
    expr.visit_with(&mut counter);
    counter.finish()
}

struct ComputedLocalMutationDetector {
    bindings: Vec<Atom>,
    shadow_depths: Vec<usize>,
    mutated: HashSet<Atom>,
}

impl ComputedLocalMutationDetector {
    fn new(mut bindings: Vec<Atom>) -> Self {
        bindings.sort_by(|left, right| left.as_ref().cmp(right.as_ref()));
        bindings.dedup();
        let shadow_depths = vec![0; bindings.len()];
        Self {
            bindings,
            shadow_depths,
            mutated: HashSet::new(),
        }
    }

    fn finish(self) -> HashSet<Atom> {
        self.mutated
    }

    fn active_index(&self, name: &str) -> Option<usize> {
        self.bindings
            .iter()
            .zip(self.shadow_depths.iter())
            .position(|(binding, shadow_depth)| binding.as_ref() == name && *shadow_depth == 0)
    }

    fn mark_name(&mut self, name: &str) {
        if let Some(index) = self.active_index(name) {
            self.mutated.insert(self.bindings[index].clone());
        }
    }

    fn mark_member_object(&mut self, member: &MemberExpr) {
        if let Expr::Ident(object) = member.obj.as_ref() {
            self.mark_name(object.sym.as_ref());
        }
    }

    fn shadowing_indices(&self, params: &[&Pat]) -> Vec<usize> {
        let mut param_bindings = HashSet::new();
        for param in params {
            collect_pat_bindings(param, &mut param_bindings);
        }
        self.bindings
            .iter()
            .enumerate()
            .filter_map(|(index, binding)| param_bindings.contains(binding).then_some(index))
            .collect()
    }

    fn enter_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] += 1;
        }
    }

    fn exit_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] -= 1;
        }
    }
}

impl Visit for ComputedLocalMutationDetector {
    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
                self.mark_name(binding.id.sym.as_ref());
            }
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
                self.mark_member_object(member);
            }
            _ => {}
        }
        assign.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        match update.arg.as_ref() {
            Expr::Ident(ident) => self.mark_name(ident.sym.as_ref()),
            Expr::Member(member) => self.mark_member_object(member),
            _ => {}
        }
        update.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Member(member) = callee.as_ref() {
                self.mark_member_object(member);
            }
        }
        call.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        let params = arrow.params.iter().collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        arrow.body.visit_with(self);
        self.exit_shadowed(&shadowed);
    }

    fn visit_function(&mut self, function: &swc_core::ecma::ast::Function) {
        let params = function
            .params
            .iter()
            .map(|param| &param.pat)
            .collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        if let Some(body) = function.body.as_ref() {
            body.visit_with(self);
        }
        self.exit_shadowed(&shadowed);
    }
}

struct ComputedLocalRefCounter {
    bindings: Vec<Atom>,
    shadow_depths: Vec<usize>,
    counts: Vec<usize>,
}

impl ComputedLocalRefCounter {
    fn new(mut bindings: Vec<Atom>) -> Self {
        bindings.sort_by(|left, right| left.as_ref().cmp(right.as_ref()));
        bindings.dedup();
        let shadow_depths = vec![0; bindings.len()];
        let counts = vec![0; bindings.len()];
        Self {
            bindings,
            shadow_depths,
            counts,
        }
    }

    fn finish(self) -> HashMap<Atom, usize> {
        self.bindings.into_iter().zip(self.counts).collect()
    }

    fn active_index(&self, name: &str) -> Option<usize> {
        self.bindings
            .iter()
            .zip(self.shadow_depths.iter())
            .position(|(binding, shadow_depth)| binding.as_ref() == name && *shadow_depth == 0)
    }

    fn shadowing_indices(&self, params: &[&Pat]) -> Vec<usize> {
        let mut param_bindings = HashSet::new();
        for param in params {
            collect_pat_bindings(param, &mut param_bindings);
        }
        self.bindings
            .iter()
            .enumerate()
            .filter_map(|(index, binding)| param_bindings.contains(binding).then_some(index))
            .collect()
    }

    fn enter_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] += 1;
        }
    }

    fn exit_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] -= 1;
        }
    }
}

impl Visit for ComputedLocalRefCounter {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Ident(ident) = expr {
            if let Some(index) = self.active_index(ident.sym.as_ref()) {
                self.counts[index] += 1;
                return;
            }
        }
        expr.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        let params = arrow.params.iter().collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        arrow.body.visit_with(self);
        self.exit_shadowed(&shadowed);
    }

    fn visit_function(&mut self, function: &swc_core::ecma::ast::Function) {
        let params = function
            .params
            .iter()
            .map(|param| &param.pat)
            .collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        if let Some(body) = function.body.as_ref() {
            body.visit_with(self);
        }
        self.exit_shadowed(&shadowed);
    }
}

struct ComputedLocalInliner {
    bindings: Vec<(Atom, Expr)>,
    replacement_refs: Vec<HashSet<Atom>>,
    shadow_depths: Vec<usize>,
    capture_depths: Vec<usize>,
}

impl ComputedLocalInliner {
    fn new(mut bindings: HashMap<Atom, Expr>) -> Self {
        let mut bindings = bindings.drain().collect::<Vec<_>>();
        bindings.sort_by(|(left, _), (right, _)| left.as_ref().cmp(right.as_ref()));
        let replacement_refs = bindings
            .iter()
            .map(|(_, expr)| expr_ident_refs(expr))
            .collect::<Vec<_>>();
        let shadow_depths = vec![0; bindings.len()];
        let capture_depths = vec![0; bindings.len()];
        Self {
            bindings,
            replacement_refs,
            shadow_depths,
            capture_depths,
        }
    }

    fn active_index(&self, name: &str) -> Option<usize> {
        self.bindings
            .iter()
            .zip(self.shadow_depths.iter())
            .zip(self.capture_depths.iter())
            .position(|(((binding, _), shadow_depth), capture_depth)| {
                binding.as_ref() == name && *shadow_depth == 0 && *capture_depth == 0
            })
    }

    fn shadowing_indices(&self, scope_bindings: &HashSet<Atom>) -> Vec<usize> {
        self.bindings
            .iter()
            .enumerate()
            .filter_map(|(index, (binding, _))| scope_bindings.contains(binding).then_some(index))
            .collect()
    }

    fn capture_indices(&self, scope_bindings: &HashSet<Atom>) -> Vec<usize> {
        self.replacement_refs
            .iter()
            .enumerate()
            .filter_map(|(index, refs)| {
                refs.iter()
                    .any(|name| scope_bindings.contains(name))
                    .then_some(index)
            })
            .collect()
    }

    fn enter_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] += 1;
        }
    }

    fn exit_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] -= 1;
        }
    }

    fn enter_captured(&mut self, indices: &[usize]) {
        for index in indices {
            self.capture_depths[*index] += 1;
        }
    }

    fn exit_captured(&mut self, indices: &[usize]) {
        for index in indices {
            self.capture_depths[*index] -= 1;
        }
    }

    fn enter_scope(&mut self, scope_bindings: &HashSet<Atom>) -> (Vec<usize>, Vec<usize>) {
        let shadowed = self.shadowing_indices(scope_bindings);
        let captured = self.capture_indices(scope_bindings);
        self.enter_shadowed(&shadowed);
        self.enter_captured(&captured);
        (shadowed, captured)
    }

    fn exit_scope(&mut self, shadowed: &[usize], captured: &[usize]) {
        self.exit_captured(captured);
        self.exit_shadowed(shadowed);
    }
}

impl VisitMut for ComputedLocalInliner {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Ident(ident) = expr {
            if let Some(index) = self.active_index(ident.sym.as_ref()) {
                *expr = self.bindings[index].1.clone();
                expr.visit_mut_children_with(self);
                return;
            }
        }
        expr.visit_mut_children_with(self);
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut swc_core::ecma::ast::ArrowExpr) {
        let scope_bindings = arrow_scope_bindings(arrow);
        let (shadowed, captured) = self.enter_scope(&scope_bindings);
        arrow.body.visit_mut_with(self);
        self.exit_scope(&shadowed, &captured);
    }

    fn visit_mut_function(&mut self, function: &mut swc_core::ecma::ast::Function) {
        let scope_bindings = function_scope_bindings(function);
        let (shadowed, captured) = self.enter_scope(&scope_bindings);
        if let Some(body) = function.body.as_mut() {
            body.visit_mut_with(self);
        }
        self.exit_scope(&shadowed, &captured);
    }
}

fn arrow_scope_bindings(arrow: &swc_core::ecma::ast::ArrowExpr) -> HashSet<Atom> {
    let mut bindings = HashSet::new();
    for param in &arrow.params {
        collect_pat_bindings(param, &mut bindings);
    }
    collect_block_or_expr_scope_bindings(arrow.body.as_ref(), &mut bindings);
    bindings
}

fn function_scope_bindings(function: &swc_core::ecma::ast::Function) -> HashSet<Atom> {
    let mut bindings = HashSet::new();
    for param in &function.params {
        collect_pat_bindings(&param.pat, &mut bindings);
    }
    if let Some(body) = function.body.as_ref() {
        collect_stmt_scope_bindings(&body.stmts, &mut bindings);
    }
    bindings
}

fn collect_block_or_expr_scope_bindings(body: &BlockStmtOrExpr, bindings: &mut HashSet<Atom>) {
    if let BlockStmtOrExpr::BlockStmt(block) = body {
        collect_stmt_scope_bindings(&block.stmts, bindings);
    }
}

fn collect_stmt_scope_bindings(stmts: &[Stmt], bindings: &mut HashSet<Atom>) {
    for stmt in stmts {
        match stmt {
            Stmt::Decl(Decl::Var(var)) => {
                for decl in &var.decls {
                    collect_pat_bindings(&decl.name, bindings);
                }
            }
            Stmt::Decl(Decl::Fn(function)) => {
                bindings.insert(function.ident.sym.clone());
            }
            Stmt::Decl(Decl::Class(class)) => {
                bindings.insert(class.ident.sym.clone());
            }
            Stmt::Block(block) => collect_stmt_scope_bindings(&block.stmts, bindings),
            Stmt::If(if_stmt) => {
                collect_stmt_scope_binding(if_stmt.cons.as_ref(), bindings);
                if let Some(alt) = if_stmt.alt.as_ref() {
                    collect_stmt_scope_binding(alt.as_ref(), bindings);
                }
            }
            _ => {}
        }
    }
}

fn collect_stmt_scope_binding(stmt: &Stmt, bindings: &mut HashSet<Atom>) {
    match stmt {
        Stmt::Block(block) => collect_stmt_scope_bindings(&block.stmts, bindings),
        stmt => collect_stmt_scope_bindings(std::slice::from_ref(stmt), bindings),
    }
}

fn computed_if_return_chain_expr(
    stmts: &[Stmt],
    ctx: &VueRecoveryContext,
) -> Result<Option<String>> {
    let mut branches = Vec::new();
    let mut aliases = HashMap::new();

    for stmt in stmts {
        match stmt {
            Stmt::Decl(Decl::Var(var))
                if branches.is_empty()
                    && collect_computed_setup_prop_alias_var(var, ctx, &mut aliases) =>
            {
                continue;
            }
            Stmt::If(if_stmt) => {
                let Some(expr) = direct_return_expr_from_stmt(if_stmt.cons.as_ref()) else {
                    return Ok(None);
                };
                if if_stmt.alt.is_some() {
                    return Ok(None);
                }
                let test = inline_computed_alias_expr(if_stmt.test.as_ref(), &aliases);
                let expr = inline_computed_alias_expr(expr, &aliases);
                branches.push((
                    clean_expr(&print_expr(&test, ctx)?, ctx),
                    clean_expr(&print_expr(&expr, ctx)?, ctx),
                ));
            }
            Stmt::Return(ReturnStmt {
                arg: Some(expr), ..
            }) if !branches.is_empty() => {
                let expr = inline_computed_alias_expr(expr, &aliases);
                let fallback = clean_expr(&print_expr(&expr, ctx)?, ctx);
                return Ok(Some(format_conditional_expr(&branches, fallback)));
            }
            _ => return Ok(None),
        }
    }

    Ok(None)
}

fn inline_computed_alias_expr(expr: &Expr, aliases: &HashMap<Atom, Expr>) -> Expr {
    if aliases.is_empty() {
        return expr.clone();
    }

    let mut expr = expr.clone();
    expr.visit_mut_with(&mut ComputedLocalInliner::new(aliases.clone()));
    expr
}

fn return_expr_from_stmt(stmt: &Stmt) -> Option<&Expr> {
    match stmt {
        Stmt::Return(ReturnStmt {
            arg: Some(expr), ..
        }) => Some(expr.as_ref()),
        Stmt::Block(block) => block.stmts.iter().find_map(return_expr_from_stmt),
        _ => None,
    }
}

fn direct_return_expr_from_stmt(stmt: &Stmt) -> Option<&Expr> {
    match stmt {
        Stmt::Return(ReturnStmt {
            arg: Some(expr), ..
        }) => Some(expr.as_ref()),
        Stmt::Block(block) => {
            let [Stmt::Return(ReturnStmt {
                arg: Some(expr), ..
            })] = block.stmts.as_slice()
            else {
                return None;
            };
            Some(expr.as_ref())
        }
        _ => None,
    }
}

fn format_conditional_expr(branches: &[(String, String)], fallback: String) -> String {
    branches
        .iter()
        .rev()
        .fold(fallback, |alternate, (condition, consequent)| {
            format!("{condition} ? {consequent} : {alternate}")
        })
}

fn resolve_component_name(expr: &Expr, ctx: &VueRecoveryContext) -> Option<String> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if helper_name(&call.callee, ctx) != Some(VueHelper::ResolveComponent) {
        return None;
    }
    call.args
        .first()
        .and_then(|arg| string_lit(arg.expr.as_ref()))
}

pub(super) fn resolve_directive_name(expr: &Expr, ctx: &VueRecoveryContext) -> Option<String> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if helper_name(&call.callee, ctx) != Some(VueHelper::ResolveDirective) {
        return None;
    }
    call.args
        .first()
        .and_then(|arg| string_lit(arg.expr.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::is_vue_helper_candidate_source;

    #[test]
    fn vue_helper_candidates_exclude_adjacent_bare_packages() {
        assert!(!is_vue_helper_candidate_source("vuex"));
        assert!(!is_vue_helper_candidate_source("vue-router"));
        assert!(!is_vue_helper_candidate_source("@vueuse/core"));
    }

    #[test]
    fn vue_helper_candidates_exclude_adjacent_relative_chunks() {
        assert!(!is_vue_helper_candidate_source("./vueuse-core.js"));
        assert!(!is_vue_helper_candidate_source("./chunks/vue-router.js"));
        assert!(!is_vue_helper_candidate_source("./chunks/vuex.js"));
    }

    #[test]
    fn vue_helper_candidates_keep_runtime_and_local_vue_chunks() {
        assert!(is_vue_helper_candidate_source("@vue/runtime-core"));
        assert!(is_vue_helper_candidate_source("@vue/runtime-dom"));
        assert!(is_vue_helper_candidate_source("./vendor-vue.js"));
    }
}
