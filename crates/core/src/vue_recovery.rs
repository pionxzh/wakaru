use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{anyhow, Result};
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, SourceMap};
use swc_core::ecma::ast::{
    ArrowExpr, BlockStmtOrExpr, CallExpr, Callee, Decl, DefaultDecl, ExportDecl, ExportSpecifier,
    Expr, FnDecl, Ident, ImportSpecifier, Module, ModuleDecl, ModuleItem, ObjectLit, Pat, Prop,
    PropOrSpread, ReturnStmt, Stmt,
};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::driver::{decompile, DecompileOptions, DecompileOutput};
use crate::vue_template::{VueSfc, VueTemplate};

mod attrs;
mod context;
mod directives;
mod expressions;
mod helpers;
mod nodes;
mod syntax;

use context::{
    collect_context, collect_render_context, collect_setup_context, component_name_from_init,
    infer_render_helpers, render_context_param, setup_props_param,
};
use expressions::print_expr;
use helpers::VueHelper;
use nodes::recover_render_root;
use syntax::{module_export_name, prop_name};

#[derive(Default, Clone)]
struct VueRecoveryContext {
    vue_helpers: HashMap<Atom, VueHelper>,
    vue_helper_candidates: HashSet<Atom>,
    object_bindings: HashMap<Atom, ObjectLit>,
    setup_value_bindings: HashMap<Atom, String>,
    component_bindings: HashMap<Atom, String>,
    directive_bindings: HashMap<Atom, String>,
    component_options: Option<ObjectLit>,
    render_context: Option<Atom>,
    setup_props_context: Option<Atom>,
    setup_props_aliases: HashSet<Atom>,
    cm: Lrc<SourceMap>,
}

#[derive(Clone, Copy)]
pub(super) enum RenderSource<'a> {
    Function(&'a FnDecl),
    SetupArrow {
        render: &'a ArrowExpr,
        setup_stmts: &'a [Stmt],
        setup_props: Option<&'a Ident>,
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
    let component_bindings = import_resolver
        .map(|resolver| collect_imported_component_bindings(&module, resolver))
        .transpose()?
        .unwrap_or_default();
    let mut ctx = collect_context(&module, cm, component_bindings);
    let Some(render) = find_render_source(&module, preferred_component_name) else {
        return Ok(None);
    };
    ctx.render_context = render_context_param(render);
    ctx.setup_props_context = setup_props_param(render);
    infer_render_helpers(render, &mut ctx);
    collect_setup_context(render, &mut ctx)?;
    collect_render_context(render, &mut ctx);
    if !render_uses_vue_helper(render, &ctx) {
        return Ok(None);
    }
    let Some(root) = recover_render_root(render, &ctx)? else {
        return Ok(None);
    };

    let script = ctx
        .component_options
        .as_ref()
        .and_then(|options| component_script(options, &ctx).transpose())
        .transpose()?;

    Ok(Some(VueSfc {
        script,
        template: VueTemplate {
            children: vec![root],
        },
    }))
}

fn collect_imported_component_bindings(
    module: &Module,
    resolve_import: &mut dyn FnMut(&str) -> Option<String>,
) -> Result<HashMap<Atom, String>> {
    let mut component_bindings = HashMap::new();
    let mut export_cache: HashMap<String, HashMap<String, String>> = HashMap::new();

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

        let source_exports = if let Some(exports) = export_cache.get(&source) {
            exports
        } else {
            let exports = resolve_import(&source)
                .and_then(|source| component_exports_from_source(&source).ok())
                .unwrap_or_default();
            export_cache.insert(source.clone(), exports);
            export_cache
                .get(&source)
                .expect("inserted source export cache")
        };
        if source_exports.is_empty() {
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
                    if let Some(component) = source_exports.get(&imported) {
                        component_bindings.insert(named.local.sym.clone(), component.clone());
                    }
                }
                ImportSpecifier::Default(default) => {
                    if let Some(component) = source_exports.get("default") {
                        component_bindings.insert(default.local.sym.clone(), component.clone());
                    }
                }
                ImportSpecifier::Namespace(_) => {}
            }
        }
    }

    Ok(component_bindings)
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

fn parse_module(source: &str, cm: Lrc<SourceMap>) -> Result<Module> {
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
                    });
                }
            }
            Prop::KeyValue(key_value) if prop_name(&key_value.key).as_deref() == Some("setup") => {
                if let Some(render) = setup_return_source_from_expr(key_value.value.as_ref()) {
                    return Some(render);
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
                })
            }
            BlockStmtOrExpr::Expr(expr) => {
                arrow_expr(expr.as_ref()).map(|render| RenderSource::SetupArrow {
                    render,
                    setup_stmts: &[],
                    setup_props: arrow.params.first().and_then(pat_binding_ident),
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
            })
        }),
        _ => None,
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

#[cfg(test)]
mod tests {
    use super::*;

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
            "<template>\n  <Notice v-if=\"isLoaded\" :key=\"0\">\n    <template v-slot:default>\n      <span v-html=\"message\" />\n    </template>\n  </Notice>\n</template>\n"
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
            "<template>\n  <VButton flat=\"\" />\n</template>\n"
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
            "<template>\n  <VButton flat=\"\" />\n</template>\n"
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
            "<template>\n  <p v-if=\"status === 'loading'\" :key=\"0\">Loading</p>\n  <p v-else-if=\"status === 'error'\" :key=\"1\">{{ error }}</p>\n  <p v-else :key=\"2\">Ready</p>\n</template>\n"
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
            "<template>\n  <p v-if=\"status === 'loading'\" :key=\"0\">Loading</p>\n  <p v-else-if=\"status === 'error'\" :key=\"1\">{{ error }}</p>\n  <p v-else :key=\"2\">Ready</p>\n</template>\n"
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
