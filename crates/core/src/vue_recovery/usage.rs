use std::collections::HashSet;

use swc_core::atoms::Atom;

use crate::js_names::is_valid_identifier_name;
use crate::vue_template::{VueAttr, VueDirectiveArg, VueNode, VueTemplateScope};

use super::{collect_js_unshadowed_ident_refs, collect_js_unshadowed_read_refs};

#[derive(Default)]
pub(super) struct VueTemplateUsage {
    pub(super) expr_refs: HashSet<Atom>,
    pub(super) read_refs: HashSet<Atom>,
    pub(super) event_refs: HashSet<Atom>,
    pub(super) for_source_refs: HashSet<Atom>,
    pub(super) static_ref_names: Vec<String>,
}

impl VueTemplateUsage {
    pub(super) fn new(root: &VueNode) -> Self {
        let mut usage = Self::default();
        let mut scopes = TemplateLocalScopes::default();
        usage.collect_node(root, &mut scopes);
        usage
            .static_ref_names
            .retain(|name| is_valid_identifier_name(name));
        usage.static_ref_names.sort();
        usage.static_ref_names.dedup();
        usage
    }

    fn collect_node(&mut self, node: &VueNode, scopes: &mut TemplateLocalScopes) {
        match node {
            VueNode::Element(element) => {
                for attr in &element.attrs {
                    self.collect_attr(attr, scopes);
                }
                let scoped_attr = element.attrs.iter().find_map(attr_template_scope);
                let pushed = scoped_attr.is_some_and(|scope| scopes.push(scope));
                for child in &element.children {
                    self.collect_node(child, scopes);
                }
                if pushed {
                    scopes.pop();
                }
            }
            VueNode::Fragment(children) => {
                for child in children {
                    self.collect_node(child, scopes);
                }
            }
            VueNode::If(branches) => {
                for branch in branches {
                    if let Some(condition) = &branch.condition {
                        scopes.collect_ident_refs(condition.as_str(), &mut self.expr_refs);
                        scopes.collect_read_refs(condition.as_str(), &mut self.read_refs);
                    }
                    self.collect_node(&branch.node, scopes);
                }
            }
            VueNode::For(for_node) => {
                scopes.collect_ident_refs(for_node.source.as_str(), &mut self.expr_refs);
                scopes.collect_read_refs(for_node.source.as_str(), &mut self.read_refs);
                scopes.collect_ident_refs(for_node.source.as_str(), &mut self.for_source_refs);
                let pushed = scopes.push(&for_node.scope);
                self.collect_node(&for_node.node, scopes);
                if pushed {
                    scopes.pop();
                }
            }
            VueNode::Interpolation(expr) | VueNode::RawExpr(expr) => {
                scopes.collect_ident_refs(expr.as_str(), &mut self.expr_refs);
                scopes.collect_read_refs(expr.as_str(), &mut self.read_refs);
            }
            VueNode::Unsupported(unsupported) => {
                let pushed = scopes.push(&unsupported.scope);
                scopes.collect_ident_refs(unsupported.expr.as_str(), &mut self.expr_refs);
                scopes.collect_read_refs(unsupported.expr.as_str(), &mut self.read_refs);
                if pushed {
                    scopes.pop();
                }
            }
            VueNode::Text(_) | VueNode::Comment(_) | VueNode::RawHtml(_) => {}
        }
    }

    fn collect_attr(&mut self, attr: &VueAttr, scopes: &TemplateLocalScopes) {
        match attr {
            VueAttr::Static {
                name,
                value: Some(value),
            } if name == "ref" => {
                self.static_ref_names.push(value.clone());
            }
            VueAttr::Bind { expr, .. } | VueAttr::On { expr, .. } | VueAttr::Spread(expr) => {
                scopes.collect_ident_refs(expr.as_str(), &mut self.expr_refs);
                scopes.collect_read_refs(expr.as_str(), &mut self.read_refs);
            }
            VueAttr::Directive(directive) if directive.name == "slot" => {
                if let Some(VueDirectiveArg::Dynamic(expr)) = &directive.arg {
                    scopes.collect_ident_refs(expr.as_str(), &mut self.expr_refs);
                    scopes.collect_read_refs(expr.as_str(), &mut self.read_refs);
                }
            }
            VueAttr::Directive(directive) => {
                if let Some(expr) = &directive.expr {
                    scopes.collect_ident_refs(expr.as_str(), &mut self.expr_refs);
                    scopes.collect_read_refs(expr.as_str(), &mut self.read_refs);
                }
                if let Some(VueDirectiveArg::Dynamic(expr)) = &directive.arg {
                    scopes.collect_ident_refs(expr.as_str(), &mut self.expr_refs);
                    scopes.collect_read_refs(expr.as_str(), &mut self.read_refs);
                }
            }
            VueAttr::Static { .. } => {}
        }

        match attr {
            VueAttr::On { expr, .. } => {
                scopes.collect_ident_refs(expr.as_str(), &mut self.event_refs);
            }
            VueAttr::Directive(directive) if directive.name == "on" => {
                if let Some(expr) = &directive.expr {
                    scopes.collect_ident_refs(expr.as_str(), &mut self.event_refs);
                }
                if let Some(VueDirectiveArg::Dynamic(expr)) = &directive.arg {
                    scopes.collect_ident_refs(expr.as_str(), &mut self.event_refs);
                }
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct TemplateLocalScopes {
    stack: Vec<HashSet<Atom>>,
}

impl TemplateLocalScopes {
    fn push(&mut self, scope: &VueTemplateScope) -> bool {
        if scope.locals.is_empty() {
            return false;
        }
        self.stack.push(
            scope
                .locals
                .iter()
                .map(|local| Atom::from(local.clone()))
                .collect(),
        );
        true
    }

    fn pop(&mut self) {
        self.stack.pop();
    }

    fn is_local(&self, name: &Atom) -> bool {
        self.stack.iter().rev().any(|scope| scope.contains(name))
    }

    fn collect_ident_refs(&self, source: &str, refs: &mut HashSet<Atom>) {
        let mut scoped_refs = HashSet::new();
        collect_js_unshadowed_ident_refs(source, &mut scoped_refs);
        refs.extend(scoped_refs.into_iter().filter(|name| !self.is_local(name)));
    }

    fn collect_read_refs(&self, source: &str, refs: &mut HashSet<Atom>) {
        let mut scoped_refs = HashSet::new();
        collect_js_unshadowed_read_refs(source, &mut scoped_refs);
        refs.extend(scoped_refs.into_iter().filter(|name| !self.is_local(name)));
    }
}

fn attr_template_scope(attr: &VueAttr) -> Option<&VueTemplateScope> {
    match attr {
        VueAttr::Directive(directive) if directive.name == "slot" => Some(&directive.scope),
        _ => None,
    }
}
