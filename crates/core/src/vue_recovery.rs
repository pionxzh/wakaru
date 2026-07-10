use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{anyhow, Result};
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, SyntaxContext, DUMMY_SP, GLOBALS};
use swc_core::ecma::ast::{
    ArrayLit, ArrowExpr, BlockStmtOrExpr, CallExpr, Decl, DefaultDecl, ExportDecl, ExportSpecifier,
    Expr, ExprStmt, FnDecl, Function, Ident, ImportSpecifier, Lit, MemberExpr, MemberProp, Module,
    ModuleDecl, ModuleItem, ObjectLit, ObjectPatProp, Pat, Prop, PropOrSpread, ReturnStmt, Stmt,
};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};

use crate::driver::{decompile, DecompileOptions, DecompileOutput};
use crate::js_names::is_valid_identifier_name;
#[cfg(test)]
use crate::vue_template::VueTemplateScope;
use crate::vue_template::{VueNode, VueSfc, VueTemplate};

mod attrs;
mod components;
mod context;
mod declarations;
mod directives;
mod expressions;
mod helpers;
mod imports;
mod js_refs;
mod locals;
mod nodes;
mod script;
mod script_imports;
mod selection;
mod setup_bindings;
mod slots;
mod syntax;
mod usage;

use components::VueComponentScriptImport;
use context::{
    call_callee_ident, collect_context, collect_render_context, collect_script_local_context,
    collect_setup_context, compiled_script_setup, component_name_from_init,
    component_name_from_options, component_options_from_init, infer_render_helpers,
    render_context_param, render_local_declaration_with_aliases, render_setup_context_param,
    setup_alias_renames, setup_context_param, setup_emit_param, setup_props_param,
    setup_props_param_ctxt, stmt_ident_refs,
};
use expressions::print_expr;
use helpers::{helper_name, VueHelper};
use js_refs::{collect_js_unshadowed_ident_refs, collect_js_unshadowed_read_refs};
use locals::{
    unique_script_local_binding, VueSetupLocalBinding, VueSetupRefBinding, VueSetupScriptBinding,
    VueSetupValueBinding,
};
use nodes::recover_render_root;
use script::VueSetupScriptPlan;
use script_imports::VueScriptImport;
#[cfg(test)]
use selection::{setup_local_declarations, VueSetupSelectionContext};
use syntax::{module_export_name, prop_name, string_lit, wtf8_to_string};
use usage::VueTemplateUsage;

#[derive(Default, Clone)]
struct VueRecoveryContext {
    /// `SyntaxContext` of each top-level binding (imports plus `var`/`fn`/`class`
    /// declarations), recorded after the module has been run through
    /// `resolver()`. Used to distinguish a real reference to an imported Vue
    /// helper from an inner-scope local reusing the (often minified) name, and to
    /// build `SyntaxContext`-keyed renames for alias resolution.
    top_level_binding_ctxts: HashMap<Atom, SyntaxContext>,
    /// The `SyntaxContext` `resolver()` assigns to unresolved (free/global)
    /// references — `SyntaxContext::empty().apply_mark(unresolved_mark)`. Used to
    /// stamp idents that recovery synthesizes as free references (e.g.
    /// `ContextMemberCleaner` collapsing `_ctx.foo` to a bare `foo`), so cleaned
    /// ASTs carry a consistent context instead of a colliding empty one. Defaults
    /// to `empty()` for test-constructed contexts that never run `resolver()`.
    unresolved_ctxt: SyntaxContext,
    vue_helpers: HashMap<Atom, VueHelper>,
    vue_namespaces: HashSet<Atom>,
    vue_helper_candidates: HashSet<Atom>,
    script_imports: HashMap<Atom, VueScriptImport>,
    setup_script_import_refs: HashSet<Atom>,
    object_bindings: HashMap<Atom, ObjectLit>,
    bindings: VueBindingTable,
    setup_script_bindings: Vec<VueSetupScriptBinding>,
    script_local_bindings: Vec<VueSetupLocalBinding>,
    setup_local_bindings: Vec<VueSetupLocalBinding>,
    setup_ref_script_bindings: Vec<VueSetupRefBinding>,
    provider_ref_bindings: HashMap<Atom, HashSet<Atom>>,
    imported_composable_ref_props: HashMap<Atom, HashSet<Atom>>,
    component_bindings: HashMap<Atom, String>,
    directive_bindings: HashMap<Atom, String>,
    component_options: Option<ObjectLit>,
    setup_component_options: Option<ObjectLit>,
    render_context: Option<Atom>,
    render_setup_context: Option<Atom>,
    setup_props_context: Option<Atom>,
    /// `SyntaxContext` of the setup `props` parameter (`setup_props_context`),
    /// recorded so props-ref rewriting can go through `BindingRenamer`.
    setup_props_context_ctxt: Option<SyntaxContext>,
    setup_props_aliases: HashSet<Atom>,
    /// `SyntaxContext` of each `setup_props_aliases` source, for the same reason.
    setup_props_alias_ctxts: HashMap<Atom, SyntaxContext>,
    setup_context: Option<Atom>,
    setup_emit_context: Option<Atom>,
    setup_emit_aliases: HashSet<Atom>,
    slot_bindings: HashSet<Atom>,
    render_child_list_bindings: HashMap<Atom, VueRenderChildListBinding>,
    render_slot_bindings: HashMap<Atom, VueRenderSlotBinding>,
    slot_result_normalizers: HashSet<Atom>,
    /// Template-local names already claimed by enclosing `v-for` callbacks.
    /// Nested list recovery uses this to avoid emitting shadowing fallback names.
    for_param_names: HashSet<String>,
    cm: Lrc<SourceMap>,
}

impl VueRecoveryContext {
    /// Whether `ident` refers to the imported binding of its name rather than an
    /// inner-scope local that shadows it. For names that are imports, the
    /// reference must carry the import binding's resolved `SyntaxContext`. Names
    /// that are not imports (e.g. helper aliases inferred from render structure,
    /// or an un-imported `Fragment` global) fall through to name-only matching,
    /// preserving prior behavior.
    fn resolves_to_import(&self, ident: &Ident) -> bool {
        match self.top_level_binding_ctxts.get(&ident.sym) {
            Some(ctxt) => *ctxt == ident.ctxt,
            None => true,
        }
    }
}

#[derive(Default, Clone)]
struct VueBindingTable {
    values: HashMap<Atom, VueSetupValueBinding>,
    props: HashMap<Atom, Atom>,
    aliases: HashMap<Atom, Atom>,
    /// `SyntaxContext` of each alias source (the `from` key of `aliases`),
    /// recorded so alias rewriting can go through `rename_utils::BindingRenamer`
    /// keyed on `(name, ctxt)` instead of a bespoke name-matching visitor.
    alias_ctxts: HashMap<Atom, SyntaxContext>,
    refs: HashSet<Atom>,
    composable_refs: HashSet<Atom>,
    template_refs: HashSet<Atom>,
    ref_objects: HashSet<Atom>,
}

impl VueBindingTable {
    fn ref_value_cleanup_bindings(&self, clean_assign_targets: bool) -> Vec<&str> {
        let mut bindings = self
            .refs
            .iter()
            .map(|binding| binding.as_ref())
            .collect::<Vec<_>>();
        let ref_bindings = self
            .refs
            .iter()
            .chain(self.template_refs.iter())
            .collect::<HashSet<_>>();
        if clean_assign_targets {
            bindings.extend(self.template_refs.iter().map(|binding| binding.as_ref()));
            bindings.extend(
                self.aliases
                    .iter()
                    .filter_map(|(from, to)| ref_bindings.contains(from).then_some(to.as_ref())),
            );
        }
        bindings.sort_unstable();
        bindings.dedup();
        bindings
    }
}

#[derive(Clone)]
struct VueRenderChildListBinding {
    source: VueRenderChildListSource,
}

#[derive(Clone, Copy)]
enum VueRenderChildListSource {
    SlotPartitionChildren,
}

#[derive(Clone)]
struct VueRenderSlotBinding {
    slot_name: String,
    props: Option<Box<Expr>>,
}

#[derive(Clone, Copy)]
pub(super) enum RenderSource<'a> {
    Function {
        render: &'a FnDecl,
        component_options: Option<&'a ObjectLit>,
    },
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

pub struct RecoveredVueSfc {
    pub name: Option<String>,
    pub sfc: VueSfc,
}

pub struct VueSfcDecompileOutput {
    pub output: DecompileOutput,
    pub recovered_sfc: bool,
}

pub type VueImportResolver<'a> = dyn FnMut(&str) -> Option<String> + 'a;

#[derive(Default)]
pub struct VueSfcRecoveryOptions<'a> {
    pub preferred_component_name: Option<&'a str>,
    pub import_resolver: Option<Box<VueImportResolver<'a>>>,
}

impl<'a> VueSfcRecoveryOptions<'a> {
    pub fn with_import_resolver<F>(mut self, resolver: F) -> Self
    where
        F: FnMut(&str) -> Option<String> + 'a,
    {
        self.import_resolver = Some(Box::new(resolver));
        self
    }

    pub fn with_preferred_component_name(mut self, name: &'a str) -> Self {
        self.preferred_component_name = Some(name);
        self
    }
}

pub struct VueSfcDecompileOptions<'a> {
    pub decompile: DecompileOptions,
    pub recovery: VueSfcRecoveryOptions<'a>,
}

impl VueSfcDecompileOptions<'_> {
    pub fn new(decompile: DecompileOptions) -> Self {
        Self {
            decompile,
            recovery: VueSfcRecoveryOptions::default(),
        }
    }
}

pub fn recover_vue_sfc_source_from_js(
    source: &str,
    options: VueSfcRecoveryOptions<'_>,
) -> Result<Option<String>> {
    Ok(recover_vue_sfc_from_js(source, options)?.map(|sfc| sfc.print()))
}

pub fn decompile_vue_sfc(
    source: &str,
    mut options: VueSfcDecompileOptions<'_>,
) -> Result<VueSfcDecompileOutput> {
    let filename_component_name = component_name_from_filename(&options.decompile.filename);
    let preferred_component_name = options
        .recovery
        .preferred_component_name
        .or(filename_component_name.as_deref());
    if let Some(output) = decompile_single_unpacked_vue_sfc(
        source,
        options.decompile.clone(),
        preferred_component_name,
        &mut options.recovery.import_resolver,
    )? {
        return Ok(VueSfcDecompileOutput {
            output,
            recovered_sfc: true,
        });
    }

    let mut output = decompile(source, options.decompile)?;
    if let Some(sfc) = recover_vue_sfc_from_js_inner(
        &output.code,
        &mut options.recovery.import_resolver,
        preferred_component_name,
    )?
    .map(|sfc| sfc.print())
    {
        output.code = sfc;
        output.source_map = None;
        return Ok(VueSfcDecompileOutput {
            output,
            recovered_sfc: true,
        });
    }

    Ok(VueSfcDecompileOutput {
        output,
        recovered_sfc: false,
    })
}

fn decompile_single_unpacked_vue_sfc(
    source: &str,
    mut options: DecompileOptions,
    preferred_component_name: Option<&str>,
    import_resolver: &mut Option<Box<VueImportResolver<'_>>>,
) -> Result<Option<DecompileOutput>> {
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
    let Some(sfc) =
        recover_vue_sfc_from_js_inner(&output.code, import_resolver, preferred_component_name)?
            .map(|sfc| sfc.print())
    else {
        return Ok(None);
    };
    output.code = sfc;
    output.source_map = None;
    Ok(Some(output))
}

pub fn recover_vue_sfc_from_js(
    source: &str,
    mut options: VueSfcRecoveryOptions<'_>,
) -> Result<Option<VueSfc>> {
    recover_vue_sfc_from_js_inner(
        source,
        &mut options.import_resolver,
        options.preferred_component_name,
    )
}

pub fn recover_vue_sfcs_from_js(
    source: &str,
    mut options: VueSfcRecoveryOptions<'_>,
) -> Result<Vec<RecoveredVueSfc>> {
    recover_vue_sfcs_from_js_inner(
        source,
        &mut options.import_resolver,
        options.preferred_component_name,
    )
}

pub fn is_likely_vue_sfc_source(source: &str) -> Result<bool> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_module(source, cm.clone())?;
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        let mut ctx = collect_context(&module, cm, HashMap::new(), HashMap::new());
        ctx.unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
        let Some(render) = find_render_source(&module, None) else {
            return Ok(false);
        };
        if let Some(options) = render_component_options(render) {
            ctx.setup_component_options = Some(options.clone());
        }
        let component_options = ctx
            .setup_component_options
            .as_ref()
            .or(ctx.component_options.as_ref());
        let setup_props_context = setup_props_param(render, component_options);
        let setup_props_context_ctxt = setup_props_param_ctxt(render, component_options);
        let setup_context = setup_context_param(render, component_options);
        let setup_emit_context = setup_emit_param(render, component_options);
        ctx.render_context = render_context_param(render);
        ctx.render_setup_context = render_setup_context_param(render);
        ctx.setup_props_context = setup_props_context;
        ctx.setup_props_context_ctxt = setup_props_context_ctxt;
        ctx.setup_context = setup_context;
        ctx.setup_emit_context = setup_emit_context;
        infer_render_helpers(render, &mut ctx);
        collect_setup_context(render, &mut ctx)?;
        collect_render_context(render, &mut ctx);

        Ok(render_uses_vue_helper(render, &ctx))
    })
}

fn recover_vue_sfc_from_js_inner(
    source: &str,
    import_resolver: &mut Option<Box<VueImportResolver<'_>>>,
    preferred_component_name: Option<&str>,
) -> Result<Option<VueSfc>> {
    Ok(
        recover_vue_sfcs_from_js_inner(source, import_resolver, preferred_component_name)?
            .into_iter()
            .next()
            .map(|recovered| recovered.sfc),
    )
}

fn recover_vue_sfcs_from_js_inner(
    source: &str,
    import_resolver: &mut Option<Box<VueImportResolver<'_>>>,
    preferred_component_name: Option<&str>,
) -> Result<Vec<RecoveredVueSfc>> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_module(source, cm.clone())?;
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        let imported_metadata = if let Some(resolver) = import_resolver.as_deref_mut() {
            collect_imported_vue_metadata(&module, resolver)?
        } else {
            ImportedVueMetadata::default()
        };
        let mut composable_ref_props = imported_metadata.composable_ref_props;
        composable_ref_props.extend(imports::local_composable_ref_props_from_module(&module));
        let mut ctx = collect_context(
            &module,
            cm,
            imported_metadata.component_bindings,
            composable_ref_props,
        );
        ctx.unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
        ctx.directive_bindings
            .extend(imported_metadata.directive_bindings);
        ctx.vue_helper_candidates
            .extend(imported_metadata.vue_helper_candidates);
        let renders = find_render_sources(&module, preferred_component_name);
        let mut recovered = Vec::new();
        for render in renders {
            if let Some(sfc) = recover_vue_sfc_from_render(&module, &ctx, render)? {
                recovered.push(RecoveredVueSfc {
                    name: component_name_from_render(render),
                    sfc,
                });
            }
        }
        Ok(recovered)
    })
}

fn recover_vue_sfc_from_render(
    module: &Module,
    base_ctx: &VueRecoveryContext,
    render: RenderSource<'_>,
) -> Result<Option<VueSfc>> {
    let mut ctx = base_ctx.clone();
    if let Some(options) = render_component_options(render) {
        ctx.setup_component_options = Some(options.clone());
    }
    let component_options = ctx
        .setup_component_options
        .as_ref()
        .or(ctx.component_options.as_ref());
    let setup_props_context = setup_props_param(render, component_options);
    let setup_props_context_ctxt = setup_props_param_ctxt(render, component_options);
    let setup_context = setup_context_param(render, component_options);
    let setup_emit_context = setup_emit_param(render, component_options);
    ctx.render_context = render_context_param(render);
    ctx.render_setup_context = render_setup_context_param(render);
    ctx.setup_props_context = setup_props_context;
    ctx.setup_props_context_ctxt = setup_props_context_ctxt;
    ctx.setup_context = setup_context;
    ctx.setup_emit_context = setup_emit_context;
    infer_render_helpers(render, &mut ctx);
    collect_setup_context(render, &mut ctx)?;
    collect_render_context(render, &mut ctx);
    collect_script_local_context(module, &mut ctx)?;
    if !render_uses_vue_helper(render, &ctx) {
        return Ok(None);
    }
    let Some(mut root) = recover_render_root(render, &ctx)? else {
        return Ok(None);
    };
    if !has_recovered_template_structure(&root) {
        return Ok(None);
    }

    let script_setup = setup_script(&ctx, &mut root, render)?;

    let component_options = render_component_options(render).or(ctx.component_options.as_ref());
    let script = if matches!(render, RenderSource::Function { .. })
        && compiled_script_setup(component_options).is_none()
    {
        component_options
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

fn has_recovered_template_structure(node: &VueNode) -> bool {
    match node {
        VueNode::Fragment(children) => children.iter().any(has_recovered_template_structure),
        VueNode::RawExpr(_) | VueNode::Unsupported(_) => false,
        VueNode::Element(_)
        | VueNode::If(_)
        | VueNode::For(_)
        | VueNode::Text(_)
        | VueNode::Interpolation(_)
        | VueNode::Comment(_)
        | VueNode::RawHtml(_) => true,
    }
}

#[derive(Default)]
struct ImportedVueMetadata {
    component_bindings: HashMap<Atom, String>,
    directive_bindings: HashMap<Atom, String>,
    composable_ref_props: HashMap<Atom, HashSet<Atom>>,
    vue_helper_candidates: HashSet<Atom>,
}

struct ResolvedImportMetadata {
    component_exports: HashMap<String, String>,
    directive_exports: HashMap<String, String>,
    composable_ref_props: HashMap<String, HashSet<Atom>>,
    vue_helper_exports: HashSet<String>,
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
                directive_exports: directive_exports_from_source(&resolved),
                composable_ref_props: imports::composable_ref_props_from_source(&resolved),
                vue_helper_exports: vue_helper_exports_from_source(&resolved),
            };
            export_cache.insert(source.clone(), source_metadata);
            export_cache
                .get(&source)
                .expect("inserted source export cache")
        };
        if source_metadata.component_exports.is_empty()
            && source_metadata.directive_exports.is_empty()
            && source_metadata.composable_ref_props.is_empty()
            && source_metadata.vue_helper_exports.is_empty()
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
                    if let Some(directive) = source_metadata.directive_exports.get(&imported) {
                        metadata
                            .directive_bindings
                            .insert(named.local.sym.clone(), directive.clone());
                    }
                    if let Some(ref_props) = source_metadata.composable_ref_props.get(&imported) {
                        metadata
                            .composable_ref_props
                            .insert(named.local.sym.clone(), ref_props.clone());
                    }
                    if source_metadata.vue_helper_exports.contains(&imported) {
                        metadata
                            .vue_helper_candidates
                            .insert(named.local.sym.clone());
                    }
                }
                ImportSpecifier::Default(default) => {
                    if let Some(component) = source_metadata.component_exports.get("default") {
                        metadata
                            .component_bindings
                            .insert(default.local.sym.clone(), component.clone());
                    }
                    if let Some(directive) = source_metadata.directive_exports.get("default") {
                        metadata
                            .directive_bindings
                            .insert(default.local.sym.clone(), directive.clone());
                    }
                    if let Some(ref_props) = source_metadata.composable_ref_props.get("default") {
                        metadata
                            .composable_ref_props
                            .insert(default.local.sym.clone(), ref_props.clone());
                    }
                    if source_metadata.vue_helper_exports.contains("default") {
                        metadata
                            .vue_helper_candidates
                            .insert(default.local.sym.clone());
                    }
                }
                ImportSpecifier::Namespace(_) => {}
            }
        }
    }

    Ok(metadata)
}

fn directive_exports_from_source(source: &str) -> HashMap<String, String> {
    let cm: Lrc<SourceMap> = Default::default();
    let Ok(module) = parse_module(source, cm) else {
        return HashMap::new();
    };
    let local_directives = local_directive_bindings(&module);
    let mut exported = HashMap::new();

    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                if let Decl::Var(var) = &export.decl {
                    for decl in &var.decls {
                        let Pat::Ident(binding) = &decl.name else {
                            continue;
                        };
                        let Some(init) = decl.init.as_deref() else {
                            continue;
                        };
                        if let Some(name) = directive_name_from_init(init) {
                            exported.insert(binding.id.sym.to_string(), name);
                        }
                    }
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(default)) => {
                if let Some(name) = directive_name_from_init(default.expr.as_ref()) {
                    exported.insert("default".to_string(), name);
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export)) if export.src.is_none() => {
                for specifier in &export.specifiers {
                    let ExportSpecifier::Named(named) = specifier else {
                        continue;
                    };
                    let local = module_export_name(&named.orig);
                    let Some(name) = local_directives.get(&local) else {
                        continue;
                    };
                    let exported_name = named
                        .exported
                        .as_ref()
                        .map(module_export_name)
                        .unwrap_or(local);
                    exported.insert(exported_name, name.clone());
                }
            }
            _ => {}
        }
    }

    exported
}

fn local_directive_bindings(module: &Module) -> HashMap<String, String> {
    let mut bindings = HashMap::new();
    for item in &module.body {
        let var = match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => var,
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => match &export.decl {
                Decl::Var(var) => var,
                _ => continue,
            },
            _ => continue,
        };
        for decl in &var.decls {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            if let Some(name) = directive_name_from_init(init) {
                bindings.insert(binding.id.sym.to_string(), name);
            }
        }
    }
    bindings
}

fn directive_name_from_init(expr: &Expr) -> Option<String> {
    let Expr::Object(object) = context::unwrap_paren_expr(expr) else {
        return None;
    };
    object.props.iter().find_map(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            return None;
        };
        (prop_name(&key_value.key).as_deref() == Some("name"))
            .then(|| string_lit(key_value.value.as_ref()))
            .flatten()
    })
}

fn vue_helper_exports_from_source(source: &str) -> HashSet<String> {
    let cm: Lrc<SourceMap> = Default::default();
    let Ok(module) = parse_module(source, cm) else {
        return HashSet::new();
    };
    let wrapper_exports = exported_vue_helper_wrapper_names(&module);
    if !wrapper_exports.is_empty() {
        return wrapper_exports;
    }
    if !is_likely_vue_runtime_module(&module) {
        return HashSet::new();
    }
    exported_binding_names(&module)
}

fn exported_vue_helper_wrapper_names(module: &Module) -> HashSet<String> {
    let imported = imported_binding_names(module);
    if imported.is_empty() {
        return HashSet::new();
    }

    let mut local_wrappers = HashSet::new();
    let mut exported = HashSet::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function)))
                if is_vue_helper_wrapper_function(&function.function, &imported) =>
            {
                local_wrappers.insert(function.ident.sym.to_string());
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                decl: Decl::Fn(function),
                ..
            })) if is_vue_helper_wrapper_function(&function.function, &imported) => {
                exported.insert(function.ident.sym.to_string());
            }
            _ => {}
        }
    }

    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export)) = item else {
            continue;
        };
        if export.src.is_some() {
            continue;
        }
        for specifier in &export.specifiers {
            let ExportSpecifier::Named(named) = specifier else {
                continue;
            };
            let local = module_export_name(&named.orig);
            if local_wrappers.contains(&local) {
                let exported_name = named
                    .exported
                    .as_ref()
                    .map(module_export_name)
                    .unwrap_or(local);
                exported.insert(exported_name);
            }
        }
    }

    exported
}

fn imported_binding_names(module: &Module) -> HashSet<Atom> {
    let mut imported = HashSet::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Named(named) => {
                    imported.insert(named.local.sym.clone());
                }
                ImportSpecifier::Default(default) => {
                    imported.insert(default.local.sym.clone());
                }
                ImportSpecifier::Namespace(namespace) => {
                    imported.insert(namespace.local.sym.clone());
                }
            }
        }
    }
    imported
}

fn is_vue_helper_wrapper_function(function: &Function, imported: &HashSet<Atom>) -> bool {
    let Some(body) = &function.body else {
        return false;
    };
    body.stmts.iter().any(|stmt| {
        let Stmt::Return(ReturnStmt {
            arg: Some(expr), ..
        }) = stmt
        else {
            return false;
        };
        contains_imported_block_helper_call(expr.as_ref(), imported)
    })
}

fn contains_imported_block_helper_call(expr: &Expr, imported: &HashSet<Atom>) -> bool {
    match context::unwrap_paren_expr(expr) {
        Expr::Call(call) => {
            is_imported_block_helper_call(call, imported)
                || call
                    .args
                    .iter()
                    .any(|arg| contains_imported_block_helper_call(arg.expr.as_ref(), imported))
        }
        _ => false,
    }
}

fn is_imported_block_helper_call(call: &CallExpr, imported: &HashSet<Atom>) -> bool {
    call_callee_ident(call).is_some_and(|callee| imported.contains(&callee.sym))
        && call.args.last().is_some_and(
            |arg| matches!(arg.expr.as_ref(), Expr::Lit(Lit::Bool(bool)) if bool.value),
        )
}

fn is_likely_vue_runtime_module(module: &Module) -> bool {
    let mut detector = VueRuntimeMarkerDetector { found: false };
    module.visit_with(&mut detector);
    detector.found
}

struct VueRuntimeMarkerDetector {
    found: bool,
}

impl Visit for VueRuntimeMarkerDetector {
    fn visit_ident(&mut self, ident: &Ident) {
        if is_vue_runtime_marker_name(ident.sym.as_ref()) {
            self.found = true;
        }
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        match &member.prop {
            MemberProp::Ident(ident) if is_vue_runtime_marker_name(ident.sym.as_ref()) => {
                self.found = true;
            }
            MemberProp::Computed(computed)
                if string_lit_expr_is_vue_runtime_marker(computed.expr.as_ref()) =>
            {
                self.found = true;
            }
            _ => {}
        }
        member.visit_children_with(self);
    }

    fn visit_lit(&mut self, lit: &Lit) {
        if lit_is_vue_runtime_marker(lit) {
            self.found = true;
        }
    }
}

fn lit_is_vue_runtime_marker(lit: &Lit) -> bool {
    let Lit::Str(str) = lit else {
        return false;
    };
    is_vue_runtime_marker_name(&wtf8_to_string(&str.value))
}

fn string_lit_expr_is_vue_runtime_marker(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(lit) if lit_is_vue_runtime_marker(lit))
}

fn is_vue_runtime_marker_name(name: &str) -> bool {
    matches!(
        name,
        "__v_isVNode"
            | "__v_isRef"
            | "__vccOpts"
            | "shapeFlag"
            | "patchFlag"
            | "dynamicChildren"
            | "slotScopeIds"
            | "v-fgt"
            | "_vte"
    )
}

fn exported_binding_names(module: &Module) -> HashSet<String> {
    let mut names = HashSet::new();
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => match &export.decl {
                Decl::Fn(function) => {
                    names.insert(function.ident.sym.to_string());
                }
                Decl::Class(class) => {
                    names.insert(class.ident.sym.to_string());
                }
                Decl::Var(var) => {
                    for decl in &var.decls {
                        collect_exported_pat_names(&decl.name, &mut names);
                    }
                }
                _ => {}
            },
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(_)) => {
                names.insert("default".to_string());
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(_)) => {
                names.insert("default".to_string());
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export)) if export.src.is_none() => {
                for specifier in &export.specifiers {
                    let ExportSpecifier::Named(named) = specifier else {
                        continue;
                    };
                    let exported = named
                        .exported
                        .as_ref()
                        .map(module_export_name)
                        .unwrap_or_else(|| module_export_name(&named.orig));
                    names.insert(exported);
                }
            }
            _ => {}
        }
    }
    names
}

fn collect_exported_pat_names(pat: &Pat, names: &mut HashSet<String>) {
    match pat {
        Pat::Ident(binding) => {
            names.insert(binding.id.sym.to_string());
        }
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_exported_pat_names(elem, names);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(key_value) => {
                        collect_exported_pat_names(key_value.value.as_ref(), names);
                    }
                    ObjectPatProp::Assign(assign) => {
                        names.insert(assign.key.sym.to_string());
                    }
                    ObjectPatProp::Rest(rest) => {
                        collect_exported_pat_names(rest.arg.as_ref(), names);
                    }
                }
            }
        }
        Pat::Rest(rest) => collect_exported_pat_names(rest.arg.as_ref(), names),
        Pat::Assign(assign) => collect_exported_pat_names(assign.left.as_ref(), names),
        Pat::Expr(_) | Pat::Invalid(_) => {}
    }
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
    find_render_sources(module, preferred_component_name)
        .into_iter()
        .next()
}

fn find_render_sources<'a>(
    module: &'a Module,
    preferred_component_name: Option<&str>,
) -> Vec<RenderSource<'a>> {
    if let Some(preferred_component_name) = preferred_component_name {
        if let Some(render) =
            setup_render_source_for_component_name(module, preferred_component_name)
        {
            return vec![render];
        }
        let scoped_renders = component_scope_render_sources(module);
        if !scoped_renders.is_empty() {
            return scoped_renders;
        }
        return find_render_fn(module)
            .map(|render| RenderSource::Function {
                render,
                component_options: None,
            })
            .into_iter()
            .collect();
    }

    let mut sources = Vec::new();
    for render in component_scope_render_sources(module) {
        push_render_source(&mut sources, render);
    }
    if let Some(render) = find_render_fn(module).map(|render| RenderSource::Function {
        render,
        component_options: None,
    }) {
        push_render_source(&mut sources, render);
    }
    for render in setup_render_sources(module) {
        push_render_source(&mut sources, render);
    }
    sources
}

fn push_render_source<'a>(sources: &mut Vec<RenderSource<'a>>, render: RenderSource<'a>) {
    let key = render_source_key(render);
    if sources
        .iter()
        .any(|existing| render_source_key(*existing) == key)
    {
        return;
    }
    sources.push(render);
}

fn render_source_key(render: RenderSource<'_>) -> usize {
    match render {
        RenderSource::Function { render, .. } => render as *const FnDecl as usize,
        RenderSource::SetupArrow { render, .. } => render as *const ArrowExpr as usize,
    }
}

fn render_component_options(render: RenderSource<'_>) -> Option<&ObjectLit> {
    match render {
        RenderSource::Function {
            component_options, ..
        } => component_options,
        RenderSource::SetupArrow {
            component_options, ..
        } => component_options,
    }
}

fn component_name_from_render(render: RenderSource<'_>) -> Option<String> {
    render_component_options(render).and_then(component_name_from_options)
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

fn setup_render_sources(module: &Module) -> Vec<RenderSource<'_>> {
    let mut sources = Vec::new();
    if let Some(render) = direct_exported_setup_render_source(module) {
        push_render_source(&mut sources, render);
    }

    for local in preferred_setup_export_names(module) {
        if let Some(render) = setup_render_source_from_binding(module, &local) {
            push_render_source(&mut sources, render);
        }
    }

    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export)) => {
                if let Some(render) = setup_render_source_from_expr(export.expr.as_ref()) {
                    push_render_source(&mut sources, render);
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                if let Decl::Var(var) = &export.decl {
                    for decl in &var.decls {
                        let Some(init) = decl.init.as_deref() else {
                            continue;
                        };
                        if let Some(render) = setup_render_source_from_expr(init) {
                            push_render_source(&mut sources, render);
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
                        push_render_source(&mut sources, render);
                    }
                }
            }
            _ => {}
        }
    }
    sources
}

fn component_scope_render_sources<'a>(module: &'a Module) -> Vec<RenderSource<'a>> {
    let functions = local_function_decls(module);
    if functions.is_empty() {
        return Vec::new();
    }
    let component_options = local_component_options(module);
    let mut sources = Vec::new();

    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export)) => {
                if let Some(render) = component_scope_render_source_from_expr(
                    export.expr.as_ref(),
                    &functions,
                    &component_options,
                ) {
                    push_render_source(&mut sources, render);
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                let Decl::Var(var) = &export.decl else {
                    continue;
                };
                for decl in &var.decls {
                    let Some(init) = decl.init.as_deref() else {
                        continue;
                    };
                    if let Some(render) = component_scope_render_source_from_expr(
                        init,
                        &functions,
                        &component_options,
                    ) {
                        push_render_source(&mut sources, render);
                    }
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    let Some(init) = decl.init.as_deref() else {
                        continue;
                    };
                    if let Some(render) = component_scope_render_source_from_expr(
                        init,
                        &functions,
                        &component_options,
                    ) {
                        push_render_source(&mut sources, render);
                    }
                }
            }
            _ => {}
        }
    }

    sources
}

fn local_function_decls(module: &Module) -> HashMap<Atom, &FnDecl> {
    let mut functions = HashMap::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function)))
            | ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                decl: Decl::Fn(function),
                ..
            })) => {
                functions.insert(function.ident.sym.clone(), function);
            }
            _ => {}
        }
    }
    functions
}

fn local_component_options(module: &Module) -> HashMap<Atom, &ObjectLit> {
    let mut options = HashMap::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var)))
            | ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                decl: Decl::Var(var),
                ..
            })) => {
                for decl in &var.decls {
                    let Pat::Ident(binding) = &decl.name else {
                        continue;
                    };
                    let Some(init) = decl.init.as_deref() else {
                        continue;
                    };
                    if let Some(object) = component_options_from_init(init) {
                        options.insert(binding.id.sym.clone(), object);
                    }
                }
            }
            _ => {}
        }
    }
    options
}

fn component_scope_render_source_from_expr<'a>(
    expr: &'a Expr,
    functions: &HashMap<Atom, &'a FnDecl>,
    component_options: &HashMap<Atom, &'a ObjectLit>,
) -> Option<RenderSource<'a>> {
    match expr {
        Expr::Paren(paren) => component_scope_render_source_from_expr(
            paren.expr.as_ref(),
            functions,
            component_options,
        ),
        Expr::Call(call) => {
            if let Some(render_name) = component_scope_render_function_name(call) {
                let render = *functions.get(render_name)?;
                let options = call.args.first().and_then(|arg| {
                    component_options_from_component_expr(arg.expr.as_ref(), component_options)
                });
                return Some(RenderSource::Function {
                    render,
                    component_options: options,
                });
            }

            call.args.first().and_then(|arg| {
                component_scope_render_source_from_expr(
                    arg.expr.as_ref(),
                    functions,
                    component_options,
                )
            })
        }
        _ => None,
    }
}

fn component_options_from_component_expr<'a>(
    expr: &'a Expr,
    local_options: &HashMap<Atom, &'a ObjectLit>,
) -> Option<&'a ObjectLit> {
    match expr {
        Expr::Paren(paren) => {
            component_options_from_component_expr(paren.expr.as_ref(), local_options)
        }
        Expr::Ident(ident) => local_options.get(&ident.sym).copied(),
        expr => component_options_from_init(expr),
    }
}

fn component_scope_render_function_name(call: &CallExpr) -> Option<&Atom> {
    let attrs = call.args.get(1)?;
    render_function_name_from_scope_attrs(attrs.expr.as_ref())
}

fn render_function_name_from_scope_attrs(expr: &Expr) -> Option<&Atom> {
    let Expr::Array(attrs) = unwrap_paren_array_expr(expr) else {
        return None;
    };
    attrs.elems.iter().flatten().find_map(|elem| {
        let Expr::Array(tuple) = unwrap_paren_array_expr(elem.expr.as_ref()) else {
            return None;
        };
        render_function_name_from_scope_tuple(tuple)
    })
}

fn render_function_name_from_scope_tuple(tuple: &ArrayLit) -> Option<&Atom> {
    let key = tuple.elems.first()?.as_ref()?;
    if string_lit(key.expr.as_ref()).as_deref() != Some("render") {
        return None;
    }
    let value = tuple.elems.get(1)?.as_ref()?;
    match value.expr.as_ref() {
        Expr::Ident(ident) => Some(&ident.sym),
        Expr::Paren(paren) => match paren.expr.as_ref() {
            Expr::Ident(ident) => Some(&ident.sym),
            _ => None,
        },
        _ => None,
    }
}

fn unwrap_paren_array_expr(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => unwrap_paren_array_expr(paren.expr.as_ref()),
        _ => expr,
    }
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
        RenderSource::Function { render, .. } => RenderSource::Function {
            render,
            component_options: Some(component_options),
        },
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
    if ctx.vue_helpers.is_empty() && ctx.vue_namespaces.is_empty() {
        return false;
    }

    struct Finder<'a> {
        ctx: &'a VueRecoveryContext,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if helper_name(&call.callee, self.ctx).is_some() {
                self.found = true;
                return;
            }

            call.visit_children_with(self);
        }
    }

    let mut finder = Finder { ctx, found: false };
    match render {
        RenderSource::Function { render, .. } => {
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
    let mut options = options.clone();
    options
        .props
        .retain(|prop| component_script_prop_name(prop).as_deref() != Some("render"));
    if options.props.is_empty() {
        return Ok(None);
    }
    let namespace_imports = component_script_vue_namespace_imports(&options, ctx);
    let printed = print_expr(&Expr::Object(options), ctx)?;
    if namespace_imports.is_empty() {
        Ok(Some(format!("export default {printed}")))
    } else {
        Ok(Some(format!(
            "{}\n\nexport default {printed}",
            namespace_imports.join("\n")
        )))
    }
}

fn component_script_vue_namespace_imports(
    options: &ObjectLit,
    ctx: &VueRecoveryContext,
) -> Vec<String> {
    if ctx.vue_namespaces.is_empty() {
        return Vec::new();
    }

    let refs = stmt_ident_refs(&Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(Expr::Object(options.clone())),
    }));
    let mut namespaces = refs
        .into_iter()
        .filter(|reference| ctx.vue_namespaces.contains(reference))
        .filter(|reference| is_valid_identifier_name(reference.as_ref()))
        .map(|reference| reference.to_string())
        .collect::<Vec<_>>();
    namespaces.sort();
    namespaces.dedup();
    namespaces
        .into_iter()
        .map(|namespace| format!("import * as {namespace} from \"vue\";"))
        .collect()
}

fn component_script_prop_name(prop: &PropOrSpread) -> Option<String> {
    let PropOrSpread::Prop(prop) = prop else {
        return None;
    };
    match prop.as_ref() {
        Prop::Shorthand(ident) => Some(ident.sym.to_string()),
        Prop::KeyValue(key_value) => prop_name(&key_value.key),
        Prop::Assign(assign) => Some(assign.key.sym.to_string()),
        Prop::Getter(getter) => prop_name(&getter.key),
        Prop::Setter(setter) => prop_name(&setter.key),
        Prop::Method(method) => prop_name(&method.key),
    }
}

fn setup_script(
    ctx: &VueRecoveryContext,
    root: &mut VueNode,
    render: RenderSource<'_>,
) -> Result<Option<String>> {
    let plan = VueSetupScriptPlan::build(ctx, root, render)?;
    if plan.is_empty() {
        return Ok(None);
    }

    Ok(Some(plan.render(ctx)))
}

fn render_setup_local_declarations(
    ctx: &VueRecoveryContext,
    local_declarations: Vec<&VueSetupLocalBinding>,
    prop_bindings: &[(String, String)],
    props_declaration: Option<&(String, String)>,
    emit_declaration: Option<&(String, String)>,
    ref_declarations: &[(String, String, String)],
    component_imports: &[VueComponentScriptImport],
) -> Result<Vec<VueSetupLocalBinding>> {
    let aliases = script_local_binding_aliases(
        ctx,
        &local_declarations,
        prop_bindings,
        props_declaration,
        emit_declaration,
        ref_declarations,
        component_imports,
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
    component_imports: &[VueComponentScriptImport],
) -> HashMap<Atom, Atom> {
    let mut used = HashSet::new();
    used.extend(ctx.script_imports.keys().cloned());
    used.extend(component_imports.iter().map(|import| import.local.clone()));
    used.extend(
        ctx.setup_script_bindings
            .iter()
            .map(|binding| binding.binding.clone()),
    );
    used.extend(
        ctx.setup_ref_script_bindings
            .iter()
            .map(|binding| binding.binding.clone()),
    );
    used.extend(ctx.bindings.values.keys().cloned());
    used.extend(ctx.bindings.aliases.keys().cloned());
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

#[cfg(test)]
mod tests;
