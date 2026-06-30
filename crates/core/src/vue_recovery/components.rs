use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;

use crate::js_names::is_valid_identifier_name;
use crate::vue_template::VueNode;

use super::locals::{unique_script_local_binding, VueSetupLocalBinding};
use super::VueRecoveryContext;

#[derive(Clone)]
pub(super) struct VueComponentScriptImport {
    pub(super) import_ref: Atom,
    pub(super) local: Atom,
}

pub(super) fn component_script_imports(
    ctx: &VueRecoveryContext,
    root: &mut VueNode,
    prop_bindings: &[(String, String)],
    props_declaration: Option<&(String, String)>,
    emit_declaration: Option<&(String, String)>,
    ref_declarations: &[(String, String, String)],
    selected_local_declarations: &[&VueSetupLocalBinding],
) -> Vec<VueComponentScriptImport> {
    let mut refs = Vec::new();
    collect_component_script_import_refs(root, &mut refs);
    if refs.is_empty() {
        return Vec::new();
    }

    let import_refs = refs
        .iter()
        .map(|(import_ref, _)| import_ref.clone())
        .collect::<HashSet<_>>();
    let mut used = component_import_reserved_bindings(
        ctx,
        prop_bindings,
        props_declaration,
        emit_declaration,
        ref_declarations,
        selected_local_declarations,
    );
    used.extend(
        ctx.script_imports
            .keys()
            .filter(|local| !import_refs.contains(*local))
            .cloned(),
    );

    let mut aliases = HashMap::new();
    let mut imports = Vec::new();
    for (import_ref, tag) in refs {
        if aliases.contains_key(&import_ref) || !ctx.script_imports.contains_key(&import_ref) {
            continue;
        }
        let mut local = if is_valid_identifier_name(tag.as_ref()) {
            tag
        } else {
            import_ref.clone()
        };
        if used.contains(&local) {
            local = unique_script_local_binding(&local, &mut used);
        } else {
            used.insert(local.clone());
        }
        aliases.insert(import_ref.clone(), local.clone());
        imports.push(VueComponentScriptImport { import_ref, local });
    }

    rename_component_import_tags(root, &aliases);
    imports
}

fn component_import_reserved_bindings(
    ctx: &VueRecoveryContext,
    prop_bindings: &[(String, String)],
    props_declaration: Option<&(String, String)>,
    emit_declaration: Option<&(String, String)>,
    ref_declarations: &[(String, String, String)],
    selected_local_declarations: &[&VueSetupLocalBinding],
) -> HashSet<Atom> {
    let mut reserved = HashSet::new();
    if let Some((binding, _)) = props_declaration {
        reserved.insert(Atom::from(binding.clone()));
    }
    reserved.extend(
        prop_bindings
            .iter()
            .map(|(_, binding)| Atom::from(binding.clone())),
    );
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
            .map(|binding| binding.binding.clone()),
    );
    reserved.extend(
        selected_local_declarations
            .iter()
            .flat_map(|declaration| declaration.emitted_bindings.iter().cloned()),
    );
    reserved
}

fn collect_component_script_import_refs(node: &VueNode, refs: &mut Vec<(Atom, Atom)>) {
    match node {
        VueNode::Element(element) => {
            if let Some(import_ref) = &element.component_import_ref {
                refs.push((
                    Atom::from(import_ref.clone()),
                    Atom::from(element.tag.clone()),
                ));
            }
            for child in &element.children {
                collect_component_script_import_refs(child, refs);
            }
        }
        VueNode::Fragment(children) => {
            for child in children {
                collect_component_script_import_refs(child, refs);
            }
        }
        VueNode::If(branches) => {
            for branch in branches {
                collect_component_script_import_refs(&branch.node, refs);
            }
        }
        VueNode::For(for_node) => collect_component_script_import_refs(&for_node.node, refs),
        VueNode::Text(_)
        | VueNode::Interpolation(_)
        | VueNode::Comment(_)
        | VueNode::RawHtml(_)
        | VueNode::RawExpr(_)
        | VueNode::Unsupported(_) => {}
    }
}

fn rename_component_import_tags(node: &mut VueNode, aliases: &HashMap<Atom, Atom>) {
    match node {
        VueNode::Element(element) => {
            if let Some(import_ref) = &element.component_import_ref {
                if let Some(alias) = aliases.get(&Atom::from(import_ref.clone())) {
                    element.tag = alias.to_string();
                }
            }
            for child in &mut element.children {
                rename_component_import_tags(child, aliases);
            }
        }
        VueNode::Fragment(children) => {
            for child in children {
                rename_component_import_tags(child, aliases);
            }
        }
        VueNode::If(branches) => {
            for branch in branches {
                rename_component_import_tags(&mut branch.node, aliases);
            }
        }
        VueNode::For(for_node) => rename_component_import_tags(&mut for_node.node, aliases),
        VueNode::Text(_)
        | VueNode::Interpolation(_)
        | VueNode::Comment(_)
        | VueNode::RawHtml(_)
        | VueNode::RawExpr(_)
        | VueNode::Unsupported(_) => {}
    }
}
