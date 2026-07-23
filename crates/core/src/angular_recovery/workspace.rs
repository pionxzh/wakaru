use std::collections::{HashMap, HashSet};

use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, CallExpr, Callee, Expr, ExprOrSpread, MemberProp, Pat,
    SimpleAssignTarget, UpdateExpr,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::syntax::{binding_key, member_prop_name, BindingKey};

/// Canonicalize stable namespace arguments passed into immediately invoked
/// functions. This is generic module-workspace normalization: it does not
/// depend on a bundle format or assign any Ivy meaning.
pub(super) fn canonicalize_immediate_iife_namespace_aliases(
    module: &mut swc_core::ecma::ast::Module,
    unresolved_ctxt: SyntaxContext,
) {
    let mut writes = BindingWriteCollector::default();
    module.visit_with(&mut writes);

    let mut collector = ImmediateIifeAliasCollector {
        unresolved_ctxt,
        aliases: HashMap::new(),
        ambiguous: HashSet::new(),
    };
    module.visit_with(&mut collector);
    collector
        .aliases
        .retain(|binding, _| !writes.bindings.contains(binding));
    if collector.aliases.is_empty() {
        return;
    }

    module.visit_mut_with(&mut NamespaceAliasRewriter {
        aliases: collector.aliases,
    });
}

#[derive(Default)]
struct BindingWriteCollector {
    bindings: HashSet<BindingKey>,
}

impl Visit for BindingWriteCollector {
    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        if let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &assignment.left {
            self.bindings.insert(binding_key(&binding.id));
        }
        assignment.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        if let Expr::Ident(identifier) = update.arg.as_ref() {
            self.bindings.insert(binding_key(identifier));
        }
        update.visit_children_with(self);
    }
}

struct ImmediateIifeAliasCollector {
    unresolved_ctxt: SyntaxContext,
    aliases: HashMap<BindingKey, Box<Expr>>,
    ambiguous: HashSet<BindingKey>,
}

impl Visit for ImmediateIifeAliasCollector {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some((parameters, arguments)) = invoked_function_parameters(call) {
            for (parameter, argument) in parameters.into_iter().zip(arguments) {
                let Pat::Ident(binding) = parameter else {
                    continue;
                };
                if argument.spread.is_some()
                    || !is_stable_namespace_expression(argument.expr.as_ref(), self.unresolved_ctxt)
                {
                    continue;
                }
                let key = binding_key(&binding.id);
                if self.ambiguous.contains(&key) {
                    continue;
                }
                if self
                    .aliases
                    .get(&key)
                    .is_some_and(|existing| existing.as_ref() != argument.expr.as_ref())
                {
                    self.aliases.remove(&key);
                    self.ambiguous.insert(key);
                    continue;
                }
                self.aliases.insert(key, argument.expr.clone());
            }
        }
        call.visit_children_with(self);
    }
}

fn invoked_function_parameters(call: &CallExpr) -> Option<(Vec<&Pat>, &[ExprOrSpread])> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if let Some(parameters) = function_parameters(callee.as_ref()) {
        return Some((parameters, &call.args));
    }

    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    if !matches!(
        &member.prop,
        MemberProp::Ident(property) if property.sym.as_ref() == "call"
    ) {
        return None;
    }
    let parameters = function_parameters(member.obj.as_ref())?;
    Some((parameters, call.args.get(1..)?))
}

fn function_parameters(expression: &Expr) -> Option<Vec<&Pat>> {
    match expression {
        Expr::Fn(function) => Some(
            function
                .function
                .params
                .iter()
                .map(|parameter| &parameter.pat)
                .collect(),
        ),
        Expr::Arrow(arrow) => Some(arrow.params.iter().collect()),
        Expr::Paren(paren) => function_parameters(paren.expr.as_ref()),
        _ => None,
    }
}

fn is_stable_namespace_expression(expression: &Expr, unresolved_ctxt: SyntaxContext) -> bool {
    match expression {
        Expr::This(_) => true,
        Expr::Ident(identifier) => identifier.ctxt == unresolved_ctxt,
        Expr::Member(member) => {
            member_prop_name(&member.prop).is_some()
                && is_stable_namespace_expression(member.obj.as_ref(), unresolved_ctxt)
        }
        Expr::Paren(paren) => is_stable_namespace_expression(paren.expr.as_ref(), unresolved_ctxt),
        _ => false,
    }
}

struct NamespaceAliasRewriter {
    aliases: HashMap<BindingKey, Box<Expr>>,
}

impl VisitMut for NamespaceAliasRewriter {
    fn visit_mut_expr(&mut self, expression: &mut Expr) {
        expression.visit_mut_children_with(self);
        let Expr::Ident(identifier) = expression else {
            return;
        };
        let Some(replacement) = self.aliases.get(&binding_key(identifier)) else {
            return;
        };
        *expression = replacement.as_ref().clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
    use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
    use swc_core::ecma::transforms::base::resolver;

    #[test]
    fn canonicalizes_unwritten_immediate_iife_namespace_parameters() {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let file = cm.new_source_file(
                FileName::Custom("fixture.js".to_string()).into(),
                "(function(namespace) { namespace.value(); }).call(this, this.shared);".to_string(),
            );
            let lexer = Lexer::new(
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                StringInput::from(&*file),
                None,
            );
            let mut module = Parser::new_from(lexer)
                .parse_module()
                .expect("fixture should parse");
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
            canonicalize_immediate_iife_namespace_aliases(
                &mut module,
                SyntaxContext::empty().apply_mark(unresolved_mark),
            );

            let mut finder = NamespaceUseFinder::default();
            module.visit_with(&mut finder);
            assert!(!finder.local_namespace_use);
            assert!(finder.global_namespace_use);
        });
    }

    #[test]
    fn does_not_canonicalize_a_reassigned_parameter() {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let file = cm.new_source_file(
                FileName::Custom("fixture.js".to_string()).into(),
                "(function(namespace) { namespace = other; namespace.value(); })(this.shared);"
                    .to_string(),
            );
            let lexer = Lexer::new(
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                StringInput::from(&*file),
                None,
            );
            let mut module = Parser::new_from(lexer)
                .parse_module()
                .expect("fixture should parse");
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
            canonicalize_immediate_iife_namespace_aliases(
                &mut module,
                SyntaxContext::empty().apply_mark(unresolved_mark),
            );

            let mut finder = NamespaceUseFinder::default();
            module.visit_with(&mut finder);
            assert!(finder.local_namespace_use);
            assert!(!finder.global_namespace_use);
        });
    }

    #[derive(Default)]
    struct NamespaceUseFinder {
        local_namespace_use: bool,
        global_namespace_use: bool,
    }

    impl Visit for NamespaceUseFinder {
        fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
            if member_prop_name(&member.prop).is_some_and(|property| property.as_ref() == "value") {
                match member.obj.as_ref() {
                    Expr::Ident(identifier) if identifier.sym.as_ref() == "namespace" => {
                        self.local_namespace_use = true;
                    }
                    Expr::Member(object)
                        if member_prop_name(&object.prop)
                            .is_some_and(|property| property.as_ref() == "shared") =>
                    {
                        self.global_namespace_use = true;
                    }
                    _ => {}
                }
            }
            member.visit_children_with(self);
        }
    }
}
