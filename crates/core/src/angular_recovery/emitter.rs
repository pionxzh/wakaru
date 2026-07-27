use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    BindingIdent, BlockStmtOrExpr, Class, ClassDecl, ClassMember, Decl, Expr, Ident, Module,
    ModuleDecl, ModuleItem, Pat, ReturnStmt, Stmt, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::artifact::ArtifactSupportPlan;
use super::roles::{IvyInstruction, IvyRoleTable};
use super::syntax::{binding_key, prop_name, BindingKey};
use super::template::RecoveredTemplate;
use crate::rules::rename_utils::{rename_bindings, BindingRename};

pub(super) struct ComponentEmitInput<'a> {
    pub(super) name: &'a str,
    pub(super) selector: &'a str,
    pub(super) styles: &'a [String],
    pub(super) class: &'a Class,
    pub(super) template: &'a RecoveredTemplate,
    pub(super) support: &'a ArtifactSupportPlan,
    pub(super) dependencies: &'a [String],
}

pub(super) struct ModuleComponentEmitInput<'a> {
    pub(super) name: &'a str,
    pub(super) selector: &'a str,
    pub(super) styles: &'a [String],
    pub(super) class: &'a Class,
    pub(super) template_source: &'a str,
    pub(super) dependencies: &'a [String],
}

pub(super) fn emit_component_source(
    input: ComponentEmitInput<'_>,
    cm: Lrc<SourceMap>,
) -> Result<String> {
    let support_source = print_support_source(input.support, &[], cm.clone())?;

    let mut source = "import { Component } from \"@angular/core\";\n".to_string();
    if let Some(support_source) = support_source {
        source.push_str(&support_source);
        source.push('\n');
    }
    append_unresolved_symbols(&mut source, input.support);
    source.push('\n');
    source.push_str(&component_source_fragment(
        ModuleComponentEmitInput {
            name: input.name,
            selector: input.selector,
            styles: input.styles,
            class: input.class,
            template_source: &input.template.source,
            dependencies: input.dependencies,
        },
        &[],
        cm,
    )?);
    Ok(source)
}

pub(super) fn emit_angular_module_source(
    components: &[ModuleComponentEmitInput<'_>],
    support: &ArtifactSupportPlan,
    renames: &[BindingRename],
    cm: Lrc<SourceMap>,
) -> Result<String> {
    let mut source = "import { Component } from \"@angular/core\";\n".to_string();
    if let Some(support_source) = print_support_source(support, renames, cm.clone())? {
        source.push_str(&support_source);
        source.push('\n');
    }
    append_unresolved_symbols(&mut source, support);
    for component in components {
        source.push('\n');
        source.push_str(&component_source_fragment(
            ModuleComponentEmitInput {
                name: component.name,
                selector: component.selector,
                styles: component.styles,
                class: component.class,
                template_source: component.template_source,
                dependencies: component.dependencies,
            },
            renames,
            cm.clone(),
        )?);
        source.push('\n');
    }
    Ok(source.trim_end().to_string())
}

fn print_support_source(
    support: &ArtifactSupportPlan,
    renames: &[BindingRename],
    cm: Lrc<SourceMap>,
) -> Result<Option<String>> {
    let items = support.module_items();
    if items.is_empty() {
        return Ok(None);
    }
    let mut module = Module {
        span: DUMMY_SP,
        body: items,
        shebang: None,
    };
    if !renames.is_empty() {
        rename_bindings(&mut module, renames);
    }
    print_module(&module, cm).map(Some)
}

fn append_unresolved_symbols(source: &mut String, support: &ArtifactSupportPlan) {
    let unresolved = support.unresolved_symbols();
    if unresolved.is_empty() {
        return;
    }
    source.push_str("\n// Unresolved artifact-local symbols: ");
    source.push_str(&unresolved.join(", "));
    source.push('\n');
}

fn component_source_fragment(
    input: ModuleComponentEmitInput<'_>,
    renames: &[BindingRename],
    cm: Lrc<SourceMap>,
) -> Result<String> {
    let mut class = Box::new(input.class.clone());
    if !renames.is_empty() {
        rename_bindings(class.as_mut(), renames);
    }
    let class_source = print_component_class(input.name, class, cm)?;
    let mut metadata = String::new();
    metadata.push_str("@Component({\n");
    metadata.push_str("  selector: ");
    metadata.push_str(&quoted_js_string(input.selector));
    metadata.push_str(",\n");
    metadata.push_str("  template: `\n");
    metadata.push_str(&indent_template_literal(input.template_source, 4));
    metadata.push_str("\n  `,\n");
    if !input.styles.is_empty() {
        metadata.push_str("  styles: [\n");
        for style in input.styles {
            metadata.push_str("    `\n");
            metadata.push_str(&indent_template_literal(&recover_scoped_styles(style), 6));
            metadata.push_str("\n    `,\n");
        }
        metadata.push_str("  ],\n");
    }
    if !input.dependencies.is_empty() {
        metadata.push_str("  imports: [");
        metadata.push_str(&input.dependencies.join(", "));
        metadata.push_str("],\n");
    }
    metadata.push_str("})\n");

    metadata.push_str(&class_source);
    Ok(metadata)
}

pub(super) fn clean_component_class(
    class: &Class,
    definition_field: Option<&Atom>,
    roles: &IvyRoleTable,
    unresolved_ctxt: SyntaxContext,
) -> Box<Class> {
    let mut class = Box::new(class.clone());
    class.decorators.clear();
    class.body.retain(|member| match member {
        ClassMember::ClassProp(property) if property.is_static => {
            let canonical_ivy_field =
                prop_name(&property.key).is_some_and(|name| name.starts_with('ɵ'));
            let assigned_component_field = definition_field
                .is_some_and(|field| prop_name(&property.key).as_deref() == Some(field.as_ref()));
            let component_initializer = property.value.as_deref().is_some_and(|value| {
                let Expr::Call(call) = value else {
                    return false;
                };
                roles.instruction_for_callee(&call.callee, unresolved_ctxt)
                    == Some(IvyInstruction::DefineComponent)
            });
            !canonical_ivy_field && !assigned_component_field && !component_initializer
        }
        _ => true,
    });
    class
}

fn print_component_class(name: &str, class: Box<Class>, cm: Lrc<SourceMap>) -> Result<String> {
    let module = Module {
        span: DUMMY_SP,
        body: vec![ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(
            swc_core::ecma::ast::ExportDecl {
                span: DUMMY_SP,
                decl: Decl::Class(ClassDecl {
                    ident: Ident::new(Atom::from(name), DUMMY_SP, SyntaxContext::empty()),
                    declare: false,
                    class,
                }),
            },
        ))],
        shebang: None,
    };
    print_module(&module, cm)
}

pub(super) fn handler_expression(
    expression: &Expr,
    component_contexts: &HashSet<BindingKey>,
    local_references: &HashMap<BindingKey, String>,
    cm: Lrc<SourceMap>,
) -> Result<String> {
    let mut expression = expression.clone();
    restore_event_parameter(&mut expression);
    match &expression {
        Expr::Fn(function) => {
            let body = function.function.body.as_ref();
            if let Some(expression) = body.and_then(single_return_expression) {
                print_template_expression(expression, component_contexts, local_references, cm)
            } else {
                print_expression(&expression, cm)
            }
        }
        Expr::Arrow(arrow) => match arrow.body.as_ref() {
            BlockStmtOrExpr::Expr(expression) => {
                print_template_expression(expression, component_contexts, local_references, cm)
            }
            BlockStmtOrExpr::BlockStmt(block) => {
                if let Some(expression) = single_return_expression(block) {
                    print_template_expression(expression, component_contexts, local_references, cm)
                } else {
                    print_expression(&expression, cm)
                }
            }
        },
        _ => print_template_expression(&expression, component_contexts, local_references, cm),
    }
}

fn restore_event_parameter(expression: &mut Expr) {
    let parameter = match expression {
        Expr::Fn(function) => function.function.params.first().map(|param| &param.pat),
        Expr::Arrow(arrow) => arrow.params.first(),
        _ => None,
    };
    let Some(Pat::Ident(parameter)) = parameter else {
        return;
    };
    let old = binding_key(&parameter.id);
    if old.0.as_ref() == "$event" {
        return;
    }
    rename_bindings(
        expression,
        &[BindingRename {
            old,
            new: Atom::from("$event"),
        }],
    );
}

fn single_return_expression(block: &swc_core::ecma::ast::BlockStmt) -> Option<&Expr> {
    let [Stmt::Return(ReturnStmt {
        arg: Some(expression),
        ..
    })] = block.stmts.as_slice()
    else {
        return None;
    };
    Some(expression.as_ref())
}

pub(super) fn print_template_expression(
    expression: &Expr,
    component_contexts: &HashSet<BindingKey>,
    local_references: &HashMap<BindingKey, String>,
    cm: Lrc<SourceMap>,
) -> Result<String> {
    print_template_expression_with_aliases(
        expression,
        component_contexts,
        local_references,
        &HashMap::new(),
        cm,
    )
}

pub(super) fn print_template_expression_with_aliases(
    expression: &Expr,
    component_contexts: &HashSet<BindingKey>,
    local_references: &HashMap<BindingKey, String>,
    expression_aliases: &HashMap<BindingKey, Box<Expr>>,
    cm: Lrc<SourceMap>,
) -> Result<String> {
    let mut expression = expression.clone();
    if !expression_aliases.is_empty() {
        expression.visit_mut_with(&mut TemplateExpressionAliasResolver {
            aliases: expression_aliases,
            active: HashSet::new(),
        });
    }
    if !component_contexts.is_empty() || !local_references.is_empty() {
        expression.visit_mut_with(&mut TemplateBindingCleaner {
            contexts: component_contexts,
            local_references,
        });
    }
    print_expression(&expression, cm)
}

struct TemplateExpressionAliasResolver<'a> {
    aliases: &'a HashMap<BindingKey, Box<Expr>>,
    active: HashSet<BindingKey>,
}

impl VisitMut for TemplateExpressionAliasResolver<'_> {
    fn visit_mut_expr(&mut self, expression: &mut Expr) {
        let Expr::Ident(identifier) = expression else {
            expression.visit_mut_children_with(self);
            return;
        };
        let key = binding_key(identifier);
        let Some(alias) = self.aliases.get(&key) else {
            return;
        };
        if !self.active.insert(key.clone()) {
            return;
        }
        let mut alias = alias.as_ref().clone();
        alias.visit_mut_with(self);
        self.active.remove(&key);
        *expression = alias;
    }
}

fn print_expression(expression: &Expr, cm: Lrc<SourceMap>) -> Result<String> {
    let module = Module {
        span: DUMMY_SP,
        body: vec![ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
            span: DUMMY_SP,
            ctxt: SyntaxContext::empty(),
            kind: VarDeclKind::Const,
            declare: false,
            decls: vec![VarDeclarator {
                span: DUMMY_SP,
                name: Pat::Ident(BindingIdent {
                    id: Ident::new("__wakaru_ivy_expr".into(), DUMMY_SP, SyntaxContext::empty()),
                    type_ann: None,
                }),
                init: Some(Box::new(expression.clone())),
                definite: false,
            }],
        }))))],
        shebang: None,
    };
    let source = print_module(&module, cm)?;
    Ok(source
        .trim()
        .strip_prefix("const __wakaru_ivy_expr = ")
        .unwrap_or(source.trim())
        .trim_end_matches(';')
        .trim()
        .to_string())
}

fn print_module(module: &Module, cm: Lrc<SourceMap>) -> Result<String> {
    let mut output = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: Config::default().with_minify(false),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm, "\n", &mut output, None),
        };
        emitter
            .emit_module(module)
            .map_err(|error| anyhow!("failed to print Angular artifact: {error:?}"))?;
    }
    String::from_utf8(output)
        .map(|source| source.trim().to_string())
        .map_err(|error| anyhow!("Angular artifact is not UTF-8: {error}"))
}

struct TemplateBindingCleaner<'a> {
    contexts: &'a HashSet<BindingKey>,
    local_references: &'a HashMap<BindingKey, String>,
}

impl VisitMut for TemplateBindingCleaner<'_> {
    fn visit_mut_expr(&mut self, expression: &mut Expr) {
        expression.visit_mut_children_with(self);
        if let Expr::Ident(identifier) = expression {
            if let Some(name) = self.local_references.get(&binding_key(identifier)) {
                *identifier = Ident::new(
                    Atom::from(name.as_str()),
                    identifier.span,
                    SyntaxContext::empty(),
                );
            }
            return;
        }
        let Expr::Member(member) = expression else {
            return;
        };
        let Expr::Ident(object) = member.obj.as_ref() else {
            return;
        };
        if !self.contexts.contains(&binding_key(object)) {
            return;
        }
        let swc_core::ecma::ast::MemberProp::Ident(property) = &member.prop else {
            return;
        };
        *expression = Expr::Ident(Ident::new(
            property.sym.clone(),
            property.span,
            SyntaxContext::empty(),
        ));
    }
}

fn recover_scoped_styles(style: &str) -> String {
    style
        .replace("[_nghost-%COMP%]", ":host")
        .replace("[_ngcontent-%COMP%]", "")
        .trim()
        .to_string()
}

fn indent_template_literal(value: &str, spaces: usize) -> String {
    let indent = " ".repeat(spaces);
    value
        .replace('`', "\\`")
        .replace("${", "\\${")
        .lines()
        .map(|line| format!("{indent}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn quoted_js_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("\"{escaped}\"")
}
