use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{Expr, ObjectPatProp, Pat, Stmt};

use crate::vue_template::VueTemplateScope;

use super::{collect_js_unshadowed_ident_refs, VueRecoveryContext, VueTemplateUsage};

#[derive(Clone)]
pub(super) struct VueSetupRefBinding {
    pub(super) binding: Atom,
    pub(super) expr: String,
    pub(super) helper: String,
    pub(super) known_ref: bool,
}

#[derive(Clone)]
pub(super) struct VueSetupValueBinding {
    pub(super) value: String,
    pub(super) expr: Option<Expr>,
}

#[derive(Clone)]
pub(super) struct VueSetupLocalBinding {
    pub(super) bindings: Vec<Atom>,
    pub(super) emitted_bindings: Vec<Atom>,
    pub(super) refs: HashSet<Atom>,
    pub(super) source: String,
    pub(super) import_refs: HashSet<Atom>,
    pub(super) stmt: Stmt,
    pub(super) module_scope: bool,
    pub(super) template_selectable: bool,
    pub(super) setup_order: usize,
}

#[derive(Clone)]
pub(super) struct VueSetupScriptBinding {
    pub(super) binding: Atom,
    pub(super) value: String,
    pub(super) setup_order: usize,
}

pub(super) fn collect_local_pat_bindings(pat: &Pat, bindings: &mut HashSet<Atom>) {
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

pub(super) fn setup_value_dependency_refs(
    ctx: &VueRecoveryContext,
    template_usage: &VueTemplateUsage,
) -> HashSet<Atom> {
    if ctx.bindings.values.is_empty() {
        return HashSet::new();
    }

    let mut refs = HashSet::new();
    for value in ctx.bindings.values.values() {
        let mut value_refs = HashSet::new();
        collect_js_unshadowed_ident_refs(&value.value, &mut value_refs);
        if value_refs
            .iter()
            .any(|local| template_usage.for_source_refs.contains(local))
        {
            refs.extend(value_refs);
        }
    }
    refs
}

pub(super) fn setup_script_binding_refs(ctx: &VueRecoveryContext) -> HashSet<Atom> {
    let mut refs = HashSet::new();
    for binding in &ctx.setup_script_bindings {
        collect_js_unshadowed_ident_refs(&binding.value, &mut refs);
    }
    refs
}
