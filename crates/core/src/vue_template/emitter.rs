use super::{
    VueAttr, VueDirective, VueDirectiveArg, VueElement, VueFor, VueIfBranch, VueNode, VueSfc,
    VueTemplate, VueUnsupported, VueUnsupportedKind, VueUnsupportedSource,
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
        if let Some(script_setup) = self
            .script_setup
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            out.push_str("<script setup>\n");
            out.push_str(script_setup);
            if !script_setup.ends_with('\n') {
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
            VueNode::RawHtml(html) => {
                for line in html.lines() {
                    self.indent(depth);
                    self.out.push_str(line);
                    self.out.push('\n');
                }
            }
            VueNode::RawExpr(expr) => {
                self.indent(depth);
                self.out.push_str("<!-- wakaru: ");
                self.out.push_str(&escape_comment(expr.as_str().trim()));
                self.out.push_str(" -->\n");
            }
            VueNode::Unsupported(unsupported) => self.emit_unsupported(unsupported, depth),
        }
    }

    fn emit_unsupported(&mut self, unsupported: &VueUnsupported, depth: usize) {
        self.indent(depth);
        self.out.push_str("<!-- wakaru: ");
        self.out.push_str(match unsupported.kind {
            VueUnsupportedKind::VNodeChildren => "vnode-children: ",
        });
        self.out
            .push_str(&escape_comment(unsupported.expr.as_str().trim()));
        if let Some(source) = &unsupported.source {
            self.emit_unsupported_source(source);
        }
        self.out.push_str(" -->\n");
    }

    fn emit_unsupported_source(&mut self, source: &VueUnsupportedSource) {
        match source {
            VueUnsupportedSource::RenderLocalSlotPartitionChildren { binding } => {
                self.out
                    .push_str("; source: render-local slot-partition children \"");
                self.out.push_str(&escape_comment(binding));
                self.out.push('"');
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
                if leading_attrs.is_empty() {
                    for child in children {
                        self.emit_node(child, depth);
                    }
                } else if children.len() == 1 {
                    self.emit_node_with_leading_attrs(&children[0], depth, leading_attrs);
                } else {
                    self.emit_template_wrapper(depth, leading_attrs, children);
                }
            }
            VueNode::If(branches) => {
                if self.emit_nested_if_with_leading_condition(branches, depth, leading_attrs) {
                    return;
                }
                if leading_directive(leading_attrs, "for").is_some() {
                    self.emit_if_template_wrapper(depth, leading_attrs, branches);
                    return;
                }

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
            | VueNode::RawHtml(_)
            | VueNode::RawExpr(_)
            | VueNode::Unsupported(_) => {
                self.emit_template_wrapper(depth, leading_attrs, std::slice::from_ref(node));
            }
        }
    }

    fn emit_nested_if_with_leading_condition(
        &mut self,
        branches: &[VueIfBranch],
        depth: usize,
        leading_attrs: &[VueAttr],
    ) -> bool {
        let Some((condition_index, condition)) = leading_condition_directive(leading_attrs) else {
            return false;
        };
        if let Some((for_index, _)) = leading_directive(leading_attrs, "for") {
            self.emit_if_with_split_leading_attrs(
                branches,
                depth,
                leading_attrs,
                condition_index,
                for_index,
            );
            return true;
        }

        if condition.name == "else" {
            self.emit_if_template_wrapper(depth, leading_attrs, branches);
            return true;
        }

        let Some(outer_condition) = condition.expr.as_ref() else {
            return false;
        };

        let non_condition_attrs = leading_attrs
            .iter()
            .enumerate()
            .filter_map(|(index, attr)| (index != condition_index).then_some(attr.clone()))
            .collect::<Vec<_>>();

        for (index, branch) in branches.iter().enumerate() {
            let directive_name = if condition.name == "if" && index == 0 {
                "if"
            } else {
                "else-if"
            };
            let branch_condition = match &branch.condition {
                Some(inner_condition) => combine_conditions(outer_condition, inner_condition),
                None => outer_condition.clone(),
            };
            let mut attrs = non_condition_attrs.clone();
            attrs.push(VueAttr::Directive(
                VueDirective::new(directive_name).with_expr(branch_condition),
            ));
            self.emit_node_with_leading_attrs(&branch.node, depth, &attrs);
        }
        true
    }

    fn emit_if_with_split_leading_attrs(
        &mut self,
        branches: &[VueIfBranch],
        depth: usize,
        leading_attrs: &[VueAttr],
        condition_index: usize,
        for_index: usize,
    ) {
        let outer_attrs = leading_attrs
            .iter()
            .enumerate()
            .filter_map(|(index, attr)| (index != for_index).then_some(attr.clone()))
            .collect::<Vec<_>>();
        let inner_attrs = leading_attrs
            .iter()
            .enumerate()
            .filter_map(|(index, attr)| (index != condition_index).then_some(attr.clone()))
            .collect::<Vec<_>>();

        self.indent(depth);
        self.out.push_str("<template");
        for attr in &outer_attrs {
            self.out.push(' ');
            self.emit_attr(attr);
        }
        self.out.push_str(">\n");
        self.emit_if_template_wrapper(depth + 1, &inner_attrs, branches);
        self.indent(depth);
        self.out.push_str("</template>\n");
    }

    fn emit_template_wrapper(&mut self, depth: usize, attrs: &[VueAttr], children: &[VueNode]) {
        self.indent(depth);
        self.out.push_str("<template");
        for attr in attrs {
            self.out.push(' ');
            self.emit_attr(attr);
        }
        self.out.push_str(">\n");
        for child in children {
            self.emit_node(child, depth + 1);
        }
        self.indent(depth);
        self.out.push_str("</template>\n");
    }

    fn emit_if_template_wrapper(
        &mut self,
        depth: usize,
        attrs: &[VueAttr],
        branches: &[VueIfBranch],
    ) {
        self.indent(depth);
        self.out.push_str("<template");
        for attr in attrs {
            self.out.push(' ');
            self.emit_attr(attr);
        }
        self.out.push_str(">\n");
        self.emit_if(branches, depth + 1);
        self.indent(depth);
        self.out.push_str("</template>\n");
    }

    fn emit_attr(&mut self, attr: &VueAttr) {
        match attr {
            VueAttr::Static { name, value } => {
                self.out.push_str(name);
                if let Some(value) = value.as_ref().filter(|value| !value.is_empty()) {
                    self.out.push_str("=\"");
                    self.out.push_str(&escape_attr(value));
                    self.out.push('"');
                }
            }
            VueAttr::Bind { name, expr } => {
                self.out.push(':');
                self.out.push_str(name);
                self.emit_expr_attr_value(expr.as_str().trim());
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
                self.emit_expr_attr_value(expr.as_str().trim());
            }
            VueAttr::Directive(directive) => self.emit_directive(directive),
            VueAttr::Spread(expr) => {
                self.out.push_str("v-bind");
                self.emit_expr_attr_value(expr.as_str().trim());
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
            self.emit_expr_attr_value(expr.as_str().trim());
        }
    }

    fn emit_expr_attr_value(&mut self, value: &str) {
        let quote = preferred_expr_attr_quote(value);
        self.out.push('=');
        self.out.push(quote);
        self.out.push_str(&escape_attr_for_quote(value, quote));
        self.out.push(quote);
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
                VueNode::Element(_)
                | VueNode::Fragment(_)
                | VueNode::Comment(_)
                | VueNode::RawHtml(_)
                | VueNode::Unsupported(_) => {
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
    escape_attr_for_quote(value, '"')
}

fn escape_attr_for_quote(value: &str, quote: char) -> String {
    let value = value.replace('&', "&amp;").replace('<', "&lt;");
    match quote {
        '"' => value.replace('"', "&quot;"),
        '\'' => value.replace('\'', "&#39;"),
        _ => value,
    }
}

fn preferred_expr_attr_quote(value: &str) -> char {
    let double_quotes = value.matches('"').count();
    let single_quotes = value.matches('\'').count();
    if double_quotes > single_quotes {
        '\''
    } else {
        '"'
    }
}

fn escape_comment(value: &str) -> String {
    value.replace("--", "- -")
}

fn leading_condition_directive(attrs: &[VueAttr]) -> Option<(usize, &VueDirective)> {
    attrs.iter().enumerate().find_map(|(index, attr)| {
        let VueAttr::Directive(directive) = attr else {
            return None;
        };
        matches!(directive.name.as_str(), "if" | "else-if" | "else").then_some((index, directive))
    })
}

fn leading_directive<'a>(attrs: &'a [VueAttr], name: &str) -> Option<(usize, &'a VueDirective)> {
    attrs.iter().enumerate().find_map(|(index, attr)| {
        let VueAttr::Directive(directive) = attr else {
            return None;
        };
        (directive.name == name).then_some((index, directive))
    })
}

fn combine_conditions(outer: &super::VueExpr, inner: &super::VueExpr) -> super::VueExpr {
    let outer = outer.as_str().trim();
    let inner = inner.as_str().trim();
    format!(
        "{} && {}",
        condition_and_part(outer),
        condition_and_part(inner)
    )
    .into()
}

fn condition_and_part(condition: &str) -> String {
    if condition.is_empty() {
        return condition.to_string();
    }
    if condition_needs_and_parens(condition) {
        format!("({condition})")
    } else {
        condition.to_string()
    }
}

fn condition_needs_and_parens(condition: &str) -> bool {
    condition.contains("||") || condition.contains('?') || condition.contains("=>")
}
