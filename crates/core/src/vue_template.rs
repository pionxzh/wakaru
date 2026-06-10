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
pub enum VueNode {
    Element(VueElement),
    Text(String),
    Interpolation(String),
    Comment(String),
    RawExpr(String),
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
        expr: String,
    },
    On {
        name: String,
        expr: String,
    },
    Directive {
        name: String,
        arg: Option<String>,
        expr: Option<String>,
    },
    Spread(String),
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
        let mut out = String::new();
        out.push_str("<template>\n");
        for child in &self.children {
            print_node(child, 1, &mut out);
        }
        out.push_str("</template>\n");
        out
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

fn print_node(node: &VueNode, depth: usize, out: &mut String) {
    match node {
        VueNode::Element(element) => print_element(element, depth, out),
        VueNode::Text(text) => {
            indent(depth, out);
            out.push_str(&escape_text(text));
            out.push('\n');
        }
        VueNode::Interpolation(expr) => {
            indent(depth, out);
            out.push_str("{{ ");
            out.push_str(expr.trim());
            out.push_str(" }}\n");
        }
        VueNode::Comment(comment) => {
            indent(depth, out);
            out.push_str("<!-- ");
            out.push_str(comment.trim());
            out.push_str(" -->\n");
        }
        VueNode::RawExpr(expr) => {
            indent(depth, out);
            out.push_str("<!-- wakaru: ");
            out.push_str(&escape_comment(expr.trim()));
            out.push_str(" -->\n");
        }
    }
}

fn print_element(element: &VueElement, depth: usize, out: &mut String) {
    indent(depth, out);
    out.push('<');
    out.push_str(&element.tag);
    for attr in &element.attrs {
        out.push(' ');
        print_attr(attr, out);
    }

    if element.children.is_empty() {
        out.push_str(" />\n");
        return;
    }

    if is_inline_children(&element.children) {
        out.push('>');
        print_inline_children(&element.children, out);
        out.push_str("</");
        out.push_str(&element.tag);
        out.push_str(">\n");
        return;
    }

    out.push_str(">\n");
    for child in &element.children {
        print_node(child, depth + 1, out);
    }
    indent(depth, out);
    out.push_str("</");
    out.push_str(&element.tag);
    out.push_str(">\n");
}

fn print_attr(attr: &VueAttr, out: &mut String) {
    match attr {
        VueAttr::Static { name, value } => {
            out.push_str(name);
            if let Some(value) = value {
                out.push_str("=\"");
                out.push_str(&escape_attr(value));
                out.push('"');
            }
        }
        VueAttr::Bind { name, expr } => {
            out.push(':');
            out.push_str(name);
            out.push_str("=\"");
            out.push_str(&escape_attr(expr.trim()));
            out.push('"');
        }
        VueAttr::On { name, expr } => {
            out.push('@');
            out.push_str(name);
            out.push_str("=\"");
            out.push_str(&escape_attr(expr.trim()));
            out.push('"');
        }
        VueAttr::Directive { name, arg, expr } => {
            out.push_str("v-");
            out.push_str(name);
            if let Some(arg) = arg {
                out.push(':');
                out.push_str(arg);
            }
            if let Some(expr) = expr {
                out.push_str("=\"");
                out.push_str(&escape_attr(expr.trim()));
                out.push('"');
            }
        }
        VueAttr::Spread(expr) => {
            out.push_str("v-bind=\"");
            out.push_str(&escape_attr(expr.trim()));
            out.push('"');
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

fn print_inline_children(children: &[VueNode], out: &mut String) {
    for child in children {
        match child {
            VueNode::Text(text) => out.push_str(&escape_text(text)),
            VueNode::Interpolation(expr) => {
                out.push_str("{{ ");
                out.push_str(expr.trim());
                out.push_str(" }}");
            }
            VueNode::RawExpr(expr) => {
                out.push_str("{{ ");
                out.push_str(expr.trim());
                out.push_str(" }}");
            }
            VueNode::Element(_) | VueNode::Comment(_) => {
                unreachable!("checked by is_inline_children")
            }
        }
    }
}

fn indent(depth: usize, out: &mut String) {
    for _ in 0..depth {
        out.push_str("  ");
    }
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
                        },
                    ])
                    .with_children(vec![VueNode::Element(
                        VueElement::new("span")
                            .with_children(vec![VueNode::Interpolation("props.count".into())]),
                    )]),
            )],
        };

        assert_eq!(
            template.print(),
            "<template>\n  <button class=\"counter\" :class=\"{ active: props.active }\" @click=\"increment\">\n    <span>{{ props.count }}</span>\n  </button>\n</template>\n"
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
