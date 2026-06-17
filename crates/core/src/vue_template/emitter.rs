use super::{
    VueAttr, VueDirective, VueDirectiveArg, VueElement, VueFor, VueIfBranch, VueNode, VueSfc,
    VueTemplate,
};

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
