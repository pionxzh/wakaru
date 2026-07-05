use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, AssignTargetPat, ObjectPatProp, Pat, SimpleAssignTarget,
};
use swc_core::ecma::visit::{Visit, VisitWith};

pub(super) struct ScopeStack(Vec<HashSet<Atom>>);

impl ScopeStack {
    pub(super) fn new() -> Self {
        Self(vec![HashSet::new()])
    }

    pub(super) fn push_scope(&mut self) {
        self.0.push(HashSet::new());
    }

    pub(super) fn pop_scope(&mut self) {
        self.0.pop();
    }

    pub(super) fn depth(&self) -> usize {
        self.0.len()
    }

    pub(super) fn declare(&mut self, sym: &Atom) {
        if let Some(scope) = self.0.last_mut() {
            scope.insert(sym.clone());
        }
    }

    pub(super) fn declare_pat(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(binding) => self.declare(&binding.id.sym),
            Pat::Array(array) => {
                for elem in array.elems.iter().flatten() {
                    self.declare_pat(elem);
                }
            }
            Pat::Object(object) => {
                for prop in &object.props {
                    match prop {
                        ObjectPatProp::KeyValue(key_value) => self.declare_pat(&key_value.value),
                        ObjectPatProp::Assign(assign) => self.declare(&assign.key.sym),
                        ObjectPatProp::Rest(rest) => self.declare_pat(&rest.arg),
                    }
                }
            }
            Pat::Rest(rest) => self.declare_pat(&rest.arg),
            Pat::Assign(assign) => self.declare_pat(&assign.left),
            Pat::Expr(_) | Pat::Invalid(_) => {}
        }
    }

    pub(super) fn is_shadowed(&self, sym: &Atom) -> bool {
        self.0.iter().rev().any(|scope| scope.contains(sym))
    }
}

pub(super) fn visit_assign_expr_refs<V: Visit + ?Sized>(assign: &AssignExpr, visitor: &mut V) {
    visit_assign_target_refs(&assign.left, visitor);
    assign.right.visit_with(visitor);
}

pub(super) fn visit_assign_target_refs<V: Visit + ?Sized>(target: &AssignTarget, visitor: &mut V) {
    match target {
        AssignTarget::Simple(target) => visit_simple_assign_target_refs(target, visitor),
        AssignTarget::Pat(target) => visit_assign_target_pat_refs(target, visitor),
    }
}

pub(super) fn visit_simple_assign_target_refs<V: Visit + ?Sized>(
    target: &SimpleAssignTarget,
    visitor: &mut V,
) {
    match target {
        SimpleAssignTarget::Ident(binding) => binding.id.visit_with(visitor),
        SimpleAssignTarget::Member(member) => member.visit_with(visitor),
        SimpleAssignTarget::Paren(paren) => paren.expr.visit_with(visitor),
        SimpleAssignTarget::TsAs(ts_as) => {
            ts_as.expr.visit_with(visitor);
            ts_as.type_ann.visit_with(visitor);
        }
        SimpleAssignTarget::TsSatisfies(ts_satisfies) => {
            ts_satisfies.expr.visit_with(visitor);
            ts_satisfies.type_ann.visit_with(visitor);
        }
        SimpleAssignTarget::TsNonNull(ts_non_null) => {
            ts_non_null.expr.visit_with(visitor);
        }
        SimpleAssignTarget::TsTypeAssertion(ts_assertion) => {
            ts_assertion.expr.visit_with(visitor);
            ts_assertion.type_ann.visit_with(visitor);
        }
        SimpleAssignTarget::TsInstantiation(ts_instantiation) => {
            ts_instantiation.expr.visit_with(visitor);
            ts_instantiation.type_args.visit_with(visitor);
        }
        _ => target.visit_children_with(visitor),
    }
}

pub(super) fn visit_assign_target_pat_refs<V: Visit + ?Sized>(
    target: &AssignTargetPat,
    visitor: &mut V,
) {
    match target {
        AssignTargetPat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                visit_assignment_pat_refs(elem, visitor);
            }
        }
        AssignTargetPat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(key_value) => {
                        key_value.key.visit_with(visitor);
                        visit_assignment_pat_refs(key_value.value.as_ref(), visitor);
                    }
                    ObjectPatProp::Assign(assign) => {
                        assign.key.visit_with(visitor);
                        if let Some(value) = &assign.value {
                            value.visit_with(visitor);
                        }
                    }
                    ObjectPatProp::Rest(rest) => {
                        visit_assignment_pat_refs(rest.arg.as_ref(), visitor);
                    }
                }
            }
        }
        AssignTargetPat::Invalid(_) => {}
    }
}

pub(super) fn visit_assignment_pat_refs<V: Visit + ?Sized>(pat: &Pat, visitor: &mut V) {
    match pat {
        Pat::Ident(binding) => binding.id.visit_with(visitor),
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                visit_assignment_pat_refs(elem, visitor);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(key_value) => {
                        key_value.key.visit_with(visitor);
                        visit_assignment_pat_refs(key_value.value.as_ref(), visitor);
                    }
                    ObjectPatProp::Assign(assign) => {
                        assign.key.visit_with(visitor);
                        if let Some(value) = &assign.value {
                            value.visit_with(visitor);
                        }
                    }
                    ObjectPatProp::Rest(rest) => {
                        visit_assignment_pat_refs(rest.arg.as_ref(), visitor);
                    }
                }
            }
        }
        Pat::Assign(assign) => {
            visit_assignment_pat_refs(assign.left.as_ref(), visitor);
            assign.right.visit_with(visitor);
        }
        Pat::Rest(rest) => visit_assignment_pat_refs(rest.arg.as_ref(), visitor),
        Pat::Expr(expr) => expr.visit_with(visitor),
        Pat::Invalid(_) => {}
    }
}
