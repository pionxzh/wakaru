use std::collections::{HashMap, HashSet};

use anyhow::Result;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignPat, AssignTarget, BindingIdent, BlockStmtOrExpr, CallExpr,
    Callee, ClassDecl, Decl, Expr, ExprOrSpread, FnDecl, Function, Ident, IfStmt, ImportSpecifier,
    KeyValuePatProp, Lit, MemberExpr, MemberProp, Module, ModuleDecl, ModuleItem, ObjectLit,
    ObjectPat, ObjectPatProp, Pat, Prop, PropName, PropOrSpread, ReturnStmt, SimpleAssignTarget,
    Stmt, UpdateExpr, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::expressions::{clean_expr, clean_setup_stmt, print_clean_setup_stmt, print_expr};
use super::helpers::{helper_name, VueHelper};
use super::syntax::{
    module_export_name, param_binding_ident, prop_name, string_lit, wtf8_to_string,
};
use super::{
    component_prop_names, RenderSource, VueRecoveryContext, VueRenderChildListBinding,
    VueRenderChildListSource, VueScriptImport, VueSetupLocalBinding, VueSetupRefBinding,
    VueSetupValueBinding,
};
use crate::js_names::is_valid_identifier_name;

pub(super) fn collect_context(
    module: &Module,
    cm: Lrc<SourceMap>,
    component_bindings: HashMap<Atom, String>,
) -> VueRecoveryContext {
    let mut ctx = VueRecoveryContext {
        cm,
        component_bindings,
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
                                    .insert(named.local.sym.clone(), component.clone());
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
                                if source.contains("vue") {
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
                                    .insert(default.local.sym.clone(), component.clone());
                            }
                        }
                        ImportSpecifier::Namespace(namespace) => {
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
                                    .insert(namespace.local.sym.clone(), component.clone());
                            }
                        }
                    }
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                collect_var_decl_context(var, &mut ctx);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                if let Decl::Var(var) = &export.decl {
                    collect_var_decl_context(var, &mut ctx);
                }
            }
            _ => {}
        }
    }
    ctx
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
            .map(|(binding, _)| binding.clone()),
    );
    reserved.extend(
        ctx.setup_local_bindings
            .iter()
            .flat_map(|binding| binding.bindings.iter().cloned()),
    );
    reserved.extend(
        ctx.setup_ref_script_bindings
            .iter()
            .map(|binding| binding.binding.clone()),
    );
    reserved.extend(ctx.setup_value_bindings.keys().cloned());

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
        ctx.script_local_bindings.push(VueSetupLocalBinding {
            bindings,
            refs: stmt_ident_refs(&cleaned_stmt),
            source,
            import_refs: stmt_import_refs(&cleaned_stmt, &ctx.script_imports),
            stmt: cleaned_stmt,
            module_scope: true,
        });
    }
    Ok(())
}

pub(super) fn render_local_declaration_with_aliases(
    ctx: &VueRecoveryContext,
    declaration: &VueSetupLocalBinding,
    aliases: &HashMap<Atom, Atom>,
) -> Result<VueSetupLocalBinding> {
    let mut stmt = declaration.stmt.clone();
    if declaration.module_scope && !aliases.is_empty() {
        rename_top_level_decl_bindings(&mut stmt, aliases);
        stmt.visit_mut_with(&mut ImportAliasRenamer::new(aliases));
    }

    let cleaned_stmt = clean_setup_stmt(&stmt, ctx);
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

    Ok(VueSetupLocalBinding {
        bindings,
        refs: stmt_ident_refs(&cleaned_stmt),
        source,
        import_refs: stmt_import_refs(&cleaned_stmt, &ctx.script_imports),
        stmt: cleaned_stmt,
        module_scope: declaration.module_scope,
    })
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

struct ImportAliasRenamer<'a> {
    aliases: &'a HashMap<Atom, Atom>,
    scopes: Vec<HashSet<Atom>>,
}

impl<'a> ImportAliasRenamer<'a> {
    fn new(aliases: &'a HashMap<Atom, Atom>) -> Self {
        Self {
            aliases,
            scopes: vec![HashSet::new()],
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashSet::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, sym: &Atom) {
        if let Some(scope) = self.scopes.last_mut() {
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
        self.scopes.iter().rev().any(|scope| scope.contains(sym))
    }
}

impl VisitMut for ImportAliasRenamer<'_> {
    fn visit_mut_ident(&mut self, ident: &mut Ident) {
        if !self.is_shadowed(&ident.sym) {
            if let Some(alias) = self.aliases.get(&ident.sym) {
                ident.sym = alias.clone();
            }
        }
    }

    fn visit_mut_binding_ident(&mut self, ident: &mut BindingIdent) {
        self.declare(&ident.id.sym);
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
        self.declare_pat(&declarator.name);
        if let Some(init) = &mut declarator.init {
            init.visit_mut_with(self);
        }
    }

    fn visit_mut_fn_decl(&mut self, function: &mut FnDecl) {
        self.declare(&function.ident.sym);
        self.visit_mut_function(&mut function.function);
    }

    fn visit_mut_function(&mut self, function: &mut Function) {
        self.push_scope();
        for param in &function.params {
            self.declare_pat(&param.pat);
        }
        if let Some(body) = &mut function.body {
            body.visit_mut_with(self);
        }
        self.pop_scope();
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        self.push_scope();
        for param in &arrow.params {
            self.declare_pat(param);
        }
        arrow.body.visit_mut_with(self);
        self.pop_scope();
    }

    fn visit_mut_class_decl(&mut self, class: &mut ClassDecl) {
        self.declare(&class.ident.sym);
        class.class.visit_mut_with(self);
    }
}

fn collect_var_decl_context(var: &VarDecl, ctx: &mut VueRecoveryContext) {
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
        if let Some(ref_props) = provider_ref_props_from_init(init, ctx) {
            ctx.provider_ref_bindings
                .insert(binding.id.sym.clone(), ref_props);
        }
        if let Some(component) = component_name_from_init(init, &ctx.component_bindings) {
            ctx.component_bindings
                .insert(binding.id.sym.clone(), component);
        }
        if binding.id.sym.as_ref() == "__sfc__" {
            if let Expr::Object(object) = init {
                ctx.component_options = Some(object.clone());
            }
        }
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

fn component_name_from_options(object: &ObjectLit) -> Option<String> {
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
    if ctx.vue_helper_candidates.is_empty() {
        return;
    }

    let mut inference = HelperInference {
        candidates: &ctx.vue_helper_candidates,
        inferred: HashMap::new(),
    };
    match render {
        RenderSource::Function(render) => {
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
    inferred: HashMap<Atom, VueHelper>,
}

impl Visit for HelperInference<'_> {
    fn visit_if_stmt(&mut self, if_stmt: &IfStmt) {
        self.infer_unref_expr(if_stmt.test.as_ref());
        if_stmt.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        self.infer_unref_expr(member.obj.as_ref());
        member.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            self.infer_unref_expr(callee.as_ref());
        }

        if let Some((callee, fragment)) = self.fragment_block_call(call) {
            self.inferred
                .insert(callee.sym.clone(), VueHelper::CreateElementBlock);
            self.inferred
                .insert(fragment.sym.clone(), VueHelper::Fragment);
        }

        if let Some(callee) = call_callee_ident(call) {
            if self.candidates.contains(&callee.sym) {
                if let Some(helper) = infer_call_helper(call) {
                    self.inferred.entry(callee.sym.clone()).or_insert(helper);
                }
            }
        }

        if let Some(VueHelper::CreateElementBlock | VueHelper::CreateElementVNode) =
            call_callee_ident(call).and_then(|callee| self.inferred.get(&callee.sym))
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
            call_callee_ident(call).and_then(|callee| self.inferred.get(&callee.sym)),
            Some(VueHelper::CreateBlock | VueHelper::CreateVNode)
        ) {
            self.infer_builtin_component_arg(call);
        }

        call.visit_children_with(self);
    }
}

impl HelperInference<'_> {
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
        if !self.candidates.contains(&fragment.sym) {
            return None;
        }
        Some((callee, fragment))
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

fn unwrap_paren_expr(expr: &Expr) -> &Expr {
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

fn is_with_ctx_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        args.first().map(|arg| arg.expr.as_ref()),
        Some(Expr::Arrow(_))
    )
}

fn is_create_static_vnode_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        args.first().map(|arg| arg.expr.as_ref()),
        Some(Expr::Lit(Lit::Str(str))) if wtf8_to_string(&str.value).contains('<')
    )
}

fn is_create_comment_vnode_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        (
            args.first().map(|arg| arg.expr.as_ref()),
            args.get(1).map(|arg| arg.expr.as_ref())
        ),
        (Some(Expr::Lit(Lit::Str(_))), Some(Expr::Lit(Lit::Bool(_))))
    )
}

fn is_create_text_vnode_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        args.get(1).map(|arg| arg.expr.as_ref()),
        Some(Expr::Lit(Lit::Num(_)))
    )
}

fn is_element_vnode_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        args.first().map(|arg| arg.expr.as_ref()),
        Some(Expr::Lit(Lit::Str(str))) if !wtf8_to_string(&str.value).contains('<')
    ) && args.len() >= 2
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
        && matches!(
            args.first().map(|arg| arg.expr.as_ref()),
            Some(Expr::Lit(Lit::Str(_)))
        )
}

fn is_display_string_call(args: &[ExprOrSpread]) -> bool {
    args.len() == 1
        && !matches!(
            args.first().map(|arg| arg.expr.as_ref()),
            Some(Expr::Lit(Lit::Str(_)))
        )
}

fn is_open_block_call(args: &[ExprOrSpread]) -> bool {
    args.is_empty()
        || matches!(
            args.first().map(|arg| arg.expr.as_ref()),
            Some(Expr::Lit(Lit::Bool(_)))
        )
}

fn call_callee_ident(call: &CallExpr) -> Option<&swc_core::ecma::ast::Ident> {
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

    let setup_template_ref_refs = setup_render_template_ref_refs(render, setup_stmts, ctx);
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
    let mut local_candidates = Vec::new();

    for stmt in setup_stmts {
        match stmt {
            Stmt::Decl(Decl::Fn(function)) => {
                local_candidates.push((vec![function.ident.sym.clone()], stmt.clone()));
            }
            Stmt::Decl(Decl::Class(class)) => {
                local_candidates.push((vec![class.ident.sym.clone()], stmt.clone()));
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
                                    ctx.setup_alias_bindings
                                        .insert(binding.id.sym.clone(), alias.sym.clone());
                                    true
                                } else {
                                    if let Some(ref_props) = setup_provider_ref_props(
                                        init,
                                        ctx,
                                        &provider_ref_object_bindings,
                                    ) {
                                        provider_ref_object_bindings
                                            .insert(binding.id.sym.clone(), ref_props);
                                    }
                                    let is_ref_object = is_ref_object_expr(init, ctx);
                                    if is_ref_object {
                                        ctx.setup_ref_object_bindings
                                            .insert(binding.id.sym.clone());
                                    }
                                    let is_ref_object_alias_source = is_ref_object
                                        && setup_ref_object_alias_refs.contains(&binding.id.sym);
                                    if let Some(value) = computed_value_expr(init, ctx)? {
                                        ctx.setup_value_bindings
                                            .insert(binding.id.sym.clone(), value);
                                        true
                                    } else if let Some((value, import_refs)) =
                                        computed_script_setup_expr(init, ctx)?
                                    {
                                        ctx.setup_script_import_refs.extend(import_refs);
                                        ctx.setup_script_bindings
                                            .push((binding.id.sym.clone(), value));
                                        ctx.setup_ref_bindings.insert(binding.id.sym.clone());
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
                                        ctx.setup_ref_bindings.insert(binding.id.sym.clone());
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
                                collect_object_pat_bindings(object, &mut ctx.setup_ref_bindings);
                                false
                            }
                            Pat::Object(object) => {
                                if let Some(ref_props) = setup_provider_ref_props(
                                    init,
                                    ctx,
                                    &provider_ref_object_bindings,
                                ) {
                                    collect_provider_object_pat_bindings(
                                        object,
                                        &ref_props,
                                        &mut ctx.setup_ref_bindings,
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
                    let is_local_candidate = match &decl.name {
                        Pat::Ident(_) | Pat::Array(_) => true,
                        Pat::Object(_) => {
                            has_template_ref
                                || has_render_ref
                                || is_ref_object_local
                                || is_imported_call_local
                        }
                        _ => false,
                    };
                    if !is_local_candidate {
                        continue;
                    }
                    if has_template_ref && matches!(decl.name, Pat::Object(_)) {
                        ctx.setup_template_ref_bindings
                            .extend(decl_bindings.iter().cloned());
                    } else {
                        ctx.setup_template_ref_bindings.extend(
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
                    local_candidates.push((bindings, Stmt::Decl(Decl::Var(Box::new(local_var)))));
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
            ctx.setup_alias_bindings.insert(from, to);
        }
    }
    refresh_setup_value_binding_sources(ctx)?;

    assign_setup_prop_bindings(ctx, &local_candidates);

    for (bindings, stmt) in local_candidates {
        let cleaned_stmt = clean_setup_stmt(&stmt, ctx);
        let source = print_clean_setup_stmt(&cleaned_stmt, ctx)?;
        if !source.is_empty() {
            ctx.setup_local_bindings.push(VueSetupLocalBinding {
                bindings,
                refs: stmt_ident_refs(&cleaned_stmt),
                source,
                import_refs: stmt_import_refs(&cleaned_stmt, &ctx.script_imports),
                stmt: cleaned_stmt,
                module_scope: false,
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
        scopes: vec![HashSet::new()],
        refs: HashSet::new(),
    };
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
    collector.refs
}

fn setup_value_member_refs(render: &ArrowExpr, setup_stmts: &[Stmt]) -> HashSet<Atom> {
    let mut collector = ValueMemberIdentRefCollector {
        scopes: vec![HashSet::new()],
        refs: HashSet::new(),
    };
    for stmt in setup_stmts {
        stmt.visit_with(&mut collector);
    }
    render.visit_with(&mut collector);
    collector.refs
}

struct ValueMemberIdentRefCollector {
    scopes: Vec<HashSet<Atom>>,
    refs: HashSet<Atom>,
}

impl ValueMemberIdentRefCollector {
    fn push_scope(&mut self) {
        self.scopes.push(HashSet::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, sym: &Atom) {
        if self.scopes.len() <= 1 {
            return;
        }
        if let Some(scope) = self.scopes.last_mut() {
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
        self.scopes.iter().rev().any(|scope| scope.contains(sym))
    }
}

impl Visit for ValueMemberIdentRefCollector {
    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "value") {
            if let Expr::Ident(object) = member.obj.as_ref() {
                if !self.is_shadowed(&object.sym) {
                    self.refs.insert(object.sym.clone());
                }
            }
        }
        member.visit_children_with(self);
    }

    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        self.declare(&ident.id.sym);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        self.declare_pat(&declarator.name);
        if let Some(init) = &declarator.init {
            init.visit_with(self);
        }
    }

    fn visit_fn_decl(&mut self, function: &FnDecl) {
        self.declare(&function.ident.sym);
        self.push_scope();
        for param in &function.function.params {
            self.declare_pat(&param.pat);
        }
        function.function.visit_with(self);
        self.pop_scope();
    }

    fn visit_function(&mut self, function: &Function) {
        self.push_scope();
        for param in &function.params {
            self.declare_pat(&param.pat);
        }
        if let Some(body) = &function.body {
            body.visit_with(self);
        }
        self.pop_scope();
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.push_scope();
        for param in &arrow.params {
            self.declare_pat(param);
        }
        arrow.body.visit_with(self);
        self.pop_scope();
    }

    fn visit_class_decl(&mut self, class: &ClassDecl) {
        self.declare(&class.ident.sym);
        class.class.visit_with(self);
    }
}

struct NonValueMemberRefCollector {
    scopes: Vec<HashSet<Atom>>,
    refs: HashSet<Atom>,
}

impl NonValueMemberRefCollector {
    fn push_scope(&mut self) {
        self.scopes.push(HashSet::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, sym: &Atom) {
        if self.scopes.len() <= 1 {
            return;
        }
        if let Some(scope) = self.scopes.last_mut() {
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
        self.scopes.iter().rev().any(|scope| scope.contains(sym))
    }
}

impl Visit for NonValueMemberRefCollector {
    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "value") {
            if let Expr::Ident(object) = member.obj.as_ref() {
                if !self.is_shadowed(&object.sym) {
                    self.refs.insert(object.sym.clone());
                }
            }
        }
        member.visit_children_with(self);
    }

    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        self.declare(&ident.id.sym);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        self.declare_pat(&declarator.name);
        if let Some(init) = &declarator.init {
            init.visit_with(self);
        }
    }

    fn visit_fn_decl(&mut self, function: &FnDecl) {
        self.declare(&function.ident.sym);
        self.visit_function(&function.function);
    }

    fn visit_function(&mut self, function: &Function) {
        self.push_scope();
        for param in &function.params {
            self.declare_pat(&param.pat);
        }
        if let Some(body) = &function.body {
            body.visit_with(self);
        }
        self.pop_scope();
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.push_scope();
        for param in &arrow.params {
            self.declare_pat(param);
        }
        arrow.body.visit_with(self);
        self.pop_scope();
    }

    fn visit_class_decl(&mut self, class: &ClassDecl) {
        self.declare(&class.ident.sym);
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
        scopes: vec![HashSet::new()],
        refs: HashSet::new(),
    };
    render.visit_with(&mut collector);
    collector.refs
}

fn setup_render_template_ref_refs(
    render: &ArrowExpr,
    setup_stmts: &[Stmt],
    ctx: &VueRecoveryContext,
) -> HashSet<Atom> {
    let mut tuple_value_candidates = HashSet::new();
    let mut object_value_candidates = HashSet::new();
    let mut unref_candidates = HashSet::new();
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
                Pat::Object(_) => {
                    collect_pat_bindings(&decl.name, &mut object_value_candidates);
                    collect_pat_bindings(&decl.name, &mut unref_candidates);
                }
                _ => {}
            }
        }
    }
    if tuple_value_candidates.is_empty()
        && (object_value_candidates.is_empty() || unref_candidates.is_empty())
    {
        return HashSet::new();
    }

    let mut collector = RenderTemplateRefCollector {
        tuple_value_candidates: &tuple_value_candidates,
        object_value_candidates: &object_value_candidates,
        unref_candidates: &unref_candidates,
        ctx,
        scopes: vec![HashSet::new()],
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

struct RenderTemplateRefCollector<'a> {
    tuple_value_candidates: &'a HashSet<Atom>,
    object_value_candidates: &'a HashSet<Atom>,
    unref_candidates: &'a HashSet<Atom>,
    ctx: &'a VueRecoveryContext,
    scopes: Vec<HashSet<Atom>>,
    tuple_value_refs: HashSet<Atom>,
    object_value_refs: HashSet<Atom>,
    unref_refs: HashSet<Atom>,
}

impl RenderTemplateRefCollector<'_> {
    fn push_scope(&mut self) {
        self.scopes.push(HashSet::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, sym: &Atom) {
        if let Some(scope) = self.scopes.last_mut() {
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
        self.scopes.iter().rev().any(|scope| scope.contains(sym))
    }

    fn collect_value_member(&mut self, member: &MemberExpr) {
        if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "value") {
            return;
        }
        let Expr::Ident(object) = member.obj.as_ref() else {
            return;
        };
        if self.is_shadowed(&object.sym) {
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
        if self.unref_candidates.contains(&object.sym) && !self.is_shadowed(&object.sym) {
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
        self.declare(&ident.id.sym);
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
        self.declare_pat(&declarator.name);
    }

    fn visit_fn_decl(&mut self, function: &FnDecl) {
        self.declare(&function.ident.sym);
        self.visit_function(&function.function);
    }

    fn visit_function(&mut self, function: &Function) {
        self.push_scope();
        for param in &function.params {
            self.declare_pat(&param.pat);
        }
        if let Some(body) = &function.body {
            body.visit_with(self);
        }
        self.pop_scope();
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.push_scope();
        for param in &arrow.params {
            self.declare_pat(param);
        }
        arrow.body.visit_with(self);
        self.pop_scope();
    }

    fn visit_class_decl(&mut self, class: &ClassDecl) {
        self.declare(&class.ident.sym);
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
    local_candidates: &[(Vec<Atom>, Stmt)],
) {
    ctx.setup_prop_bindings.clear();
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
    reserved.extend(ctx.setup_alias_bindings.keys().cloned());
    reserved.extend(
        local_candidates
            .iter()
            .flat_map(|(bindings, _)| bindings.iter().cloned()),
    );
    reserved.extend(
        ctx.setup_script_bindings
            .iter()
            .map(|(binding, _)| binding.clone()),
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
        ctx.setup_prop_bindings.insert(prop, binding);
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
    match prop {
        MemberProp::Ident(ident) => ident.sym.as_ref() == name,
        MemberProp::Computed(computed) => {
            string_lit(computed.expr.as_ref()).as_deref() == Some(name)
        }
        MemberProp::PrivateName(_) => false,
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
    ctx.setup_ref_object_bindings.contains(&ident.sym)
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

fn setup_provider_ref_props(
    expr: &Expr,
    ctx: &VueRecoveryContext,
    bindings: &HashMap<Atom, HashSet<Atom>>,
) -> Option<HashSet<Atom>> {
    provider_ref_props_from_expr(expr, ctx)
        .cloned()
        .or_else(|| provider_ref_props_from_alias(expr, bindings).cloned())
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
        RenderSource::Function(render) => render
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
        RenderSource::Function(render) => render
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
    let bindings = ctx.setup_value_bindings.clone();
    for (binding, value) in bindings {
        let Some(expr) = value.expr else {
            continue;
        };
        let value = clean_expr(&print_expr(&expr, ctx)?, ctx);
        if let Some(binding) = ctx.setup_value_bindings.get_mut(&binding) {
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
    computed_getter_expr(arg.expr.as_ref(), ctx)
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
    let getter = clean_expr(&print_expr(arg.expr.as_ref(), ctx)?, ctx);
    let import_refs = script_import_refs(arg.expr.as_ref(), &ctx.script_imports);
    Ok(Some((format!("computed({getter})"), import_refs)))
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
        scopes: vec![HashSet::new()],
        refs: HashSet::new(),
    };
    expr.visit_with(&mut collector);
    collector.refs
}

fn stmt_import_refs(stmt: &Stmt, imports: &HashMap<Atom, VueScriptImport>) -> HashSet<Atom> {
    let mut collector = ScriptImportRefCollector {
        imports,
        scopes: vec![HashSet::new()],
        refs: HashSet::new(),
    };
    stmt.visit_with(&mut collector);
    collector.refs
}

fn stmt_ident_refs(stmt: &Stmt) -> HashSet<Atom> {
    let mut collector = IdentRefCollector {
        scopes: vec![HashSet::new()],
        refs: HashSet::new(),
    };
    stmt.visit_with(&mut collector);
    collector.refs
}

struct IdentRefCollector {
    scopes: Vec<HashSet<Atom>>,
    refs: HashSet<Atom>,
}

impl IdentRefCollector {
    fn push_scope(&mut self) {
        self.scopes.push(HashSet::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, sym: &Atom) {
        if let Some(scope) = self.scopes.last_mut() {
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
        self.scopes.iter().rev().any(|scope| scope.contains(sym))
    }
}

impl Visit for IdentRefCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        if !self.is_shadowed(&ident.sym) {
            self.refs.insert(ident.sym.clone());
        }
    }

    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        self.declare(&ident.id.sym);
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
        self.declare_pat(&declarator.name);
        if let Some(init) = &declarator.init {
            init.visit_with(self);
        }
    }

    fn visit_fn_decl(&mut self, function: &FnDecl) {
        self.declare(&function.ident.sym);
        self.visit_function(&function.function);
    }

    fn visit_function(&mut self, function: &Function) {
        self.push_scope();
        for param in &function.params {
            self.declare_pat(&param.pat);
        }
        if let Some(body) = &function.body {
            body.visit_with(self);
        }
        self.pop_scope();
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.push_scope();
        for param in &arrow.params {
            self.declare_pat(param);
        }
        arrow.body.visit_with(self);
        self.pop_scope();
    }

    fn visit_class_decl(&mut self, class: &ClassDecl) {
        self.declare(&class.ident.sym);
        class.class.visit_with(self);
    }
}

struct ScriptImportRefCollector<'a> {
    imports: &'a HashMap<Atom, VueScriptImport>,
    scopes: Vec<HashSet<Atom>>,
    refs: HashSet<Atom>,
}

impl ScriptImportRefCollector<'_> {
    fn push_scope(&mut self) {
        self.scopes.push(HashSet::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, sym: &Atom) {
        if let Some(scope) = self.scopes.last_mut() {
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
        self.scopes.iter().rev().any(|scope| scope.contains(sym))
    }
}

impl Visit for ScriptImportRefCollector<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if self.imports.contains_key(&ident.sym) && !self.is_shadowed(&ident.sym) {
            self.refs.insert(ident.sym.clone());
        }
    }

    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        self.declare(&ident.id.sym);
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
        self.declare_pat(&declarator.name);
        if let Some(init) = &declarator.init {
            init.visit_with(self);
        }
    }

    fn visit_fn_decl(&mut self, function: &FnDecl) {
        self.declare(&function.ident.sym);
        self.visit_function(&function.function);
    }

    fn visit_function(&mut self, function: &Function) {
        self.push_scope();
        for param in &function.params {
            self.declare_pat(&param.pat);
        }
        if let Some(body) = &function.body {
            body.visit_with(self);
        }
        self.pop_scope();
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.push_scope();
        for param in &arrow.params {
            self.declare_pat(param);
        }
        arrow.body.visit_with(self);
        self.pop_scope();
    }

    fn visit_class_decl(&mut self, class: &ClassDecl) {
        self.declare(&class.ident.sym);
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
    shadow_depths: Vec<usize>,
}

impl ComputedLocalInliner {
    fn new(mut bindings: HashMap<Atom, Expr>) -> Self {
        let mut bindings = bindings.drain().collect::<Vec<_>>();
        bindings.sort_by(|(left, _), (right, _)| left.as_ref().cmp(right.as_ref()));
        let shadow_depths = vec![0; bindings.len()];
        Self {
            bindings,
            shadow_depths,
        }
    }

    fn active_index(&self, name: &str) -> Option<usize> {
        self.bindings
            .iter()
            .zip(self.shadow_depths.iter())
            .position(|((binding, _), shadow_depth)| binding.as_ref() == name && *shadow_depth == 0)
    }

    fn shadowing_indices(&self, params: &[&Pat]) -> Vec<usize> {
        let mut param_bindings = HashSet::new();
        for param in params {
            collect_pat_bindings(param, &mut param_bindings);
        }
        self.bindings
            .iter()
            .enumerate()
            .filter_map(|(index, (binding, _))| param_bindings.contains(binding).then_some(index))
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
        let params = arrow.params.iter().collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        arrow.body.visit_mut_with(self);
        self.exit_shadowed(&shadowed);
    }

    fn visit_mut_function(&mut self, function: &mut swc_core::ecma::ast::Function) {
        let params = function
            .params
            .iter()
            .map(|param| &param.pat)
            .collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        if let Some(body) = function.body.as_mut() {
            body.visit_mut_with(self);
        }
        self.exit_shadowed(&shadowed);
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
                let Some(expr) = return_expr_from_stmt(if_stmt.cons.as_ref()) else {
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
