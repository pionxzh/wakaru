use crate::collections::{HashMap, HashSet};

use swc_core::ecma::ast::{
    ArrowFunctionBody, AssignExpr, AssignOp, AssignTarget, BindingIdent, CallExpr, Callee, Class,
    Expr, Function, ModuleItem, Pat, ReturnStmt, SimpleAssignTarget, Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::utils::paren::strip_parens;

use super::helper_matcher::{binding_key, expr_binding_key, member_prop_name, BindingKey};

/// Bindings whose current uses still require an ordinary function's
/// `[[Call]]`. Function-to-class rules must not recover these bindings until
/// the requiring call has been consumed by another proven class recovery.
pub(crate) struct CallabilityIndex {
    required: HashSet<BindingKey>,
}

impl CallabilityIndex {
    pub(crate) fn collect_module_items(items: &[ModuleItem]) -> Self {
        collect(items)
    }

    pub(crate) fn collect_stmts(stmts: &[Stmt]) -> Self {
        collect(stmts)
    }

    pub(crate) fn requires_call(&self, binding: &BindingKey) -> bool {
        self.required.contains(binding)
    }
}

fn collect<N>(node: &N) -> CallabilityIndex
where
    N: VisitWith<CallabilityCollector> + ?Sized,
{
    let mut collector = CallabilityCollector::default();
    node.visit_with(&mut collector);

    let mut sources_by_target: HashMap<BindingKey, Vec<BindingKey>> = HashMap::default();
    for (target, source) in collector.aliases {
        sources_by_target.entry(target).or_default().push(source);
    }

    let mut pending = collector.required.iter().cloned().collect::<Vec<_>>();
    while let Some(target) = pending.pop() {
        let Some(sources) = sources_by_target.get(&target) else {
            continue;
        };
        for source in sources {
            if collector.required.insert(source.clone()) {
                pending.push(source.clone());
            }
        }
    }

    CallabilityIndex {
        required: collector.required,
    }
}

#[derive(Default)]
struct CallabilityCollector {
    required: HashSet<BindingKey>,
    /// `target` evaluates to `source`: requiring `target.[[Call]]` therefore
    /// requires `source.[[Call]]` too.
    aliases: Vec<(BindingKey, BindingKey)>,
}

impl CallabilityCollector {
    fn record_iife_param_aliases(&mut self, call: &CallExpr) {
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let inner = strip_parens(callee);
        let params: Vec<&Pat> = match inner {
            Expr::Fn(function) => function
                .function
                .params
                .iter()
                .map(|param| &param.pat)
                .collect(),
            Expr::Arrow(arrow) => arrow.params.iter().collect(),
            _ => return,
        };

        let mut spreads_seen = 0;
        for (argument_index, argument) in call.args.iter().enumerate() {
            if argument.spread.is_some() {
                spreads_seen += 1;
                continue;
            }
            let Some(argument_key) = expr_binding_key(strip_parens(&argument.expr)) else {
                continue;
            };

            if spreads_seen == 0 {
                if let Some(parameter_key) = params
                    .get(argument_index)
                    .and_then(|parameter| pat_binding_key(parameter))
                {
                    self.aliases.push((parameter_key, argument_key));
                }
                continue;
            }

            // A spread can contribute any number of arguments. A later fixed
            // argument may therefore feed any parameter from its minimum
            // position onward; keep every such source callable.
            let minimum_parameter_index = argument_index - spreads_seen;
            for parameter in params.iter().skip(minimum_parameter_index) {
                if let Some(parameter_key) = pat_binding_key(parameter) {
                    self.aliases.push((parameter_key, argument_key.clone()));
                }
            }
        }
    }

    fn record_iife_result_alias(&mut self, target: BindingKey, init: &Expr) {
        let Expr::Call(call) = strip_parens(init) else {
            return;
        };
        let Callee::Expr(callee) = &call.callee else {
            return;
        };

        let mut returns = IifeReturnCollector::default();
        match strip_parens(callee) {
            Expr::Fn(function) => {
                let Some(body) = &function.function.body else {
                    return;
                };
                body.visit_with(&mut returns);
            }
            Expr::Arrow(arrow) => match arrow.body.as_ref() {
                ArrowFunctionBody::FunctionBody(body) => body.visit_with(&mut returns),
                ArrowFunctionBody::Expr(expr) => {
                    if let Some(source) = expr_binding_key(strip_parens(expr)) {
                        returns.bindings.insert(source);
                    }
                }
            },
            _ => return,
        }

        self.aliases.extend(
            returns
                .bindings
                .into_iter()
                .map(|source| (target.clone(), source)),
        );
    }
}

impl Visit for CallabilityCollector {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Some(target), Some(init)) = (pat_binding_key(&declarator.name), &declarator.init) {
            self.record_iife_result_alias(target, init);
        }
        declarator.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        if assignment.op == AssignOp::Assign {
            if let AssignTarget::Simple(SimpleAssignTarget::Ident(target)) = &assignment.left {
                self.record_iife_result_alias(binding_key(&target.id), &assignment.right);
            }
        }
        assignment.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some(binding) = ident_call_or_apply_binding(call) {
            self.required.insert(binding);
        }
        self.record_iife_param_aliases(call);
        call.visit_children_with(self);
    }
}

fn pat_binding_key(pat: &Pat) -> Option<BindingKey> {
    let Pat::Ident(BindingIdent { id, .. }) = pat else {
        return None;
    };
    Some(binding_key(id))
}

fn ident_call_or_apply_binding(call: &CallExpr) -> Option<BindingKey> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return None;
    };
    if !member_prop_name(&member.prop, "call") && !member_prop_name(&member.prop, "apply") {
        return None;
    }
    let Expr::Ident(ident) = strip_parens(&member.obj) else {
        return None;
    };
    Some(binding_key(ident))
}

#[derive(Default)]
struct IifeReturnCollector {
    bindings: HashSet<BindingKey>,
}

impl Visit for IifeReturnCollector {
    fn visit_return_stmt(&mut self, statement: &ReturnStmt) {
        let Some(argument) = statement.arg.as_deref() else {
            return;
        };
        if let Some(binding) = expr_binding_key(strip_parens(argument)) {
            self.bindings.insert(binding);
        }
    }

    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

    fn visit_class(&mut self, _: &Class) {}
}
