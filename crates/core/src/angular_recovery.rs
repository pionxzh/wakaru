//! Best-effort recovery of production Angular Ivy component artifacts.
//!
//! The analyzer consumes ordinary resolved JavaScript modules. Bundle-format
//! concerns stay in unpackers; this module knows only module ASTs and semantic
//! Ivy instruction identities.

mod emitter;
mod roles;
mod syntax;
mod template;

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, SyntaxContext, GLOBALS};
use swc_core::ecma::ast::{
    Class, Decl, Expr, Function, Module, ModuleDecl, ModuleItem, Pat, Prop, PropOrSpread, Stmt,
};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{VisitMutWith, VisitWith};

use emitter::{emit_component_source, ComponentEmitInput};
use roles::{IvyInstruction, IvyRoleTable};
use syntax::{binding_key, prop_name, string_lit, BindingKey};
use template::recover_template;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AngularRecoveryCompleteness {
    Complete,
    Partial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecoveredAngularComponent {
    pub name: String,
    pub selector: String,
    pub source: String,
    pub completeness: AngularRecoveryCompleteness,
}

#[derive(Debug, Clone, Copy)]
pub struct AngularModuleSource<'a> {
    pub filename: &'a str,
    pub source: &'a str,
}

#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct AngularRecoveryOptions {}

struct PreparedAngularModule {
    module: Module,
    unresolved_ctxt: SyntaxContext,
    cm: Lrc<SourceMap>,
}

#[derive(Clone)]
struct ComponentClass {
    name: Atom,
    class: Box<Class>,
}

struct ComponentDescriptor {
    class: ComponentClass,
    selector: String,
    styles: Vec<String>,
    template: Function,
}

pub fn recover_angular_components_from_js(
    source: &str,
    options: AngularRecoveryOptions,
) -> Result<Vec<RecoveredAngularComponent>> {
    recover_angular_components_from_modules(
        &[AngularModuleSource {
            filename: "angular-recovery.js",
            source,
        }],
        options,
    )
}

pub fn recover_angular_components_from_modules(
    sources: &[AngularModuleSource<'_>],
    _options: AngularRecoveryOptions,
) -> Result<Vec<RecoveredAngularComponent>> {
    GLOBALS.set(&Default::default(), || {
        let modules = sources
            .iter()
            .map(|source| prepare_module(source))
            .collect::<Result<Vec<_>>>()?;
        let roles = IvyRoleTable::collect(&modules);

        let mut recovered = Vec::new();
        for prepared in &modules {
            let classes = collect_component_classes(&prepared.module);
            let mut calls = roles::IvyCallCollector::new(&roles, prepared.unresolved_ctxt);
            prepared.module.visit_with(&mut calls);

            for call in &calls.define_component_calls {
                let Some(descriptor) =
                    parse_component_descriptor(call, &classes, &roles, prepared.unresolved_ctxt)
                else {
                    continue;
                };
                let recovered_template = recover_template(
                    &descriptor.template,
                    call,
                    &roles,
                    prepared.unresolved_ctxt,
                    prepared.cm.clone(),
                )?;
                let name = descriptor.class.name.to_string();
                let source = emit_component_source(
                    ComponentEmitInput {
                        name: &name,
                        selector: &descriptor.selector,
                        styles: &descriptor.styles,
                        class: &descriptor.class.class,
                        roles: &roles,
                        unresolved_ctxt: prepared.unresolved_ctxt,
                        template: &recovered_template,
                    },
                    prepared.cm.clone(),
                )?;
                recovered.push(RecoveredAngularComponent {
                    name,
                    selector: descriptor.selector,
                    source,
                    completeness: if recovered_template.unsupported_instructions.is_empty() {
                        AngularRecoveryCompleteness::Complete
                    } else {
                        AngularRecoveryCompleteness::Partial
                    },
                });
            }
        }

        Ok(recovered)
    })
}

fn prepare_module(source: &AngularModuleSource<'_>) -> Result<PreparedAngularModule> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        FileName::Custom(source.filename.to_string()).into(),
        source.source.to_string(),
    );
    let lexer = Lexer::new(
        Syntax::Es(EsSyntax {
            jsx: true,
            decorators: true,
            ..Default::default()
        }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let mut module = parser
        .parse_module()
        .map_err(|error| anyhow!("failed to parse {}: {error:?}", source.filename))?;
    let errors = parser.take_errors();
    if !errors.is_empty() {
        return Err(anyhow!(
            "failed to parse {} without recovery errors: {:?}",
            source.filename,
            errors[0]
        ));
    }

    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

    Ok(PreparedAngularModule {
        module,
        unresolved_ctxt: SyntaxContext::empty().apply_mark(unresolved_mark),
        cm,
    })
}

fn collect_component_classes(module: &Module) -> HashMap<BindingKey, ComponentClass> {
    let mut classes = HashMap::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Class(class_decl)))
            | ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(swc_core::ecma::ast::ExportDecl {
                decl: Decl::Class(class_decl),
                ..
            })) => {
                classes.insert(
                    binding_key(&class_decl.ident),
                    ComponentClass {
                        name: class_decl.ident.sym.clone(),
                        class: class_decl.class.clone(),
                    },
                );
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl)))
            | ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(swc_core::ecma::ast::ExportDecl {
                decl: Decl::Var(var_decl),
                ..
            })) => {
                for declarator in &var_decl.decls {
                    let Pat::Ident(binding) = &declarator.name else {
                        continue;
                    };
                    let Some(Expr::Class(class_expr)) = declarator.init.as_deref() else {
                        continue;
                    };
                    classes.insert(
                        binding_key(&binding.id),
                        ComponentClass {
                            name: binding.id.sym.clone(),
                            class: class_expr.class.clone(),
                        },
                    );
                }
            }
            _ => {}
        }
    }
    classes
}

fn parse_component_descriptor(
    call: &swc_core::ecma::ast::CallExpr,
    classes: &HashMap<BindingKey, ComponentClass>,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
) -> Option<ComponentDescriptor> {
    if roles.instruction_for_callee(&call.callee, unresolved_ctxt)
        != Some(IvyInstruction::DefineComponent)
    {
        return None;
    }
    let Expr::Object(object) = call.args.first()?.expr.as_ref() else {
        return None;
    };

    let class = object.props.iter().find_map(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            return None;
        };
        let Expr::Ident(ident) = key_value.value.as_ref() else {
            return None;
        };
        let candidate = classes.get(&binding_key(ident))?;
        (prop_name(&key_value.key).as_deref() == Some("type")).then(|| candidate.clone())
    })?;

    let selector = object.props.iter().find_map(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            return None;
        };
        (prop_name(&key_value.key).as_deref() == Some("selectors"))
            .then(|| first_selector(key_value.value.as_ref()))
            .flatten()
    })?;

    let styles = object
        .props
        .iter()
        .find_map(|prop| {
            let PropOrSpread::Prop(prop) = prop else {
                return None;
            };
            let Prop::KeyValue(key_value) = prop.as_ref() else {
                return None;
            };
            (prop_name(&key_value.key).as_deref() == Some("styles"))
                .then(|| string_array(key_value.value.as_ref()))
        })
        .flatten()
        .unwrap_or_default();

    let template = object.props.iter().find_map(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        match prop.as_ref() {
            Prop::KeyValue(key_value)
                if prop_name(&key_value.key).as_deref() == Some("template") =>
            {
                match key_value.value.as_ref() {
                    Expr::Fn(function) => Some(function.function.as_ref().clone()),
                    _ => None,
                }
            }
            Prop::Method(method) if prop_name(&method.key).as_deref() == Some("template") => {
                Some(method.function.as_ref().clone())
            }
            _ => None,
        }
    })?;

    Some(ComponentDescriptor {
        class,
        selector,
        styles,
        template,
    })
}

fn first_selector(expr: &Expr) -> Option<String> {
    let Expr::Array(outer) = expr else {
        return None;
    };
    let Expr::Array(selector) = outer.elems.first()?.as_ref()?.expr.as_ref() else {
        return None;
    };
    string_lit(selector.elems.first()?.as_ref()?.expr.as_ref())
}

fn string_array(expr: &Expr) -> Option<Vec<String>> {
    let Expr::Array(array) = expr else {
        return None;
    };
    array
        .elems
        .iter()
        .map(|element| string_lit(element.as_ref()?.expr.as_ref()))
        .collect()
}

#[cfg(test)]
#[path = "angular_recovery/tests.rs"]
mod tests;
