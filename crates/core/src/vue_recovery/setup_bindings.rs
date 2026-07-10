use std::collections::HashSet;

use anyhow::Result;
use swc_core::atoms::Atom;
use swc_core::ecma::ast::{Expr, MemberExpr, MemberProp, ObjectLit, Prop, PropOrSpread};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::js_names::is_valid_identifier_name;

use super::context::compiled_script_setup;
use super::expressions::print_expr;
use super::locals::{setup_script_binding_refs, VueSetupLocalBinding};
use super::syntax::{prop_name, string_lit};
use super::{RenderSource, VueRecoveryContext, VueTemplateUsage};

pub(super) fn component_props_source(ctx: &VueRecoveryContext) -> Result<Option<String>> {
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

pub(super) fn setup_emit_declaration(
    ctx: &VueRecoveryContext,
    template_usage: &VueTemplateUsage,
    local_declarations: &[&VueSetupLocalBinding],
) -> Result<Option<(String, String)>> {
    let Some(binding) = setup_emit_script_binding(ctx, template_usage, local_declarations) else {
        return Ok(None);
    };
    let Some(emits_source) = component_emits_source(ctx)? else {
        return Ok(None);
    };

    Ok(Some((binding, emits_source)))
}

pub(super) fn setup_ref_declarations(
    ctx: &VueRecoveryContext,
    template_usage: &VueTemplateUsage,
    render: RenderSource<'_>,
) -> Vec<(String, String, String)> {
    let mut expr_refs = template_usage.expr_refs.clone();
    expr_refs.extend(setup_script_binding_refs(ctx));
    let template_refs = &template_usage.static_ref_names;
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
            declarations.push((name.clone(), "ref(null)".to_string(), "ref".to_string()));
        }
    }

    declarations
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
        RenderSource::Function {
            render,
            component_options,
        } => {
            let options = component_options
                .or(ctx.setup_component_options.as_ref())
                .or(ctx.component_options.as_ref());
            if let Some(setup) = compiled_script_setup(options) {
                for stmt in setup.setup_stmts {
                    stmt.visit_with(&mut collector);
                }
            }
            render.function.visit_with(&mut collector);
        }
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

pub(super) fn setup_prop_bindings(
    valid_prop_names: &[String],
    ctx: &VueRecoveryContext,
) -> Vec<(String, String)> {
    valid_prop_names
        .iter()
        .map(|prop| {
            let binding = ctx
                .bindings
                .props
                .get(&Atom::from(prop.clone()))
                .map(ToString::to_string)
                .unwrap_or_else(|| prop.clone());
            (prop.clone(), binding)
        })
        .collect()
}

pub(super) fn setup_props_script_binding(
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

pub(super) fn props_binding_reserved_names(
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
            .map(|binding| binding.binding.clone()),
    );
    reserved.extend(
        ctx.setup_local_bindings
            .iter()
            .flat_map(|declaration| declaration.emitted_bindings.iter().cloned()),
    );
    reserved.extend(ctx.bindings.aliases.keys().cloned());
    reserved.extend(ctx.script_imports.keys().cloned());
    reserved
}

fn setup_emit_script_binding(
    ctx: &VueRecoveryContext,
    template_usage: &VueTemplateUsage,
    local_declarations: &[&VueSetupLocalBinding],
) -> Option<String> {
    let mut expr_refs = template_usage.expr_refs.clone();
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

pub(super) fn component_prop_names(options: &ObjectLit) -> Vec<String> {
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
            .filter_map(|elem| string_lit(elem.expr.as_ref()))
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
