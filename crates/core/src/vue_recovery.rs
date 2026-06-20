use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{anyhow, Result};
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, SourceMap};
use swc_core::ecma::ast::{
    ArrowExpr, BlockStmtOrExpr, CallExpr, Callee, Decl, DefaultDecl, ExportDecl, ExportSpecifier,
    Expr, FnDecl, Ident, ImportSpecifier, MemberExpr, MemberProp, Module, ModuleDecl, ModuleItem,
    ObjectLit, ObjectPatProp, Pat, Prop, PropOrSpread, ReturnStmt, Stmt,
};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::driver::{decompile, DecompileOptions, DecompileOutput};
use crate::js_names::is_valid_identifier_name;
use crate::vue_template::{
    VueAttr, VueDirectiveArg, VueNode, VueSfc, VueTemplate, VueTemplateScope,
};

mod attrs;
mod context;
mod directives;
mod expressions;
mod helpers;
mod imports;
mod nodes;
mod slots;
mod syntax;

use context::{
    collect_context, collect_render_context, collect_script_local_context, collect_setup_context,
    component_name_from_init, infer_render_helpers, is_ref_object_alias, is_ref_object_expr,
    render_context_param, render_local_declaration_with_aliases, setup_context_param,
    setup_emit_param, setup_props_param, stmt_ident_refs,
};
use expressions::print_expr;
use helpers::VueHelper;
use nodes::recover_render_root;
use syntax::{module_export_name, prop_name};

#[derive(Default, Clone)]
struct VueRecoveryContext {
    vue_helpers: HashMap<Atom, VueHelper>,
    vue_helper_candidates: HashSet<Atom>,
    script_imports: HashMap<Atom, VueScriptImport>,
    setup_script_import_refs: HashSet<Atom>,
    object_bindings: HashMap<Atom, ObjectLit>,
    setup_value_bindings: HashMap<Atom, VueSetupValueBinding>,
    setup_prop_bindings: HashMap<Atom, Atom>,
    setup_alias_bindings: HashMap<Atom, Atom>,
    setup_script_bindings: Vec<(Atom, String)>,
    script_local_bindings: Vec<VueSetupLocalBinding>,
    setup_local_bindings: Vec<VueSetupLocalBinding>,
    setup_ref_script_bindings: Vec<VueSetupRefBinding>,
    setup_ref_bindings: HashSet<Atom>,
    setup_composable_ref_bindings: HashSet<Atom>,
    setup_template_ref_bindings: HashSet<Atom>,
    setup_ref_object_bindings: HashSet<Atom>,
    provider_ref_bindings: HashMap<Atom, HashSet<Atom>>,
    imported_composable_ref_props: HashMap<Atom, HashSet<Atom>>,
    component_bindings: HashMap<Atom, String>,
    directive_bindings: HashMap<Atom, String>,
    component_options: Option<ObjectLit>,
    setup_component_options: Option<ObjectLit>,
    render_context: Option<Atom>,
    setup_props_context: Option<Atom>,
    setup_props_aliases: HashSet<Atom>,
    setup_context: Option<Atom>,
    setup_emit_context: Option<Atom>,
    setup_emit_aliases: HashSet<Atom>,
    slot_bindings: HashSet<Atom>,
    render_child_list_bindings: HashMap<Atom, VueRenderChildListBinding>,
    cm: Lrc<SourceMap>,
}

#[derive(Clone)]
enum VueScriptImport {
    Named { source: String, imported: String },
    Default { source: String },
    Namespace { source: String },
}

#[derive(Clone)]
struct VueSetupRefBinding {
    binding: Atom,
    expr: String,
    helper: String,
    known_ref: bool,
}

#[derive(Clone)]
struct VueSetupValueBinding {
    value: String,
    expr: Option<Expr>,
}

#[derive(Clone)]
struct VueSetupLocalBinding {
    bindings: Vec<Atom>,
    emitted_bindings: Vec<Atom>,
    refs: HashSet<Atom>,
    source: String,
    import_refs: HashSet<Atom>,
    stmt: Stmt,
    module_scope: bool,
    template_selectable: bool,
}

#[derive(Clone)]
struct VueRenderChildListBinding {
    source: VueRenderChildListSource,
}

#[derive(Clone, Copy)]
enum VueRenderChildListSource {
    SlotPartitionChildren,
}

#[derive(Clone, Copy)]
pub(super) enum RenderSource<'a> {
    Function(&'a FnDecl),
    SetupArrow {
        render: &'a ArrowExpr,
        setup_stmts: &'a [Stmt],
        setup_props: Option<&'a Ident>,
        setup_context: Option<&'a Ident>,
        setup_emit: Option<&'a Ident>,
        setup_slots: Option<&'a Ident>,
        component_options: Option<&'a ObjectLit>,
    },
}

pub fn recover_vue_sfc_source_from_js(source: &str) -> Result<Option<String>> {
    Ok(recover_vue_sfc_from_js(source)?.map(|sfc| sfc.print()))
}

pub fn recover_vue_sfc_source_from_js_with_import_resolver<F>(
    source: &str,
    mut resolve_import: F,
) -> Result<Option<String>>
where
    F: FnMut(&str) -> Option<String>,
{
    Ok(
        recover_vue_sfc_from_js_with_import_resolver(source, &mut resolve_import)?
            .map(|sfc| sfc.print()),
    )
}

pub fn decompile_vue_sfc(source: &str, options: DecompileOptions) -> Result<DecompileOutput> {
    decompile_vue_sfc_with_import_resolver(source, options, |_| None)
}

pub fn decompile_vue_sfc_with_import_resolver<F>(
    source: &str,
    options: DecompileOptions,
    mut resolve_import: F,
) -> Result<DecompileOutput>
where
    F: FnMut(&str) -> Option<String>,
{
    let preferred_component_name = component_name_from_filename(&options.filename);
    if let Some(output) = decompile_single_unpacked_vue_sfc(
        source,
        options.clone(),
        preferred_component_name.as_deref(),
        &mut resolve_import,
    )? {
        return Ok(output);
    }

    let mut output = decompile(source, options)?;
    if let Some(sfc) = recover_vue_sfc_from_js_inner(
        &output.code,
        Some(&mut resolve_import),
        preferred_component_name.as_deref(),
    )?
    .map(|sfc| sfc.print())
    {
        output.code = sfc;
        return Ok(output);
    }

    Ok(output)
}

fn decompile_single_unpacked_vue_sfc<F>(
    source: &str,
    mut options: DecompileOptions,
    preferred_component_name: Option<&str>,
    resolve_import: &mut F,
) -> Result<Option<DecompileOutput>>
where
    F: FnMut(&str) -> Option<String>,
{
    let Some(result) = crate::unpacker::unpack_bundle(source) else {
        return Ok(None);
    };
    if result.modules.len() != 1 {
        return Ok(None);
    }
    let module = result
        .modules
        .into_iter()
        .next()
        .expect("checked single unpacked module");

    options.filename = module.filename;
    options.sourcemap = None;
    let mut output = decompile(&module.code, options)?;
    let Some(sfc) = recover_vue_sfc_from_js_inner(
        &output.code,
        Some(resolve_import),
        preferred_component_name,
    )?
    .map(|sfc| sfc.print()) else {
        return Ok(None);
    };
    output.code = sfc;
    Ok(Some(output))
}

pub fn recover_vue_sfc_from_js(source: &str) -> Result<Option<VueSfc>> {
    recover_vue_sfc_from_js_inner(source, None, None)
}

pub fn recover_vue_sfc_from_js_with_import_resolver<F>(
    source: &str,
    mut resolve_import: F,
) -> Result<Option<VueSfc>>
where
    F: FnMut(&str) -> Option<String>,
{
    recover_vue_sfc_from_js_inner(source, Some(&mut resolve_import), None)
}

fn recover_vue_sfc_from_js_inner(
    source: &str,
    import_resolver: Option<&mut dyn FnMut(&str) -> Option<String>>,
    preferred_component_name: Option<&str>,
) -> Result<Option<VueSfc>> {
    let cm: Lrc<SourceMap> = Default::default();
    let module = parse_module(source, cm.clone())?;
    let imported_metadata = import_resolver
        .map(|resolver| collect_imported_vue_metadata(&module, resolver))
        .transpose()?
        .unwrap_or_default();
    let mut composable_ref_props = imported_metadata.composable_ref_props;
    composable_ref_props.extend(imports::local_composable_ref_props_from_module(&module));
    let mut ctx = collect_context(
        &module,
        cm,
        imported_metadata.component_bindings,
        composable_ref_props,
    );
    let Some(render) = find_render_source(&module, preferred_component_name) else {
        return Ok(None);
    };
    if let Some(options) = render_component_options(render) {
        ctx.setup_component_options = Some(options.clone());
    }
    ctx.render_context = render_context_param(render);
    ctx.setup_props_context = setup_props_param(render);
    ctx.setup_context = setup_context_param(render);
    ctx.setup_emit_context = setup_emit_param(render);
    infer_render_helpers(render, &mut ctx);
    collect_setup_context(render, &mut ctx)?;
    collect_render_context(render, &mut ctx);
    collect_script_local_context(&module, &mut ctx)?;
    if !render_uses_vue_helper(render, &ctx) {
        return Ok(None);
    }
    let Some(root) = recover_render_root(render, &ctx)? else {
        return Ok(None);
    };

    let script_setup = setup_script(&ctx, &root, render)?;

    let script = if script_setup.is_none() {
        ctx.component_options
            .as_ref()
            .and_then(|options| component_script(options, &ctx).transpose())
            .transpose()?
    } else {
        None
    };

    Ok(Some(VueSfc {
        script,
        script_setup,
        template: VueTemplate {
            children: vec![root],
        },
    }))
}

#[derive(Default)]
struct ImportedVueMetadata {
    component_bindings: HashMap<Atom, String>,
    composable_ref_props: HashMap<Atom, HashSet<Atom>>,
}

struct ResolvedImportMetadata {
    component_exports: HashMap<String, String>,
    composable_ref_props: HashMap<String, HashSet<Atom>>,
}

fn collect_imported_vue_metadata(
    module: &Module,
    resolve_import: &mut dyn FnMut(&str) -> Option<String>,
) -> Result<ImportedVueMetadata> {
    let mut metadata = ImportedVueMetadata::default();
    let mut export_cache: HashMap<String, ResolvedImportMetadata> = HashMap::new();

    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        if import.specifiers.is_empty() {
            continue;
        }
        let source = syntax::wtf8_to_string(&import.src.value);
        if source == "vue" {
            continue;
        }

        let source_metadata = if let Some(metadata) = export_cache.get(&source) {
            metadata
        } else {
            let resolved = resolve_import(&source).unwrap_or_default();
            let source_metadata = ResolvedImportMetadata {
                component_exports: component_exports_from_source(&resolved).unwrap_or_default(),
                composable_ref_props: imports::composable_ref_props_from_source(&resolved),
            };
            export_cache.insert(source.clone(), source_metadata);
            export_cache
                .get(&source)
                .expect("inserted source export cache")
        };
        if source_metadata.component_exports.is_empty()
            && source_metadata.composable_ref_props.is_empty()
        {
            continue;
        }

        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Named(named) => {
                    let imported = named
                        .imported
                        .as_ref()
                        .map(module_export_name)
                        .unwrap_or_else(|| named.local.sym.to_string());
                    if let Some(component) = source_metadata.component_exports.get(&imported) {
                        metadata
                            .component_bindings
                            .insert(named.local.sym.clone(), component.clone());
                    }
                    if let Some(ref_props) = source_metadata.composable_ref_props.get(&imported) {
                        metadata
                            .composable_ref_props
                            .insert(named.local.sym.clone(), ref_props.clone());
                    }
                }
                ImportSpecifier::Default(default) => {
                    if let Some(component) = source_metadata.component_exports.get("default") {
                        metadata
                            .component_bindings
                            .insert(default.local.sym.clone(), component.clone());
                    }
                    if let Some(ref_props) = source_metadata.composable_ref_props.get("default") {
                        metadata
                            .composable_ref_props
                            .insert(default.local.sym.clone(), ref_props.clone());
                    }
                }
                ImportSpecifier::Namespace(_) => {}
            }
        }
    }

    Ok(metadata)
}

fn component_exports_from_source(source: &str) -> Result<HashMap<String, String>> {
    let cm: Lrc<SourceMap> = Default::default();
    let exports = parse_module(source, cm)
        .map(|module| component_exports_from_module(&module))
        .unwrap_or_default();
    if !exports.is_empty() {
        return Ok(exports);
    }

    let Some(result) = crate::unpacker::unpack_bundle(source) else {
        return Ok(exports);
    };
    if result.modules.len() != 1 {
        return Ok(exports);
    }
    let Some(module) = result.modules.into_iter().next() else {
        return Ok(exports);
    };
    let cm: Lrc<SourceMap> = Default::default();
    let module = parse_module(&module.code, cm)?;
    Ok(component_exports_from_module(&module))
}

fn component_exports_from_module(module: &Module) -> HashMap<String, String> {
    let component_bindings = collect_local_component_bindings(module);
    collect_component_exports(module, &component_bindings)
}

fn collect_local_component_bindings(module: &Module) -> HashMap<Atom, String> {
    let mut component_bindings = HashMap::new();

    for item in &module.body {
        let (ModuleItem::Stmt(Stmt::Decl(Decl::Var(var)))
        | ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
            decl: Decl::Var(var),
            ..
        }))) = item
        else {
            continue;
        };

        for decl in &var.decls {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            if let Some(component) = component_name_from_init(init, &component_bindings) {
                component_bindings.insert(binding.id.sym.clone(), component);
            }
        }
    }

    component_bindings
}

fn collect_component_exports(
    module: &Module,
    component_bindings: &HashMap<Atom, String>,
) -> HashMap<String, String> {
    let mut exports = HashMap::new();

    for item in &module.body {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::ExportDecl(export) => {
                if let Decl::Var(var) = &export.decl {
                    for decl in &var.decls {
                        let Pat::Ident(binding) = &decl.name else {
                            continue;
                        };
                        if let Some(component) = component_bindings.get(&binding.id.sym) {
                            exports.insert(binding.id.sym.to_string(), component.clone());
                        }
                    }
                }
            }
            ModuleDecl::ExportDefaultExpr(export) => match export.expr.as_ref() {
                Expr::Ident(ident) => {
                    if let Some(component) = component_bindings.get(&ident.sym) {
                        exports.insert("default".to_string(), component.clone());
                    }
                }
                expr => {
                    if let Some(component) = component_name_from_init(expr, component_bindings) {
                        exports.insert("default".to_string(), component);
                    }
                }
            },
            ModuleDecl::ExportDefaultDecl(export) => {
                let local = match &export.decl {
                    DefaultDecl::Fn(function) => function.ident.as_ref().map(|ident| &ident.sym),
                    DefaultDecl::Class(class) => class.ident.as_ref().map(|ident| &ident.sym),
                    DefaultDecl::TsInterfaceDecl(_) => None,
                };
                if let Some(component) = local.and_then(|local| component_bindings.get(local)) {
                    exports.insert("default".to_string(), component.clone());
                }
            }
            ModuleDecl::ExportNamed(named) if named.src.is_none() => {
                for specifier in &named.specifiers {
                    match specifier {
                        ExportSpecifier::Named(named) => {
                            let local = Atom::from(module_export_name(&named.orig));
                            let exported = named
                                .exported
                                .as_ref()
                                .map(module_export_name)
                                .unwrap_or_else(|| local.to_string());
                            if let Some(component) = component_bindings.get(&local) {
                                exports.insert(exported, component.clone());
                            }
                        }
                        ExportSpecifier::Default(default) => {
                            if let Some(component) = component_bindings.get(&default.exported.sym) {
                                exports.insert("default".to_string(), component.clone());
                            }
                        }
                        ExportSpecifier::Namespace(_) => {}
                    }
                }
            }
            _ => {}
        }
    }

    exports
}

fn component_name_from_filename(filename: &str) -> Option<String> {
    let leaf = Path::new(filename).file_name()?.to_str()?;
    let name = leaf
        .split(".vue")
        .next()
        .filter(|name| *name != leaf)
        .or_else(|| leaf.rsplit_once('.').map(|(stem, _)| stem))
        .unwrap_or(leaf)
        .split('-')
        .next()
        .unwrap_or(leaf);
    let starts_with_uppercase = name
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase());
    (starts_with_uppercase && !name.is_empty()).then(|| name.to_string())
}

pub(super) fn parse_module(source: &str, cm: Lrc<SourceMap>) -> Result<Module> {
    let fm = cm.new_source_file(
        FileName::Custom("vue-recovery.js".into()).into(),
        source.to_string(),
    );
    let lexer = Lexer::new(
        Syntax::Es(EsSyntax {
            jsx: true,
            ..Default::default()
        }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    parser
        .parse_module()
        .map_err(|error| anyhow!("failed to parse decompiled Vue module: {error:?}"))
}

fn find_render_source<'a>(
    module: &'a Module,
    preferred_component_name: Option<&str>,
) -> Option<RenderSource<'a>> {
    if let Some(preferred_component_name) = preferred_component_name {
        if let Some(render) =
            setup_render_source_for_component_name(module, preferred_component_name)
        {
            return Some(render);
        }
    }

    find_render_fn(module)
        .map(RenderSource::Function)
        .or_else(|| find_setup_render_source(module))
}

fn render_component_options(render: RenderSource<'_>) -> Option<&ObjectLit> {
    match render {
        RenderSource::SetupArrow {
            component_options, ..
        } => component_options,
        RenderSource::Function(_) => None,
    }
}

fn find_render_fn(module: &Module) -> Option<&FnDecl> {
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                decl: Decl::Fn(fn_decl),
                ..
            })) if fn_decl.ident.sym.as_ref() == "render" => return Some(fn_decl),
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl)))
                if fn_decl.ident.sym.as_ref() == "render" =>
            {
                return Some(fn_decl);
            }
            _ => {}
        }
    }
    None
}

fn setup_render_source_for_component_name<'a>(
    module: &'a Module,
    component_name: &str,
) -> Option<RenderSource<'a>> {
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export)) => {
                if let Some(render) =
                    setup_render_source_from_component_expr(export.expr.as_ref(), component_name)
                {
                    return Some(render);
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                if let Decl::Var(var) = &export.decl {
                    for decl in &var.decls {
                        let Some(init) = decl.init.as_deref() else {
                            continue;
                        };
                        if let Some(render) =
                            setup_render_source_from_component_expr(init, component_name)
                        {
                            return Some(render);
                        }
                    }
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    let Some(init) = decl.init.as_deref() else {
                        continue;
                    };
                    if let Some(render) =
                        setup_render_source_from_component_expr(init, component_name)
                    {
                        return Some(render);
                    }
                }
            }
            _ => {}
        }
    }

    None
}

fn setup_render_source_from_component_expr<'a>(
    expr: &'a Expr,
    component_name: &str,
) -> Option<RenderSource<'a>> {
    let component_bindings = HashMap::new();
    if component_name_from_init(expr, &component_bindings).as_deref() != Some(component_name) {
        return None;
    }
    setup_render_source_from_expr(expr)
}

fn find_setup_render_source(module: &Module) -> Option<RenderSource<'_>> {
    if let Some(render) = direct_exported_setup_render_source(module) {
        return Some(render);
    }

    for local in preferred_setup_export_names(module) {
        if let Some(render) = setup_render_source_from_binding(module, &local) {
            return Some(render);
        }
    }

    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export)) => {
                if let Some(render) = setup_render_source_from_expr(export.expr.as_ref()) {
                    return Some(render);
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                if let Decl::Var(var) = &export.decl {
                    for decl in &var.decls {
                        let Some(init) = decl.init.as_deref() else {
                            continue;
                        };
                        if let Some(render) = setup_render_source_from_expr(init) {
                            return Some(render);
                        }
                    }
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    let Some(init) = decl.init.as_deref() else {
                        continue;
                    };
                    if let Some(render) = setup_render_source_from_expr(init) {
                        return Some(render);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn direct_exported_setup_render_source(module: &Module) -> Option<RenderSource<'_>> {
    for preferred_name in ["_", "default"] {
        for item in &module.body {
            let ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) = item else {
                continue;
            };
            let Decl::Var(var) = &export.decl else {
                continue;
            };
            for decl in &var.decls {
                let Pat::Ident(binding) = &decl.name else {
                    continue;
                };
                if binding.id.sym.as_ref() != preferred_name {
                    continue;
                }
                let Some(init) = decl.init.as_deref() else {
                    continue;
                };
                if let Some(render) = setup_render_source_from_expr(init) {
                    return Some(render);
                }
            }
        }
    }
    None
}

fn preferred_setup_export_names(module: &Module) -> Vec<String> {
    let mut names = Vec::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export)) = item else {
            continue;
        };
        for specifier in &export.specifiers {
            let ExportSpecifier::Named(named) = specifier else {
                continue;
            };
            let local = module_export_name(&named.orig);
            let exported = named
                .exported
                .as_ref()
                .map(module_export_name)
                .unwrap_or_else(|| local.clone());
            if exported == "_" || exported == "default" {
                names.push(local);
            }
        }
    }
    names
}

fn setup_render_source_from_binding<'a>(
    module: &'a Module,
    local: &str,
) -> Option<RenderSource<'a>> {
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            if binding.id.sym.as_ref() != local {
                continue;
            }
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            if let Some(render) = setup_render_source_from_expr(init) {
                return Some(render);
            }
        }
    }
    None
}

fn setup_render_source_from_expr(expr: &Expr) -> Option<RenderSource<'_>> {
    match expr {
        Expr::Paren(paren) => setup_render_source_from_expr(paren.expr.as_ref()),
        Expr::Call(call) => call
            .args
            .first()
            .and_then(|arg| setup_render_source_from_expr(arg.expr.as_ref())),
        Expr::Object(object) => setup_render_source_from_options(object),
        _ => None,
    }
}

fn setup_render_source_from_options(object: &ObjectLit) -> Option<RenderSource<'_>> {
    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            continue;
        };
        match prop.as_ref() {
            Prop::Method(method) if prop_name(&method.key).as_deref() == Some("setup") => {
                let Some(body) = method.function.body.as_ref() else {
                    continue;
                };
                if let Some(render) = return_arrow_from_stmts(&body.stmts) {
                    return Some(RenderSource::SetupArrow {
                        render,
                        setup_stmts: body.stmts.as_slice(),
                        setup_props: method
                            .function
                            .params
                            .first()
                            .and_then(syntax::param_binding_ident),
                        setup_context: method
                            .function
                            .params
                            .get(1)
                            .and_then(|param| pat_binding_ident(&param.pat)),
                        setup_emit: method
                            .function
                            .params
                            .get(1)
                            .and_then(|param| setup_emit_binding_ident(&param.pat)),
                        setup_slots: method
                            .function
                            .params
                            .get(1)
                            .and_then(|param| setup_slots_binding_ident(&param.pat)),
                        component_options: Some(object),
                    });
                }
            }
            Prop::KeyValue(key_value) if prop_name(&key_value.key).as_deref() == Some("setup") => {
                if let Some(render) = setup_return_source_from_expr(key_value.value.as_ref()) {
                    return Some(with_component_options(render, object));
                }
            }
            _ => {}
        }
    }
    None
}

fn setup_return_source_from_expr(expr: &Expr) -> Option<RenderSource<'_>> {
    match expr {
        Expr::Paren(paren) => setup_return_source_from_expr(paren.expr.as_ref()),
        Expr::Arrow(arrow) => match arrow.body.as_ref() {
            BlockStmtOrExpr::BlockStmt(block) => {
                return_arrow_from_stmts(&block.stmts).map(|render| RenderSource::SetupArrow {
                    render,
                    setup_stmts: block.stmts.as_slice(),
                    setup_props: arrow.params.first().and_then(pat_binding_ident),
                    setup_context: arrow.params.get(1).and_then(pat_binding_ident),
                    setup_emit: arrow.params.get(1).and_then(setup_emit_binding_ident),
                    setup_slots: arrow.params.get(1).and_then(setup_slots_binding_ident),
                    component_options: None,
                })
            }
            BlockStmtOrExpr::Expr(expr) => {
                arrow_expr(expr.as_ref()).map(|render| RenderSource::SetupArrow {
                    render,
                    setup_stmts: &[],
                    setup_props: arrow.params.first().and_then(pat_binding_ident),
                    setup_context: arrow.params.get(1).and_then(pat_binding_ident),
                    setup_emit: arrow.params.get(1).and_then(setup_emit_binding_ident),
                    setup_slots: arrow.params.get(1).and_then(setup_slots_binding_ident),
                    component_options: None,
                })
            }
        },
        Expr::Fn(fn_expr) => fn_expr.function.body.as_ref().and_then(|body| {
            return_arrow_from_stmts(&body.stmts).map(|render| RenderSource::SetupArrow {
                render,
                setup_stmts: body.stmts.as_slice(),
                setup_props: fn_expr
                    .function
                    .params
                    .first()
                    .and_then(syntax::param_binding_ident),
                setup_context: fn_expr
                    .function
                    .params
                    .get(1)
                    .and_then(|param| pat_binding_ident(&param.pat)),
                setup_emit: fn_expr
                    .function
                    .params
                    .get(1)
                    .and_then(|param| setup_emit_binding_ident(&param.pat)),
                setup_slots: fn_expr
                    .function
                    .params
                    .get(1)
                    .and_then(|param| setup_slots_binding_ident(&param.pat)),
                component_options: None,
            })
        }),
        _ => None,
    }
}

fn with_component_options<'a>(
    render: RenderSource<'a>,
    component_options: &'a ObjectLit,
) -> RenderSource<'a> {
    match render {
        RenderSource::SetupArrow {
            render,
            setup_stmts,
            setup_props,
            setup_context,
            setup_emit,
            setup_slots,
            ..
        } => RenderSource::SetupArrow {
            render,
            setup_stmts,
            setup_props,
            setup_context,
            setup_emit,
            setup_slots,
            component_options: Some(component_options),
        },
        RenderSource::Function(function) => RenderSource::Function(function),
    }
}

fn return_arrow_from_stmts(stmts: &[Stmt]) -> Option<&ArrowExpr> {
    stmts.iter().rev().find_map(|stmt| match stmt {
        Stmt::Return(ReturnStmt {
            arg: Some(expr), ..
        }) => arrow_expr(expr.as_ref()),
        _ => None,
    })
}

fn arrow_expr(expr: &Expr) -> Option<&ArrowExpr> {
    match expr {
        Expr::Paren(paren) => arrow_expr(paren.expr.as_ref()),
        Expr::Arrow(arrow) => Some(arrow),
        _ => None,
    }
}

fn pat_binding_ident(pat: &Pat) -> Option<&Ident> {
    match pat {
        Pat::Ident(binding) => Some(&binding.id),
        _ => None,
    }
}

fn setup_emit_binding_ident(pat: &Pat) -> Option<&Ident> {
    let Pat::Object(object) = pat else {
        return None;
    };

    object.props.iter().find_map(|prop| match prop {
        ObjectPatProp::KeyValue(key_value)
            if prop_name(&key_value.key).as_deref() == Some("emit") =>
        {
            pat_binding_ident(key_value.value.as_ref())
        }
        ObjectPatProp::Assign(assign) if assign.key.sym.as_ref() == "emit" => Some(&assign.key),
        _ => None,
    })
}

fn setup_slots_binding_ident(pat: &Pat) -> Option<&Ident> {
    let Pat::Object(object) = pat else {
        return None;
    };

    object.props.iter().find_map(|prop| match prop {
        ObjectPatProp::KeyValue(key_value)
            if prop_name(&key_value.key).as_deref() == Some("slots") =>
        {
            pat_binding_ident(key_value.value.as_ref())
        }
        ObjectPatProp::Assign(assign) if assign.key.sym.as_ref() == "slots" => Some(&assign.key),
        _ => None,
    })
}

fn render_uses_vue_helper(render: RenderSource<'_>, ctx: &VueRecoveryContext) -> bool {
    if ctx.vue_helpers.is_empty() {
        return false;
    }

    struct Finder<'a> {
        helpers: &'a HashMap<Atom, VueHelper>,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Ident(ident) = callee.as_ref() {
                    if self.helpers.contains_key(&ident.sym) {
                        self.found = true;
                        return;
                    }
                }
            }

            call.visit_children_with(self);
        }
    }

    let mut finder = Finder {
        helpers: &ctx.vue_helpers,
        found: false,
    };
    match render {
        RenderSource::Function(render) => {
            let Some(body) = render.function.body.as_ref() else {
                return false;
            };
            body.visit_with(&mut finder);
        }
        RenderSource::SetupArrow { render, .. } => render.body.visit_with(&mut finder),
    }
    finder.found
}

fn component_script(options: &ObjectLit, ctx: &VueRecoveryContext) -> Result<Option<String>> {
    if options.props.is_empty() {
        return Ok(None);
    }
    let printed = print_expr(&Expr::Object(options.clone()), ctx)?;
    Ok(Some(format!("export default {printed}")))
}

fn setup_script(
    ctx: &VueRecoveryContext,
    root: &VueNode,
    render: RenderSource<'_>,
) -> Result<Option<String>> {
    let ref_declarations = setup_ref_declarations(ctx, root, render);
    let selected_local_declarations = setup_local_declarations(ctx, root);
    let emit_declaration = setup_emit_declaration(ctx, root, &selected_local_declarations)?;
    let prop_names = ctx
        .setup_component_options
        .as_ref()
        .or(ctx.component_options.as_ref())
        .map(component_prop_names)
        .unwrap_or_default();
    let valid_prop_names = prop_names
        .iter()
        .filter(|name| is_valid_identifier_name(name))
        .cloned()
        .collect::<Vec<_>>();
    let prop_bindings = setup_prop_bindings(&valid_prop_names, ctx);
    let props_binding_reserved = props_binding_reserved_names(
        ctx,
        &valid_prop_names,
        emit_declaration.as_ref(),
        &ref_declarations,
    );
    let props_declaration = setup_props_script_binding(ctx, &props_binding_reserved)
        .map(|binding| {
            component_props_source(ctx).map(|source| source.map(|source| (binding, source)))
        })
        .transpose()?
        .flatten();
    let local_declarations = render_setup_local_declarations(
        ctx,
        selected_local_declarations,
        &prop_bindings,
        props_declaration.as_ref(),
        emit_declaration.as_ref(),
        &ref_declarations,
    )?;
    let declared_bindings = script_setup_declared_bindings(
        ctx,
        &prop_bindings,
        props_declaration.as_ref(),
        emit_declaration.as_ref(),
        &ref_declarations,
        &local_declarations,
    );
    let script_imports =
        referenced_script_imports(ctx, root, &declared_bindings, &local_declarations);
    if ctx.setup_script_bindings.is_empty()
        && local_declarations.is_empty()
        && ref_declarations.is_empty()
        && props_declaration.is_none()
        && emit_declaration.is_none()
        && script_imports.is_empty()
    {
        return Ok(None);
    }

    let mut body = String::new();

    if let Some((binding, props_source)) = &props_declaration {
        body.push_str("const ");
        body.push_str(binding);
        body.push_str(" = defineProps(");
        body.push_str(props_source);
        body.push_str(");\n");
        if !valid_prop_names.is_empty() {
            body.push_str("const { ");
            body.push_str(&format_prop_destructure_bindings(&prop_bindings));
            body.push_str(" } = ");
            body.push_str(binding);
            body.push_str(";\n");
        }
        body.push('\n');
    }

    if let Some((binding, emits_source)) = &emit_declaration {
        body.push_str("const ");
        body.push_str(binding);
        body.push_str(" = defineEmits(");
        body.push_str(emits_source);
        body.push_str(");\n");
        if !ref_declarations.is_empty()
            || !local_declarations.is_empty()
            || !ctx.setup_script_bindings.is_empty()
        {
            body.push('\n');
        }
    }

    for (binding, expr, _) in &ref_declarations {
        body.push_str("const ");
        body.push_str(binding);
        body.push_str(" = ");
        body.push_str(expr.trim());
        body.push_str(";\n");
    }
    if !ref_declarations.is_empty()
        && (!local_declarations.is_empty() || !ctx.setup_script_bindings.is_empty())
    {
        body.push('\n');
    }

    for declaration in &local_declarations {
        body.push_str(declaration.source.trim());
        body.push('\n');
    }
    if !local_declarations.is_empty() && !ctx.setup_script_bindings.is_empty() {
        body.push('\n');
    }

    let mut bindings = ctx.setup_script_bindings.clone();
    bindings.sort_by(|(left, _), (right, _)| left.as_ref().cmp(right.as_ref()));
    for (binding, expr) in bindings {
        if !is_valid_identifier_name(binding.as_ref()) {
            continue;
        }
        body.push_str("const ");
        body.push_str(binding.as_ref());
        body.push_str(" = ");
        body.push_str(expr.trim());
        body.push_str(";\n");
    }

    let mut out = String::new();
    if let Some(vue_import) = vue_script_import_line(ctx, &ref_declarations, &local_declarations) {
        out.push_str(&vue_import);
        out.push('\n');
    }
    for import in script_imports {
        out.push_str(&import);
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&body);
    Ok(Some(out))
}

fn render_setup_local_declarations(
    ctx: &VueRecoveryContext,
    local_declarations: Vec<&VueSetupLocalBinding>,
    prop_bindings: &[(String, String)],
    props_declaration: Option<&(String, String)>,
    emit_declaration: Option<&(String, String)>,
    ref_declarations: &[(String, String, String)],
) -> Result<Vec<VueSetupLocalBinding>> {
    let aliases = script_local_binding_aliases(
        ctx,
        &local_declarations,
        prop_bindings,
        props_declaration,
        emit_declaration,
        ref_declarations,
    );
    let mut rendered = Vec::new();
    let mut rendered_module_bindings = HashSet::new();
    for declaration in local_declarations {
        if declaration.module_scope {
            if declaration
                .emitted_bindings
                .iter()
                .any(|binding| rendered_module_bindings.contains(binding))
            {
                continue;
            }
            rendered_module_bindings.extend(declaration.emitted_bindings.iter().cloned());
        }
        let declaration = render_local_declaration_with_aliases(
            ctx,
            declaration,
            &aliases,
            props_declaration
                .as_ref()
                .map(|(binding, _)| binding.as_str()),
        )?;
        if !declaration.source.is_empty() {
            rendered.push(declaration);
        }
    }
    Ok(rendered)
}

fn script_local_binding_aliases(
    ctx: &VueRecoveryContext,
    local_declarations: &[&VueSetupLocalBinding],
    prop_bindings: &[(String, String)],
    props_declaration: Option<&(String, String)>,
    emit_declaration: Option<&(String, String)>,
    ref_declarations: &[(String, String, String)],
) -> HashMap<Atom, Atom> {
    let mut used = HashSet::new();
    used.extend(ctx.script_imports.keys().cloned());
    used.extend(
        ctx.setup_script_bindings
            .iter()
            .map(|(binding, _)| binding.clone()),
    );
    used.extend(
        ctx.setup_ref_script_bindings
            .iter()
            .map(|binding| binding.binding.clone()),
    );
    used.extend(ctx.setup_value_bindings.keys().cloned());
    used.extend(ctx.setup_alias_bindings.keys().cloned());
    used.extend(ctx.setup_emit_aliases.iter().cloned());
    used.extend(
        prop_bindings
            .iter()
            .map(|(_, binding)| Atom::from(binding.clone())),
    );
    if let Some((binding, _)) = props_declaration {
        used.insert(Atom::from(binding.clone()));
    }
    if let Some((binding, _)) = emit_declaration {
        used.insert(Atom::from(binding.clone()));
    }
    used.extend(
        ref_declarations
            .iter()
            .map(|(binding, _, _)| Atom::from(binding.clone())),
    );
    used.extend(
        local_declarations
            .iter()
            .filter(|declaration| !declaration.module_scope)
            .flat_map(|declaration| declaration.emitted_bindings.iter().cloned()),
    );

    let mut aliases = HashMap::new();
    let mut seen_module_bindings = HashSet::new();
    for declaration in local_declarations {
        if !declaration.module_scope {
            continue;
        }
        for binding in &declaration.bindings {
            if aliases.contains_key(binding) {
                continue;
            }
            let emitted_binding = emitted_binding_for_alias(declaration, binding);
            if !seen_module_bindings.insert(emitted_binding.clone()) {
                continue;
            }
            if !is_valid_identifier_name(emitted_binding.as_ref()) {
                continue;
            }
            if used.contains(emitted_binding) {
                let alias = unique_script_local_binding(emitted_binding, &mut used);
                aliases.insert(binding.clone(), alias);
            } else {
                used.insert(emitted_binding.clone());
            }
        }
    }
    aliases
}

fn emitted_binding_for_alias<'a>(
    declaration: &'a VueSetupLocalBinding,
    binding: &'a Atom,
) -> &'a Atom {
    if declaration
        .emitted_bindings
        .iter()
        .any(|emitted| emitted == binding)
    {
        return binding;
    }
    if declaration.bindings.len() == 1 && declaration.emitted_bindings.len() == 1 {
        &declaration.emitted_bindings[0]
    } else {
        binding
    }
}

fn unique_script_local_binding(binding: &Atom, used: &mut HashSet<Atom>) -> Atom {
    let mut index = 1;
    loop {
        let candidate = Atom::from(format!("{}_{index}", binding.as_ref()));
        if used.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

fn component_props_source(ctx: &VueRecoveryContext) -> Result<Option<String>> {
    let Some(props_expr) = ctx
        .setup_component_options
        .as_ref()
        .or(ctx.component_options.as_ref())
        .and_then(component_props_expr)
    else {
        return Ok(None);
    };

    Ok(Some(print_expr(props_expr, ctx)?))
}

fn component_emits_source(ctx: &VueRecoveryContext) -> Result<Option<String>> {
    let Some(emits_expr) = ctx
        .setup_component_options
        .as_ref()
        .or(ctx.component_options.as_ref())
        .and_then(component_emits_expr)
    else {
        return Ok(None);
    };

    Ok(Some(print_expr(emits_expr, ctx)?))
}

fn setup_emit_declaration(
    ctx: &VueRecoveryContext,
    root: &VueNode,
    local_declarations: &[&VueSetupLocalBinding],
) -> Result<Option<(String, String)>> {
    let Some(binding) = setup_emit_script_binding(ctx, root, local_declarations) else {
        return Ok(None);
    };
    let Some(emits_source) = component_emits_source(ctx)? else {
        return Ok(None);
    };

    Ok(Some((binding, emits_source)))
}

fn vue_script_import_line(
    ctx: &VueRecoveryContext,
    ref_declarations: &[(String, String, String)],
    local_declarations: &[VueSetupLocalBinding],
) -> Option<String> {
    let mut imports = Vec::<(String, String)>::new();
    if !ctx.setup_script_bindings.is_empty() {
        imports.push(("computed".to_string(), "computed".to_string()));
    }
    for (_, _, helper) in ref_declarations {
        imports.push((helper.clone(), helper.clone()));
    }
    for declaration in local_declarations {
        for local in stmt_ident_refs(&declaration.stmt) {
            if ctx.script_imports.contains_key(&local) {
                continue;
            }
            let Some(helper) = ctx.vue_helpers.get(&local) else {
                continue;
            };
            imports.push((
                vue_helper_import_name(helper).to_string(),
                local.as_ref().to_string(),
            ));
        }
    }
    imports.sort();
    imports.dedup();
    if imports.is_empty() {
        None
    } else {
        let specifiers = imports
            .into_iter()
            .map(|(imported, local)| {
                if imported == local {
                    imported
                } else {
                    format!("{imported} as {local}")
                }
            })
            .collect::<Vec<_>>();
        Some(format!(
            "import {{ {} }} from \"vue\";",
            specifiers.join(", ")
        ))
    }
}

fn vue_helper_import_name(helper: &VueHelper) -> &str {
    match helper {
        VueHelper::Computed => "computed",
        VueHelper::CreateBlock => "createBlock",
        VueHelper::CreateCommentVNode => "createCommentVNode",
        VueHelper::CreateElementBlock => "createElementBlock",
        VueHelper::CreateElementVNode => "createElementVNode",
        VueHelper::CreateSlots => "createSlots",
        VueHelper::CreateStaticVNode => "createStaticVNode",
        VueHelper::CreateTextVNode => "createTextVNode",
        VueHelper::CreateVNode => "createVNode",
        VueHelper::Fragment => "Fragment",
        VueHelper::OpenBlock => "openBlock",
        VueHelper::RenderList => "renderList",
        VueHelper::RenderSlot => "renderSlot",
        VueHelper::ResolveComponent => "resolveComponent",
        VueHelper::ResolveDirective => "resolveDirective",
        VueHelper::ResolveDynamicComponent => "resolveDynamicComponent",
        VueHelper::ToDisplayString => "toDisplayString",
        VueHelper::Unref => "unref",
        VueHelper::VModel(name) | VueHelper::Other(name) => name.as_ref(),
        VueHelper::VShow => "vShow",
        VueHelper::WithCtx => "withCtx",
        VueHelper::WithDirectives => "withDirectives",
        VueHelper::WithKeys => "withKeys",
        VueHelper::WithMemo => "withMemo",
        VueHelper::WithModifiers => "withModifiers",
    }
}

fn setup_ref_declarations(
    ctx: &VueRecoveryContext,
    root: &VueNode,
    render: RenderSource<'_>,
) -> Vec<(String, String, String)> {
    let expr_refs = template_expr_refs(root);
    let template_refs = template_static_ref_names(root);
    let render_value_refs = render_value_member_refs(render, ctx);
    let mut declared = HashSet::new();
    let mut declarations = Vec::new();

    let mut bindings = ctx.setup_ref_script_bindings.clone();
    bindings.sort_by(|left, right| left.binding.as_ref().cmp(right.binding.as_ref()));
    for binding in bindings {
        let name = binding.binding.as_ref();
        if !is_valid_identifier_name(name) {
            continue;
        }
        if !binding.known_ref
            && !render_value_refs.contains(&binding.binding)
            && !template_refs.iter().any(|ref_name| ref_name == name)
        {
            continue;
        }
        if !expr_refs.contains(&binding.binding)
            && !template_refs.iter().any(|ref_name| ref_name == name)
        {
            continue;
        }
        if declared.insert(name.to_string()) {
            declarations.push((name.to_string(), binding.expr, binding.helper));
        }
    }

    for name in template_refs {
        if declared.insert(name.clone()) {
            declarations.push((name, "ref(null)".to_string(), "ref".to_string()));
        }
    }

    declarations
}

fn setup_local_declarations<'a>(
    ctx: &'a VueRecoveryContext,
    root: &VueNode,
) -> Vec<&'a VueSetupLocalBinding> {
    if ctx.script_local_bindings.is_empty() && ctx.setup_local_bindings.is_empty() {
        return Vec::new();
    }

    let candidates = ctx
        .script_local_bindings
        .iter()
        .chain(ctx.setup_local_bindings.iter())
        .collect::<Vec<_>>();
    let setup_scope_bindings = setup_scope_bindings(ctx, &candidates);
    let event_refs = template_event_expr_refs(root);
    let expr_refs = template_expr_refs(root);
    let expr_read_refs = template_expr_read_refs(root);
    let mut setup_wanted_refs = event_refs;
    let mut module_wanted_refs = HashSet::new();
    setup_wanted_refs.extend(
        expr_refs
            .iter()
            .filter(|binding| ctx.setup_composable_ref_bindings.contains(*binding))
            .cloned(),
    );
    for declaration in &candidates {
        if selects_safe_template_expr_local(ctx, declaration, &expr_refs, &expr_read_refs) {
            setup_wanted_refs.extend(
                declaration
                    .bindings
                    .iter()
                    .chain(declaration.emitted_bindings.iter())
                    .filter(|binding| expr_refs.contains(*binding))
                    .cloned(),
            );
        }
    }
    setup_wanted_refs.extend(setup_value_dependency_refs(ctx, root));
    setup_wanted_refs.extend(setup_script_binding_refs(ctx));
    let mut selected = HashSet::new();

    loop {
        let mut changed = false;
        module_wanted_refs.extend(
            setup_wanted_refs
                .iter()
                .filter(|binding| !setup_scope_bindings.contains(*binding))
                .cloned(),
        );
        for (index, declaration) in candidates.iter().enumerate() {
            if selected.contains(&index) {
                continue;
            }
            let wanted_refs = if declaration.module_scope {
                &module_wanted_refs
            } else {
                &setup_wanted_refs
            };
            let binds_wanted_ref = declaration
                .bindings
                .iter()
                .chain(declaration.emitted_bindings.iter())
                .any(|binding| {
                    is_valid_identifier_name(binding.as_ref()) && wanted_refs.contains(binding)
                });
            if !binds_wanted_ref {
                continue;
            }

            selected.insert(index);
            if declaration.module_scope {
                module_wanted_refs.extend(declaration.refs.iter().cloned());
            } else {
                setup_wanted_refs.extend(declaration.refs.iter().cloned());
            }
            changed = true;
        }

        if !changed {
            break;
        }
    }

    candidates
        .into_iter()
        .enumerate()
        .filter_map(|(index, declaration)| selected.contains(&index).then_some(declaration))
        .collect()
}

fn setup_scope_bindings(
    ctx: &VueRecoveryContext,
    candidates: &[&VueSetupLocalBinding],
) -> HashSet<Atom> {
    let mut bindings = candidates
        .iter()
        .filter(|declaration| !declaration.module_scope)
        .flat_map(|declaration| {
            declaration
                .bindings
                .iter()
                .chain(declaration.emitted_bindings.iter())
                .cloned()
        })
        .collect::<HashSet<_>>();
    if let Some(binding) = &ctx.setup_props_context {
        bindings.insert(binding.clone());
    }
    bindings.extend(ctx.setup_props_aliases.iter().cloned());
    if let Some(binding) = &ctx.setup_context {
        bindings.insert(binding.clone());
    }
    if let Some(binding) = &ctx.setup_emit_context {
        bindings.insert(binding.clone());
    }
    bindings.extend(ctx.setup_emit_aliases.iter().cloned());
    bindings.extend(ctx.slot_bindings.iter().cloned());
    bindings
}

fn selects_safe_template_expr_local(
    ctx: &VueRecoveryContext,
    declaration: &VueSetupLocalBinding,
    expr_refs: &HashSet<Atom>,
    expr_read_refs: &HashSet<Atom>,
) -> bool {
    if !declaration.template_selectable {
        return false;
    }
    if !any_binding_ref(declaration, expr_refs) {
        return false;
    }
    if declaration.module_scope {
        return true;
    }
    match &declaration.stmt {
        Stmt::Decl(Decl::Fn(_)) | Stmt::Decl(Decl::Class(_)) => true,
        Stmt::Decl(Decl::Var(var)) => var.decls.iter().any(|decl| {
            let mut decl_bindings = HashSet::new();
            collect_local_pat_bindings(&decl.name, &mut decl_bindings);
            if !decl_bindings
                .iter()
                .any(|binding| expr_refs.contains(binding))
                && !declaration
                    .emitted_bindings
                    .iter()
                    .any(|binding| expr_refs.contains(binding))
            {
                return false;
            }
            if matches!(decl.name, Pat::Ident(_))
                && decl
                    .init
                    .as_deref()
                    .is_some_and(|init| is_opaque_vue_helper_candidate_call(init, ctx))
            {
                return false;
            }
            matches!(decl.name, Pat::Ident(_) | Pat::Array(_))
                || (matches!(decl.name, Pat::Object(_))
                    && (decl_bindings
                        .iter()
                        .any(|binding| expr_read_refs.contains(binding))
                        || declaration
                            .emitted_bindings
                            .iter()
                            .any(|binding| expr_read_refs.contains(binding)))
                    && decl.init.as_deref().is_some_and(|init| {
                        is_ref_object_expr(init, ctx)
                            || is_ref_object_alias(init, ctx)
                            || declaration_refs_setup_props(ctx, declaration)
                    }))
        }),
        _ => false,
    }
}

fn any_binding_ref(declaration: &VueSetupLocalBinding, refs: &HashSet<Atom>) -> bool {
    declaration
        .bindings
        .iter()
        .chain(declaration.emitted_bindings.iter())
        .any(|binding| refs.contains(binding))
}

fn declaration_refs_setup_props(
    ctx: &VueRecoveryContext,
    declaration: &VueSetupLocalBinding,
) -> bool {
    ctx.setup_props_context
        .as_ref()
        .is_some_and(|binding| declaration.refs.contains(binding))
        || ctx
            .setup_props_aliases
            .iter()
            .any(|binding| declaration.refs.contains(binding))
}

fn is_opaque_vue_helper_candidate_call(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    match expr {
        Expr::Paren(paren) => is_opaque_vue_helper_candidate_call(paren.expr.as_ref(), ctx),
        Expr::Call(call) => {
            let Callee::Expr(callee) = &call.callee else {
                return false;
            };
            let Expr::Ident(ident) = callee.as_ref() else {
                return false;
            };
            ctx.vue_helper_candidates.contains(&ident.sym)
                && !ctx.vue_helpers.contains_key(&ident.sym)
        }
        _ => false,
    }
}

fn collect_local_pat_bindings(pat: &Pat, bindings: &mut HashSet<Atom>) {
    match pat {
        Pat::Ident(binding) => {
            bindings.insert(binding.id.sym.clone());
        }
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_local_pat_bindings(elem, bindings);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(key_value) => {
                        collect_local_pat_bindings(key_value.value.as_ref(), bindings);
                    }
                    ObjectPatProp::Assign(assign) => {
                        bindings.insert(assign.key.sym.clone());
                    }
                    ObjectPatProp::Rest(rest) => {
                        collect_local_pat_bindings(rest.arg.as_ref(), bindings);
                    }
                }
            }
        }
        Pat::Rest(rest) => collect_local_pat_bindings(rest.arg.as_ref(), bindings),
        Pat::Assign(assign) => collect_local_pat_bindings(assign.left.as_ref(), bindings),
        Pat::Expr(_) | Pat::Invalid(_) => {}
    }
}

pub(super) fn template_scope_from_pat(pat: &Pat) -> VueTemplateScope {
    let mut bindings = HashSet::new();
    collect_local_pat_bindings(pat, &mut bindings);
    VueTemplateScope::from_locals(bindings.into_iter().map(|binding| binding.to_string()))
}

fn setup_value_dependency_refs(ctx: &VueRecoveryContext, root: &VueNode) -> HashSet<Atom> {
    if ctx.setup_value_bindings.is_empty() {
        return HashSet::new();
    }

    let template_refs = template_for_source_refs(root);
    let mut refs = HashSet::new();
    for value in ctx.setup_value_bindings.values() {
        let mut value_refs = HashSet::new();
        collect_js_unshadowed_ident_refs(&value.value, &mut value_refs);
        if value_refs.iter().any(|local| template_refs.contains(local)) {
            refs.extend(value_refs);
        }
    }
    refs
}

fn setup_script_binding_refs(ctx: &VueRecoveryContext) -> HashSet<Atom> {
    let mut refs = HashSet::new();
    for (_, expr) in &ctx.setup_script_bindings {
        collect_js_unshadowed_ident_refs(expr, &mut refs);
    }
    refs
}

fn template_for_source_refs(root: &VueNode) -> HashSet<Atom> {
    let mut refs = HashSet::new();
    let mut scopes = TemplateLocalScopes::default();
    collect_template_for_source_refs(root, &mut refs, &mut scopes);
    refs
}

fn render_value_member_refs(render: RenderSource<'_>, ctx: &VueRecoveryContext) -> HashSet<Atom> {
    let candidates = ctx
        .setup_ref_script_bindings
        .iter()
        .map(|binding| binding.binding.clone())
        .collect::<HashSet<_>>();
    if candidates.is_empty() {
        return HashSet::new();
    }

    let mut collector = ValueMemberRefCollector {
        candidates: &candidates,
        refs: HashSet::new(),
    };
    match render {
        RenderSource::Function(function) => function.function.visit_with(&mut collector),
        RenderSource::SetupArrow {
            render,
            setup_stmts,
            ..
        } => {
            for stmt in setup_stmts {
                stmt.visit_with(&mut collector);
            }
            render.visit_with(&mut collector);
        }
    }
    collector.refs
}

struct ValueMemberRefCollector<'a> {
    candidates: &'a HashSet<Atom>,
    refs: HashSet<Atom>,
}

impl Visit for ValueMemberRefCollector<'_> {
    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "value") {
            if let Expr::Ident(obj) = member.obj.as_ref() {
                if self.candidates.contains(&obj.sym) {
                    self.refs.insert(obj.sym.clone());
                }
            }
        }
        member.visit_children_with(self);
    }
}

fn template_static_ref_names(root: &VueNode) -> Vec<String> {
    let mut refs = HashSet::new();
    collect_template_static_ref_names(root, &mut refs);
    let mut refs = refs
        .into_iter()
        .filter(|name| is_valid_identifier_name(name))
        .collect::<Vec<_>>();
    refs.sort();
    refs
}

fn collect_template_static_ref_names(node: &VueNode, refs: &mut HashSet<String>) {
    match node {
        VueNode::Element(element) => {
            for attr in &element.attrs {
                if let VueAttr::Static {
                    name,
                    value: Some(value),
                } = attr
                {
                    if name == "ref" {
                        refs.insert(value.clone());
                    }
                }
            }
            for child in &element.children {
                collect_template_static_ref_names(child, refs);
            }
        }
        VueNode::Fragment(children) => {
            for child in children {
                collect_template_static_ref_names(child, refs);
            }
        }
        VueNode::If(branches) => {
            for branch in branches {
                collect_template_static_ref_names(&branch.node, refs);
            }
        }
        VueNode::For(for_node) => collect_template_static_ref_names(&for_node.node, refs),
        VueNode::Text(_)
        | VueNode::Interpolation(_)
        | VueNode::Comment(_)
        | VueNode::RawHtml(_)
        | VueNode::RawExpr(_)
        | VueNode::Unsupported(_) => {}
    }
}

fn template_expr_refs(root: &VueNode) -> HashSet<Atom> {
    let mut refs = HashSet::new();
    let mut scopes = TemplateLocalScopes::default();
    collect_template_expr_refs(root, &mut refs, &mut scopes);
    refs
}

fn template_expr_read_refs(root: &VueNode) -> HashSet<Atom> {
    let mut refs = HashSet::new();
    let mut scopes = TemplateLocalScopes::default();
    collect_template_expr_read_refs(root, &mut refs, &mut scopes);
    refs
}

fn template_event_expr_refs(root: &VueNode) -> HashSet<Atom> {
    let mut refs = HashSet::new();
    let mut scopes = TemplateLocalScopes::default();
    collect_template_event_expr_refs(root, &mut refs, &mut scopes);
    refs
}

#[derive(Default)]
struct TemplateLocalScopes {
    stack: Vec<HashSet<Atom>>,
}

impl TemplateLocalScopes {
    fn push(&mut self, scope: &VueTemplateScope) -> bool {
        if scope.locals.is_empty() {
            return false;
        }
        self.stack.push(
            scope
                .locals
                .iter()
                .map(|local| Atom::from(local.clone()))
                .collect(),
        );
        true
    }

    fn pop(&mut self) {
        self.stack.pop();
    }

    fn is_local(&self, name: &Atom) -> bool {
        self.stack.iter().rev().any(|scope| scope.contains(name))
    }

    fn collect_ident_refs(&self, source: &str, refs: &mut HashSet<Atom>) {
        let mut scoped_refs = HashSet::new();
        collect_js_unshadowed_ident_refs(source, &mut scoped_refs);
        refs.extend(scoped_refs.into_iter().filter(|name| !self.is_local(name)));
    }

    fn collect_read_refs(&self, source: &str, refs: &mut HashSet<Atom>) {
        let mut scoped_refs = HashSet::new();
        collect_js_unshadowed_read_refs(source, &mut scoped_refs);
        refs.extend(scoped_refs.into_iter().filter(|name| !self.is_local(name)));
    }
}

fn collect_template_expr_refs(
    node: &VueNode,
    refs: &mut HashSet<Atom>,
    scopes: &mut TemplateLocalScopes,
) {
    match node {
        VueNode::Element(element) => {
            for attr in &element.attrs {
                collect_attr_expr_refs(attr, refs, scopes);
            }
            let scoped_attr = element.attrs.iter().find_map(attr_template_scope);
            let pushed = scoped_attr.is_some_and(|scope| scopes.push(scope));
            for child in &element.children {
                collect_template_expr_refs(child, refs, scopes);
            }
            if pushed {
                scopes.pop();
            }
        }
        VueNode::Fragment(children) => {
            for child in children {
                collect_template_expr_refs(child, refs, scopes);
            }
        }
        VueNode::If(branches) => {
            for branch in branches {
                if let Some(condition) = &branch.condition {
                    scopes.collect_ident_refs(condition.as_str(), refs);
                }
                collect_template_expr_refs(&branch.node, refs, scopes);
            }
        }
        VueNode::For(for_node) => {
            scopes.collect_ident_refs(for_node.source.as_str(), refs);
            let pushed = scopes.push(&for_node.scope);
            collect_template_expr_refs(&for_node.node, refs, scopes);
            if pushed {
                scopes.pop();
            }
        }
        VueNode::Interpolation(expr) | VueNode::RawExpr(expr) => {
            scopes.collect_ident_refs(expr.as_str(), refs);
        }
        VueNode::Unsupported(unsupported) => {
            let pushed = scopes.push(&unsupported.scope);
            scopes.collect_ident_refs(unsupported.expr.as_str(), refs);
            if pushed {
                scopes.pop();
            }
        }
        VueNode::Text(_) | VueNode::Comment(_) | VueNode::RawHtml(_) => {}
    }
}

fn collect_template_expr_read_refs(
    node: &VueNode,
    refs: &mut HashSet<Atom>,
    scopes: &mut TemplateLocalScopes,
) {
    match node {
        VueNode::Element(element) => {
            for attr in &element.attrs {
                collect_attr_expr_read_refs(attr, refs, scopes);
            }
            let scoped_attr = element.attrs.iter().find_map(attr_template_scope);
            let pushed = scoped_attr.is_some_and(|scope| scopes.push(scope));
            for child in &element.children {
                collect_template_expr_read_refs(child, refs, scopes);
            }
            if pushed {
                scopes.pop();
            }
        }
        VueNode::Fragment(children) => {
            for child in children {
                collect_template_expr_read_refs(child, refs, scopes);
            }
        }
        VueNode::If(branches) => {
            for branch in branches {
                if let Some(condition) = &branch.condition {
                    scopes.collect_read_refs(condition.as_str(), refs);
                }
                collect_template_expr_read_refs(&branch.node, refs, scopes);
            }
        }
        VueNode::For(for_node) => {
            scopes.collect_read_refs(for_node.source.as_str(), refs);
            let pushed = scopes.push(&for_node.scope);
            collect_template_expr_read_refs(&for_node.node, refs, scopes);
            if pushed {
                scopes.pop();
            }
        }
        VueNode::Interpolation(expr) | VueNode::RawExpr(expr) => {
            scopes.collect_read_refs(expr.as_str(), refs);
        }
        VueNode::Unsupported(unsupported) => {
            let pushed = scopes.push(&unsupported.scope);
            scopes.collect_read_refs(unsupported.expr.as_str(), refs);
            if pushed {
                scopes.pop();
            }
        }
        VueNode::Text(_) | VueNode::Comment(_) | VueNode::RawHtml(_) => {}
    }
}

fn collect_template_event_expr_refs(
    node: &VueNode,
    refs: &mut HashSet<Atom>,
    scopes: &mut TemplateLocalScopes,
) {
    match node {
        VueNode::Element(element) => {
            for attr in &element.attrs {
                match attr {
                    VueAttr::On { expr, .. } => scopes.collect_ident_refs(expr.as_str(), refs),
                    VueAttr::Directive(directive) if directive.name == "on" => {
                        if let Some(expr) = &directive.expr {
                            scopes.collect_ident_refs(expr.as_str(), refs);
                        }
                        if let Some(VueDirectiveArg::Dynamic(expr)) = &directive.arg {
                            scopes.collect_ident_refs(expr.as_str(), refs);
                        }
                    }
                    _ => {}
                }
            }
            let scoped_attr = element.attrs.iter().find_map(attr_template_scope);
            let pushed = scoped_attr.is_some_and(|scope| scopes.push(scope));
            for child in &element.children {
                collect_template_event_expr_refs(child, refs, scopes);
            }
            if pushed {
                scopes.pop();
            }
        }
        VueNode::Fragment(children) => {
            for child in children {
                collect_template_event_expr_refs(child, refs, scopes);
            }
        }
        VueNode::If(branches) => {
            for branch in branches {
                collect_template_event_expr_refs(&branch.node, refs, scopes);
            }
        }
        VueNode::For(for_node) => {
            let pushed = scopes.push(&for_node.scope);
            collect_template_event_expr_refs(&for_node.node, refs, scopes);
            if pushed {
                scopes.pop();
            }
        }
        VueNode::Text(_)
        | VueNode::Interpolation(_)
        | VueNode::Comment(_)
        | VueNode::RawHtml(_)
        | VueNode::RawExpr(_)
        | VueNode::Unsupported(_) => {}
    }
}

fn collect_template_for_source_refs(
    node: &VueNode,
    refs: &mut HashSet<Atom>,
    scopes: &mut TemplateLocalScopes,
) {
    match node {
        VueNode::Element(element) => {
            let scoped_attr = element.attrs.iter().find_map(attr_template_scope);
            let pushed = scoped_attr.is_some_and(|scope| scopes.push(scope));
            for child in &element.children {
                collect_template_for_source_refs(child, refs, scopes);
            }
            if pushed {
                scopes.pop();
            }
        }
        VueNode::Fragment(children) => {
            for child in children {
                collect_template_for_source_refs(child, refs, scopes);
            }
        }
        VueNode::If(branches) => {
            for branch in branches {
                collect_template_for_source_refs(&branch.node, refs, scopes);
            }
        }
        VueNode::For(for_node) => {
            scopes.collect_ident_refs(for_node.source.as_str(), refs);
            let pushed = scopes.push(&for_node.scope);
            collect_template_for_source_refs(&for_node.node, refs, scopes);
            if pushed {
                scopes.pop();
            }
        }
        VueNode::Text(_)
        | VueNode::Interpolation(_)
        | VueNode::Comment(_)
        | VueNode::RawHtml(_)
        | VueNode::RawExpr(_)
        | VueNode::Unsupported(_) => {}
    }
}

fn attr_template_scope(attr: &VueAttr) -> Option<&VueTemplateScope> {
    match attr {
        VueAttr::Directive(directive) if directive.name == "slot" => Some(&directive.scope),
        _ => None,
    }
}

fn collect_attr_expr_refs(attr: &VueAttr, refs: &mut HashSet<Atom>, scopes: &TemplateLocalScopes) {
    match attr {
        VueAttr::Bind { expr, .. } | VueAttr::On { expr, .. } | VueAttr::Spread(expr) => {
            scopes.collect_ident_refs(expr.as_str(), refs);
        }
        VueAttr::Directive(directive) if directive.name == "slot" => {
            if let Some(VueDirectiveArg::Dynamic(expr)) = &directive.arg {
                scopes.collect_ident_refs(expr.as_str(), refs);
            }
        }
        VueAttr::Directive(directive) => {
            if let Some(expr) = &directive.expr {
                scopes.collect_ident_refs(expr.as_str(), refs);
            }
            if let Some(VueDirectiveArg::Dynamic(expr)) = &directive.arg {
                scopes.collect_ident_refs(expr.as_str(), refs);
            }
        }
        VueAttr::Static { .. } => {}
    }
}

fn collect_attr_expr_read_refs(
    attr: &VueAttr,
    refs: &mut HashSet<Atom>,
    scopes: &TemplateLocalScopes,
) {
    match attr {
        VueAttr::Bind { expr, .. } | VueAttr::On { expr, .. } | VueAttr::Spread(expr) => {
            scopes.collect_read_refs(expr.as_str(), refs);
        }
        VueAttr::Directive(directive) if directive.name == "slot" => {
            if let Some(VueDirectiveArg::Dynamic(expr)) = &directive.arg {
                scopes.collect_read_refs(expr.as_str(), refs);
            }
        }
        VueAttr::Directive(directive) => {
            if let Some(expr) = &directive.expr {
                scopes.collect_read_refs(expr.as_str(), refs);
            }
            if let Some(VueDirectiveArg::Dynamic(expr)) = &directive.arg {
                scopes.collect_read_refs(expr.as_str(), refs);
            }
        }
        VueAttr::Static { .. } => {}
    }
}

fn collect_js_unshadowed_ident_refs(source: &str, refs: &mut HashSet<Atom>) {
    let mut scoped_refs = HashSet::new();
    collect_js_ident_refs(source, &mut scoped_refs);
    extend_unshadowed_expr_refs(source, scoped_refs, refs);
}

fn collect_js_unshadowed_read_refs(source: &str, refs: &mut HashSet<Atom>) {
    let mut scoped_refs = HashSet::new();
    collect_js_read_refs(source, &mut scoped_refs);
    extend_unshadowed_expr_refs(source, scoped_refs, refs);
}

fn extend_unshadowed_expr_refs(source: &str, scoped_refs: HashSet<Atom>, refs: &mut HashSet<Atom>) {
    let mut shadowed_names = HashSet::new();
    collect_js_arrow_param_names(source, &mut shadowed_names);
    refs.extend(
        scoped_refs
            .into_iter()
            .filter(|name| !shadowed_names.contains(name)),
    );
}

fn collect_js_ident_refs(source: &str, refs: &mut HashSet<Atom>) {
    let chars = source.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        if is_ident_start(chars[index]) {
            let start = index;
            index += 1;
            while index < chars.len() && is_ident_continue(chars[index]) {
                index += 1;
            }
            let ident = chars[start..index].iter().collect::<String>();
            refs.insert(Atom::from(ident));
            continue;
        }
        index += 1;
    }
}

fn collect_js_read_refs(source: &str, refs: &mut HashSet<Atom>) {
    let chars = source.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '"' | '\'' | '`' => {
                index = if chars[index] == '`' {
                    collect_template_literal_read_refs(&chars, index, refs)
                } else {
                    skip_quoted_js_string(&chars, index)
                };
                continue;
            }
            ch if is_ident_start(ch) => {
                let start = index;
                index += 1;
                while index < chars.len() && is_ident_continue(chars[index]) {
                    index += 1;
                }
                if js_ident_token_is_read(&chars, start, index) {
                    let ident = chars[start..index].iter().collect::<String>();
                    refs.insert(Atom::from(ident));
                }
                continue;
            }
            _ => {}
        }
        index += 1;
    }
}

fn collect_template_literal_read_refs(
    chars: &[char],
    start: usize,
    refs: &mut HashSet<Atom>,
) -> usize {
    let mut index = start + 1;
    while index < chars.len() {
        if chars[index] == '\\' {
            index += 2;
            continue;
        }
        if chars[index] == '`' {
            return index + 1;
        }
        if chars[index] == '$' && chars.get(index + 1) == Some(&'{') {
            let expr_start = index + 2;
            if let Some(expr_end) = template_literal_expr_end(chars, expr_start) {
                let expr = chars[expr_start..expr_end].iter().collect::<String>();
                collect_js_read_refs(&expr, refs);
                index = expr_end + 1;
                continue;
            }
        }
        index += 1;
    }
    index
}

fn template_literal_expr_end(chars: &[char], start: usize) -> Option<usize> {
    let mut index = start;
    let mut depth = 1usize;
    while index < chars.len() {
        match chars[index] {
            '"' | '\'' | '`' => {
                index = skip_quoted_js_string(chars, index);
                continue;
            }
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn skip_quoted_js_string(chars: &[char], start: usize) -> usize {
    let quote = chars[start];
    let mut index = start + 1;
    while index < chars.len() {
        if chars[index] == '\\' {
            index += 2;
            continue;
        }
        if chars[index] == quote {
            return index + 1;
        }
        index += 1;
    }
    index
}

fn js_ident_token_is_read(chars: &[char], start: usize, end: usize) -> bool {
    let ident = chars[start..end].iter().collect::<String>();
    if matches!(
        ident.as_str(),
        "true"
            | "false"
            | "null"
            | "undefined"
            | "if"
            | "else"
            | "return"
            | "const"
            | "let"
            | "var"
            | "new"
    ) {
        return false;
    }

    let prev = chars[..start]
        .iter()
        .rposition(|ch| !ch.is_whitespace())
        .map(|index| chars[index]);
    if matches!(prev, Some('.')) {
        return false;
    }

    let next = chars[end..]
        .iter()
        .position(|ch| !ch.is_whitespace())
        .map(|offset| chars[end + offset]);
    !matches!(next, Some(':'))
}

fn collect_js_arrow_param_names(source: &str, names: &mut HashSet<Atom>) {
    let mut cursor = 0;
    while let Some(offset) = source[cursor..].find("=>") {
        let arrow = cursor + offset;
        for name in arrow_param_names(&source[..arrow]) {
            names.insert(Atom::from(name));
        }
        cursor = arrow + 2;
    }
    collect_js_declared_names(source, names);
}

fn arrow_param_names(left: &str) -> Vec<String> {
    let left = left.trim_end();
    if let Some(params) = left.strip_suffix(')') {
        let Some(open) = params.rfind('(') else {
            return Vec::new();
        };
        return params[open + 1..]
            .split(',')
            .map(str::trim)
            .filter(|param| is_valid_identifier_name(param))
            .map(ToString::to_string)
            .collect();
    }

    let end = left.len();
    let start = left
        .char_indices()
        .rev()
        .find_map(|(index, ch)| (!is_ident_continue(ch)).then_some(index + ch.len_utf8()))
        .unwrap_or(0);
    let param = left[start..end].trim();
    is_valid_identifier_name(param)
        .then(|| param.to_string())
        .into_iter()
        .collect()
}

fn collect_js_declared_names(source: &str, names: &mut HashSet<Atom>) {
    let chars = source.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        let Some(keyword_len) = declaration_keyword_len(&chars, index) else {
            index += 1;
            continue;
        };
        index += keyword_len;

        loop {
            while index < chars.len() && chars[index].is_whitespace() {
                index += 1;
            }
            if index >= chars.len() || !is_ident_start(chars[index]) {
                break;
            }

            let start = index;
            index += 1;
            while index < chars.len() && is_ident_continue(chars[index]) {
                index += 1;
            }
            let ident = chars[start..index].iter().collect::<String>();
            names.insert(Atom::from(ident));

            let mut depth = 0usize;
            while index < chars.len() {
                match chars[index] {
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' => depth = depth.saturating_sub(1),
                    ',' | ';' if depth == 0 => break,
                    _ => {}
                }
                index += 1;
            }
            if index >= chars.len() || chars[index] != ',' {
                break;
            }
            index += 1;
        }
    }
}

fn declaration_keyword_len(chars: &[char], index: usize) -> Option<usize> {
    ["const", "let", "var"].iter().find_map(|keyword| {
        let end = index + keyword.len();
        if end > chars.len() {
            return None;
        }
        let matches_keyword = keyword
            .chars()
            .enumerate()
            .all(|(offset, ch)| chars[index + offset] == ch);
        if !matches_keyword {
            return None;
        }
        let before_ok =
            index == 0 || (!is_ident_continue(chars[index - 1]) && chars[index - 1] != '$');
        let after_ok = end == chars.len() || !is_ident_continue(chars[end]);
        (before_ok && after_ok).then_some(keyword.len())
    })
}

fn referenced_script_imports(
    ctx: &VueRecoveryContext,
    root: &VueNode,
    declared_bindings: &HashSet<Atom>,
    local_declarations: &[VueSetupLocalBinding],
) -> Vec<String> {
    let mut refs = ctx.setup_script_import_refs.clone();
    refs.extend(setup_script_binding_refs(ctx));
    for declaration in local_declarations {
        refs.extend(declaration.import_refs.iter().cloned());
    }
    refs.extend(
        template_expr_read_refs(root)
            .into_iter()
            .filter(|local| !declared_bindings.contains(local)),
    );

    let mut imports = refs
        .iter()
        .filter(|local| local.as_ref() != "$")
        .filter(|local| !declared_bindings.contains(*local))
        .filter_map(|local| ctx.script_imports.get(local).map(|import| (local, import)))
        .map(|(local, import)| script_import_line(local.as_ref(), import))
        .collect::<Vec<_>>();
    imports.sort();
    imports.dedup();
    imports
}

fn script_setup_declared_bindings(
    ctx: &VueRecoveryContext,
    prop_bindings: &[(String, String)],
    props_declaration: Option<&(String, String)>,
    emit_declaration: Option<&(String, String)>,
    ref_declarations: &[(String, String, String)],
    local_declarations: &[VueSetupLocalBinding],
) -> HashSet<Atom> {
    let mut declared = HashSet::new();
    if let Some((binding, _)) = props_declaration {
        declared.insert(Atom::from(binding.clone()));
        declared.extend(
            prop_bindings
                .iter()
                .map(|(_, binding)| Atom::from(binding.clone())),
        );
    }
    if let Some((binding, _)) = emit_declaration {
        declared.insert(Atom::from(binding.clone()));
    }
    declared.extend(
        ref_declarations
            .iter()
            .map(|(binding, _, _)| Atom::from(binding.clone())),
    );
    declared.extend(
        ctx.setup_script_bindings
            .iter()
            .map(|(binding, _)| binding.clone()),
    );
    declared.extend(
        local_declarations
            .iter()
            .flat_map(|declaration| declaration.emitted_bindings.iter().cloned()),
    );
    declared
}

fn setup_prop_bindings(
    valid_prop_names: &[String],
    ctx: &VueRecoveryContext,
) -> Vec<(String, String)> {
    valid_prop_names
        .iter()
        .map(|prop| {
            let binding = ctx
                .setup_prop_bindings
                .get(&Atom::from(prop.clone()))
                .map(ToString::to_string)
                .unwrap_or_else(|| prop.clone());
            (prop.clone(), binding)
        })
        .collect()
}

fn format_prop_destructure_bindings(prop_bindings: &[(String, String)]) -> String {
    prop_bindings
        .iter()
        .map(|(prop, binding)| {
            if prop == binding {
                prop.clone()
            } else {
                format!("{prop}: {binding}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn script_import_line(local: &str, import: &VueScriptImport) -> String {
    match import {
        VueScriptImport::Named { source, imported } if imported == local => {
            format!("import {{ {imported} }} from {};", quote_js_string(source))
        }
        VueScriptImport::Named { source, imported } => {
            format!(
                "import {{ {imported} as {local} }} from {};",
                quote_js_string(source)
            )
        }
        VueScriptImport::Default { source } => {
            format!("import {local} from {};", quote_js_string(source))
        }
        VueScriptImport::Namespace { source } => {
            format!("import * as {local} from {};", quote_js_string(source))
        }
    }
}

fn setup_props_script_binding(
    ctx: &VueRecoveryContext,
    reserved_bindings: &HashSet<Atom>,
) -> Option<String> {
    if ctx.setup_props_context.is_some() || !ctx.setup_props_aliases.is_empty() {
        let props = Atom::from("props");
        if !reserved_bindings.contains(&props) {
            return Some("props".to_string());
        }
    }

    let mut aliases = ctx
        .setup_props_aliases
        .iter()
        .filter(|alias| is_valid_identifier_name(alias.as_ref()))
        .filter(|alias| !reserved_bindings.contains(*alias))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    aliases.sort();
    aliases.into_iter().next().or_else(|| {
        ctx.setup_props_context
            .as_ref()
            .filter(|binding| is_valid_identifier_name(binding.as_ref()))
            .filter(|binding| !reserved_bindings.contains(*binding))
            .map(ToString::to_string)
    })
}

fn props_binding_reserved_names(
    ctx: &VueRecoveryContext,
    valid_prop_names: &[String],
    emit_declaration: Option<&(String, String)>,
    ref_declarations: &[(String, String, String)],
) -> HashSet<Atom> {
    let mut reserved = valid_prop_names
        .iter()
        .cloned()
        .map(Atom::from)
        .collect::<HashSet<_>>();
    if let Some((binding, _)) = emit_declaration {
        reserved.insert(Atom::from(binding.clone()));
    }
    reserved.extend(
        ref_declarations
            .iter()
            .map(|(binding, _, _)| Atom::from(binding.clone())),
    );
    reserved.extend(
        ctx.setup_script_bindings
            .iter()
            .map(|(binding, _)| binding.clone()),
    );
    reserved.extend(
        ctx.setup_local_bindings
            .iter()
            .flat_map(|declaration| declaration.emitted_bindings.iter().cloned()),
    );
    reserved.extend(ctx.setup_alias_bindings.keys().cloned());
    reserved.extend(ctx.script_imports.keys().cloned());
    reserved
}

fn setup_emit_script_binding(
    ctx: &VueRecoveryContext,
    root: &VueNode,
    local_declarations: &[&VueSetupLocalBinding],
) -> Option<String> {
    let mut expr_refs = template_expr_refs(root);
    for declaration in local_declarations {
        expr_refs.extend(declaration.refs.iter().cloned());
    }
    let mut aliases = ctx
        .setup_emit_aliases
        .iter()
        .filter(|alias| is_valid_identifier_name(alias.as_ref()))
        .filter(|alias| expr_refs.contains(*alias))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    aliases.sort();
    aliases.into_iter().next().or_else(|| {
        ctx.setup_emit_context
            .as_ref()
            .filter(|binding| is_valid_identifier_name(binding.as_ref()))
            .filter(|binding| expr_refs.contains(*binding))
            .map(ToString::to_string)
    })
}

fn component_prop_names(options: &ObjectLit) -> Vec<String> {
    let Some(props_expr) = component_props_expr(options) else {
        return Vec::new();
    };

    let mut names = match props_expr {
        Expr::Object(object) => object
            .props
            .iter()
            .filter_map(|prop| {
                let PropOrSpread::Prop(prop) = prop else {
                    return None;
                };
                match prop.as_ref() {
                    Prop::KeyValue(key_value) => prop_name(&key_value.key),
                    Prop::Assign(assign) => Some(assign.key.sym.to_string()),
                    _ => None,
                }
            })
            .collect::<Vec<_>>(),
        Expr::Array(array) => array
            .elems
            .iter()
            .flatten()
            .filter_map(|elem| syntax::string_lit(elem.expr.as_ref()))
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    names.sort();
    names.dedup();
    names
}

fn component_props_expr(options: &ObjectLit) -> Option<&Expr> {
    options.props.iter().find_map(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        match prop.as_ref() {
            Prop::KeyValue(key_value) if prop_name(&key_value.key).as_deref() == Some("props") => {
                Some(key_value.value.as_ref())
            }
            _ => None,
        }
    })
}

fn component_emits_expr(options: &ObjectLit) -> Option<&Expr> {
    options.props.iter().find_map(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        match prop.as_ref() {
            Prop::KeyValue(key_value) if prop_name(&key_value.key).as_deref() == Some("emits") => {
                Some(key_value.value.as_ref())
            }
            _ => None,
        }
    })
}

fn quote_js_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vue_template::VueExpr;

    fn test_stmt(source: &str) -> Stmt {
        let cm = Lrc::new(SourceMap::default());
        let module = parse_module(source, cm).unwrap();
        match module.body.into_iter().next().unwrap() {
            ModuleItem::Stmt(stmt) => stmt,
            _ => panic!("expected statement"),
        }
    }

    fn test_atoms(names: &[&str]) -> Vec<Atom> {
        names.iter().map(|name| Atom::from(*name)).collect()
    }

    fn test_atom_set(names: &[&str]) -> HashSet<Atom> {
        names.iter().map(|name| Atom::from(*name)).collect()
    }

    fn test_local_binding(
        source: &str,
        bindings: &[&str],
        emitted_bindings: &[&str],
        refs: &[&str],
    ) -> VueSetupLocalBinding {
        test_local_binding_with_scope(source, bindings, emitted_bindings, refs, false)
    }

    fn test_local_binding_with_scope(
        source: &str,
        bindings: &[&str],
        emitted_bindings: &[&str],
        refs: &[&str],
        module_scope: bool,
    ) -> VueSetupLocalBinding {
        VueSetupLocalBinding {
            bindings: test_atoms(bindings),
            emitted_bindings: test_atoms(emitted_bindings),
            refs: test_atom_set(refs),
            source: source.to_string(),
            import_refs: HashSet::new(),
            stmt: test_stmt(source),
            module_scope,
            template_selectable: true,
        }
    }

    #[test]
    fn ignores_plain_render_function_without_vue_signal() {
        let input = r#"
export function render() {
  return "not a Vue render";
}
"#;

        assert!(recover_vue_sfc_source_from_js(input).unwrap().is_none());
    }

    #[test]
    fn ignores_vue_import_without_render_helper_call() {
        let input = r#"
import { ref } from "vue";
const __sfc__ = { props: { msg: String } };
export function render() {
  return "not a Vue render";
}
"#;

        assert!(recover_vue_sfc_source_from_js(input).unwrap().is_none());
    }

    #[test]
    fn recovers_aliased_vue_helper_signal() {
        let input = r#"
import { openBlock as o, createElementBlock as h } from "vue";
export function render(_ctx, _cache) {
  return o(), h("main", null, "Aliased");
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <main>Aliased</main>\n</template>\n"
        );
    }

    #[test]
    fn decompiles_then_recovers_vue_sfc() {
        let input = r#"
import { toDisplayString as _toDisplayString, openBlock as _openBlock, createElementBlock as _createElementBlock } from "vue";
const __sfc__ = { props: { msg: String } };
export function render(_ctx, _cache) {
  return (_openBlock(), _createElementBlock("div", null, _toDisplayString(_ctx.msg), 1));
}
__sfc__.render = render;
export default __sfc__;
"#;

        assert_eq!(
            decompile_vue_sfc(input, DecompileOptions::default())
                .unwrap()
                .code,
            "<script>\nexport default {\n    props: {\n        msg: String\n    }\n}\n</script>\n\n<template>\n  <div>{{ msg }}</div>\n</template>\n"
        );
    }

    #[test]
    fn decompiles_single_system_register_vue_sfc() {
        let input = r#"
System.register(["./vendor-vue.js"], function (exports) {
  "use strict";
  var defineComponent, openBlock, createElementBlock;
  return {
    setters: [
      function (module) {
        defineComponent = module.d, openBlock = module.q, createElementBlock = module.X;
      }
    ],
    execute: function () {
      exports("_", defineComponent({
        __name: "LegacyGreeting",
        setup: function () {
          return function () {
            return openBlock(), createElementBlock("p", null, "Legacy");
          };
        }
      }));
    }
  };
});
"#;

        assert_eq!(
            decompile_vue_sfc(input, DecompileOptions::default())
                .unwrap()
                .code,
            "<template>\n  <p>Legacy</p>\n</template>\n"
        );
    }

    #[test]
    fn decompiles_component_matching_vue_filename() {
        let input = r#"
import { d as dc, q as ob, X as ce } from "./vendor-vue.js";
const InnerPanel = dc({
  __name: "InnerPanel",
  setup() {
    return () => (ob(), ce("p", null, "Inner"));
  }
});
export const Z = dc({
  __name: "TargetPanel",
  setup() {
    return () => (ob(), ce("p", null, "Target"));
  }
});
"#;

        assert_eq!(
            decompile_vue_sfc(
                input,
                DecompileOptions {
                    filename: "TargetPanel.vue_vue_type_script_setup_true_lang.js".to_string(),
                    ..Default::default()
                }
            )
            .unwrap()
            .code,
            "<template>\n  <p>Target</p>\n</template>\n"
        );
    }

    #[test]
    fn recovers_static_element_with_hoisted_props() {
        let input = r#"
import { openBlock, createElementBlock } from "vue";
const __sfc__ = {};
const _hoisted_1 = { class: "card" };
export function render(_ctx, _cache) {
  openBlock();
  return createElementBlock("section", _hoisted_1, "Hello Vue");
}
__sfc__.render = render;
export default __sfc__;
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <section class=\"card\">Hello Vue</section>\n</template>\n"
        );
    }

    #[test]
    fn recovers_interpolation_and_component_options() {
        let input = r#"
import { toDisplayString, openBlock, createElementBlock } from "vue";
const __sfc__ = { props: { msg: String } };
export function render(_ctx, _cache) {
  openBlock();
  return createElementBlock("div", null, toDisplayString(_ctx.msg), 1);
}
__sfc__.render = render;
export default __sfc__;
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script>\nexport default {\n    props: {\n        msg: String\n    }\n}\n</script>\n\n<template>\n  <div>{{ msg }}</div>\n</template>\n"
        );
    }

    #[test]
    fn recovers_minified_render_context_interpolation() {
        let input = r#"
import { toDisplayString, openBlock, createElementBlock } from "vue";
const e = { props: { msg: String } };
export function render(e, o) {
  openBlock();
  return createElementBlock("div", null, toDisplayString(e.msg), 1);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <div>{{ msg }}</div>\n</template>\n"
        );
    }

    #[test]
    fn preserves_value_member_after_minified_render_context() {
        let input = r#"
import { openBlock, createElementBlock } from "vue";
export function render(e, _cache) {
  return openBlock(), createElementBlock("div", {
    title: e.title,
    count: items.value.filter((e) => e.ok).length
  }, null, 8, ["title", "count"]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <div :title=\"title\" :count=\"items.value.filter((e)=>e.ok).length\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_setup_returned_render_arrow() {
        let input = r#"
import { defineComponent, toDisplayString, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "Greeting",
  setup(__props) {
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("h1", null, toDisplayString(_ctx.title), 1)
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <h1>{{ title }}</h1>\n</template>\n"
        );
    }

    #[test]
    fn recovers_setup_render_block_component_context() {
        let input = r#"
import { defineComponent, resolveComponent, openBlock, createBlock } from "vue";
const _sfc_main = defineComponent({
  __name: "WrappedPanel",
  setup(__props) {
    return (_ctx, _cache) => {
      const _component_Panel = resolveComponent("Panel");
      return openBlock(), createBlock(_component_Panel, { title: _ctx.title }, null, 8, ["title"]);
    };
  }
});
export default _sfc_main;
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <Panel :title=\"title\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_setup_props_context() {
        let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "PropsInput",
  setup(props) {
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("input", {
        id: props.id,
        disabled: props.disabled,
        onInput: _cache[0] || (_cache[0] = (event) => props.onChange(event.target.value))
      }, null, 40, ["id", "disabled", "onInput"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <input :id=\"id\" :disabled=\"disabled\" @input=\"onChange($event.target.value)\" />\n</template>\n"
        );
    }

    #[test]
    fn emits_define_props_for_props_only_template_refs() {
        let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  props: {
    id: String,
    disabled: Boolean,
    onChange: Function,
  },
  setup(props) {
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("input", {
        id: props.id,
        disabled: props.disabled,
        onInput: _cache[0] || (_cache[0] = (event) => props.onChange(event.target.value))
      }, null, 40, ["id", "disabled", "onInput"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nconst props = defineProps({\n    id: String,\n    disabled: Boolean,\n    onChange: Function\n});\nconst { disabled, id, onChange } = props;\n</script>\n\n<template>\n  <input :id=\"id\" :disabled=\"disabled\" @input=\"onChange($event.target.value)\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_setup_props_alias_context() {
        let input = r#"
import { defineComponent, toDisplayString, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "PropsAlias",
  setup(props) {
    const p = props;
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("span", { title: p.title }, toDisplayString(p.label), 9, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <span :title=\"title\">{{ label }}</span>\n</template>\n"
        );
    }

    #[test]
    fn recovers_vite_vendor_vue_helper_aliases() {
        let input = r#"
import { d as dc, q as ob, X as ce, J as td } from "./vendor-vue-C85wAS_L.js";
const _sfc_main = dc({
  __name: "Greeting",
  setup(__props) {
    return (_ctx, _cache) => (
      ob(), ce("h1", null, td(_ctx.title), 1)
    );
  }
});
export default _sfc_main;
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <h1>{{ title }}</h1>\n</template>\n"
        );
    }

    #[test]
    fn recovers_vite_vendor_vue_component_slot_aliases() {
        let input = r#"
import { d as dc, a7 as rc, q as ob, C as cv, R as wc, X as ce, J as td } from "./vendor-vue-C85wAS_L.js";
const _sfc_main = dc({
  __name: "WrappedPanel",
  setup(__props) {
    return (_ctx, _cache) => {
      const _component_Panel = rc("Panel");
      return ob(), cv(_component_Panel, { title: _ctx.title }, {
        default: wc(() => [
          ce("span", null, td(_ctx.message), 1)
        ]),
        _: 1
      }, 8, ["title"]);
    };
  }
});
export default _sfc_main;
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <Panel :title=\"title\">\n    <template v-slot:default>\n      <span>{{ message }}</span>\n    </template>\n  </Panel>\n</template>\n"
        );
    }

    #[test]
    fn prefers_vite_exported_component_when_chunk_has_multiple_setup_renders() {
        let input = r#"
import { d as dc, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
const _sfc_banner = dc({
  __name: "Banner",
  setup() {
    return () => (ob(), ce("aside", null, "Banner"));
  }
});
const _sfc_main = dc({
  __name: "Main",
  setup() {
    return () => (ob(), ce("main", null, "Main"));
  }
});
export { _sfc_banner as T, _sfc_main as _ };
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <main>Main</main>\n</template>\n"
        );
    }

    #[test]
    fn prefers_decompiled_vite_exported_component_decl() {
        let input = r#"
import { d as dc, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
const _sfc_banner = dc({
  __name: "Banner",
  setup() {
    return () => (ob(), ce("aside", null, "Banner"));
  }
});
export const _ = dc({
  __name: "Main",
  setup() {
    return () => (ob(), ce("main", null, "Main"));
  }
});
export { _sfc_banner as T };
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <main>Main</main>\n</template>\n"
        );
    }

    #[test]
    fn recovers_setup_render_if_return_chain() {
        let input = r#"
import { defineComponent, openBlock, createBlock, createElementVNode, createCommentVNode, withCtx } from "vue";
const _sfc_main = defineComponent({
  __name: "MaybeNotice",
  setup() {
    return (_ctx, _cache) => {
      if (_ctx.isLoaded) {
        return openBlock(), createBlock(Notice, { key: 0 }, {
          default: withCtx(() => [
            createElementVNode("span", { innerHTML: _ctx.message }, null, 8, ["innerHTML"])
          ]),
          _: 1
        });
      }
      return createCommentVNode("", true);
    };
  }
});
export default _sfc_main;
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <Notice v-if=\"isLoaded\">\n    <template v-slot:default>\n      <span v-html=\"message\" />\n    </template>\n  </Notice>\n</template>\n"
        );
    }

    #[test]
    fn recovers_vue_file_component_import_alias() {
        let input = r#"
import { _ as __1 } from "./Notification.vue_vue_type_script_setup_true_lang-D4OJlsAz.js";
import { d as dc, q as ob, aa as cb } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "UsesNotification",
  setup() {
    return () => (ob(), cb(__1, { key: 0 }, null));
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <Notification :key=\"0\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_scoped_local_component_alias() {
        let input = r#"
import { d as dc, _ as scope, q as ob, aa as cb } from "./vendor-vue-C85wAS_L.js";
const local = dc({
  __name: "LocalPanel",
  setup() {
    return () => (ob(), cb("section", null, "Local"));
  }
});
const scoped = scope(local, [["__scopeId", "data-v-test"]]);
export const _ = dc({
  __name: "UsesLocalPanel",
  setup() {
    return () => (ob(), cb(scoped, { title: "Ready" }, null));
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <LocalPanel title=\"Ready\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_nested_scoped_local_component_alias() {
        let input = r#"
import { d as dc, _ as scope, q as ob, aa as cb } from "./vendor-vue-C85wAS_L.js";
const scoped = scope(dc({
  __name: "MyBetRow",
  setup() {
    return () => null;
  }
}), [["__scopeId", "data-v-test"]]);
export const _ = dc({
  __name: "UsesMyBetRow",
  setup() {
    return () => (ob(), cb(scoped, { title: "Ready" }, null));
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <MyBetRow title=\"Ready\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_exported_local_component_alias() {
        let input = r#"
import { d as dc, q as ob, aa as cb, X as ce, R as wc } from "./vendor-vue-C85wAS_L.js";
export const r = dc({
  __name: "NavbarRowItem",
  setup() {
    return () => null;
  }
});
export const _ = dc({
  __name: "Navbar",
  setup() {
    return () => (
      ob(), cb(r, null, {
        default: wc(() => [
          ce("span", null, "Title")
        ]),
        _: 1
      })
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <NavbarRowItem>\n    <template v-slot:default>\n      <span>Title</span>\n    </template>\n  </NavbarRowItem>\n</template>\n"
        );
    }

    #[test]
    fn recovers_cross_module_component_export_alias() {
        let input = r#"
import { q as ob, aa as cb, _ as rd } from "./vendor-vue.js";
import { B as B_1 } from "./main.js";
export function render(_ctx, _cache) {
  return ob(), cb(rd(B_1), { text: "Details" }, null, 8, ["text"]);
}
"#;
        let shared = r#"
import { defineComponent } from "vue";
const YP = defineComponent({
  name: "VTooltip",
  props: { text: String }
});
export { YP as B };
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js_with_import_resolver(input, |source| {
                (source == "./main.js").then(|| shared.to_string())
            })
            .unwrap()
            .unwrap(),
            "<template>\n  <VTooltip text=\"Details\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_cross_module_systemjs_component_export_alias() {
        let input = r#"
import { q as ob, aa as cb } from "./vendor-vue.js";
import { V as V_1 } from "./main-legacy.js";
export function render(_ctx, _cache) {
  return ob(), cb(V_1, { flat: "" }, null, 8, ["flat"]);
}
"#;
        let shared = r#"
System.register(["./vendor-vue.js"], function (_export) {
  var defineComponent;
  return {
    setters: [
      function (module) {
        defineComponent = module.d;
      }
    ],
    execute: function () {
      _export("V", defineComponent({
        __name: "VButton",
        setup: function () {
          return function () {
            return null;
          };
        }
      }));
    }
  };
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js_with_import_resolver(input, |source| {
                (source == "./main-legacy.js").then(|| shared.to_string())
            })
            .unwrap()
            .unwrap(),
            "<template>\n  <VButton flat />\n</template>\n"
        );
    }

    #[test]
    fn decompiles_single_system_register_with_component_export_alias() {
        let input = r#"
System.register(["./main-legacy.js", "./vendor-vue.js"], function (_export) {
  var VButton, defineComponent, openBlock, createBlock;
  return {
    setters: [
      function (module) {
        VButton = module.V;
      },
      function (module) {
        defineComponent = module.d;
        openBlock = module.q;
        createBlock = module.aa;
      }
    ],
    execute: function () {
      _export("_", defineComponent({
        __name: "UsesButton",
        setup: function () {
          return function () {
            return openBlock(), createBlock(VButton, { flat: "" }, null, 8, ["flat"]);
          };
        }
      }));
    }
  };
});
"#;
        let shared = r#"
!function () {
  function scope(component, attrs) {
    return component;
  }
  System.register(["./side-effect.js", "./vendor-vue.js"], function (_export) {
    var defineComponent;
    return {
      setters: [
        null,
        function (module) {
          defineComponent = module.d;
        }
      ],
      execute: function () {
        var base = defineComponent({
          __name: "VButton",
          setup: function () {
            return function () {
              return null;
            };
          }
        }), scoped = scope(base, [["__scopeId", "data-v-test"]]);
        _export("V", scoped);
      }
    };
  });
}();
"#;

        assert_eq!(
            decompile_vue_sfc_with_import_resolver(input, DecompileOptions::default(), |source| {
                (source == "./main-legacy.js").then(|| shared.to_string())
            })
            .unwrap()
            .code,
            "<template>\n  <VButton flat />\n</template>\n"
        );
    }

    #[test]
    fn decompiles_system_register_style_sequence_direct_export() {
        let input = r#"
System.register(["./Badge.vue", "./vendor-vue.js"], function (_export) {
  var Badge, defineComponent, openBlock, createBlock;
  return {
    setters: [
      function (module) {
        Badge = module.B;
      },
      function (module) {
        defineComponent = module.d;
        openBlock = module.q;
        createBlock = module.aa;
      }
    ],
    execute: function () {
      var style = document.createElement("style");
      style.textContent = ".badge{}", document.head.appendChild(style), _export("_", defineComponent({
        __name: "TeamBadge",
        setup: function (props) {
          return function (_ctx, _cache) {
            return openBlock(), createBlock(Badge, { text: props.team.name }, null, 8, ["text"]);
          };
        }
      }));
    }
  };
});
"#;

        assert_eq!(
            decompile_vue_sfc(input, DecompileOptions::default())
                .unwrap()
                .code,
            "<template>\n  <Badge :text=\"team.name\" />\n</template>\n"
        );
    }

    #[test]
    fn ignores_unparseable_import_source_when_resolving_component_aliases() {
        let input = r#"
import data from "./config.json";
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("div", null, "Ready");
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js_with_import_resolver(input, |_| {
                Some("{ not javascript".to_string())
            })
            .unwrap()
            .unwrap(),
            "<template>\n  <div>Ready</div>\n</template>\n"
        );
    }

    #[test]
    fn recovers_pascal_case_chunk_component_import_alias() {
        let input = r#"
import { S as __1 } from "./SvgIcon-Dg6MjH_p.js";
import { d as dc, q as ob, aa as cb } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "UsesSvgIcon",
  setup() {
    return () => (ob(), cb(__1, { name: "icon-system-play-video-cycle" }, null));
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <SvgIcon name=\"icon-system-play-video-cycle\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_unref_helper_alias_in_conditions_and_expressions() {
        let input = r#"
import { d as dc, _ as ur, q as ob, aa as cb, X as ce, J as td, Z as cc } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "MaybeNotice",
  setup() {
    return () => {
      if (ur(isLoaded)) {
        return ob(), cb(Notice, null, {
          default: () => [
            ce("span", null, td(ur(i18n).t("loaded")), 1)
          ],
          _: 1
        });
      }
      return cc("", true);
    };
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <Notice v-if=\"isLoaded\">\n    <template v-slot:default>\n      <span>{{ i18n.t(\"loaded\") }}</span>\n    </template>\n  </Notice>\n</template>\n"
        );
    }

    #[test]
    fn recovers_unref_helper_alias_in_component_props_and_events() {
        let input = r#"
import { P as Panel } from "./Panel.vue";
import { d as dc, _ as ur, q as ob, aa as cb } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "PanelHost",
  setup() {
    return () => (
      ob(), cb(Panel, {
        disabled: !ur(open),
        items: ur(items),
        onClose: ur(closePanel)
      }, null, 8, ["disabled", "items", "onClose"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <Panel :disabled=\"!open\" :items=\"items\" @close=\"closePanel\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_unref_helper_alias_in_render_conditions_and_lists() {
        let input = r#"
import { d as dc, _ as ur, q as ob, X as ce, F as Fragment, R as rl, Z as cc } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "PanelList",
  setup() {
    return () => (
      ob(), ce(Fragment, null, [
        ur(open) && ur(enabled)
          ? (ob(), ce("p", { key: 0 }, "Open"))
          : cc("", true),
        (ob(true), ce(Fragment, null, rl(ur(items), (item) => (
          ob(), ce("span", { key: item.id }, item.name, 1)
        )), 128))
      ], 64)
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <p v-if=\"open &amp;&amp; enabled\">Open</p>\n  <span v-for=\"item in items\" :key=\"item.id\">{{ item.name }}</span>\n</template>\n"
        );
    }

    #[test]
    fn recovers_setup_computed_value_alias() {
        let input = r#"
import { defineComponent, computed, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "ComputedLabel",
  setup() {
    const label = computed(() => format(total.value));
    return () => (
      openBlock(), createElementBlock("span", { innerHTML: label.value }, null, 8, ["innerHTML"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <span v-html=\"format(total.value)\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_vite_setup_computed_value_alias() {
        let input = r#"
import { d as dc, c as cp, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "ComputedMessage",
  setup() {
    const formatted = cp(() => format(total.value));
    const message = cp(() => t("max_payout_message", { value: formatted.value }));
    return () => (
      ob(), ce("span", { innerHTML: message.value }, null, 8, ["innerHTML"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <span v-html='t(\"max_payout_message\", { value: (format(total.value)) })' />\n</template>\n"
        );
    }

    #[test]
    fn recovers_computed_value_inside_template_literal() {
        let input = r#"
import { d as dc, c as cp, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "ComputedStyle",
  setup() {
    const height = cp(() => itemHeight.value + gap.value);
    return () => (
      ob(), ce("div", { style: { height: `${height.value}px` } }, null, 4)
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <div :style=\"{ height: `${(itemHeight.value + gap.value)}px` }\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_computed_block_local_return_alias() {
        let input = r#"
import { defineComponent, ref, computed, openBlock, createVNode } from "vue";
import { I as ItemPicker } from "./ItemPicker.vue";
export default defineComponent({
  __name: "ItemFilters",
  setup() {
    const sortedItems = ref([]);
    const itemFilters = computed(() => {
      const ids = sortedItems.value.map((item) => item.id);
      return uniqueBy(ids, (id) => id);
    });
    return () => (
      openBlock(), createVNode(ItemPicker, { itemFilters: itemFilters.value }, null, 8, ["itemFilters"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst sortedItems = ref([]);\n</script>\n\n<template>\n  <ItemPicker :itemFilters=\"uniqueBy(sortedItems.map((item)=>item.id), (id)=>id)\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_computed_block_destructured_setup_props() {
        let input = r#"
import { defineComponent, computed, openBlock, createElementBlock, createCommentVNode } from "vue";
const _sfc_main = defineComponent({
  props: {
    show: Boolean,
    progressDuration: Number,
  },
  setup(__props) {
    const props = __props;
    const duration = computed(() => {
      const { show: isShown, progressDuration: ms } = props;
      if (isShown) {
        return ms;
      }
      return 0;
    });
    return (_ctx, _cache) => (
      openBlock(),
      createElementBlock("div", null, [
        duration.value !== void 0
          ? (openBlock(), createElementBlock("div", {
              style: `animation-duration: ${duration.value}ms;`
            }, null, 4))
          : createCommentVNode("", true)
      ])
    );
  }
});
export default _sfc_main;
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nconst props = defineProps({\n    show: Boolean,\n    progressDuration: Number\n});\nconst { progressDuration, show } = props;\n</script>\n\n<template>\n  <div>\n    <div v-if=\"(show ? progressDuration : 0) !== void 0\" :style=\"`animation-duration: ${(show ? progressDuration : 0)}ms;`\" />\n  </div>\n</template>\n"
        );
    }

    #[test]
    fn preserves_mutated_computed_block_local_binding() {
        let input = r#"
import { defineComponent, computed, openBlock, createElementBlock } from "vue";
const _sfc_main = defineComponent({
  props: {
    padding: String,
  },
  setup(__props) {
    const props = __props;
    const style = computed(() => {
      const result = {};
      if (props.padding) {
        result.padding = props.padding;
      }
      return result;
    });
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("div", { style: style.value }, null, 4)
    );
  }
});
export default _sfc_main;
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\n\nconst props = defineProps({\n    padding: String\n});\nconst { padding } = props;\n\nconst style = computed(()=>{\n    const result = {};\n    if (padding) {\n        result.padding = padding;\n    }\n    return result;\n});\n</script>\n\n<template>\n  <div :style=\"style\" />\n</template>\n"
        );
    }

    #[test]
    fn imports_helpers_used_by_script_setup_computed_bindings() {
        let input = r#"
import { normalizePadding } from "./format.js";
import { defineComponent, computed, openBlock, createElementBlock } from "vue";
const _sfc_main = defineComponent({
  props: {
    padding: String,
  },
  setup(props) {
    const style = computed(() => {
      const result = {};
      const value = normalizePadding(props.padding);
      if (value) {
        result.padding = value;
      }
      return result;
    });
    return () => (
      openBlock(), createElementBlock("div", { style: style.value }, null, 4)
    );
  }
});
export default _sfc_main;
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\nimport { normalizePadding } from \"./format.js\";\n\nconst props = defineProps({\n    padding: String\n});\nconst { padding } = props;\n\nconst style = computed(()=>{\n    const result = {};\n    const value = normalizePadding(padding);\n    if (value) {\n        result.padding = value;\n    }\n    return result;\n});\n</script>\n\n<template>\n  <div :style=\"style\" />\n</template>\n"
        );
    }

    #[test]
    fn setup_dependencies_do_not_select_shadowed_module_locals() {
        let ctx = VueRecoveryContext {
            script_local_bindings: vec![test_local_binding_with_scope(
                "const t = document.createElement(\"style\");",
                &["t"],
                &["t"],
                &[],
                true,
            )],
            setup_local_bindings: vec![
                test_local_binding(
                    "const t = toRefs(props);",
                    &["t"],
                    &["t"],
                    &["props", "toRefs"],
                ),
                test_local_binding("const value = t.event;", &["value"], &["value"], &["t"]),
            ],
            ..Default::default()
        };
        let root = VueNode::Interpolation(VueExpr::new("value.name"));

        let selected = setup_local_declarations(&ctx, &root)
            .into_iter()
            .map(|declaration| declaration.source.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            selected,
            vec!["const t = toRefs(props);", "const value = t.event;"]
        );
    }

    #[test]
    fn imports_inlined_computed_script_setup_dependencies() {
        let input = r#"
import { sections } from "./sections.js";
import { useViewState } from "./state.js";
import { defineComponent, computed, openBlock, createElementBlock, Fragment, renderList, toDisplayString } from "vue";
export default defineComponent({
  setup() {
    const { page } = useViewState();
    const labels = computed(() => ({
      [sections.Home]: {
        title: page.name
      }
    }));
    const links = computed(() => {
      const list = page.meta.steps ?? [];
      return list.map((name, index) => ({
        title: labels.value[name]?.title ?? "",
        enabled: index < list.length - 1
      }));
    });
    return () => (
      openBlock(), createElementBlock("ul", null, [
        (openBlock(true), createElementBlock(Fragment, null, renderList(links.value, (item) => (
          openBlock(), createElementBlock("li", { key: item.title }, toDisplayString(item.title), 1)
        )), 128))
      ])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\nimport { sections } from \"./sections.js\";\nimport { useViewState } from \"./state.js\";\n\nconst { page } = useViewState();\n\nconst links = computed(()=>{\n    const list = page.meta.steps ?? [];\n    return list.map((name, index)=>({\n            title: (({\n    [sections.Home]: {\n        title: page.name\n    }\n}))[name]?.title ?? \"\",\n            enabled: index < list.length - 1\n        }));\n});\n</script>\n\n<template>\n  <ul>\n    <li v-for=\"item in links\" :key=\"item.title\">{{ item.title }}</li>\n  </ul>\n</template>\n"
        );
    }

    #[test]
    fn imports_template_expression_refs_into_script_setup() {
        let input = r#"
import { formatStatus } from "./status.js";
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    return () => (
      openBlock(), createElementBlock("span", { title: formatStatus("ok") }, "Ok", 8, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { formatStatus } from \"./status.js\";\n</script>\n\n<template>\n  <span :title='formatStatus(\"ok\")'>Ok</span>\n</template>\n"
        );
    }

    #[test]
    fn imports_template_helpers_without_importing_component_tags() {
        let input = r#"
import { S as StatusTag } from "./StatusTag.vue";
import { statusLevel } from "./status.js";
import { defineComponent, openBlock, createVNode } from "vue";
export default defineComponent({
  props: {
    status: String,
  },
  setup(props) {
    return () => (
      openBlock(), createVNode(StatusTag, { level: statusLevel(props.status) }, null, 8, ["level"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { statusLevel } from \"./status.js\";\n\nconst props = defineProps({\n    status: String\n});\nconst { status } = props;\n</script>\n\n<template>\n  <StatusTag :level=\"statusLevel(status)\" />\n</template>\n"
        );
    }

    #[test]
    fn uses_readable_define_props_binding_for_minified_setup_param() {
        let input = r#"
import { formatMsg } from "./format.js";
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  props: {
    msg: String,
  },
  setup(e) {
    return () => (
      openBlock(), createElementBlock("div", { title: formatMsg(e.msg) }, null, 8, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { formatMsg } from \"./format.js\";\n\nconst props = defineProps({\n    msg: String\n});\nconst { msg } = props;\n</script>\n\n<template>\n  <div :title=\"formatMsg(msg)\" />\n</template>\n"
        );
    }

    #[test]
    fn rewrites_whole_setup_props_param_in_selected_local() {
        let input = r#"
import { useState } from "./state.js";
import { defineComponent, toDisplayString, openBlock, createElementBlock } from "vue";
export default defineComponent({
  props: {
    msg: String,
  },
  setup(e) {
    const state = useState(e);
    return () => (
      openBlock(), createElementBlock("span", null, toDisplayString(state.msg), 1)
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { useState } from \"./state.js\";\n\nconst props = defineProps({\n    msg: String\n});\nconst { msg } = props;\n\nconst state = useState(props);\n</script>\n\n<template>\n  <span>{{ state.msg }}</span>\n</template>\n"
        );
    }

    #[test]
    fn avoids_props_binding_when_props_is_a_prop_name() {
        let input = r#"
import { formatMsg } from "./format.js";
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  props: {
    props: String,
  },
  setup(e) {
    return () => (
      openBlock(), createElementBlock("div", { title: formatMsg(e.props) }, null, 8, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { formatMsg } from \"./format.js\";\n\nconst e = defineProps({\n    props: String\n});\nconst { props } = e;\n</script>\n\n<template>\n  <div :title=\"formatMsg(props)\" />\n</template>\n"
        );
    }

    #[test]
    fn does_not_import_template_arrow_params() {
        let input = r#"
import { item } from "./format.js";
import { next } from "./format.js";
import { total } from "./format.js";
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  props: {
    list: Array,
  },
  setup(props) {
    return () => (
      openBlock(), createElementBlock("span", {
        title: props.list.reduce((total, item) => {
          const next = item.count;
          return total + next;
        }, 0)
      }, null, 8, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nconst props = defineProps({\n    list: Array\n});\nconst { list } = props;\n</script>\n\n<template>\n  <span :title=\"list.reduce((total, item)=>{ const next = item.count; return total + next; }, 0)\" />\n</template>\n"
        );
    }

    #[test]
    fn template_arrow_param_does_not_hide_setup_local_elsewhere() {
        let input = r#"
import { defineComponent, openBlock, createElementBlock, createElementVNode, toDisplayString } from "vue";
export default defineComponent({
  setup() {
    const list = useList();
    const item = useSelectedItem();
    return () => (
      openBlock(), createElementBlock("section", {
        title: list.map(item => item.name).join(",")
      }, [
        createElementVNode("p", null, toDisplayString(item.label), 1)
      ], 8, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nconst list = useList();\nconst item = useSelectedItem();\n</script>\n\n<template>\n  <section :title='list.map((item)=>item.name).join(\",\")'>\n    <p>{{ item.label }}</p>\n  </section>\n</template>\n"
        );
    }

    #[test]
    fn does_not_import_identifiers_used_only_as_props_or_properties() {
        let input = r#"
import { padding } from "./format.js";
import { defineComponent, computed, openBlock, createElementBlock } from "vue";
const _sfc_main = defineComponent({
  props: {
    padding: String,
  },
  setup(props) {
    const style = computed(() => {
      const result = {};
      if (props.padding) {
        result.padding = props.padding;
      }
      return result;
    });
    return () => (
      openBlock(), createElementBlock("div", { style: style.value }, null, 4)
    );
  }
});
export default _sfc_main;
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\n\nconst props = defineProps({\n    padding: String\n});\nconst { padding } = props;\n\nconst style = computed(()=>{\n    const result = {};\n    if (padding) {\n        result.padding = padding;\n    }\n    return result;\n});\n</script>\n\n<template>\n  <div :style=\"style\" />\n</template>\n"
        );
    }

    #[test]
    fn does_not_import_member_property_names() {
        let input = r#"
import { i, t } from "./format.js";
import { defineComponent, toDisplayString, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    return () => (
      openBlock(), createElementBlock("span", null, toDisplayString(i.t("hello")), 1)
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { i } from \"./format.js\";\n</script>\n\n<template>\n  <span>{{ i.t(\"hello\") }}</span>\n</template>\n"
        );
    }

    #[test]
    fn emits_script_setup_refs_used_by_template() {
        let input = r#"
import { defineComponent, ref, openBlock, createElementBlock, createElementVNode, normalizeStyle } from "vue";
export default defineComponent({
  props: {
    show: { type: Boolean, default: false },
  },
  setup(props) {
    const innerRef = ref(null);
    const height = ref(0);
    return () => (
      openBlock(), createElementBlock("section", {
        style: normalizeStyle({ height: props.show ? `${height.value}px` : 0 })
      }, [
        createElementVNode("div", { ref_key: "innerRef", ref: innerRef }, null, 512)
      ], 4)
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst props = defineProps({\n    show: {\n        type: Boolean,\n        default: false\n    }\n});\nconst { show } = props;\n\nconst height = ref(0);\nconst innerRef = ref(null);\n</script>\n\n<template>\n  <section :style=\"{ height: show ? `${height}px` : 0 }\">\n    <div ref=\"innerRef\" />\n  </section>\n</template>\n"
        );
    }

    #[test]
    fn emits_define_emits_for_setup_emit_alias() {
        let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  emits: ["click"],
  setup(props, { emit }) {
    const send = emit;
    return () => (
      openBlock(), createElementBlock("button", { onClick: () => send("click") }, "More", 8, ["onClick"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nconst send = defineEmits([\n    \"click\"\n]);\n</script>\n\n<template>\n  <button @click='send(\"click\")'>More</button>\n</template>\n"
        );
    }

    #[test]
    fn emits_define_emits_for_direct_setup_emit() {
        let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  emits: ["click"],
  setup(props, { emit }) {
    return () => (
      openBlock(), createElementBlock("button", { onClick: () => emit("click") }, "More", 8, ["onClick"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nconst emit = defineEmits([\n    \"click\"\n]);\n</script>\n\n<template>\n  <button @click='emit(\"click\")'>More</button>\n</template>\n"
        );
    }

    #[test]
    fn does_not_emit_define_emits_for_unused_setup_emit() {
        let input = r#"
import { defineComponent, ref, openBlock, createElementBlock } from "vue";
export default defineComponent({
  emits: ["click"],
  setup(props, { emit }) {
    const count = ref(0);
    return () => (
      openBlock(), createElementBlock("button", { title: count.value }, "More", 8, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst count = ref(0);\n</script>\n\n<template>\n  <button :title=\"count\">More</button>\n</template>\n"
        );
    }

    #[test]
    fn does_not_emit_ref_for_candidate_without_value_usage() {
        let input = r#"
import { d as dc, x as useSlots, _ as unref, q as ob, X as ce } from "./vendor-vue.js";
export const _ = dc({
  __name: "SlotsPanel",
  setup() {
    const slots = useSlots();
    return () => (
      ob(), ce("div", { title: unref(slots).All }, null, 8, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <div :title=\"slots.All\" />\n</template>\n"
        );
    }

    #[test]
    fn emits_opaque_helper_object_used_by_script_handler() {
        let input = r#"
import { d as dc, Q as useRouter, q as ob, X as ce } from "./vendor-vue.js";
import { sections } from "./sections.js";
export const _ = dc({
  __name: "ErrorPanel",
  setup() {
    const router = useRouter();
    function backToHome() {
      router.push({ name: sections.Home });
    }
    return () => (
      ob(), ce("button", { onClick: backToHome }, "Back", 8, ["onClick"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { Q as useRouter } from \"./vendor-vue.js\";\nimport { sections } from \"./sections.js\";\n\nconst router = useRouter();\nfunction backToHome() {\n    router.push({\n        name: sections.Home\n    });\n}\n</script>\n\n<template>\n  <button @click=\"backToHome\">Back</button>\n</template>\n"
        );
    }

    #[test]
    fn preserves_callable_vendor_helper_candidate_used_by_event() {
        let input = r#"
import { d as dc, _ as ur, h as debounce, q as ob, X as ce } from "./vendor-vue.js";
import { submit } from "./api.js";
export const _ = dc({
  __name: "SubmitButton",
  setup() {
    const send = debounce(submit, 1000);
    const payload = { kind: "save" };
    return () => (
      ob(), ce("button", {
        onClick: () => ur(send)(payload)
      }, "Save", 8, ["onClick"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { h as debounce } from \"./vendor-vue.js\";\nimport { submit } from \"./api.js\";\n\nconst send = debounce(submit, 1000);\nconst payload = {\n    kind: \"save\"\n};\n</script>\n\n<template>\n  <button @click=\"send(payload)\">Save</button>\n</template>\n"
        );
    }

    #[test]
    fn emits_module_local_helpers_used_by_setup_declarations() {
        let input = r#"
import { d as dc, r, c as cp, q as ob, X as ce } from "./vendor-vue.js";
import { n as normalize } from "./format.js";
const decorate = (item) => normalize(item.name);
function useItems(kind) {
  return {
    items: r([decorate(kind.value)]),
    loaded: r(true)
  };
}
export const _ = dc({
  __name: "ItemsPanel",
  setup() {
    const kind = { value: "soccer" };
    const r = [","];
    const { items, loaded } = useItems(kind);
    const label = cp(() => {
      const names = [];
      items.value.forEach((item) => names.push(item.name));
      return names.join(r[0]);
    });
    return () => (
      ob(), ce("p", { title: label.value }, loaded.value ? "Ready" : "Wait", 9, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\nimport { n as normalize } from \"./format.js\";\nimport { r as r_1 } from \"./vendor-vue.js\";\n\nconst decorate = (item)=>normalize(item.name);\nfunction useItems(kind) {\n    return {\n        items: r_1([\n            decorate(kind.value)\n        ]),\n        loaded: r_1(true)\n    };\n}\nconst kind = {\n    value: \"soccer\"\n};\nconst r = [\n    \",\"\n];\nconst { items, loaded } = useItems(kind);\n\nconst label = computed(()=>{\n    const names = [];\n    items.value.forEach((item)=>names.push(item.name));\n    return names.join(r[0]);\n});\n</script>\n\n<template>\n  <p :title=\"label\">\n    <template v-if=\"loaded.value\">\n      Ready\n    </template>\n    <template v-else>\n      Wait\n    </template>\n  </p>\n</template>\n"
        );
    }

    #[test]
    fn aliases_module_local_helper_when_setup_local_collides() {
        let input = r#"
import { d as dc, r as rf, c as cp, q as ob, X as ce } from "./vendor-vue.js";
import { n as normalize } from "./format.js";
const r = (item) => normalize(item.name);
function useItems(kind) {
  return {
    items: rf([r(kind.value)]),
    loaded: rf(true)
  };
}
export const _ = dc({
  __name: "ItemsPanel",
  setup() {
    const kind = { value: "soccer" };
    const r = [","];
    const { items, loaded } = useItems(kind);
    const label = cp(() => {
      const names = [];
      items.value.forEach((item) => names.push(r[0] + item));
      return names.join("");
    });
    return () => (
      ob(), ce("p", { title: label.value }, loaded.value ? "Ready" : "Wait", 9, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\nimport { n as normalize } from \"./format.js\";\nimport { r as rf } from \"./vendor-vue.js\";\n\nconst r_1 = (item)=>normalize(item.name);\nfunction useItems(kind) {\n    return {\n        items: rf([\n            r_1(kind.value)\n        ]),\n        loaded: rf(true)\n    };\n}\nconst kind = {\n    value: \"soccer\"\n};\nconst r = [\n    \",\"\n];\nconst { items, loaded } = useItems(kind);\n\nconst label = computed(()=>{\n    const names = [];\n    items.value.forEach((item)=>names.push(r[0] + item));\n    return names.join(\"\");\n});\n</script>\n\n<template>\n  <p :title=\"label\">\n    <template v-if=\"loaded.value\">\n      Ready\n    </template>\n    <template v-else>\n      Wait\n    </template>\n  </p>\n</template>\n"
        );
    }

    #[test]
    fn does_not_rewrite_setup_local_refs_to_module_aliases() {
        let input = r#"
import { d as dc, q as ob, X as ce } from "./vendor-vue.js";
const source = () => "module";
function useItems() {
  return source();
}
export const _ = dc({
  __name: "ItemsPanel",
  setup() {
    const source = { value: "setup" };
    function onClick() {
      return source.value + useItems();
    }
    return () => (
      ob(), ce("button", { title: source.value, onClick: onClick }, "Ready", 8, ["title", "onClick"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nconst source_1 = ()=>\"module\";\nfunction useItems() {\n    return source_1();\n}\nconst source = {\n    value: \"setup\"\n};\nfunction onClick() {\n    return source.value + useItems();\n}\n</script>\n\n<template>\n  <button :title=\"source.value\" @click=\"onClick\">Ready</button>\n</template>\n"
        );
    }

    #[test]
    fn omits_later_duplicate_module_local_candidates() {
        let input = r#"
import { d as dc, q as ob, X as ce } from "./vendor-vue.js";
function r(step) {
  return step();
}
var r = document.createElement("style");
function useItems() {
  return r(() => "ready");
}
export const _ = dc({
  __name: "ItemsPanel",
  setup() {
    function onClick() {
      return useItems();
    }
    return () => (
      ob(), ce("button", { onClick: onClick }, "Ready", 8, ["onClick"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nfunction r(step) {\n    return step();\n}\nfunction useItems() {\n    return r(()=>\"ready\");\n}\nfunction onClick() {\n    return useItems();\n}\n</script>\n\n<template>\n  <button @click=\"onClick\">Ready</button>\n</template>\n"
        );
    }

    #[test]
    fn omits_transpiler_runtime_helpers_from_module_dependencies() {
        let input = r#"
import { d as dc, q as ob, X as ce } from "./vendor-vue.js";
function runtime() {
  const start = "suspendedStart";
  const iterator = "@@iterator";
  function invoke() {
    return "_invoke";
  }
  return { start, iterator, invoke };
}
function useLabel() {
  return runtime().invoke();
}
export const _ = dc({
  setup() {
    function onClick() {
      return useLabel();
    }
    return () => (
      ob(), ce("button", { onClick: onClick }, "Ready", 8, ["onClick"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nfunction useLabel() {\n    return runtime().invoke();\n}\nfunction onClick() {\n    return useLabel();\n}\n</script>\n\n<template>\n  <button @click=\"onClick\">Ready</button>\n</template>\n"
        );
    }

    #[test]
    fn emits_candidate_ref_used_by_inlined_setup_computed() {
        let input = r#"
import { d as dc, r as rf, c as cp, q as ob, X as ce } from "./vendor-vue.js";
export const _ = dc({
  __name: "HeightPanel",
  setup() {
    const height = rf(0);
    const style = cp(() => ({ height: `${height.value}px` }));
    return () => (
      ob(), ce("div", { title: style.value }, null, 8, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst height = ref(0);\n</script>\n\n<template>\n  <div :title=\"{ height: `${height}px` }\" />\n</template>\n"
        );
    }

    #[test]
    fn preserves_computed_block_local_shadowing() {
        let input = r#"
import { defineComponent, computed, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "ShadowedLocal",
  setup() {
    const label = computed(() => {
      const values = items.value;
      return values.map((values) => values.value).join(",");
    });
    return () => (
      openBlock(), createElementBlock("p", { title: label.value }, null, 8, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <p :title='items.value.map((values)=>values.value).join(\",\")' />\n</template>\n"
        );
    }

    #[test]
    fn recovers_setup_ref_value_alias() {
        let input = r#"
import { defineComponent, ref, toDisplayString, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "Counter",
  setup() {
    const count = ref(0);
    return () => (
      openBlock(), createElementBlock("button", { title: count.value }, toDisplayString(count.value), 9, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst count = ref(0);\n</script>\n\n<template>\n  <button :title=\"count\">{{ count }}</button>\n</template>\n"
        );
    }

    #[test]
    fn recovers_vite_setup_ref_value_alias() {
        let input = r#"
import { d as dc, r as rf, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "Viewport",
  setup() {
    const height = rf(0);
    return () => (
      ob(), ce("div", { style: { height: `${height.value}px` } }, null, 4)
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst height = ref(0);\n</script>\n\n<template>\n  <div :style=\"{ height: `${height}px` }\" />\n</template>\n"
        );
    }

    #[test]
    fn preserves_shadowed_ref_value_member() {
        let input = r#"
import { defineComponent, ref, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "ShadowedCounter",
  setup() {
    const count = ref(0);
    return () => (
      openBlock(), createElementBlock("div", { title: [count].map((count) => count.value).join(",") }, null, 8, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <div :title='[ count ].map((count)=>count.value).join(\",\")' />\n</template>\n"
        );
    }

    #[test]
    fn recovers_store_to_refs_destructured_values() {
        let input = r#"
import { defineComponent, toDisplayString, openBlock, createElementBlock } from "vue";
import { storeToRefs } from "pinia";
export default defineComponent({
  __name: "StoreStatus",
  setup() {
    const store = useStore();
    const { currentUser, isLoaded } = storeToRefs(store);
    return () => (
      openBlock(), createElementBlock("p", { title: currentUser.value.name }, toDisplayString(isLoaded.value), 9, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { storeToRefs } from \"pinia\";\n\nconst store = useStore();\nconst { currentUser, isLoaded } = storeToRefs(store);\n</script>\n\n<template>\n  <p :title=\"currentUser.name\">{{ isLoaded }}</p>\n</template>\n"
        );
    }

    #[test]
    fn recovers_vite_store_to_refs_destructured_values() {
        let input = r#"
import { d as dc, K as sr, c as cp, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "StoreStatus",
  setup() {
    const { currentUser } = sr(useStore());
    const label = cp(() => currentUser.value.name);
    return () => (
      ob(), ce("p", { title: label.value }, null, 8, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { K as sr } from \"./vendor-vue-C85wAS_L.js\";\n\nconst { currentUser } = sr(useStore());\n</script>\n\n<template>\n  <p :title=\"currentUser.name\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_vite_store_to_refs_destructured_alias_values() {
        let input = r#"
import { d as dc, K as sr, c as cp, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "StoreStatus",
  setup() {
    const refs = sr(useStore());
    const { currentUser } = refs;
    const label = cp(() => currentUser.value.name);
    return () => (
      ob(), ce("p", { title: label.value }, null, 8, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { K as sr } from \"./vendor-vue-C85wAS_L.js\";\n\nconst refs = sr(useStore());\nconst { currentUser } = refs;\n</script>\n\n<template>\n  <p :title=\"currentUser.name\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_ref_object_member_extracted_values() {
        let input = r#"
import { defineComponent, toDisplayString, openBlock, createElementBlock } from "vue";
import { storeToRefs } from "pinia";
export default defineComponent({
  __name: "StoreStatus",
  setup() {
    const currentUser = storeToRefs(useStore()).currentUser;
    const refs = storeToRefs(useOtherStore());
    const isLoaded = refs.isLoaded;
    return () => (
      openBlock(), createElementBlock("p", { title: currentUser.value.name }, toDisplayString(isLoaded.value), 9, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { storeToRefs } from \"pinia\";\n\nconst currentUser = storeToRefs(useStore()).currentUser;\nconst refs = storeToRefs(useOtherStore());\nconst isLoaded = refs.isLoaded;\n</script>\n\n<template>\n  <p :title=\"currentUser.name\">{{ isLoaded }}</p>\n</template>\n"
        );
    }

    #[test]
    fn emits_dependencies_for_inlined_setup_computed_values() {
        let input = r#"
import { defineComponent, computed, openBlock, createElementBlock, Fragment, renderList } from "vue";
import { storeToRefs } from "pinia";
export default defineComponent({
  setup() {
    const { items, selected } = storeToRefs(useStore());
    const visibleItems = computed(() => items.value.filter((item) => selected.value.includes(item.id)));
    return () => (
      openBlock(), createElementBlock("ul", null, [
        (openBlock(true), createElementBlock(Fragment, null, renderList(visibleItems.value, (item) => (
          openBlock(), createElementBlock("li", { key: item.id }, item.name, 1)
        )), 128))
      ])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { storeToRefs } from \"pinia\";\n\nconst { items, selected } = storeToRefs(useStore());\n</script>\n\n<template>\n  <ul>\n    <li v-for=\"item in items.filter((item)=>selected.includes(item.id))\" :key=\"item.id\">{{ item.name }}</li>\n  </ul>\n</template>\n"
        );
    }

    #[test]
    fn emits_alias_dependencies_for_inlined_setup_computed_values() {
        let input = r#"
import { defineComponent, computed, openBlock, createElementBlock, Fragment, renderList } from "vue";
import { a } from "./vendor-vue.js";
export default defineComponent({
  setup() {
    const refs = a(useStore());
    const { items } = refs;
    const visibleItems = computed(() => items.value.filter((item) => item.visible));
    return () => (
      openBlock(), createElementBlock("ul", null, [
        (openBlock(true), createElementBlock(Fragment, null, renderList(visibleItems.value, (item) => (
          openBlock(), createElementBlock("li", { key: item.id }, item.name, 1)
        )), 128))
      ])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { a } from \"./vendor-vue.js\";\n\nconst refs = a(useStore());\nconst { items } = refs;\n</script>\n\n<template>\n  <ul>\n    <li v-for=\"item in items.filter((item)=>item.visible)\" :key=\"item.id\">{{ item.name }}</li>\n  </ul>\n</template>\n"
        );
    }

    #[test]
    fn cleans_template_ref_alias_in_opaque_ref_object_dependency() {
        let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
import { c, r } from "./vendor-vue.js";
export default defineComponent({
  setup() {
    const D = r(null);
    const scroller = c(D, { offset: { left: 1 } });
    const { x } = scroller;
    const scroll = () => x.value;
    return () => (
      openBlock(), createElementBlock("div", { ref_key: "scrollContainer", ref: D, onClick: scroll }, null, 8, ["onClick"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\nimport { c } from \"./vendor-vue.js\";\n\nconst scrollContainer = ref(null);\n\nconst scroller = c(scrollContainer, {\n    offset: {\n        left: 1\n    }\n});\nconst { x } = scroller;\nconst scroll = ()=>x;\n</script>\n\n<template>\n  <div ref=\"scrollContainer\" @click=\"scroll\" />\n</template>\n"
        );
    }

    #[test]
    fn preserves_plain_destructured_value_members() {
        let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "PlainValue",
  setup() {
    const { currentUser } = usePlainStore();
    return () => (
      openBlock(), createElementBlock("p", { title: currentUser.value.name }, null, 8, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <p :title=\"currentUser.value.name\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_imported_composable_returned_ref_values() {
        let input = r#"
import { defineComponent, computed, openBlock, createElementBlock } from "vue";
import { u as useViewState } from "./state.js";
export default defineComponent({
  __name: "UsesViewState",
  setup() {
    const { page, selectedKey, raw } = useViewState();
    const label = computed(() => {
      const parts = [];
      parts.push(page.name);
      parts.push(selectedKey.value);
      parts.push(raw.value);
      return parts.join(":");
    });
    return () => (
      openBlock(), createElementBlock("p", { title: label.value }, null, 8, ["title"])
    );
  }
});
"#;
        let state = r#"
function trackedValue(source) {
  const value = createRef();
  watch(source, (next) => {
    value.value = next;
  });
  return readonly(value);
}
const useViewState = () => {
  const page = usePage();
  const selectedKey = trackedValue(() => page.params.kind);
  const raw = { value: "plain" };
  return { page, selectedKey, raw };
};
export { useViewState as u };
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js_with_import_resolver(input, |source| {
                (source == "./state.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\nimport { u as useViewState } from \"./state.js\";\n\nconst { page, selectedKey, raw } = useViewState();\n\nconst label = computed(()=>{\n    const parts = [];\n    parts.push(page.name);\n    parts.push(selectedKey);\n    parts.push(raw.value);\n    return parts.join(\":\");\n});\n</script>\n\n<template>\n  <p :title=\"label\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_imported_composable_member_ref_values() {
        let input = r#"
import { defineComponent, toDisplayString, openBlock, createElementBlock } from "vue";
import { u as useViewState } from "./state.js";
export default defineComponent({
  __name: "UsesViewState",
  setup() {
    const selectedKey = useViewState().selectedKey;
    return () => (
      openBlock(), createElementBlock("p", { title: selectedKey.value }, toDisplayString(selectedKey.value), 9, ["title"])
    );
  }
});
"#;
        let state = r#"
function trackedValue(source) {
  const value = createRef();
  watch(source, (next) => {
    value.value = next;
  });
  return readonly(value);
}
const useViewState = () => {
  const selectedKey = trackedValue(() => route.params.kind);
  const raw = { value: "plain" };
  return { selectedKey, raw };
};
export { useViewState as u };
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js_with_import_resolver(input, |source| {
                (source == "./state.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { u as useViewState } from \"./state.js\";\n\nconst selectedKey = useViewState().selectedKey;\n</script>\n\n<template>\n  <p :title=\"selectedKey\">{{ selectedKey }}</p>\n</template>\n"
        );
    }

    #[test]
    fn recovers_imported_composable_tuple_member_ref_values() {
        let input = r#"
import { defineComponent, normalizeClass, openBlock, createElementBlock } from "vue";
import { u as useStatus } from "./status.js";
export default defineComponent({
  __name: "UsesStatus",
  setup() {
    const selectedStatus = useStatus().selectedStatus;
    return () => (
      openBlock(), createElementBlock("div", { class: normalizeClass({ rise: selectedStatus.value === "rise" }) }, null, 2)
    );
  }
});
"#;
        let state = r#"
export const u = () => {
  const [status, setStatus] = useResetState("remain");
  if (status.value === "drop") {
    setStatus("remain");
  }
  return { selectedStatus: status };
};
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js_with_import_resolver(input, |source| {
                (source == "./status.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { u as useStatus } from \"./status.js\";\n\nconst selectedStatus = useStatus().selectedStatus;\n</script>\n\n<template>\n  <div :class='{ rise: selectedStatus === \"rise\" }' />\n</template>\n"
        );
    }

    #[test]
    fn recovers_imported_composable_written_ref_values() {
        let input = r#"
import { defineComponent, openBlock, createBlock } from "vue";
import { L as ListView } from "./ListView.vue";
import { u as useListState } from "./state.js";
export default defineComponent({
  __name: "UsesListState",
  setup() {
    const { items, raw } = useListState();
    return () => (
      openBlock(), createBlock(ListView, { items: items.value, title: raw.value.name }, null, 8, ["items", "title"])
    );
  }
});
"#;
        let state = r#"
export const u = () => {
  const itemList = createList([]);
  itemList.value.push("ready");
  const raw = { value: { name: "plain" } };
  return { items: itemList, raw };
};
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js_with_import_resolver(input, |source| {
                (source == "./state.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { u as useListState } from \"./state.js\";\n\nconst { items, raw } = useListState();\n</script>\n\n<template>\n  <ListView :items=\"items\" :title=\"raw.value.name\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_imported_composable_callback_written_ref_values() {
        let input = r#"
import { defineComponent, openBlock, createBlock } from "vue";
import { L as ListView } from "./ListView.vue";
import { u as useListState } from "./state.js";
export default defineComponent({
  __name: "UsesListState",
  setup() {
    const { items, raw } = useListState();
    return () => (
      openBlock(), createBlock(ListView, { items: items.value, title: raw.value.name }, null, 8, ["items", "title"])
    );
  }
});
"#;
        let state = r#"
export const u = () => {
  const itemList = createList([]);
  subscribe(() => {
    itemList.value.push("ready");
  });
  const raw = { value: { name: "plain" } };
  return { items: itemList, raw };
};
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js_with_import_resolver(input, |source| {
                (source == "./state.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { u as useListState } from \"./state.js\";\n\nconst { items, raw } = useListState();\n</script>\n\n<template>\n  <ListView :items=\"items\" :title=\"raw.value.name\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_imported_composable_legacy_tuple_member_ref_values() {
        let input = r#"
import { defineComponent, normalizeClass, openBlock, createElementBlock } from "vue";
import { u as useStatus } from "./status-legacy.js";
export default defineComponent({
  __name: "UsesStatus",
  setup() {
    const selectedStatus = useStatus().selectedStatus;
    return () => (
      openBlock(), createElementBlock("div", { class: normalizeClass({ rise: selectedStatus.value === "rise" }) }, null, 2)
    );
  }
});
"#;
        let state = r#"
System.register([], function (_export) {
  return {
    setters: [],
    execute: function () {
      _export("u", () => {
        const pair = _slicedToArray(useResetState("remain"), 2);
        const status = pair[0];
        const setStatus = pair[1];
        if (status.value === "drop") {
          setStatus("remain");
        }
        return { selectedStatus: status };
      });
    }
  };
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js_with_import_resolver(input, |source| {
                (source == "./status-legacy.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { u as useStatus } from \"./status-legacy.js\";\n\nconst selectedStatus = useStatus().selectedStatus;\n</script>\n\n<template>\n  <div :class='{ rise: selectedStatus === \"rise\" }' />\n</template>\n"
        );
    }

    #[test]
    fn recovers_local_composable_written_ref_values() {
        let input = r#"
import { defineComponent, openBlock, createBlock } from "vue";
import { L as ListView } from "./ListView.vue";
function useListState() {
  const itemList = createList([]);
  itemList.value.push("ready");
  const raw = { value: { name: "plain" } };
  return { items: itemList, raw };
}
export default defineComponent({
  __name: "UsesListState",
  setup() {
    const { items, raw } = useListState();
    return () => (
      openBlock(), createBlock(ListView, { items: items.value, title: raw.value.name }, null, 8, ["items", "title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nfunction useListState() {\n    const itemList = createList([]);\n    itemList.value.push(\"ready\");\n    const raw = {\n        value: {\n            name: \"plain\"\n        }\n    };\n    return {\n        items: itemList,\n        raw\n    };\n}\nconst { items, raw } = useListState();\n</script>\n\n<template>\n  <ListView :items=\"items\" :title=\"raw.value.name\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_iife_composable_result_ref_values() {
        let input = r#"
import { defineComponent, openBlock, createBlock } from "vue";
import { L as ListView } from "./ListView.vue";
export default defineComponent({
  __name: "UsesListState",
  setup() {
    const state = ((enabled) => {
      const itemList = createList([]);
      subscribe(() => {
        itemList.value.push("ready");
      });
      const raw = { value: { name: "plain" } };
      return { items: itemList, raw };
    })(true);
    const { items, raw } = state;
    return () => (
      openBlock(), createBlock(ListView, { items: items.value, title: raw.value.name }, null, 8, ["items", "title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nconst state = ((enabled)=>{\n    const itemList = createList([]);\n    subscribe(()=>{\n        itemList.value.push(\"ready\");\n    });\n    const raw = {\n        value: {\n            name: \"plain\"\n        }\n    };\n    return {\n        items: itemList,\n        raw\n    };\n})(true);\nconst { items, raw } = state;\n</script>\n\n<template>\n  <ListView :items=\"items\" :title=\"raw.value.name\" />\n</template>\n"
        );
    }

    #[test]
    fn preserves_iife_composable_shadowed_callback_value_members() {
        let input = r#"
import { defineComponent, openBlock, createBlock } from "vue";
import { L as ListView } from "./ListView.vue";
export default defineComponent({
  __name: "UsesListState",
  setup() {
    const state = ((enabled) => {
      const itemList = createList([]);
      subscribe((itemList) => {
        itemList.value.push("nested");
      });
      return { items: itemList };
    })(true);
    const { items } = state;
    return () => (
      openBlock(), createBlock(ListView, { items: items.value.name }, null, 8, ["items"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <ListView :items=\"items.value.name\" />\n</template>\n"
        );
    }

    #[test]
    fn preserves_imported_composable_shadowed_callback_value_members() {
        let input = r#"
import { defineComponent, openBlock, createBlock } from "vue";
import { L as ListView } from "./ListView.vue";
import { u as useListState } from "./state.js";
export default defineComponent({
  __name: "UsesListState",
  setup() {
    const { items } = useListState();
    return () => (
      openBlock(), createBlock(ListView, { items: items.value.name }, null, 8, ["items"])
    );
  }
});
"#;
        let state = r#"
export const u = () => {
  const itemList = createList([]);
  subscribe((itemList) => {
    itemList.value.push("nested");
  });
  return { items: itemList };
};
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js_with_import_resolver(input, |source| {
                (source == "./state.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<template>\n  <ListView :items=\"items.value.name\" />\n</template>\n"
        );
    }

    #[test]
    fn preserves_imported_composable_member_plain_value_members() {
        let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
import { u as usePlainState } from "./state.js";
export default defineComponent({
  __name: "UsesPlainState",
  setup() {
    const currentUser = usePlainState().currentUser;
    return () => (
      openBlock(), createElementBlock("p", { title: currentUser.value.name }, null, 8, ["title"])
    );
  }
});
"#;
        let state = r#"
const usePlainState = () => {
  const currentUser = { value: { name: "Ada" } };
  return { currentUser };
};
export { usePlainState as u };
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js_with_import_resolver(input, |source| {
                (source == "./state.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { u as usePlainState } from \"./state.js\";\n\nconst currentUser = usePlainState().currentUser;\n</script>\n\n<template>\n  <p :title=\"currentUser.value.name\" />\n</template>\n"
        );
    }

    #[test]
    fn preserves_imported_composable_tuple_plain_value_members() {
        let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
import { u as usePlainState } from "./state.js";
export default defineComponent({
  __name: "UsesPlainState",
  setup() {
    const currentUser = usePlainState().currentUser;
    return () => (
      openBlock(), createElementBlock("p", { title: currentUser.value.name }, null, 8, ["title"])
    );
  }
});
"#;
        let state = r#"
export const u = () => {
  const [currentUser] = usePlainTuple();
  const label = currentUser.value.name;
  return { currentUser, label };
};
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js_with_import_resolver(input, |source| {
                (source == "./state.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { u as usePlainState } from \"./state.js\";\n\nconst currentUser = usePlainState().currentUser;\n</script>\n\n<template>\n  <p :title=\"currentUser.value.name\" />\n</template>\n"
        );
    }

    #[test]
    fn preserves_imported_composable_returned_plain_value_members() {
        let input = r#"
import { defineComponent, computed, openBlock, createElementBlock } from "vue";
import { u as usePlainState } from "./state.js";
export default defineComponent({
  __name: "UsesPlainState",
  setup() {
    const { currentUser } = usePlainState();
    const label = computed(() => currentUser.value.name);
    return () => (
      openBlock(), createElementBlock("p", { title: label.value }, null, 8, ["title"])
    );
  }
});
"#;
        let state = r#"
const usePlainState = () => {
  const currentUser = { value: { name: "Ada" } };
  return { currentUser };
};
export { usePlainState as u };
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js_with_import_resolver(input, |source| {
                (source == "./state.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<template>\n  <p :title=\"currentUser.value.name\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_imported_systemjs_composable_returned_ref_values() {
        let input = r#"
import { defineComponent, computed, openBlock, createElementBlock } from "vue";
import { u as useViewState } from "./state-legacy.js";
export default defineComponent({
  __name: "UsesLegacyViewState",
  setup() {
    const { page, selectedKey, raw } = useViewState();
    const label = computed(() => {
      const parts = [];
      parts.push(page.name);
      parts.push(selectedKey.value);
      parts.push(raw.value);
      return parts.join(":");
    });
    return () => (
      openBlock(), createElementBlock("p", { title: label.value }, null, 8, ["title"])
    );
  }
});
"#;
        let state = r#"
System.register(["./vendor-vue.js"], function (_export) {
  var ref, watch, readonly;
  return {
    setters: [
      function (module) {
        ref = module.B;
        watch = module.w;
        readonly = module.aB;
      }
    ],
    execute: function () {
      function trackedValue(source) {
        const value = ref();
        watch(source, (next) => {
          value.value = next;
        });
        return readonly(value);
      }
      _export("u", () => {
        const page = usePage();
        const selectedKey = trackedValue(() => page.params.kind);
        const raw = { value: "plain" };
        return { page, selectedKey, raw };
      });
    }
  };
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js_with_import_resolver(input, |source| {
                (source == "./state-legacy.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\nimport { u as useViewState } from \"./state-legacy.js\";\n\nconst { page, selectedKey, raw } = useViewState();\n\nconst label = computed(()=>{\n    const parts = [];\n    parts.push(page.name);\n    parts.push(selectedKey);\n    parts.push(raw.value);\n    return parts.join(\":\");\n});\n</script>\n\n<template>\n  <p :title=\"label\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_provider_returned_ref_values() {
        let input = r#"
import { d as dc, c as cp, q as ob, aa as cb } from "./vendor-vue.js";
import { S as SummaryPanel } from "./SummaryPanel.vue";
const state = createProvider("State", () => {
  const visibleItems = cp(() => items.value.filter((item) => item.enabled));
  const loaded = cp(() => ready.value);
  return { visibleItems, loaded };
});
export const _ = dc({
  __name: "UsesState",
  setup() {
    const { visibleItems, loaded } = state.provide();
    const hasItems = cp(() => visibleItems.value.length > 0);
    return () => (
      ob(), cb(SummaryPanel, { hasItems: hasItems.value, loaded: loaded.value }, null, 8, ["hasItems", "loaded"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <SummaryPanel :hasItems=\"visibleItems.length > 0\" :loaded=\"loaded\" />\n</template>\n"
        );
    }

    #[test]
    fn emits_setup_dependencies_for_provider_computed_aliases() {
        let input = r#"
import { defineComponent, computed, ref, openBlock, createElementBlock, createVNode, createCommentVNode, Fragment } from "vue";
import { P as ListPanel } from "./ListPanel.vue";
import { I as ItemPicker } from "./ItemPicker.vue";
const state = createProvider("State", () => {
  const items = computed(() => source.value);
  const loaded = computed(() => ready.value);
  return { items, loaded };
});
function prepare(filters) {
  return { isOpen: ref(false), setIsOpen(value) {} };
}
export default defineComponent({
  __name: "UsesStateBlock",
  setup() {
    const { items, loaded } = state.provide();
    const visibleItems = computed(() => items.value.filter((item) => item.enabled));
    const itemFilters = computed(() => {
      const mapped = items.value.map((item) => ({ id: item.id, name: item.name, size: item.size }));
      return uniqueBy(mapped, (item) => item.id);
    });
    const { isOpen, setIsOpen } = prepare(itemFilters);
    const isSticky = true;
    return (_ctx, _cache) => (
      openBlock(), createElementBlock(Fragment, null, [
        visibleItems.value.length > 0 ? (openBlock(), createVNode(ListPanel, { active: true, isSticky }, null, 8, ["isSticky"])) : createCommentVNode("", true),
        createVNode(ItemPicker, { itemFilters: itemFilters.value, loaded: loaded.value, onClose: _cache[0] || (_cache[0] = (event) => setIsOpen(false)) }, null, 8, ["itemFilters", "loaded", "onClose"])
      ], 64)
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { computed, ref } from \"vue\";\n\nconst state = createProvider(\"State\", ()=>{\n    const items = computed(()=>source.value);\n    const loaded = computed(()=>ready.value);\n    return {\n        items,\n        loaded\n    };\n});\nfunction prepare(filters) {\n    return {\n        isOpen: ref(false),\n        setIsOpen (value) {}\n    };\n}\nconst { items, loaded } = state.provide();\nconst itemFilters = computed(()=>{\n    const mapped = items.map((item)=>({\n            id: item.id,\n            name: item.name,\n            size: item.size\n        }));\n    return uniqueBy(mapped, (item)=>item.id);\n});\nconst { isOpen, setIsOpen } = prepare(itemFilters);\nconst isSticky = true;\n</script>\n\n<template>\n  <ListPanel v-if=\"(items.filter((item)=>item.enabled)).length > 0\" active :isSticky=\"isSticky\" />\n  <ItemPicker :itemFilters=\"uniqueBy(items.map((item)=>({ id: item.id, name: item.name, size: item.size })), (item)=>item.id)\" :loaded=\"loaded\" @close=\"setIsOpen(false)\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_provider_returned_ref_alias_values() {
        let input = r#"
import { d as dc, c as cp, q as ob, aa as cb } from "./vendor-vue.js";
import { S as SummaryPanel } from "./SummaryPanel.vue";
const state = createProvider("State", () => {
  const loaded_1 = cp(() => ready.value);
  return { loaded: loaded_1 };
});
export const _ = dc({
  __name: "UsesState",
  setup() {
    const { loaded: isLoaded } = state.provide();
    return () => (
      ob(), cb(SummaryPanel, { loaded: isLoaded.value }, null, 8, ["loaded"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <SummaryPanel :loaded=\"isLoaded\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_provider_returned_direct_ref_values() {
        let input = r#"
import { d as dc, c as cp, q as ob, aa as cb } from "./vendor-vue.js";
import { S as SummaryPanel } from "./SummaryPanel.vue";
const state = createProvider("State", () => {
  return { visibleItems: cp(() => items.value) };
});
export const _ = dc({
  __name: "UsesState",
  setup() {
    const { visibleItems } = state.provide();
    return () => (
      ob(), cb(SummaryPanel, { hasItems: visibleItems.value.length > 0 }, null, 8, ["hasItems"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <SummaryPanel :hasItems=\"visibleItems.length > 0\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_provider_result_alias_ref_values() {
        let input = r#"
import { d as dc, c as cp, q as ob, aa as cb } from "./vendor-vue.js";
import { S as SummaryPanel } from "./SummaryPanel.vue";
const state = createProvider("State", () => {
  return { visibleItems: cp(() => items.value) };
});
export const _ = dc({
  __name: "UsesState",
  setup() {
    const provided = state.provide();
    const { visibleItems } = provided;
    const hasItems = cp(() => visibleItems.value.length > 0);
    return () => (
      ob(), cb(SummaryPanel, { hasItems: hasItems.value }, null, 8, ["hasItems"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <SummaryPanel :hasItems=\"visibleItems.length > 0\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_provider_injected_ref_values() {
        let input = r#"
import { d as dc, c as cp, q as ob, aa as cb } from "./vendor-vue.js";
import { S as SummaryPanel } from "./SummaryPanel.vue";
const state = createProvider("State", () => {
  return { items: cp(() => loadedItems.value) };
});
export const _ = dc({
  __name: "UsesState",
  setup() {
    const injected = state.inject();
    const { items } = injected;
    return () => (
      ob(), cb(SummaryPanel, { count: items.value.length }, null, 8, ["count"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <SummaryPanel :count=\"items.length\" />\n</template>\n"
        );
    }

    #[test]
    fn preserves_provider_returned_plain_value_members() {
        let input = r#"
import { d as dc, q as ob, X as ce } from "./vendor-vue.js";
const state = createProvider("State", () => {
  const value = { value: 1 };
  return { value };
});
export const _ = dc({
  __name: "UsesState",
  setup() {
    const { value } = state.provide();
    return () => (
      ob(), ce("p", { title: value.value }, null, 8, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <p :title=\"value.value\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_computed_if_return_chain() {
        let input = r#"
import { d as dc, c as cp, q as ob, aa as cb } from "./vendor-vue.js";
import { S as StatusTag } from "./StatusTag.vue";
export const _ = dc({
  __name: "BetStatusTag",
  setup(props) {
    const level = cp(() => {
      if (props.status === 1) {
        return "danger";
      }
      if (props.status === 2) {
        return "warning";
      }
      return "info";
    });
    return () => (ob(), cb(StatusTag, { level: level.value }, null, 8, ["level"]));
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <StatusTag :level='status === 1 ? \"danger\" : status === 2 ? \"warning\" : \"info\"' />\n</template>\n"
        );
    }

    #[test]
    fn ignores_setup_render_like_code_without_vue_import_signal() {
        let input = r#"
import { x as element } from "./render-helpers.js";
export default {
  setup() {
    return () => element("h1", null, "Not Vue");
  }
};
"#;

        assert!(recover_vue_sfc_source_from_js(input).unwrap().is_none());
    }

    #[test]
    fn recovers_class_binding_and_event_handler() {
        let input = r#"
import { toDisplayString, normalizeClass, openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  openBlock();
  return createElementBlock("button", {
    class: normalizeClass({ active: props.active }),
    onClick: increment
  }, toDisplayString(props.count), 3);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <button :class=\"{ active: props.active }\" @click=\"increment\">{{ props.count }}</button>\n</template>\n"
        );
    }

    #[test]
    fn recovers_template_ref_key_attrs() {
        let input = r#"
import { openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  openBlock();
  return createElementBlock("div", {
    ref_key: "innerRef",
    ref: innerRef
  }, null, 512);
}
__sfc__.render = render;
export default __sfc__;
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst innerRef = ref(null);\n</script>\n\n<template>\n  <div ref=\"innerRef\" />\n</template>\n"
        );
    }

    #[test]
    fn omits_generated_numeric_if_branch_keys() {
        let input = r#"
import { openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  openBlock();
  return _ctx.ok
    ? createElementBlock("p", { key: 0 }, "Ready")
    : createElementBlock("span", { key: 1 }, "Waiting");
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <p v-if=\"ok\">Ready</p>\n  <span v-else>Waiting</span>\n</template>\n"
        );
    }

    #[test]
    fn preserves_non_numeric_if_branch_keys() {
        let input = r#"
import { openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  openBlock();
  return _ctx.ok
    ? createElementBlock("p", { key: _ctx.item.id }, "Ready", 8, ["key"])
    : createElementBlock("span", { key: "fallback" }, "Waiting");
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <p v-if=\"ok\" :key=\"item.id\">Ready</p>\n  <span v-else key=\"fallback\">Waiting</span>\n</template>\n"
        );
    }

    #[test]
    fn preserves_empty_if_branch_keys() {
        let input = r#"
import { openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  openBlock();
  return _ctx.ok
    ? createElementBlock("p", { key: "" }, "Ready")
    : createElementBlock("span", { key: 1 }, "Waiting");
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <p v-if=\"ok\" key>Ready</p>\n  <span v-else>Waiting</span>\n</template>\n"
        );
    }

    #[test]
    fn omits_template_ref_for_attrs() {
        let input = r#"
import { openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  openBlock();
  return createElementBlock("div", {
    ref_for: true,
    ref: setItemRef
  }, null, 512);
}
__sfc__.render = render;
export default __sfc__;
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <div :ref=\"setItemRef\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_html_and_text_directive_props() {
        let input = r#"
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("section", null, [
    createElementBlock("span", { innerHTML: _ctx.message }, null, 8, ["innerHTML"]),
    createElementBlock("p", { textContent: _ctx.label }, null, 8, ["textContent"])
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <section>\n    <span v-html=\"message\" />\n    <p v-text=\"label\" />\n  </section>\n</template>\n"
        );
    }

    #[test]
    fn recovers_static_vnode_html() {
        let input = r#"
import { createStaticVNode, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("section", null, [
    createStaticVNode('<svg viewBox="0 0 10 10"><path d="M0 0h10v10H0z"></path></svg>', 1)
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <section>\n    <svg viewBox=\"0 0 10 10\"><path d=\"M0 0h10v10H0z\"></path></svg>\n  </section>\n</template>\n"
        );
    }

    #[test]
    fn recovers_with_memo_directive() {
        let input = r#"
import { withMemo, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return withMemo([_ctx.stakeDisplay, () => _ctx.i18n.locale], () => (
    openBlock(), createElementBlock("input", { value: _ctx.stakeDisplay }, null, 8, ["value"])
  ), _cache, 0);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <input :value=\"stakeDisplay\" v-memo=\"[ stakeDisplay, ()=>i18n.locale ]\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_event_handler_modifiers() {
        let input = r#"
import { withKeys, withModifiers, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return (openBlock(), createElementBlock("input", {
    onKeyup: withKeys(withModifiers(_cache[0] || (_cache[0] = (...args) => (_ctx.submit && _ctx.submit(...args))), ["stop", "prevent"]), ["enter"])
  }, null, 40));
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <input @keyup.enter.stop.prevent=\"submit\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_vue_cached_event_and_class_array() {
        let input = r#"
import { toDisplayString, normalizeClass, openBlock, createElementBlock } from "vue";
const __sfc__ = { props: { active: Boolean, count: Number } };
export function render(_ctx, _cache) {
  return (openBlock(), createElementBlock("button", {
    class: normalizeClass(["counter", { active: _ctx.props.active }]),
    onClick: _cache[0] || (_cache[0] = (...args) => (_ctx.increment && _ctx.increment(...args)))
  }, toDisplayString(_ctx.props.count), 3));
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script>\nexport default {\n    props: {\n        active: Boolean,\n        count: Number\n    }\n}\n</script>\n\n<template>\n  <button class=\"counter\" :class=\"{ active: props.active }\" @click=\"increment\">{{ props.count }}</button>\n</template>\n"
        );
    }

    #[test]
    fn recovers_cached_event_direct_call() {
        let input = r#"
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("input", {
    onInput: _cache[0] || (_cache[0] = (t) => _ctx.onChange(t.target.checked))
  }, null, 40);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <input @input=\"onChange($event.target.checked)\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_cached_event_ref_assignment() {
        let input = r#"
import { defineComponent, ref, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    const ready = ref(false);
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("button", {
        onClick: _cache[0] || (_cache[0] = (event) => ready.value = true)
      }, "Go", 40, ["onClick"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst ready = ref(false);\n</script>\n\n<template>\n  <button @click=\"ready = true\">Go</button>\n</template>\n"
        );
    }

    #[test]
    fn recovers_tuple_ref_event_assignment() {
        let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
import { u as useState } from "./state.js";
export default defineComponent({
  setup() {
    const [ready] = useState(false);
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("iframe", {
        onLoad: _cache[0] || (_cache[0] = (event) => ready.value = true),
        style: { height: ready.value ? "100px" : 0 }
      }, null, 44, ["onLoad", "style"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { u as useState } from \"./state.js\";\n\nconst [ready] = useState(false);\n</script>\n\n<template>\n  <iframe @load=\"ready = true\" :style='{ height: ready ? \"100px\" : 0 }' />\n</template>\n"
        );
    }

    #[test]
    fn recovers_tuple_local_used_only_by_template_bindings() {
        let input = r#"
import { defineComponent, unref, openBlock, createElementBlock, createCommentVNode } from "vue";
import { u as useState } from "./state.js";
export default defineComponent({
  setup() {
    const [open, setOpen] = useState(false);
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("section", {
        disabled: !unref(open)
      }, [
        unref(open)
          ? (openBlock(), createElementBlock("p", { key: 0 }, "Open"))
          : createCommentVNode("", true)
      ], 8, ["disabled"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { u as useState } from \"./state.js\";\n\nconst [open, setOpen] = useState(false);\n</script>\n\n<template>\n  <section :disabled=\"!open\">\n    <p v-if=\"open\">Open</p>\n  </section>\n</template>\n"
        );
    }

    #[test]
    fn recovers_tuple_ref_inside_class_binding() {
        let input = r#"
import { defineComponent, normalizeClass, openBlock, createElementBlock } from "vue";
import { u as useState } from "./state.js";
export default defineComponent({
  setup() {
    const [open, setOpen] = useState(false);
    const left = false;
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("div", {
        class: normalizeClass({ hidden: !(open.value && left === false) })
      }, null, 2)
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { u as useState } from \"./state.js\";\n\nconst [open, setOpen] = useState(false);\nconst left = false;\n</script>\n\n<template>\n  <div :class=\"{ hidden: !(open &amp;&amp; left === false) }\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_tuple_ref_inside_inlined_computed_class_binding() {
        let input = r#"
import { defineComponent, computed, normalizeClass, openBlock, createElementBlock } from "vue";
import { u as useState } from "./state.js";
export default defineComponent({
  setup() {
    const [open, setOpen] = useState(false);
    const left = false;
    const hidden = computed(() => open.value && left === false);
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("div", {
        class: normalizeClass({ hidden: !hidden.value })
      }, null, 2)
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { u as useState } from \"./state.js\";\n\nconst [open, setOpen] = useState(false);\nconst left = false;\n</script>\n\n<template>\n  <div :class=\"{ hidden: !(open &amp;&amp; left === false) }\" />\n</template>\n"
        );
    }

    #[test]
    fn preserves_tuple_ref_assignment_in_script_handler() {
        let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
import { u as useState } from "./state.js";
export default defineComponent({
  setup() {
    const [ready] = useState(false);
    function markReady() {
      ready.value = true;
    }
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("button", {
        onClick: _cache[0] || (_cache[0] = (event) => ready.value = false),
        onDblclick: markReady
      }, "Go", 40, ["onClick", "onDblclick"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { u as useState } from \"./state.js\";\n\nconst [ready] = useState(false);\nfunction markReady() {\n    ready.value = true;\n}\n</script>\n\n<template>\n  <button @click=\"ready = false\" @dblclick=\"markReady\">Go</button>\n</template>\n"
        );
    }

    #[test]
    fn recovers_tuple_element_ref_event_assignment() {
        let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
import { s as slice } from "./helpers.js";
import { u as useState } from "./state.js";
export default defineComponent({
  setup() {
    const ready = slice(useState(false), 1)[0];
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("iframe", {
        onLoad: _cache[0] || (_cache[0] = (event) => ready.value = true),
        style: { height: ready.value ? "100px" : 0 }
      }, null, 44, ["onLoad", "style"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { s as slice } from \"./helpers.js\";\nimport { u as useState } from \"./state.js\";\n\nconst ready = slice(useState(false), 1)[0];\n</script>\n\n<template>\n  <iframe @load=\"ready = true\" :style='{ height: ready ? \"100px\" : 0 }' />\n</template>\n"
        );
    }

    #[test]
    fn recovers_object_destructured_ref_event_assignment() {
        let input = r#"
import { defineComponent, unref, openBlock, createElementBlock } from "vue";
import { C as AppContext } from "./context.js";
export default defineComponent({
  setup() {
    const { selectedKind, isGrouped } = AppContext.inject();
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("div", null, [
        createElementBlock("button", {
          class: unref(selectedKind) === "primary" ? "active" : "",
          title: unref(isGrouped) ? "grouped" : "single",
          onClick: _cache[0] || (_cache[0] = (event) => selectedKind.value = "primary")
        }, "Primary", 42, ["class", "title", "onClick"])
      ])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { C as AppContext } from \"./context.js\";\n\nconst { selectedKind, isGrouped } = AppContext.inject();\n</script>\n\n<template>\n  <div>\n    <button :class='selectedKind === \"primary\" ? \"active\" : \"\"' :title='isGrouped ? \"grouped\" : \"single\"' @click='selectedKind = \"primary\"'>Primary</button>\n  </div>\n</template>\n"
        );
    }

    #[test]
    fn recovers_object_destructured_sibling_ref_in_inlined_computed() {
        let input = r#"
import { defineComponent, computed, unref, openBlock, createElementBlock, Fragment, renderList } from "vue";
import { C as AppContext } from "./context.js";
export default defineComponent({
  setup() {
    const { selected, isReady } = AppContext.inject();
    const visibleItems = computed(() => isReady.value ? ["one"] : []);
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("div", null, [
        createElementBlock("button", {
          class: unref(selected) === "one" ? "active" : "",
          onClick: _cache[0] || (_cache[0] = (event) => selected.value = "one")
        }, "One", 42, ["class", "onClick"]),
        (openBlock(true), createElementBlock(Fragment, null, renderList(visibleItems.value, (item) => (
          openBlock(), createElementBlock("span", { key: item }, item, 1)
        )), 128))
      ])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { C as AppContext } from \"./context.js\";\n\nconst { selected, isReady } = AppContext.inject();\n</script>\n\n<template>\n  <div>\n    <button :class='selected === \"one\" ? \"active\" : \"\"' @click='selected = \"one\"'>One</button>\n    <span v-for='item in isReady ? [ \"one\" ] : []' :key=\"item\">{{ item }}</span>\n  </div>\n</template>\n"
        );
    }

    #[test]
    fn recovers_object_destructure_depending_on_template_ref_key() {
        let input = r#"
import { defineComponent, ref, openBlock, createElementBlock } from "vue";
import { useScroll } from "@vueuse/core";
export default defineComponent({
  props: {
    disabled: { type: Boolean, default: false }
  },
  setup(t) {
    const target = ref(null);
    const { x, arrivedState } = useScroll(target);
    const scrollLeft = () => {
      let t;
      if (!arrivedState.left) {
        if (!((t = target.value) === null || t === undefined)) {
          t.scroll({ left: x.value - 200 });
        }
      }
    };
    return () => (
      openBlock(), createElementBlock("div", {
        ref_key: "scrollContainer",
        ref: target
      }, [
        createElementBlock("button", {
          disabled: t.disabled || arrivedState.left,
          onClick: scrollLeft
        }, "Left", 8, ["disabled", "onClick"])
      ], 512)
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\nimport { useScroll } from \"@vueuse/core\";\n\nconst props = defineProps({\n    disabled: {\n        type: Boolean,\n        default: false\n    }\n});\nconst { disabled } = props;\n\nconst scrollContainer = ref(null);\n\nconst { x, arrivedState } = useScroll(scrollContainer);\nconst scrollLeft = ()=>{\n    let t;\n    if (!arrivedState.left) {\n        if (!((t = scrollContainer.value) === null || t === undefined)) {\n            t.scroll({\n                left: x - 200\n            });\n        }\n    }\n};\n</script>\n\n<template>\n  <div ref=\"scrollContainer\">\n    <button :disabled=\"disabled || arrivedState.left\" @click=\"scrollLeft\">Left</button>\n  </div>\n</template>\n"
        );
    }

    #[test]
    fn cleans_template_ref_key_alias_value_in_template_expression() {
        let input = r#"
import { defineComponent, ref, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    const target = ref(null);
    return () => (
      openBlock(), createElementBlock("div", {
        ref_key: "scrollContainer",
        ref: target,
        title: target.value ? "ready" : "idle"
      }, null, 520, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst scrollContainer = ref(null);\n</script>\n\n<template>\n  <div ref=\"scrollContainer\" :title='scrollContainer ? \"ready\" : \"idle\"' />\n</template>\n"
        );
    }

    #[test]
    fn does_not_emit_object_destructure_for_unref_read_only() {
        let input = r#"
import { defineComponent, unref, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    const { status } = useStatus();
    return () => (
      openBlock(), createElementBlock("p", {
        title: unref(status).label
      }, null, 8, ["title"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <p :title=\"status.label\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_ref_object_destructure_used_only_by_template_bindings() {
        let input = r#"
import { d as dc, K as sr, c as cp, q as ob, X as ce, Z as cc } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "BannerGate",
  setup() {
    const { isBannerEnabled, isFallbackEnabled } = sr(useSettings());
    const showFallback = cp(() => isFallbackEnabled.value);
    return () => (
      ob(), ce("section", null, [
        isBannerEnabled.value
          ? (ob(), ce("p", { key: 0 }, "Banner"))
          : cc("", true),
        showFallback.value
          ? (ob(), ce("p", { key: 1 }, "Fallback"))
          : cc("", true)
      ])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { K as sr } from \"./vendor-vue-C85wAS_L.js\";\n\nconst { isBannerEnabled, isFallbackEnabled } = sr(useSettings());\n</script>\n\n<template>\n  <section>\n    <p v-if=\"isBannerEnabled\">Banner</p>\n    <p v-if=\"isFallbackEnabled\">Fallback</p>\n  </section>\n</template>\n"
        );
    }

    #[test]
    fn does_not_select_ref_object_destructure_used_only_as_template_object_key() {
        let input = r#"
import { d as dc, K as sr, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "StaticSize",
  setup() {
    const { width, height } = sr(useWindowSize());
    return () => (
      ob(), ce("div", { style: { height: "100%" } }, null, 4)
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <div :style='{ height: \"100%\" }' />\n</template>\n"
        );
    }

    #[test]
    fn preserves_setup_ref_assignment_in_script_handler() {
        let input = r#"
import { defineComponent, ref, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    const ready = ref(false);
    function markReady() {
      ready.value = true;
    }
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("button", {
        onClick: _cache[0] || (_cache[0] = (event) => ready.value = false),
        onDblclick: markReady
      }, "Go", 40, ["onClick", "onDblclick"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst ready = ref(false);\n\nfunction markReady() {\n    ready.value = true;\n}\n</script>\n\n<template>\n  <button @click=\"ready = false\" @dblclick=\"markReady\">Go</button>\n</template>\n"
        );
    }

    #[test]
    fn preserves_nested_event_shadowing() {
        let input = r#"
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("button", {
    onClick: _cache[0] || (_cache[0] = (e) => _ctx.report([1].map((e) => e + 1), e.target.checked))
  }, null, 8, ["onClick"]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <button @click=\"report([ 1 ].map((e)=>e + 1), $event.target.checked)\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_cached_event_unref_call() {
        let input = r#"
import { d as dc, _ as ur, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "SubTab",
  setup() {
    return (_ctx, _cache) => (
      ob(), ce("li", {
        onClick: _cache[0] || (_cache[0] = (event) => ur(selectTab)(name))
      }, "Tab", 8, ["onClick"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <li @click=\"selectTab(name)\">Tab</li>\n</template>\n"
        );
    }

    #[test]
    fn recovers_conditional_branch_chain() {
        let input = r#"
import { toDisplayString, openBlock, createElementBlock } from "vue";
const _hoisted_1 = { key: 0 };
const _hoisted_2 = { key: 1 };
const _hoisted_3 = { key: 2 };
export function render(_ctx, _cache) {
  return (_ctx.status === 'loading')
    ? (openBlock(), createElementBlock("p", _hoisted_1, "Loading"))
    : (_ctx.status === 'error')
      ? (openBlock(), createElementBlock("p", _hoisted_2, toDisplayString(_ctx.error), 1))
      : (openBlock(), createElementBlock("p", _hoisted_3, "Ready"));
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <p v-if=\"status === 'loading'\">Loading</p>\n  <p v-else-if=\"status === 'error'\">{{ error }}</p>\n  <p v-else>Ready</p>\n</template>\n"
        );
    }

    #[test]
    fn recovers_decompiled_if_return_branch_chain() {
        let input = r#"
import { toDisplayString, openBlock, createElementBlock } from "vue";
const _hoisted_1 = { key: 0 };
const _hoisted_2 = { key: 1 };
const _hoisted_3 = { key: 2 };
export function render(_ctx, _cache) {
  if (_ctx.status === "loading") {
    return openBlock(), createElementBlock("p", _hoisted_1, "Loading");
  }
  if (_ctx.status === 'error') {
    return openBlock(), createElementBlock("p", _hoisted_2, toDisplayString(_ctx.error), 1);
  }
  return openBlock(), createElementBlock("p", _hoisted_3, "Ready");
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <p v-if=\"status === 'loading'\">Loading</p>\n  <p v-else-if=\"status === 'error'\">{{ error }}</p>\n  <p v-else>Ready</p>\n</template>\n"
        );
    }

    #[test]
    fn omits_empty_comment_vnode_else_branch() {
        let input = r#"
import { createCommentVNode, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return _ctx.visible
    ? (openBlock(), createElementBlock("p", null, "Visible"))
    : createCommentVNode("v-if", true);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <p v-if=\"visible\">Visible</p>\n</template>\n"
        );
    }

    #[test]
    fn inverts_condition_when_empty_comment_vnode_is_consequent() {
        let input = r#"
import { createCommentVNode, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return _ctx.visible
    ? createCommentVNode("v-if", true)
    : (openBlock(), createElementBlock("p", null, "Hidden"));
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <p v-if=\"!visible\">Hidden</p>\n</template>\n"
        );
    }

    #[test]
    fn recovers_render_list_fragment_with_mangled_item_param() {
        let input = r#"
import { renderList as r, Fragment as t, openBlock as n, createElementBlock as o, toDisplayString as s } from "vue";
export function render(e, a) {
  return n(), o("ul", null, [
    (n(true), o(t, null, r(e.items, e => (n(), o("li", { key: e.id }, s(e.name), 1))), 128))
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <ul>\n    <li v-for=\"item in items\" :key=\"item.id\">{{ item.name }}</li>\n  </ul>\n</template>\n"
        );
    }

    #[test]
    fn recovers_render_list_index_param() {
        let input = r#"
import { renderList, Fragment, openBlock, createElementBlock, toDisplayString } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("ol", null, [
    (openBlock(true), createElementBlock(Fragment, null, renderList(_ctx.items, (e, i) => (
      openBlock(), createElementBlock("li", { key: i, title: i, class: i % 2 === 0 ? "even" : "odd" }, toDisplayString(e.name), 9, ["title", "class"])
    )), 128))
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <ol>\n    <li v-for=\"(item, index) in items\" :key=\"index\" :title=\"index\" :class='index % 2 === 0 ? \"even\" : \"odd\"'>{{ item.name }}</li>\n  </ol>\n</template>\n"
        );
    }

    #[test]
    fn recovers_render_list_outer_context_member() {
        let input = r#"
import { renderList, Fragment, openBlock, createElementBlock, createCommentVNode } from "vue";
export function render(e, _cache) {
  return openBlock(), createElementBlock("ul", null, [
    (openBlock(true), createElementBlock(Fragment, null, renderList(e.items, (t, i) => (
      e.$slots.placeholder
        ? (openBlock(), createElementBlock("li", { key: t.id, title: i }, "Placeholder", 8, ["title"]))
        : createCommentVNode("", true)
    )), 128))
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <ul>\n    <template v-for=\"(item, index) in items\">\n      <li v-if=\"$slots.placeholder\" :key=\"item.id\" :title=\"index\">Placeholder</li>\n    </template>\n  </ul>\n</template>\n"
        );
    }

    #[test]
    fn recovers_template_literal_text_children() {
        let input = r#"
import { renderList, Fragment, openBlock, createElementBlock, toDisplayString } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("section", null, [
    (openBlock(true), createElementBlock(Fragment, null, renderList(_ctx.items, (e, i) => (
      openBlock(), createElementBlock("p", { key: e.id }, `${toDisplayString(e.name)} - ${i}`, 1)
    )), 128))
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <section>\n    <p v-for=\"(item, index) in items\" :key=\"item.id\">{{ item.name }} - {{ index }}</p>\n  </section>\n</template>\n"
        );
    }

    #[test]
    fn recovers_render_list_destructured_param() {
        let input = r#"
import { renderList, Fragment, openBlock, createElementBlock, toDisplayString } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("section", null, [
    (openBlock(true), createElementBlock(Fragment, null, renderList(_ctx.entries, ([groupId, rows]) => (
      openBlock(), createElementBlock("article", { key: groupId }, toDisplayString(rows.length), 1)
    )), 128))
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <section>\n    <article v-for=\"[groupId, rows] in entries\" :key=\"groupId\">{{ rows.length }}</article>\n  </section>\n</template>\n"
        );
    }

    #[test]
    fn recovers_vite_fragment_alias_from_block() {
        let input = r#"
import { d as dc, q as ob, X as ce, F as fr, a0 as tv, R as td } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "FragmentBlock",
  setup() {
    return () => (
      ob(), ce(fr, { key: 0 }, [
        tv(td(count), 1)
      ], 64)
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  {{ count }}\n</template>\n"
        );
    }

    #[test]
    fn recovers_component_vnode_and_named_slot() {
        let input = r#"
import { resolveComponent, createVNode, renderSlot, createTextVNode, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  const _component_PanelHeader = resolveComponent("PanelHeader");
  return openBlock(), createElementBlock("article", null, [
    createVNode(_component_PanelHeader, { title: _ctx.title }, null, 8, ["title"]),
    renderSlot(_ctx.$slots, "body", {}, () => [
      _cache[0] || (_cache[0] = createTextVNode("Empty", -1))
    ])
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <article>\n    <PanelHeader :title=\"title\" />\n    <slot name=\"body\">Empty</slot>\n  </article>\n</template>\n"
        );
    }

    #[test]
    fn recovers_vite_render_slot_alias() {
        let input = r#"
import { d as dc, q as ob, X as ce, Y as rs } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "SlotForwarder",
  setup() {
    return (_ctx, _cache) => (
      ob(), ce("div", null, [
        rs(_ctx.$slots, "default")
      ])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <div>\n    <slot />\n  </div>\n</template>\n"
        );
    }

    #[test]
    fn recovers_slot_bucket_children_and_logical_vnodes() {
        let input = r#"
import { h } from "./vendor-vue.js";
export default {
  setup(props, context) {
    const slots = context.slots;
    return () => {
      const slotState = partitionSlots(slots);
      const { slots: namedSlots } = slotState;
      return h(props.tag, null, [
        namedSlots["container-start"],
        h("main", null, [
          namedSlots["wrapper-start"],
          namedSlots["wrapper-end"]
        ]),
        props.showControls && [
          h("button", { class: "prev" }),
          h("button", { class: "next" })
        ],
        props.showBar && h("div", { class: "bar" }),
        namedSlots["container-end"]
      ]);
    };
  }
};
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <component :is=\"tag\">\n    <slot name=\"container-start\" />\n    <main>\n      <slot name=\"wrapper-start\" />\n      <slot name=\"wrapper-end\" />\n    </main>\n    <template v-if=\"showControls\">\n      <button class=\"prev\" />\n      <button class=\"next\" />\n    </template>\n    <div v-if=\"showBar\" class=\"bar\" />\n    <slot name=\"container-end\" />\n  </component>\n</template>\n"
        );
    }

    #[test]
    fn recovers_render_local_slot_partition_vnode_children_as_default_slot() {
        let input = r#"
import { h } from "./vendor-vue.js";
function getConfig(props) {
  return props;
}
export default {
  props: {
    tag: String,
    wrapperTag: String,
    config: Object,
  },
  setup(props, context) {
    const slots = context.slots;
    const { params: p } = getConfig(props);
    return () => {
      const slotState = partitionSlots(slots);
      const { slides, slots: namedSlots } = slotState;
      return h(props.tag, null, [
        h(props.wrapperTag, { class: p.wrapperClass }, [
          namedSlots["wrapper-start"],
          renderSlides(slides),
          namedSlots["wrapper-end"]
        ])
      ]);
    };
  }
};
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nconst props = defineProps({\n    tag: String,\n    wrapperTag: String,\n    config: Object\n});\nconst { config, tag, wrapperTag } = props;\n\nfunction getConfig(props) {\n    return props;\n}\nconst { params: p } = getConfig(props);\n</script>\n\n<template>\n  <component :is=\"tag\">\n    <component :is=\"wrapperTag\" :class=\"p.wrapperClass\">\n      <slot name=\"wrapper-start\" />\n      <slot />\n      <slot name=\"wrapper-end\" />\n    </component>\n  </component>\n</template>\n"
        );
    }

    #[test]
    fn scoped_slot_props_do_not_select_setup_locals_with_same_name() {
        let input = r#"
import { defineComponent, resolveComponent, createVNode, withCtx, createElementVNode, toDisplayString, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    const item = useSelectedItem();
    return () => {
      const _component_Card = resolveComponent("Card");
      return openBlock(), createElementBlock("section", null, [
        createVNode(_component_Card, null, {
          default: withCtx(({ item }) => [
            createElementVNode("span", { title: item.id }, toDisplayString(item.name), 9, ["title"])
          ]),
          _: 1
        })
      ]);
    };
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <section>\n    <Card>\n      <template v-slot:default=\"{ item }\">\n        <span :title=\"item.id\">{{ item.name }}</span>\n      </template>\n    </Card>\n  </section>\n</template>\n"
        );
    }

    #[test]
    fn scoped_slot_aliased_props_keep_setup_ref_with_same_property_name() {
        let input = r#"
import { defineComponent, resolveComponent, createVNode, withCtx, createElementVNode, toDisplayString, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    const item = useSelectedItem();
    return () => {
      const _component_Card = resolveComponent("Card");
      return openBlock(), createElementBlock("section", null, [
        createVNode(_component_Card, null, {
          default: withCtx(({ item: row }) => [
            createElementVNode("span", null, toDisplayString(item.label + row.name), 1)
          ]),
          _: 1
        })
      ]);
    };
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nconst item = useSelectedItem();\n</script>\n\n<template>\n  <section>\n    <Card>\n      <template v-slot:default=\"{ item: row }\">\n        <span>{{ item.label + row.name }}</span>\n      </template>\n    </Card>\n  </section>\n</template>\n"
        );
    }

    #[test]
    fn v_for_locals_do_not_select_setup_locals_with_same_name() {
        let input = r#"
import { defineComponent, renderList, Fragment, openBlock, createElementBlock, toDisplayString } from "vue";
export default defineComponent({
  setup() {
    const items = useItems();
    const item = useSelectedItem();
    return () => (
      openBlock(), createElementBlock("ul", null, [
        (openBlock(true), createElementBlock(Fragment, null, renderList(items, item => (
          openBlock(), createElementBlock("li", { key: item.id }, toDisplayString(item.name), 1)
        )), 128))
      ])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nconst items = useItems();\n</script>\n\n<template>\n  <ul>\n    <li v-for=\"item in items\" :key=\"item.id\">{{ item.name }}</li>\n  </ul>\n</template>\n"
        );
    }

    #[test]
    fn v_for_aliased_destructure_keeps_setup_ref_with_same_property_name() {
        let input = r#"
import { defineComponent, renderList, Fragment, openBlock, createElementBlock, toDisplayString } from "vue";
export default defineComponent({
  setup() {
    const rows = useRows();
    const item = useSelectedItem();
    return () => (
      openBlock(), createElementBlock("ul", null, [
        (openBlock(true), createElementBlock(Fragment, null, renderList(rows, ({ item: row }) => (
          openBlock(), createElementBlock("li", { key: row.id }, toDisplayString(item.label + row.name), 1)
        )), 128))
      ])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nconst rows = useRows();\nconst item = useSelectedItem();\n</script>\n\n<template>\n  <ul>\n    <li v-for=\"{ item: row } in rows\" :key=\"row.id\">{{ item.label + row.name }}</li>\n  </ul>\n</template>\n"
        );
    }

    #[test]
    fn v_for_event_locals_do_not_select_setup_locals_with_same_name() {
        let input = r#"
import { defineComponent, renderList, Fragment, openBlock, createElementBlock, toDisplayString } from "vue";
export default defineComponent({
  setup() {
    const items = useItems();
    const item = useSelectedItem();
    function select(row) {
      return row.id;
    }
    return () => (
      openBlock(), createElementBlock("ul", null, [
        (openBlock(true), createElementBlock(Fragment, null, renderList(items, item => (
          openBlock(), createElementBlock("button", {
            key: item.id,
            onClick: event => select(item)
          }, toDisplayString(item.name), 9, ["onClick"])
        )), 128))
      ])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nconst items = useItems();\nfunction select(row) {\n    return row.id;\n}\n</script>\n\n<template>\n  <ul>\n    <button v-for=\"item in items\" :key=\"item.id\" @click=\"select(item)\">{{ item.name }}</button>\n  </ul>\n</template>\n"
        );
    }

    #[test]
    fn recovers_component_slot_object_children() {
        let input = r#"
import { resolveComponent, createVNode, withCtx, createElementVNode, toDisplayString, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  const _component_DashboardCard = resolveComponent("DashboardCard");
  return openBlock(), createElementBlock("section", null, [
    createVNode(_component_DashboardCard, { title: _ctx.title }, {
      header: withCtx(() => [
        createElementVNode("h2", null, "Latest")
      ]),
      default: withCtx(({ item }) => [
        createElementVNode("span", null, toDisplayString(item.name), 1)
      ]),
      _: 1
    }, 8, ["title"])
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <section>\n    <DashboardCard :title=\"title\">\n      <template v-slot:header>\n        <h2>Latest</h2>\n      </template>\n      <template v-slot:default=\"{ item }\">\n        <span>{{ item.name }}</span>\n      </template>\n    </DashboardCard>\n  </section>\n</template>\n"
        );
    }

    #[test]
    fn recovers_create_slots_dynamic_component_children() {
        let input = r#"
import { resolveComponent, createVNode, createSlots, withCtx, createElementVNode, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  const _component_Navbar = resolveComponent("Navbar");
  return openBlock(), createElementBlock("section", null, [
    createVNode(_component_Navbar, null, createSlots({
      topRow: withCtx(() => [
        createElementVNode("div", null, "Top")
      ]),
      _: 2
    }, [
      _ctx.showTitle ? {
        name: "navbarTitle",
        fn: withCtx(() => [
          createElementVNode("strong", null, "Title")
        ]),
        key: "0"
      } : undefined
    ]), 1024)
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <section>\n    <Navbar>\n      <template v-slot:topRow>\n        <div>Top</div>\n      </template>\n      <template v-if=\"showTitle\" v-slot:navbarTitle>\n        <strong>Title</strong>\n      </template>\n    </Navbar>\n  </section>\n</template>\n"
        );
    }

    #[test]
    fn recovers_render_list_dynamic_slot_names() {
        let input = r#"
import { resolveComponent, createVNode, createSlots, renderList, withCtx, createElementVNode, toDisplayString, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  const _component_I18nT = resolveComponent("I18nT");
  return openBlock(), createElementBlock("section", null, [
    createVNode(_component_I18nT, { keypath: _ctx.configKey }, createSlots({ _: 2 }, [
      renderList(_ctx.props.config.slots, slot => ({
        name: slot.name,
        fn: withCtx(() => [
          createElementVNode("span", null, toDisplayString(slot.content), 1)
        ]),
        key: slot.name
      }))
    ]), 1024)
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <section>\n    <I18nT :keypath=\"configKey\">\n      <template v-for=\"slot in props.config.slots\" v-slot:[slot.name] :key=\"slot.name\">\n        <span>{{ slot.content }}</span>\n      </template>\n    </I18nT>\n  </section>\n</template>\n"
        );
    }

    #[test]
    fn recovers_aliased_vue_builtin_component() {
        let input = r##"
import { Teleport as _Teleport, createBlock, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createBlock(_Teleport, { to: "#portal" }, [
    createElementBlock("div", null, "Popup")
  ]);
}
"##;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <Teleport to=\"#portal\">\n    <div>Popup</div>\n  </Teleport>\n</template>\n"
        );
    }

    #[test]
    fn recovers_vendor_vue_transition_component_alias() {
        let input = r#"
import { d as defineComponent, n as openBlock, aa as createBlock, $ as withCtx, Y as renderSlot, aj } from "./vendor-vue.js";
export default defineComponent({
  emits: ["after-enter"],
  setup(props, context) {
    const send = context.emit;
    const cleanup = () => send("after-enter");
    const afterEnter = cleanup;
    return (ctx) => (
      openBlock(),
      createBlock(aj, {
        name: "fade",
        onAfterEnter: afterEnter
      }, {
        default: withCtx(() => [
          renderSlot(ctx.$slots, "default")
        ]),
        _: 3
      }, 8, ["onAfterEnter"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nconst send = defineEmits([\n    \"after-enter\"\n]);\n\nconst cleanup = ()=>send(\"after-enter\");\n</script>\n\n<template>\n  <Transition name=\"fade\" @afterEnter=\"cleanup\">\n    <template v-slot:default>\n      <slot />\n    </template>\n  </Transition>\n</template>\n"
        );
    }

    #[test]
    fn renames_setup_prop_when_consumed_alias_collides() {
        let input = r#"
import { defineComponent, openBlock, createBlock, Transition, unref } from "vue";
export default defineComponent({
  props: {
    x: {
      type: Boolean
    }
  },
  emits: ["done"],
  setup(props, context) {
    const p = props;
    const emit = context.emit;
    const mode = p.x ? "wide" : "tall";
    function finish() {
      if (mode) {
        emit("done");
      }
    }
    const x = finish;
    return () => (
      openBlock(),
      createBlock(Transition, {
        name: mode,
        onAfterLeave: finish,
        onLeaveCancelled: unref(x)
      }, null, 8, ["name", "onLeaveCancelled"])
    );
  }
});
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script setup>\nconst props = defineProps({\n    x: {\n        type: Boolean\n    }\n});\nconst { x: x_1 } = props;\n\nconst emit = defineEmits([\n    \"done\"\n]);\n\nconst mode = x_1 ? \"wide\" : \"tall\";\nfunction finish() {\n    if (mode) {\n        emit(\"done\");\n    }\n}\n</script>\n\n<template>\n  <Transition :name=\"mode\" @afterLeave=\"finish\" @leaveCancelled=\"finish\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_component_v_model_pairs() {
        let input = r#"
import { resolveComponent, createVNode, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  const _component_FormInput = resolveComponent("FormInput");
  return openBlock(), createElementBlock("section", null, [
    createVNode(_component_FormInput, {
      modelValue: _ctx.name,
      "onUpdate:modelValue": $event => _ctx.name = $event,
      modelModifiers: { trim: true },
      filter: _ctx.filter,
      "onUpdate:filter": $event => _ctx.filter = $event,
      filterModifiers: { number: true, lazy: true },
      label: "Name"
    }, null, 8, ["modelValue", "filter"])
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <section>\n    <FormInput v-model.trim=\"name\" v-model:filter.number.lazy=\"filter\" label=\"Name\" />\n  </section>\n</template>\n"
        );
    }

    #[test]
    fn recovers_dynamic_component() {
        let input = r#"
import { resolveDynamicComponent, openBlock, createBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createBlock(resolveDynamicComponent(_ctx.currentView), {
    class: "panel"
  }, null, 512);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <component :is=\"currentView\" class=\"panel\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_direct_dynamic_component_target() {
        let input = r#"
import { openBlock, createVNode } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createVNode(_ctx.currentView, {
    class: "panel"
  }, null, 512);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <component :is=\"currentView\" class=\"panel\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_conditional_direct_dynamic_component_target() {
        let input = r#"
import { openBlock, createVNode, createCommentVNode } from "vue";
export function render(_ctx, _cache) {
  return _ctx.streamDisplay
    ? (openBlock(), createVNode(_ctx.streamDisplay.component))
    : createCommentVNode("", true);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <component v-if=\"streamDisplay\" :is=\"streamDisplay.component\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_model_and_show_directives() {
        let input = r#"
import { vModelText, vShow, withDirectives, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return withDirectives((openBlock(), createElementBlock("input", {
    "onUpdate:modelValue": _cache[0] || (_cache[0] = $event => _ctx.value = $event)
  }, null, 512)), [
    [vModelText, _ctx.value, void 0, { trim: true, number: true }],
    [vShow, _ctx.visible]
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <input v-model.trim.number=\"value\" v-show=\"visible\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_custom_directive_payload() {
        let input = r#"
import { resolveDirective, withDirectives, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  const _directive_focus = resolveDirective("focus");
  return withDirectives((openBlock(), createElementBlock("div", null, null, 512)), [
    [_directive_focus, _ctx.value, "current", { trim: true, deep: true }]
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <div v-focus:current.trim.deep=\"value\" />\n</template>\n"
        );
    }
}
