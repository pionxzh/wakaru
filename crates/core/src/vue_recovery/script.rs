use anyhow::Result;

use crate::js_names::is_valid_identifier_name;

use super::locals::VueSetupLocalBinding;
use super::selection::setup_local_declarations;
use super::{
    component_prop_names, component_props_source, component_script_imports,
    props_binding_reserved_names, referenced_script_imports, render_setup_local_declarations,
    script_setup_declarations, script_setup_declared_bindings, setup_emit_declaration,
    setup_prop_bindings, setup_props_script_binding, setup_ref_declarations, RenderSource, VueNode,
    VueRecoveryContext, VueScriptImport, VueScriptSetupDeclaration, VueTemplateUsage,
};

pub(super) struct VueSetupScriptPlan {
    pub(super) valid_prop_names: Vec<String>,
    pub(super) prop_bindings: Vec<(String, String)>,
    pub(super) props_declaration: Option<(String, String)>,
    pub(super) emit_declaration: Option<(String, String)>,
    pub(super) ref_declarations: Vec<(String, String, String)>,
    pub(super) local_declarations: Vec<VueSetupLocalBinding>,
    pub(super) scheduled_declarations: Vec<VueScriptSetupDeclaration>,
    pub(super) script_imports: Vec<String>,
}

impl VueSetupScriptPlan {
    pub(super) fn build(
        ctx: &VueRecoveryContext,
        root: &mut VueNode,
        render: RenderSource<'_>,
    ) -> Result<Self> {
        let template_usage = VueTemplateUsage::new(root);
        let ref_declarations = setup_ref_declarations(ctx, &template_usage, render);
        let selected_local_declarations = setup_local_declarations(ctx, &template_usage);
        let emit_declaration =
            setup_emit_declaration(ctx, &template_usage, &selected_local_declarations)?;
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
        let component_imports = component_script_imports(
            ctx,
            root,
            &prop_bindings,
            props_declaration.as_ref(),
            emit_declaration.as_ref(),
            &ref_declarations,
            &selected_local_declarations,
        );
        let local_declarations = render_setup_local_declarations(
            ctx,
            selected_local_declarations,
            &prop_bindings,
            props_declaration.as_ref(),
            emit_declaration.as_ref(),
            &ref_declarations,
            &component_imports,
        )?;
        let scheduled_declarations = script_setup_declarations(ctx, &local_declarations);
        let declared_bindings = script_setup_declared_bindings(
            ctx,
            &prop_bindings,
            props_declaration.as_ref(),
            emit_declaration.as_ref(),
            &ref_declarations,
            &local_declarations,
        );
        let script_imports = referenced_script_imports(
            ctx,
            &template_usage,
            &declared_bindings,
            &local_declarations,
            &component_imports,
        );

        Ok(Self {
            valid_prop_names,
            prop_bindings,
            props_declaration,
            emit_declaration,
            ref_declarations,
            local_declarations,
            scheduled_declarations,
            script_imports,
        })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.scheduled_declarations.is_empty()
            && self.ref_declarations.is_empty()
            && self.props_declaration.is_none()
            && self.emit_declaration.is_none()
            && self.script_imports.is_empty()
    }

    pub(super) fn render(&self, ctx: &VueRecoveryContext) -> String {
        let mut body = String::new();

        if let Some((binding, props_source)) = &self.props_declaration {
            body.push_str("const ");
            body.push_str(binding);
            body.push_str(" = defineProps(");
            body.push_str(props_source);
            body.push_str(");\n");
            if !self.valid_prop_names.is_empty() {
                body.push_str("const { ");
                body.push_str(&format_prop_destructure_bindings(&self.prop_bindings));
                body.push_str(" } = ");
                body.push_str(binding);
                body.push_str(";\n");
            }
            body.push('\n');
        }

        if let Some((binding, emits_source)) = &self.emit_declaration {
            body.push_str("const ");
            body.push_str(binding);
            body.push_str(" = defineEmits(");
            body.push_str(emits_source);
            body.push_str(");\n");
            if !self.ref_declarations.is_empty() || !self.scheduled_declarations.is_empty() {
                body.push('\n');
            }
        }

        for (binding, expr, _) in &self.ref_declarations {
            body.push_str("const ");
            body.push_str(binding);
            body.push_str(" = ");
            body.push_str(expr.trim());
            body.push_str(";\n");
        }
        if !self.ref_declarations.is_empty() && !self.scheduled_declarations.is_empty() {
            body.push('\n');
        }

        push_script_setup_declarations(&mut body, &self.scheduled_declarations);

        let mut out = String::new();
        if let Some(vue_import) =
            vue_script_import_line(ctx, &self.ref_declarations, &self.local_declarations)
        {
            out.push_str(&vue_import);
            out.push('\n');
        }
        for import in &self.script_imports {
            out.push_str(import);
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&body);
        out
    }
}

fn push_script_setup_declarations(body: &mut String, declarations: &[VueScriptSetupDeclaration]) {
    for (index, declaration) in declarations.iter().enumerate() {
        if index > 0 && declarations[index - 1].kind != declaration.kind {
            body.push('\n');
        }
        body.push_str(declaration.source.trim());
        body.push('\n');
    }
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
        for local in super::stmt_ident_refs(&declaration.stmt) {
            if ctx.script_imports.contains_key(&local) {
                continue;
            }
            let Some(helper) = ctx.vue_helpers.get(&local) else {
                continue;
            };
            imports.push((
                super::vue_helper_import_name(helper).to_string(),
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

pub(super) fn script_import_line(local: &str, import: &VueScriptImport) -> String {
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

fn quote_js_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
