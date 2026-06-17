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
    RawHtml(String),
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
            "<template>\n  <div title=\"&quot;quoted&quot; &lt;tag>\">Tom &amp; &lt;Jerry&gt;{{ a--b }}</div>\n</template>\n"
        );
    }
}
