#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VueSfc {
    pub script: Option<String>,
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
    RawExpr(VueExpr),
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VueElement {
    pub tag: String,
    pub attrs: Vec<VueAttr>,
    pub children: Vec<VueNode>,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VueDirectiveArg {
    Static(String),
    Dynamic(VueExpr),
}

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
}

impl VueSfc {
    pub fn print(&self) -> String {
        let mut out = String::new();
        if let Some(script) = self
            .script
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            out.push_str("<script>\n");
            out.push_str(script);
            if !script.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("</script>\n\n");
        }
        out.push_str(&self.template.print());
        out
    }
}

impl VueTemplate {
    pub fn print(&self) -> String {
        let mut emitter = TemplateEmitter::new();
        emitter.emit_template(self);
        emitter.finish()
    }
}

impl VueElement {
    pub fn new(tag: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            attrs: Vec::new(),
            children: Vec::new(),
        }
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

struct TemplateEmitter {
    out: String,
}

impl TemplateEmitter {
    fn new() -> Self {
        Self { out: String::new() }
    }

    fn finish(self) -> String {
        self.out
    }

    fn emit_template(&mut self, template: &VueTemplate) {
        self.out.push_str("<template>\n");
        for child in &template.children {
            self.emit_node(child, 1);
        }
        self.out.push_str("</template>\n");
    }

    fn emit_node(&mut self, node: &VueNode, depth: usize) {
        match node {
            VueNode::Element(element) => self.emit_element(element, depth),
            VueNode::Fragment(children) => {
                for child in children {
                    self.emit_node(child, depth);
                }
            }
            VueNode::If(branches) => self.emit_if(branches, depth),
            VueNode::For(for_node) => self.emit_for(for_node, depth),
            VueNode::Text(text) => {
                self.indent(depth);
                self.out.push_str(&escape_text(text));
                self.out.push('\n');
            }
            VueNode::Interpolation(expr) => {
                self.indent(depth);
                self.out.push_str("{{ ");
                self.out.push_str(expr.as_str().trim());
                self.out.push_str(" }}\n");
            }
            VueNode::Comment(comment) => {
                self.indent(depth);
                self.out.push_str("<!-- ");
                self.out.push_str(comment.trim());
                self.out.push_str(" -->\n");
            }
            VueNode::RawExpr(expr) => {
                self.indent(depth);
                self.out.push_str("<!-- wakaru: ");
                self.out.push_str(&escape_comment(expr.as_str().trim()));
                self.out.push_str(" -->\n");
            }
        }
    }

    fn emit_element(&mut self, element: &VueElement, depth: usize) {
        self.emit_element_with_leading_attrs(element, depth, &[]);
    }

    fn emit_element_with_leading_attrs(
        &mut self,
        element: &VueElement,
        depth: usize,
        leading_attrs: &[VueAttr],
    ) {
        self.indent(depth);
        self.out.push('<');
        self.out.push_str(&element.tag);
        for attr in leading_attrs {
            self.out.push(' ');
            self.emit_attr(attr);
        }
        for attr in &element.attrs {
            self.out.push(' ');
            self.emit_attr(attr);
        }

        if element.children.is_empty() {
            self.out.push_str(" />\n");
            return;
        }

        if is_inline_children(&element.children) {
            self.out.push('>');
            self.emit_inline_children(&element.children);
            self.out.push_str("</");
            self.out.push_str(&element.tag);
            self.out.push_str(">\n");
            return;
        }

        self.out.push_str(">\n");
        for child in &element.children {
            self.emit_node(child, depth + 1);
        }
        self.indent(depth);
        self.out.push_str("</");
        self.out.push_str(&element.tag);
        self.out.push_str(">\n");
    }

    fn emit_if(&mut self, branches: &[VueIfBranch], depth: usize) {
        for (index, branch) in branches.iter().enumerate() {
            let directive = match (&branch.condition, index) {
                (Some(condition), 0) => VueDirective::new("if").with_expr(condition),
                (Some(condition), _) => VueDirective::new("else-if").with_expr(condition),
                (None, _) => VueDirective::new("else"),
            };
            let attr = VueAttr::Directive(directive);
            self.emit_node_with_leading_attrs(&branch.node, depth, &[attr]);
        }
    }

    fn emit_for(&mut self, for_node: &VueFor, depth: usize) {
        let attr = VueAttr::Directive(
            VueDirective::new("for")
                .with_expr(format!("{} in {}", for_node.value, for_node.source)),
        );
        self.emit_node_with_leading_attrs(&for_node.node, depth, &[attr]);
    }

    fn emit_node_with_leading_attrs(
        &mut self,
        node: &VueNode,
        depth: usize,
        leading_attrs: &[VueAttr],
    ) {
        match node {
            VueNode::Element(element) => {
                self.emit_element_with_leading_attrs(element, depth, leading_attrs)
            }
            VueNode::Fragment(children) => {
                if let Some((first, rest)) = children.split_first() {
                    self.emit_node_with_leading_attrs(first, depth, leading_attrs);
                    for child in rest {
                        self.emit_node(child, depth);
                    }
                }
            }
            VueNode::If(branches) => {
                for (index, branch) in branches.iter().enumerate() {
                    let directive = match (&branch.condition, index) {
                        (Some(condition), 0) => VueDirective::new("if").with_expr(condition),
                        (Some(condition), _) => VueDirective::new("else-if").with_expr(condition),
                        (None, _) => VueDirective::new("else"),
                    };
                    let mut attrs = leading_attrs.to_vec();
                    attrs.push(VueAttr::Directive(directive));
                    self.emit_node_with_leading_attrs(&branch.node, depth, &attrs);
                }
            }
            VueNode::For(for_node) => {
                let mut attrs = leading_attrs.to_vec();
                attrs
                    .push(VueAttr::Directive(VueDirective::new("for").with_expr(
                        format!("{} in {}", for_node.value, for_node.source),
                    )));
                self.emit_node_with_leading_attrs(&for_node.node, depth, &attrs);
            }
            VueNode::Text(_)
            | VueNode::Interpolation(_)
            | VueNode::Comment(_)
            | VueNode::RawExpr(_) => {
                let directive_names = leading_attrs
                    .iter()
                    .filter_map(|attr| match attr {
                        VueAttr::Directive(directive) => Some(format!("v-{}", directive.name)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                self.emit_node(
                    &VueNode::Comment(format!("wakaru: {directive_names}")),
                    depth,
                );
                self.emit_node(node, depth);
            }
        }
    }

    fn emit_attr(&mut self, attr: &VueAttr) {
        match attr {
            VueAttr::Static { name, value } => {
                self.out.push_str(name);
                if let Some(value) = value {
                    self.out.push_str("=\"");
                    self.out.push_str(&escape_attr(value));
                    self.out.push('"');
                }
            }
            VueAttr::Bind { name, expr } => {
                self.out.push(':');
                self.out.push_str(name);
                self.out.push_str("=\"");
                self.out.push_str(&escape_attr(expr.as_str().trim()));
                self.out.push('"');
            }
            VueAttr::On {
                name,
                expr,
                modifiers,
            } => {
                self.out.push('@');
                self.out.push_str(name);
                for modifier in modifiers {
                    self.out.push('.');
                    self.out.push_str(modifier);
                }
                self.out.push_str("=\"");
                self.out.push_str(&escape_attr(expr.as_str().trim()));
                self.out.push('"');
            }
            VueAttr::Directive(directive) => self.emit_directive(directive),
            VueAttr::Spread(expr) => {
                self.out.push_str("v-bind=\"");
                self.out.push_str(&escape_attr(expr.as_str().trim()));
                self.out.push('"');
            }
        }
    }

    fn emit_directive(&mut self, directive: &VueDirective) {
        self.out.push_str("v-");
        self.out.push_str(&directive.name);
        if let Some(arg) = &directive.arg {
            match arg {
                VueDirectiveArg::Static(arg) => {
                    self.out.push(':');
                    self.out.push_str(arg);
                }
                VueDirectiveArg::Dynamic(arg) => {
                    self.out.push_str(":[");
                    self.out.push_str(arg.as_str());
                    self.out.push(']');
                }
            }
        }
        for modifier in &directive.modifiers {
            self.out.push('.');
            self.out.push_str(modifier);
        }
        if let Some(expr) = &directive.expr {
            self.out.push_str("=\"");
            self.out.push_str(&escape_attr(expr.as_str().trim()));
            self.out.push('"');
        }
    }

    fn emit_inline_children(&mut self, children: &[VueNode]) {
        for child in children {
            match child {
                VueNode::Text(text) => self.out.push_str(&escape_text(text)),
                VueNode::Interpolation(expr) => {
                    self.out.push_str("{{ ");
                    self.out.push_str(expr.as_str().trim());
                    self.out.push_str(" }}");
                }
                VueNode::RawExpr(expr) => {
                    self.out.push_str("{{ ");
                    self.out.push_str(expr.as_str().trim());
                    self.out.push_str(" }}");
                }
                VueNode::Element(_) | VueNode::Fragment(_) | VueNode::Comment(_) => {
                    unreachable!("checked by is_inline_children")
                }
                VueNode::If(_) | VueNode::For(_) => unreachable!("checked by is_inline_children"),
            }
        }
    }

    fn indent(&mut self, depth: usize) {
        for _ in 0..depth {
            self.out.push_str("  ");
        }
    }
}

fn is_inline_children(children: &[VueNode]) -> bool {
    !children.is_empty()
        && children.iter().all(|child| {
            matches!(
                child,
                VueNode::Text(_) | VueNode::Interpolation(_) | VueNode::RawExpr(_)
            )
        })
}

fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(value: &str) -> String {
    escape_text(value).replace('"', "&quot;")
}

fn escape_comment(value: &str) -> String {
    value.replace("--", "- -")
}

fn rename_expr_prefix(expr: &str, from: &str, to: &str) -> String {
    let mut renamed = expr.replace(&format!("{from}."), &format!("{to}."));
    if renamed == from {
        renamed = to.to_string();
    }
    renamed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_basic_sfc() {
        let sfc = VueSfc {
            script: Some("export default { props: { msg: String } }".into()),
            template: VueTemplate {
                children: vec![VueNode::Element(
                    VueElement::new("div")
                        .with_children(vec![VueNode::Interpolation("msg".into())]),
                )],
            },
        };

        assert_eq!(
            sfc.print(),
            "<script>\nexport default { props: { msg: String } }\n</script>\n\n<template>\n  <div>{{ msg }}</div>\n</template>\n"
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
            "<template>\n  <button class=\"counter\" :class=\"{ active: props.active }\" @click.stop=\"increment\" v-slot:[slotName].foo=\"{ item }\">\n    <span>{{ props.count }}</span>\n  </button>\n</template>\n"
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
                }),
            ],
        };

        assert_eq!(
            template.print(),
            "<template>\n  <p v-if=\"status === 'loading'\">Loading</p>\n  <p v-else>Ready</p>\n  <span v-for=\"item in items\">{{ item.name }}</span>\n</template>\n"
        );
    }

    #[test]
    fn escapes_text_attrs_and_comments() {
        let template = VueTemplate {
            children: vec![VueNode::Element(
                VueElement::new("div")
                    .with_attrs(vec![VueAttr::Static {
                        name: "title".into(),
                        value: Some("\"quoted\" <tag>".into()),
                    }])
                    .with_children(vec![
                        VueNode::Text("Tom & <Jerry>".into()),
                        VueNode::RawExpr("a--b".into()),
                    ]),
            )],
        };

        assert_eq!(
            template.print(),
            "<template>\n  <div title=\"&quot;quoted&quot; &lt;tag&gt;\">Tom &amp; &lt;Jerry&gt;{{ a--b }}</div>\n</template>\n"
        );
    }
}
