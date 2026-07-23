use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignTarget, BlockStmt, BlockStmtOrExpr, CallExpr, Callee, Expr,
    FnDecl, Function, Pat, ReturnStmt, SimpleAssignTarget, Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::{symbol_identity, SymbolIdentity};
use crate::angular_recovery::syntax::{binding_key, member_prop_name, BindingKey};
use crate::angular_recovery::PreparedAngularModule;

pub(super) fn infer_ivy_roles(
    modules: &[PreparedAngularModule],
) -> Vec<(SymbolIdentity, &'static str)> {
    let mut functions = Vec::new();
    for prepared in modules {
        let mut collector = RuntimeFunctionCollector {
            unresolved_ctxt: prepared.unresolved_ctxt,
            functions: Vec::new(),
        };
        prepared.module.visit_with(&mut collector);
        functions.extend(collector.functions);
    }

    let mut inferred = functions
        .iter()
        .filter(|function| is_define_component_shape(function))
        .map(|function| (function.identity.clone(), "ɵɵdefineComponent"))
        .collect::<Vec<_>>();
    inferred.extend(infer_element_family(&functions));
    inferred
}

#[derive(Clone)]
struct RuntimeFunction {
    identity: SymbolIdentity,
    params: Vec<Pat>,
    body: BlockStmt,
    unresolved_ctxt: SyntaxContext,
}

struct RuntimeFunctionCollector {
    unresolved_ctxt: SyntaxContext,
    functions: Vec<RuntimeFunction>,
}

impl RuntimeFunctionCollector {
    fn record_function(&mut self, target: &Expr, function: &Function) {
        let Some(identity) = symbol_identity(target, self.unresolved_ctxt) else {
            return;
        };
        let Some(body) = function.body.as_ref() else {
            return;
        };
        self.functions.push(RuntimeFunction {
            identity,
            params: function
                .params
                .iter()
                .map(|param| param.pat.clone())
                .collect(),
            body: body.clone(),
            unresolved_ctxt: self.unresolved_ctxt,
        });
    }

    fn record_arrow(&mut self, target: &Expr, arrow: &ArrowExpr) {
        let Some(identity) = symbol_identity(target, self.unresolved_ctxt) else {
            return;
        };
        let body = match arrow.body.as_ref() {
            BlockStmtOrExpr::BlockStmt(body) => body.clone(),
            BlockStmtOrExpr::Expr(expression) => BlockStmt {
                span: DUMMY_SP,
                ctxt: SyntaxContext::empty(),
                stmts: vec![Stmt::Return(ReturnStmt {
                    span: DUMMY_SP,
                    arg: Some(expression.clone()),
                })],
            },
        };
        self.functions.push(RuntimeFunction {
            identity,
            params: arrow.params.clone(),
            body,
            unresolved_ctxt: self.unresolved_ctxt,
        });
    }

    fn record_expression(&mut self, target: &Expr, value: &Expr) {
        match value {
            Expr::Fn(function) => self.record_function(target, function.function.as_ref()),
            Expr::Arrow(arrow) => self.record_arrow(target, arrow),
            Expr::Paren(paren) => self.record_expression(target, paren.expr.as_ref()),
            _ => {}
        }
    }
}

impl Visit for RuntimeFunctionCollector {
    fn visit_fn_decl(&mut self, declaration: &FnDecl) {
        self.record_function(
            &Expr::Ident(declaration.ident.clone()),
            declaration.function.as_ref(),
        );
        declaration.function.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Pat::Ident(binding), Some(value)) = (&declarator.name, declarator.init.as_deref()) {
            self.record_expression(&Expr::Ident(binding.id.clone()), value);
        }
        declarator.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        if let Some(target) = assignment_target_expression(&assignment.left) {
            self.record_expression(&target, assignment.right.as_ref());
        }
        assignment.visit_children_with(self);
    }
}

fn assignment_target_expression(target: &AssignTarget) -> Option<Expr> {
    match target {
        AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
            Some(Expr::Ident(binding.id.clone()))
        }
        AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
            Some(Expr::Member(member.clone()))
        }
        AssignTarget::Simple(SimpleAssignTarget::Paren(paren)) => Some(paren.expr.as_ref().clone()),
        _ => None,
    }
}

fn is_define_component_shape(function: &RuntimeFunction) -> bool {
    let [Pat::Ident(parameter)] = function.params.as_slice() else {
        return false;
    };
    let parameter = binding_key(&parameter.id);

    let mut returns = ReturnExpressionCollector::default();
    function.body.visit_with(&mut returns);
    returns.expressions.iter().any(|expression| {
        let mut evidence = ReturnedDescriptorBuilder {
            parameter: &parameter,
            unresolved_ctxt: function.unresolved_ctxt,
            matched: false,
        };
        expression.visit_with(&mut evidence);
        evidence.matched
    })
}

#[derive(Default)]
struct ReturnExpressionCollector {
    expressions: Vec<Box<Expr>>,
}

impl Visit for ReturnExpressionCollector {
    fn visit_return_stmt(&mut self, statement: &ReturnStmt) {
        if let Some(expression) = &statement.arg {
            self.expressions.push(expression.clone());
        }
    }

    fn visit_function(&mut self, _function: &Function) {}

    fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
}

struct ReturnedDescriptorBuilder<'a> {
    parameter: &'a BindingKey,
    unresolved_ctxt: SyntaxContext,
    matched: bool,
}

impl Visit for ReturnedDescriptorBuilder<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if self.matched {
            return;
        }
        for argument in &call.args {
            let Expr::Arrow(arrow) = argument.expr.as_ref() else {
                continue;
            };
            let mut evidence = DescriptorBuilderEvidence {
                parameter: self.parameter,
                unresolved_ctxt: self.unresolved_ctxt,
                parameter_fields: HashSet::new(),
                has_object_assign: false,
            };
            arrow.visit_with(&mut evidence);
            if evidence.has_object_assign
                && ["template", "dependencies", "styles"].iter().all(|name| {
                    evidence
                        .parameter_fields
                        .iter()
                        .any(|field| field.as_ref() == *name)
                })
            {
                self.matched = true;
                return;
            }
        }
        call.visit_children_with(self);
    }
}

struct DescriptorBuilderEvidence<'a> {
    parameter: &'a BindingKey,
    unresolved_ctxt: SyntaxContext,
    parameter_fields: HashSet<Atom>,
    has_object_assign: bool,
}

impl Visit for DescriptorBuilderEvidence<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if call.args.len() >= 3 && is_unresolved_object_assign(&call.callee, self.unresolved_ctxt) {
            self.has_object_assign = true;
        }
        call.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
        if let Expr::Ident(object) = member.obj.as_ref() {
            if binding_key(object) == *self.parameter {
                if let Some(property) = member_prop_name(&member.prop) {
                    self.parameter_fields.insert(property);
                }
            }
        }
        member.visit_children_with(self);
    }
}

fn is_unresolved_object_assign(callee: &Callee, unresolved_ctxt: SyntaxContext) -> bool {
    let Callee::Expr(callee) = callee else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    let Expr::Ident(object) = member.obj.as_ref() else {
        return false;
    };
    object.ctxt == unresolved_ctxt
        && object.sym.as_ref() == "Object"
        && member_prop_name(&member.prop).is_some_and(|property| property.as_ref() == "assign")
}

fn infer_element_family(functions: &[RuntimeFunction]) -> Vec<(SymbolIdentity, &'static str)> {
    let mut by_identity: HashMap<&SymbolIdentity, Vec<&RuntimeFunction>> = HashMap::new();
    for function in functions {
        by_identity
            .entry(&function.identity)
            .or_default()
            .push(function);
    }

    let mut inferred = Vec::new();
    for wrapper in functions {
        let Some(parameters) = plain_parameter_bindings(wrapper) else {
            continue;
        };
        if parameters.len() != 4 || !returns_identity(wrapper, &wrapper.identity) {
            continue;
        }
        let calls = direct_calls(wrapper);
        if calls.len() != 2 {
            continue;
        }
        let start = &calls[0];
        let end = &calls[1];
        if !forwards_parameters(start, &parameters)
            || !end.arguments.is_empty()
            || start.callee == end.callee
            || start.callee == wrapper.identity
            || end.callee == wrapper.identity
        {
            continue;
        }
        if !has_unique_self_returning_arity(&by_identity, &start.callee, 4)
            || !has_unique_self_returning_arity(&by_identity, &end.callee, 0)
        {
            continue;
        }
        inferred.push((wrapper.identity.clone(), "ɵɵelement"));
        inferred.push((start.callee.clone(), "ɵɵelementStart"));
        inferred.push((end.callee.clone(), "ɵɵelementEnd"));
    }
    inferred
}

fn plain_parameter_bindings(function: &RuntimeFunction) -> Option<Vec<BindingKey>> {
    function
        .params
        .iter()
        .map(|parameter| {
            let Pat::Ident(binding) = parameter else {
                return None;
            };
            Some(binding_key(&binding.id))
        })
        .collect()
}

fn returns_identity(function: &RuntimeFunction, identity: &SymbolIdentity) -> bool {
    let mut returns = ReturnExpressionCollector::default();
    function.body.visit_with(&mut returns);
    returns.expressions.iter().any(|expression| {
        let mut finder = IdentityFinder {
            wanted: identity,
            unresolved_ctxt: function.unresolved_ctxt,
            found: false,
        };
        expression.visit_with(&mut finder);
        finder.found
    })
}

struct IdentityFinder<'a> {
    wanted: &'a SymbolIdentity,
    unresolved_ctxt: SyntaxContext,
    found: bool,
}

impl Visit for IdentityFinder<'_> {
    fn visit_expr(&mut self, expression: &Expr) {
        if symbol_identity(expression, self.unresolved_ctxt).as_ref() == Some(self.wanted) {
            self.found = true;
            return;
        }
        expression.visit_children_with(self);
    }

    fn visit_function(&mut self, _function: &Function) {}

    fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
}

struct DirectCall {
    callee: SymbolIdentity,
    arguments: Vec<Box<Expr>>,
}

fn direct_calls(function: &RuntimeFunction) -> Vec<DirectCall> {
    let mut collector = DirectCallCollector {
        unresolved_ctxt: function.unresolved_ctxt,
        calls: Vec::new(),
    };
    function.body.visit_with(&mut collector);
    collector.calls
}

struct DirectCallCollector {
    unresolved_ctxt: SyntaxContext,
    calls: Vec<DirectCall>,
}

impl Visit for DirectCallCollector {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        if let Some(identity) = symbol_identity(callee.as_ref(), self.unresolved_ctxt) {
            self.calls.push(DirectCall {
                callee: identity,
                arguments: call
                    .args
                    .iter()
                    .map(|argument| argument.expr.clone())
                    .collect(),
            });
        }
    }

    fn visit_function(&mut self, _function: &Function) {}

    fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
}

fn forwards_parameters(call: &DirectCall, parameters: &[BindingKey]) -> bool {
    call.arguments.len() == parameters.len()
        && call
            .arguments
            .iter()
            .zip(parameters)
            .all(|(argument, parameter)| {
                matches!(
                    argument.as_ref(),
                    Expr::Ident(identifier) if binding_key(identifier) == *parameter
                )
            })
}

fn has_unique_self_returning_arity(
    functions: &HashMap<&SymbolIdentity, Vec<&RuntimeFunction>>,
    identity: &SymbolIdentity,
    arity: usize,
) -> bool {
    let Some(candidates) = functions.get(identity) else {
        return false;
    };
    let mut matching = candidates.iter().filter(|candidate| {
        candidate.params.len() == arity && returns_identity(candidate, identity)
    });
    matching.next().is_some() && matching.next().is_none()
}
