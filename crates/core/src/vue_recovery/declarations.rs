use std::collections::HashSet;

use swc_core::atoms::Atom;

use crate::js_names::is_valid_identifier_name;

use super::locals::VueSetupLocalBinding;
use super::{collect_js_unshadowed_ident_refs, VueRecoveryContext};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum VueScriptSetupDeclarationKind {
    Local,
    Computed,
}

#[derive(Clone)]
pub(super) struct VueScriptSetupDeclaration {
    pub(super) kind: VueScriptSetupDeclarationKind,
    pub(super) source: String,
    pub(super) bindings: Vec<Atom>,
    pub(super) refs: HashSet<Atom>,
    pub(super) order: usize,
    pub(super) sequence: usize,
}

pub(super) fn script_setup_declarations(
    ctx: &VueRecoveryContext,
    local_declarations: &[VueSetupLocalBinding],
) -> Vec<VueScriptSetupDeclaration> {
    let module_declaration_count = local_declarations
        .iter()
        .filter(|declaration| declaration.module_scope)
        .count();
    let mut declarations = Vec::new();
    for (sequence, declaration) in local_declarations.iter().enumerate() {
        let order = if declaration.module_scope {
            sequence
        } else {
            module_declaration_count + declaration.setup_order
        };
        declarations.push(VueScriptSetupDeclaration {
            kind: VueScriptSetupDeclarationKind::Local,
            source: declaration.source.clone(),
            bindings: declaration.emitted_bindings.clone(),
            refs: declaration.refs.clone(),
            order,
            sequence,
        });
    }

    let mut bindings = ctx.setup_script_bindings.clone();
    bindings.sort_by(|left, right| {
        left.setup_order
            .cmp(&right.setup_order)
            .then_with(|| left.binding.as_ref().cmp(right.binding.as_ref()))
    });
    let next_sequence = declarations.len();
    for (offset, binding) in bindings.into_iter().enumerate() {
        if !is_valid_identifier_name(binding.binding.as_ref()) {
            continue;
        }
        let mut refs = HashSet::new();
        collect_js_unshadowed_ident_refs(&binding.value, &mut refs);
        declarations.push(VueScriptSetupDeclaration {
            kind: VueScriptSetupDeclarationKind::Computed,
            source: format!(
                "const {} = {};",
                binding.binding.as_ref(),
                binding.value.trim()
            ),
            bindings: vec![binding.binding],
            refs,
            order: module_declaration_count + binding.setup_order,
            sequence: next_sequence + offset,
        });
    }

    declarations.sort_by(|left, right| {
        left.order
            .cmp(&right.order)
            .then_with(|| left.sequence.cmp(&right.sequence))
    });

    order_script_setup_declarations(&declarations)
}

fn order_script_setup_declarations(
    declarations: &[VueScriptSetupDeclaration],
) -> Vec<VueScriptSetupDeclaration> {
    let mut remaining = vec![true; declarations.len()];
    let mut remaining_count = declarations.len();
    let mut ordered = Vec::with_capacity(declarations.len());

    while remaining_count > 0 {
        let mut progressed = false;
        for index in 0..declarations.len() {
            if !remaining[index] || !script_setup_declaration_ready(index, declarations, &remaining)
            {
                continue;
            }
            remaining[index] = false;
            remaining_count -= 1;
            ordered.push(declarations[index].clone());
            progressed = true;
        }

        if !progressed {
            for (index, declaration) in declarations.iter().enumerate() {
                if remaining[index] {
                    ordered.push(declaration.clone());
                }
            }
            break;
        }
    }

    ordered
}

fn script_setup_declaration_ready(
    index: usize,
    declarations: &[VueScriptSetupDeclaration],
    remaining: &[bool],
) -> bool {
    let declaration = &declarations[index];
    !declarations
        .iter()
        .enumerate()
        .filter(|(other_index, _)| *other_index != index && remaining[*other_index])
        .any(|(_, other)| {
            other
                .bindings
                .iter()
                .any(|binding| declaration.refs.contains(binding))
        })
}

pub(super) fn push_script_setup_declarations(
    body: &mut String,
    declarations: &[VueScriptSetupDeclaration],
) {
    for (index, declaration) in declarations.iter().enumerate() {
        if index > 0 && declarations[index - 1].kind != declaration.kind {
            body.push('\n');
        }
        body.push_str(declaration.source.trim());
        body.push('\n');
    }
}

pub(super) fn script_setup_declared_bindings(
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
            .map(|binding| binding.binding.clone()),
    );
    declared.extend(
        local_declarations
            .iter()
            .flat_map(|declaration| declaration.emitted_bindings.iter().cloned()),
    );
    declared
}
