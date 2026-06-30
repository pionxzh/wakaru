use std::collections::HashSet;

use swc_core::atoms::Atom;

use super::components::VueComponentScriptImport;
use super::locals::{setup_script_binding_refs, VueSetupLocalBinding};
use super::{VueRecoveryContext, VueTemplateUsage};

#[derive(Clone)]
pub(super) enum VueScriptImport {
    Named { source: String, imported: String },
    Default { source: String },
    Namespace { source: String },
}

pub(super) fn referenced_script_imports(
    ctx: &VueRecoveryContext,
    template_usage: &VueTemplateUsage,
    declared_bindings: &HashSet<Atom>,
    local_declarations: &[VueSetupLocalBinding],
    component_imports: &[VueComponentScriptImport],
) -> Vec<String> {
    let mut refs = ctx.setup_script_import_refs.clone();
    refs.extend(setup_script_binding_refs(ctx));
    for declaration in local_declarations {
        refs.extend(declaration.import_refs.iter().cloned());
    }
    refs.extend(
        template_usage
            .read_refs
            .iter()
            .filter(|&local| !declared_bindings.contains(local))
            .cloned(),
    );

    let mut imports = component_imports
        .iter()
        .filter_map(|component_import| {
            ctx.script_imports
                .get(&component_import.import_ref)
                .map(|import| script_import_line(component_import.local.as_ref(), import))
        })
        .collect::<Vec<_>>();
    imports.extend(
        refs.iter()
            .filter(|local| local.as_ref() != "$")
            .filter(|local| !declared_bindings.contains(*local))
            .filter_map(|local| ctx.script_imports.get(local).map(|import| (local, import)))
            .map(|(local, import)| script_import_line(local.as_ref(), import)),
    );
    imports.sort();
    imports.dedup();
    imports
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

fn quote_js_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
