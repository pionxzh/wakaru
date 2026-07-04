use swc_core::common::{sync::Lrc, FileName, SourceMap, Span};
use swc_core::ecma::ast::{
    ArrowExpr, CatchClause, ClassDecl, ClassExpr, Expr, FnDecl, FnExpr, Function, Ident,
    ModuleItem, ObjectPatProp, Pat, Prop, Stmt, VarDeclarator,
};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::visit::{Visit, VisitWith};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VueSfc {
    pub script: Option<String>,
    pub script_setup: Option<String>,
    pub template: VueTemplate,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VueTemplate {
    pub children: Vec<VueNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VueExpr(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VueNode {
    Element(VueElement),
    Fragment(Vec<VueNode>),
    If(Vec<VueIfBranch>),
    For(VueFor),
    Text(String),
    Interpolation(VueExpr),
    Comment(String),
    RawHtml(String),
    RawExpr(VueExpr),
    Unsupported(VueUnsupported),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VueIfBranch {
    pub condition: Option<VueExpr>,
    pub node: Box<VueNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VueFor {
    pub value: String,
    pub source: VueExpr,
    pub node: Box<VueNode>,
    pub scope: VueTemplateScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VueElement {
    pub tag: String,
    pub component_import_ref: Option<String>,
    pub attrs: Vec<VueAttr>,
    pub children: Vec<VueNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VueUnsupported {
    pub kind: VueUnsupportedKind,
    pub expr: VueExpr,
    pub source: Option<VueUnsupportedSource>,
    pub scope: VueTemplateScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VueUnsupportedKind {
    VNodeChildren,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VueUnsupportedSource {
    RenderLocalSlotPartitionChildren { binding: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VueAttr {
    Static {
        name: String,
        value: Option<String>,
    },
    Bind {
        name: String,
        expr: VueExpr,
    },
    On {
        name: String,
        expr: VueExpr,
        modifiers: Vec<String>,
    },
    Directive(VueDirective),
    Spread(VueExpr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VueDirective {
    pub name: String,
    pub arg: Option<VueDirectiveArg>,
    pub expr: Option<VueExpr>,
    pub modifiers: Vec<String>,
    pub scope: VueTemplateScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VueDirectiveArg {
    Static(String),
    Dynamic(VueExpr),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VueTemplateScope {
    pub locals: Vec<String>,
}

mod emitter;

impl VueExpr {
    pub fn new(expr: impl Into<String>) -> Self {
        Self(expr.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn replace_prefix(&mut self, from: &str, to: &str) {
        self.0 = rename_expr_prefix(&self.0, from, to);
    }
}

impl From<String> for VueExpr {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for VueExpr {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<&VueExpr> for VueExpr {
    fn from(value: &VueExpr) -> Self {
        value.clone()
    }
}

impl std::fmt::Display for VueExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl VueDirective {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            arg: None,
            expr: None,
            modifiers: Vec::new(),
            scope: VueTemplateScope::default(),
        }
    }

    pub fn with_arg(mut self, arg: impl Into<String>) -> Self {
        self.arg = Some(VueDirectiveArg::Static(arg.into()));
        self
    }

    pub fn with_dynamic_arg(mut self, arg: impl Into<VueExpr>) -> Self {
        self.arg = Some(VueDirectiveArg::Dynamic(arg.into()));
        self
    }

    pub fn with_expr(mut self, expr: impl Into<VueExpr>) -> Self {
        self.expr = Some(expr.into());
        self
    }

    pub fn with_modifiers(mut self, modifiers: Vec<String>) -> Self {
        self.modifiers = modifiers;
        self
    }

    pub fn with_scope(mut self, scope: VueTemplateScope) -> Self {
        self.scope = scope;
        self
    }
}

impl VueElement {
    pub fn new(tag: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            component_import_ref: None,
            attrs: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn with_component_import_ref(mut self, import_ref: impl Into<String>) -> Self {
        self.component_import_ref = Some(import_ref.into());
        self
    }

    pub fn with_attrs(mut self, attrs: Vec<VueAttr>) -> Self {
        self.attrs = attrs;
        self
    }

    pub fn with_children(mut self, children: Vec<VueNode>) -> Self {
        self.children = children;
        self
    }
}

impl VueUnsupported {
    pub fn vnode_children(expr: impl Into<VueExpr>) -> Self {
        Self {
            kind: VueUnsupportedKind::VNodeChildren,
            expr: expr.into(),
            source: None,
            scope: VueTemplateScope::default(),
        }
    }

    pub fn vnode_children_from_render_local_slot_partition(
        expr: impl Into<VueExpr>,
        binding: impl Into<String>,
    ) -> Self {
        let binding = binding.into();
        Self {
            kind: VueUnsupportedKind::VNodeChildren,
            expr: expr.into(),
            source: Some(VueUnsupportedSource::RenderLocalSlotPartitionChildren {
                binding: binding.clone(),
            }),
            scope: VueTemplateScope::from_local(binding),
        }
    }
}

impl VueTemplateScope {
    pub fn from_local(local: impl Into<String>) -> Self {
        Self {
            locals: vec![local.into()],
        }
    }

    pub fn from_locals(locals: impl IntoIterator<Item = String>) -> Self {
        let mut locals = locals.into_iter().collect::<Vec<_>>();
        locals.sort();
        locals.dedup();
        Self { locals }
    }
}

fn rename_expr_prefix(expr: &str, from: &str, to: &str) -> String {
    if from.is_empty() {
        return expr.to_string();
    }

    if let Some(renamed) = rename_expr_prefix_with_ast(expr, from, to) {
        return renamed;
    }

    let chars = expr.chars().collect::<Vec<_>>();
    rename_code_segment(&chars, 0, from, to, false).0
}

fn rename_expr_prefix_with_ast(expr: &str, from: &str, to: &str) -> Option<String> {
    let prefix = "const __wakaru_expr = ";
    let source = format!("{prefix}{expr};");
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        FileName::Custom("vue-template-expr.js".into()).into(),
        source,
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
    let module = parser.parse_module().ok()?;
    let expr_ast = parsed_initializer(&module.body)?;

    let mut collector = ExprPrefixRenameCollector::new(from, to);
    expr_ast.visit_with(&mut collector);
    apply_expr_replacements(expr, prefix.len(), fm.start_pos.0, collector.replacements)
}

fn parsed_initializer(items: &[ModuleItem]) -> Option<&Expr> {
    let ModuleItem::Stmt(Stmt::Decl(decl)) = items.first()? else {
        return None;
    };
    let swc_core::ecma::ast::Decl::Var(var) = decl else {
        return None;
    };
    var.decls.first()?.init.as_deref()
}

fn apply_expr_replacements(
    expr: &str,
    prefix_len: usize,
    start_pos: u32,
    replacements: Vec<ExprReplacement>,
) -> Option<String> {
    if replacements.is_empty() {
        return Some(expr.to_string());
    }

    let mut ranges = replacements
        .into_iter()
        .map(|replacement| {
            let start = replacement
                .span
                .lo
                .0
                .checked_sub(start_pos)?
                .try_into()
                .ok()?;
            let end = replacement
                .span
                .hi
                .0
                .checked_sub(start_pos)?
                .try_into()
                .ok()?;
            let start = usize::checked_sub(start, prefix_len)?;
            let end = usize::checked_sub(end, prefix_len)?;
            (start <= end && end <= expr.len()).then_some((start, end, replacement.with))
        })
        .collect::<Option<Vec<_>>>()?;
    ranges.sort_by_key(|(start, _, _)| *start);
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return None;
    }

    let mut output = expr.to_string();
    for (start, end, replacement) in ranges.into_iter().rev() {
        output.replace_range(start..end, &replacement);
    }
    Some(output)
}

struct ExprReplacement {
    span: Span,
    with: String,
}

struct ExprPrefixRenameCollector<'a> {
    from: &'a str,
    to: &'a str,
    shadow_depth: usize,
    replacements: Vec<ExprReplacement>,
}

impl<'a> ExprPrefixRenameCollector<'a> {
    fn new(from: &'a str, to: &'a str) -> Self {
        Self {
            from,
            to,
            shadow_depth: 0,
            replacements: Vec::new(),
        }
    }

    fn active(&self) -> bool {
        self.shadow_depth == 0
    }

    fn replace_ident(&mut self, ident: &Ident) {
        if self.active() && ident.sym.as_ref() == self.from {
            self.replacements.push(ExprReplacement {
                span: ident.span,
                with: self.to.to_string(),
            });
        }
    }

    fn replace_shorthand(&mut self, ident: &Ident) {
        if self.active() && ident.sym.as_ref() == self.from {
            self.replacements.push(ExprReplacement {
                span: ident.span,
                with: format!("{}: {}", ident.sym, self.to),
            });
        }
    }

    fn with_shadowed_scope(&mut self, shadowed: bool, visit: impl FnOnce(&mut Self)) {
        if shadowed {
            self.shadow_depth += 1;
        }
        visit(self);
        if shadowed {
            self.shadow_depth -= 1;
        }
    }
}

impl Visit for ExprPrefixRenameCollector<'_> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Ident(ident) = expr {
            self.replace_ident(ident);
            return;
        }
        expr.visit_children_with(self);
    }

    fn visit_prop(&mut self, prop: &Prop) {
        if let Prop::Shorthand(ident) = prop {
            self.replace_shorthand(ident);
            return;
        }
        prop.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        let shadowed = arrow_scope_binds_name(arrow, self.from);
        self.with_shadowed_scope(shadowed, |collector| {
            arrow.visit_children_with(collector);
        });
    }

    fn visit_function(&mut self, function: &Function) {
        let shadowed = function_scope_binds_name(function, self.from);
        self.with_shadowed_scope(shadowed, |collector| {
            function.visit_children_with(collector);
        });
    }

    fn visit_fn_expr(&mut self, function: &FnExpr) {
        let shadows_name = function
            .ident
            .as_ref()
            .is_some_and(|ident| ident.sym.as_ref() == self.from);
        self.with_shadowed_scope(shadows_name, |collector| {
            function.visit_children_with(collector);
        });
    }
}

fn arrow_scope_binds_name(arrow: &ArrowExpr, name: &str) -> bool {
    let mut collector = ExprScopeBindingCollector::new(name);
    arrow.visit_children_with(&mut collector);
    collector.found()
}

fn function_scope_binds_name(function: &Function, name: &str) -> bool {
    let mut collector = ExprScopeBindingCollector::new(name);
    function.visit_children_with(&mut collector);
    collector.found()
}

struct ExprScopeBindingCollector<'a> {
    name: &'a str,
    found: bool,
}

impl<'a> ExprScopeBindingCollector<'a> {
    fn new(name: &'a str) -> Self {
        Self { name, found: false }
    }

    fn found(&self) -> bool {
        self.found
    }

    fn collect_pat(&mut self, pat: &Pat) {
        self.found |= pat_binds_name(pat, self.name);
    }

    fn collect_ident(&mut self, ident: &Ident) {
        self.found |= ident.sym.as_ref() == self.name;
    }
}

impl Visit for ExprScopeBindingCollector<'_> {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        self.collect_pat(&declarator.name);
    }

    fn visit_param(&mut self, param: &swc_core::ecma::ast::Param) {
        self.collect_pat(&param.pat);
    }

    fn visit_pat(&mut self, pat: &Pat) {
        self.collect_pat(pat);
    }

    fn visit_fn_decl(&mut self, function: &FnDecl) {
        self.collect_ident(&function.ident);
    }

    fn visit_class_decl(&mut self, class: &ClassDecl) {
        self.collect_ident(&class.ident);
    }

    fn visit_catch_clause(&mut self, catch: &CatchClause) {
        if let Some(param) = &catch.param {
            self.collect_pat(param);
        }
        catch.body.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

    fn visit_function(&mut self, _: &Function) {}

    fn visit_fn_expr(&mut self, _: &FnExpr) {}

    fn visit_class_expr(&mut self, _: &ClassExpr) {}
}

fn pat_binds_name(pat: &Pat, name: &str) -> bool {
    match pat {
        Pat::Ident(binding) => binding.id.sym.as_ref() == name,
        Pat::Array(array) => array
            .elems
            .iter()
            .flatten()
            .any(|elem| pat_binds_name(elem, name)),
        Pat::Rest(rest) => pat_binds_name(rest.arg.as_ref(), name),
        Pat::Object(object) => object.props.iter().any(|prop| match prop {
            ObjectPatProp::KeyValue(key_value) => pat_binds_name(key_value.value.as_ref(), name),
            ObjectPatProp::Assign(assign) => assign.key.sym.as_ref() == name,
            ObjectPatProp::Rest(rest) => pat_binds_name(rest.arg.as_ref(), name),
        }),
        Pat::Assign(assign) => pat_binds_name(assign.left.as_ref(), name),
        Pat::Expr(_) | Pat::Invalid(_) => false,
    }
}

fn rename_code_segment(
    chars: &[char],
    mut index: usize,
    from: &str,
    to: &str,
    stop_on_closing_brace: bool,
) -> (String, usize) {
    let mut renamed = String::new();
    let mut brace_depth = 0usize;

    while index < chars.len() {
        let ch = chars[index];
        if stop_on_closing_brace && brace_depth == 0 && ch == '}' {
            break;
        }

        match ch {
            '\'' | '"' => copy_quoted(chars, &mut index, &mut renamed, ch),
            '`' => copy_template(chars, &mut index, &mut renamed, from, to),
            '{' => {
                brace_depth += 1;
                renamed.push(ch);
                index += 1;
            }
            '}' if brace_depth > 0 => {
                brace_depth -= 1;
                renamed.push(ch);
                index += 1;
            }
            _ if is_ident_start(ch) => {
                let start = index;
                index += 1;
                while index < chars.len() && is_ident_continue(chars[index]) {
                    index += 1;
                }
                let ident = chars[start..index].iter().collect::<String>();
                if ident == from && !is_property_access_tail(&renamed) {
                    renamed.push_str(to);
                } else {
                    renamed.push_str(&ident);
                }
            }
            _ => {
                renamed.push(ch);
                index += 1;
            }
        }
    }

    (renamed, index)
}

fn copy_quoted(chars: &[char], index: &mut usize, output: &mut String, quote: char) {
    output.push(chars[*index]);
    *index += 1;

    while *index < chars.len() {
        let ch = chars[*index];
        output.push(ch);
        *index += 1;

        if ch == '\\' && *index < chars.len() {
            output.push(chars[*index]);
            *index += 1;
            continue;
        }
        if ch == quote {
            break;
        }
    }
}

fn copy_template(chars: &[char], index: &mut usize, output: &mut String, from: &str, to: &str) {
    output.push('`');
    *index += 1;

    while *index < chars.len() {
        let ch = chars[*index];
        match ch {
            '\\' => {
                output.push(ch);
                *index += 1;
                if *index < chars.len() {
                    output.push(chars[*index]);
                    *index += 1;
                }
            }
            '`' => {
                output.push(ch);
                *index += 1;
                break;
            }
            '$' if chars.get(*index + 1) == Some(&'{') => {
                output.push_str("${");
                *index += 2;
                let (renamed, next_index) = rename_code_segment(chars, *index, from, to, true);
                output.push_str(&renamed);
                *index = next_index;
                if *index < chars.len() && chars[*index] == '}' {
                    output.push('}');
                    *index += 1;
                }
            }
            _ => {
                output.push(ch);
                *index += 1;
            }
        }
    }
}

fn is_property_access_tail(expr: &str) -> bool {
    let expr = expr.trim_end();
    expr.ends_with('.') && !expr.ends_with("...")
}

fn is_ident_start(ch: char) -> bool {
    ch == '$' || ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_basic_sfc() {
        let sfc = VueSfc {
            script: Some("export default { props: { msg: String } }".into()),
            script_setup: Some("const count = computed(() => 1)".into()),
            template: VueTemplate {
                children: vec![VueNode::Element(
                    VueElement::new("div")
                        .with_children(vec![VueNode::Interpolation("msg".into())]),
                )],
            },
        };

        assert_eq!(
            sfc.print(),
            "<script>\nexport default { props: { msg: String } }\n</script>\n\n<script setup>\nconst count = computed(() => 1)\n</script>\n\n<template>\n  <div>{{ msg }}</div>\n</template>\n"
        );
    }

    #[test]
    fn prints_attrs_and_nested_children() {
        let template = VueTemplate {
            children: vec![VueNode::Element(
                VueElement::new("button")
                    .with_attrs(vec![
                        VueAttr::Static {
                            name: "class".into(),
                            value: Some("counter".into()),
                        },
                        VueAttr::Static {
                            name: "disabled".into(),
                            value: Some("".into()),
                        },
                        VueAttr::Bind {
                            name: "class".into(),
                            expr: "{ active: props.active }".into(),
                        },
                        VueAttr::On {
                            name: "click".into(),
                            expr: "increment".into(),
                            modifiers: vec!["stop".into()],
                        },
                        VueAttr::Directive(
                            VueDirective::new("slot")
                                .with_dynamic_arg("slotName")
                                .with_expr("{ item }")
                                .with_modifiers(vec!["foo".into()]),
                        ),
                    ])
                    .with_children(vec![VueNode::Element(
                        VueElement::new("span")
                            .with_children(vec![VueNode::Interpolation("props.count".into())]),
                    )]),
            )],
        };

        assert_eq!(
            template.print(),
            "<template>\n  <button class=\"counter\" disabled :class=\"{ active: props.active }\" @click.stop=\"increment\" v-slot:[slotName].foo=\"{ item }\">\n    <span>{{ props.count }}</span>\n  </button>\n</template>\n"
        );
    }

    #[test]
    fn prints_text_runs_between_element_children() {
        let template = VueTemplate {
            children: vec![VueNode::Element(VueElement::new("button").with_children(
                vec![
                    VueNode::Element(VueElement::new("i")),
                    VueNode::Text(" ".into()),
                    VueNode::Interpolation("label".into()),
                    VueNode::Text(" ".into()),
                    VueNode::Interpolation("name".into()),
                    VueNode::Element(VueElement::new("span")),
                    VueNode::Text(" tail".into()),
                ],
            ))],
        };

        assert_eq!(
            template.print(),
            "<template>\n  <button>\n    <i />\n     {{ label }} {{ name }}\n    <span />\n     tail\n  </button>\n</template>\n"
        );
    }

    #[test]
    fn prints_empty_event_attrs_without_value() {
        let template = VueTemplate {
            children: vec![VueNode::Element(VueElement::new("button").with_attrs(
                vec![VueAttr::On {
                    name: "click".into(),
                    expr: "".into(),
                    modifiers: vec!["stop".into()],
                }],
            ))],
        };

        assert_eq!(
            template.print(),
            "<template>\n  <button @click.stop />\n</template>\n"
        );
    }

    #[test]
    fn prefers_single_quoted_expression_attrs_when_expr_contains_double_quotes() {
        let template = VueTemplate {
            children: vec![VueNode::Element(VueElement::new("button").with_attrs(
                vec![
                    VueAttr::Bind {
                        name: "class".into(),
                        expr: "[ active ? \"is-active\" : \"\" ]".into(),
                    },
                    VueAttr::On {
                        name: "click".into(),
                        expr: "emit(\"select\")".into(),
                        modifiers: Vec::new(),
                    },
                    VueAttr::Directive(VueDirective::new("if").with_expr("status === \"ready\"")),
                    VueAttr::Spread("{ title: \"Ready\" }".into()),
                ],
            ))],
        };

        assert_eq!(
            template.print(),
            "<template>\n  <button :class='[ active ? \"is-active\" : \"\" ]' @click='emit(\"select\")' v-if='status === \"ready\"' v-bind='{ title: \"Ready\" }' />\n</template>\n"
        );
    }

    #[test]
    fn renames_standalone_identifiers_in_expressions() {
        let mut expr = VueExpr::new("isMyBets ? P % 2 === 0 ? \"P.\" : `${P.id}.${P}` : row.P");

        expr.replace_prefix("P", "index");

        assert_eq!(
            expr.as_str(),
            "isMyBets ? index % 2 === 0 ? \"P.\" : `${index.id}.${index}` : row.P"
        );
    }

    #[test]
    fn renames_identifiers_without_rewriting_object_keys() {
        let mut expr = VueExpr::new("{ P, kept: P, nested: { P }, [P]: P, ...P }");

        expr.replace_prefix("P", "index");

        assert_eq!(
            expr.as_str(),
            "{ P: index, kept: index, nested: { P: index }, [index]: index, ...index }"
        );
    }

    #[test]
    fn renames_identifiers_without_touching_nested_bindings() {
        let mut expr = VueExpr::new("items.map(P => P.id).filter(item => P.includes(item))");

        expr.replace_prefix("P", "index");

        assert_eq!(
            expr.as_str(),
            "items.map(P => P.id).filter(item => index.includes(item))"
        );
    }

    #[test]
    fn renames_identifiers_without_touching_nested_local_decls() {
        let mut expr =
            VueExpr::new("items.map(() => { const P = get(); return P; }).filter(() => P)");

        expr.replace_prefix("P", "index");

        assert_eq!(
            expr.as_str(),
            "items.map(() => { const P = get(); return P; }).filter(() => index)"
        );
    }

    #[test]
    fn prints_control_flow_nodes() {
        let template = VueTemplate {
            children: vec![
                VueNode::If(vec![
                    VueIfBranch {
                        condition: Some("status === 'loading'".into()),
                        node: Box::new(VueNode::Element(
                            VueElement::new("p")
                                .with_children(vec![VueNode::Text("Loading".into())]),
                        )),
                    },
                    VueIfBranch {
                        condition: None,
                        node: Box::new(VueNode::Element(
                            VueElement::new("p").with_children(vec![VueNode::Text("Ready".into())]),
                        )),
                    },
                ]),
                VueNode::For(VueFor {
                    value: "item".into(),
                    source: "items".into(),
                    node: Box::new(VueNode::Element(
                        VueElement::new("span")
                            .with_children(vec![VueNode::Interpolation("item.name".into())]),
                    )),
                    scope: Default::default(),
                }),
            ],
        };

        assert_eq!(
            template.print(),
            "<template>\n  <p v-if=\"status === 'loading'\">Loading</p>\n  <p v-else>Ready</p>\n  <span v-for=\"item in items\">{{ item.name }}</span>\n</template>\n"
        );
    }

    #[test]
    fn combines_nested_control_flow_conditions() {
        let template = VueTemplate {
            children: vec![VueNode::If(vec![
                VueIfBranch {
                    condition: Some("isLoaded".into()),
                    node: Box::new(VueNode::If(vec![
                        VueIfBranch {
                            condition: Some("bets.length === 0".into()),
                            node: Box::new(VueNode::Element(
                                VueElement::new("p")
                                    .with_children(vec![VueNode::Text("Empty".into())]),
                            )),
                        },
                        VueIfBranch {
                            condition: None,
                            node: Box::new(VueNode::Element(
                                VueElement::new("p")
                                    .with_children(vec![VueNode::Text("Loaded".into())]),
                            )),
                        },
                    ])),
                },
                VueIfBranch {
                    condition: None,
                    node: Box::new(VueNode::Element(
                        VueElement::new("p").with_children(vec![VueNode::Text("Loading".into())]),
                    )),
                },
            ])],
        };

        assert_eq!(
            template.print(),
            "<template>\n  <p v-if=\"isLoaded &amp;&amp; bets.length === 0\">Empty</p>\n  <p v-else-if=\"isLoaded\">Loaded</p>\n  <p v-else>Loading</p>\n</template>\n"
        );
    }

    #[test]
    fn separates_conditional_and_loop_control_flow_wrappers() {
        let template = VueTemplate {
            children: vec![VueNode::If(vec![
                VueIfBranch {
                    condition: Some("hasAll".into()),
                    node: Box::new(VueNode::Element(VueElement::new("slot").with_attrs(vec![
                        VueAttr::Static {
                            name: "name".into(),
                            value: Some("All".into()),
                        },
                    ]))),
                },
                VueIfBranch {
                    condition: None,
                    node: Box::new(VueNode::For(VueFor {
                        value: "item".into(),
                        source: "tabs".into(),
                        node: Box::new(VueNode::If(vec![VueIfBranch {
                            condition: Some("item.name === currentTab".into()),
                            node: Box::new(VueNode::Element(VueElement::new("slot").with_attrs(
                                vec![VueAttr::Bind {
                                    name: "key".into(),
                                    expr: "item.name".into(),
                                }],
                            ))),
                        }])),
                        scope: Default::default(),
                    })),
                },
            ])],
        };

        assert_eq!(
            template.print(),
            "<template>\n  <slot v-if=\"hasAll\" name=\"All\" />\n  <template v-else>\n    <template v-for=\"item in tabs\">\n      <slot v-if=\"item.name === currentTab\" :key=\"item.name\" />\n    </template>\n  </template>\n</template>\n"
        );
    }

    #[test]
    fn wraps_fragment_and_text_control_flow_in_template() {
        let template = VueTemplate {
            children: vec![
                VueNode::If(vec![VueIfBranch {
                    condition: Some("visible".into()),
                    node: Box::new(VueNode::Fragment(vec![
                        VueNode::Element(
                            VueElement::new("span").with_children(vec![VueNode::Text("A".into())]),
                        ),
                        VueNode::Element(
                            VueElement::new("strong")
                                .with_children(vec![VueNode::Text("B".into())]),
                        ),
                    ])),
                }]),
                VueNode::If(vec![VueIfBranch {
                    condition: Some("ready".into()),
                    node: Box::new(VueNode::Interpolation("message".into())),
                }]),
            ],
        };

        assert_eq!(
            template.print(),
            "<template>\n  <template v-if=\"visible\">\n    <span>A</span>\n    <strong>B</strong>\n  </template>\n  <template v-if=\"ready\">\n    {{ message }}\n  </template>\n</template>\n"
        );
    }

    #[test]
    fn prints_raw_static_html() {
        let template = VueTemplate {
            children: vec![VueNode::Element(VueElement::new("section").with_children(
                vec![VueNode::RawHtml(
                    "<svg viewBox=\"0 0 10 10\"><path d=\"M0 0h10v10H0z\"></path></svg>".into(),
                )],
            ))],
        };

        assert_eq!(
            template.print(),
            "<template>\n  <section>\n    <svg viewBox=\"0 0 10 10\"><path d=\"M0 0h10v10H0z\"></path></svg>\n  </section>\n</template>\n"
        );
    }

    #[test]
    fn escapes_text_mustache_markers() {
        let template = VueTemplate {
            children: vec![VueNode::Element(
                VueElement::new("p")
                    .with_children(vec![VueNode::Text("literal {{ value }}".into())]),
            )],
        };

        assert_eq!(
            template.print(),
            "<template>\n  <p>literal &#123;&#123; value }}</p>\n</template>\n"
        );
    }

    #[test]
    fn raw_static_html_cannot_close_template_block() {
        let template = VueTemplate {
            children: vec![VueNode::RawHtml(
                "</template><script>alert(1)</script>".into(),
            )],
        };

        assert_eq!(
            template.print(),
            "<template>\n  &lt;/template><script>alert(1)</script>\n</template>\n"
        );
    }

    #[test]
    fn interpolation_string_cannot_close_mustache() {
        let template = VueTemplate {
            children: vec![VueNode::Element(VueElement::new("p").with_children(vec![
                VueNode::Interpolation(r#"msg + "}}note""#.into()),
            ]))],
        };

        assert_eq!(
            template.print(),
            "<template>\n  <p>{{ msg + \"}\\u007dnote\" }}</p>\n</template>\n"
        );
    }

    #[test]
    fn raw_static_html_escapes_mixed_case_template_and_mustache() {
        let template = VueTemplate {
            children: vec![VueNode::RawHtml("</Template><span>{{ raw }}</span>".into())],
        };

        assert_eq!(
            template.print(),
            "<template>\n  &lt;/Template><span>&#123;&#123; raw }}</span>\n</template>\n"
        );
    }

    #[test]
    fn script_strings_cannot_close_script_block() {
        let sfc = VueSfc {
            script: Some(r#"const html = "</script><div>";"#.into()),
            script_setup: None,
            template: VueTemplate::default(),
        };

        assert_eq!(
            sfc.print(),
            "<script>\nconst html = \"<\\/script><div>\";\n</script>\n\n<template>\n</template>\n"
        );
    }

    #[test]
    fn escapes_text_attrs_and_comments() {
        let template = VueTemplate {
            children: vec![
                VueNode::Element(
                    VueElement::new("div")
                        .with_attrs(vec![VueAttr::Static {
                            name: "title".into(),
                            value: Some("\"quoted\" <tag>".into()),
                        }])
                        .with_children(vec![
                            VueNode::Text("Tom & <Jerry>".into()),
                            VueNode::RawExpr("a--b".into()),
                        ]),
                ),
                VueNode::Comment("a--b".into()),
            ],
        };

        assert_eq!(
            template.print(),
            "<template>\n  <div title=\"&quot;quoted&quot; &lt;tag>\">Tom &amp; &lt;Jerry&gt;{{ a--b }}</div>\n  <!-- a- -b -->\n</template>\n"
        );
    }

    #[test]
    fn labels_unsupported_vnode_children() {
        let template = VueTemplate {
            children: vec![VueNode::Element(VueElement::new("div").with_children(
                vec![VueNode::Unsupported(VueUnsupported::vnode_children(
                    "renderSlides(slides)",
                ))],
            ))],
        };

        assert_eq!(
            template.print(),
            "<template>\n  <div>\n    <!-- wakaru: vnode-children: renderSlides(slides) -->\n  </div>\n</template>\n"
        );
    }
}
