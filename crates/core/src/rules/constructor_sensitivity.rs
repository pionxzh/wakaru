use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, CallExpr, Callee, Class, Expr, Ident,
    Lit, MemberProp, Module, NewExpr, Pat, SimpleAssignTarget, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::analysis::binding_uses::BindingId;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ValueKey {
    root: BindingId,
    properties: Vec<Atom>,
}

impl ValueKey {
    fn binding(ident: &Ident) -> Self {
        Self {
            root: (ident.sym.clone(), ident.ctxt),
            properties: Vec::new(),
        }
    }

    pub(crate) fn with_property(&self, property: Atom) -> Self {
        let mut key = self.clone();
        key.properties.push(property);
        key
    }
}

pub(crate) fn static_member_name(prop: &MemberProp) -> Option<Atom> {
    match prop {
        MemberProp::Ident(ident) => Some(ident.sym.clone()),
        MemberProp::Computed(computed) => match computed.expr.as_ref() {
            Expr::Lit(Lit::Str(value)) => value.value.as_str().map(Atom::from),
            _ => None,
        },
        MemberProp::PrivateName(_) => None,
    }
}

pub(crate) fn expr_value_key(expr: &Expr) -> Option<ValueKey> {
    match expr {
        Expr::Ident(ident) => Some(ValueKey::binding(ident)),
        Expr::Member(member) => {
            let mut key = expr_value_key(&member.obj)?;
            key.properties.push(static_member_name(&member.prop)?);
            Some(key)
        }
        Expr::Paren(paren) => expr_value_key(&paren.expr),
        _ => None,
    }
}

pub(crate) fn pat_value_key(pat: &Pat) -> Option<ValueKey> {
    let Pat::Ident(binding) = pat else {
        return None;
    };
    Some(ValueKey::binding(&binding.id))
}

pub(crate) fn assign_target_value_key(target: &AssignTarget) -> Option<ValueKey> {
    let AssignTarget::Simple(target) = target else {
        return None;
    };
    match target {
        SimpleAssignTarget::Ident(binding) => Some(ValueKey::binding(&binding.id)),
        SimpleAssignTarget::Member(member) => expr_value_key(&Expr::Member(member.clone())),
        _ => None,
    }
}

pub(crate) fn is_construct_call(call: &CallExpr) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    // Conservatively preserve a function passed as the third argument to any
    // `.construct` call. Reflect.construct requires a constructible newTarget,
    // and skipping an arrow recovery for a user-defined method is harmless.
    static_member_name(&member.prop).is_some_and(|name| name == "construct")
}

#[derive(Default)]
struct ConstructorSensitiveUseCollector {
    sensitive: HashSet<ValueKey>,
    aliases: Vec<(ValueKey, ValueKey)>,
}

impl ConstructorSensitiveUseCollector {
    fn mark_expr(&mut self, expr: &Expr) {
        if let Some(key) = expr_value_key(expr) {
            self.sensitive.insert(key);
        }
    }
}

impl Visit for ConstructorSensitiveUseCollector {
    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        if let (Some(target), Some(source)) = (
            pat_value_key(&decl.name),
            decl.init.as_deref().and_then(expr_value_key),
        ) {
            self.aliases.push((target, source));
        }
        decl.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, expr: &AssignExpr) {
        if expr.op == AssignOp::Assign {
            if let (Some(target), Some(source)) = (
                assign_target_value_key(&expr.left),
                expr_value_key(&expr.right),
            ) {
                self.aliases.push((target, source));
            }
        }
        expr.visit_children_with(self);
    }

    fn visit_new_expr(&mut self, expr: &NewExpr) {
        self.mark_expr(&expr.callee);
        expr.visit_children_with(self);
    }

    fn visit_bin_expr(&mut self, expr: &BinExpr) {
        if expr.op == BinaryOp::InstanceOf {
            self.mark_expr(&expr.right);
        }
        expr.visit_children_with(self);
    }

    fn visit_class(&mut self, class: &Class) {
        if let Some(super_class) = &class.super_class {
            self.mark_expr(super_class);
        }
        class.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
        if static_member_name(&member.prop).is_some_and(|name| name == "prototype") {
            self.mark_expr(&member.obj);
        }
        member.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if is_construct_call(call) {
            if let Some(target) = call.args.first() {
                self.mark_expr(&target.expr);
            }
            if let Some(new_target) = call.args.get(2) {
                self.mark_expr(&new_target.expr);
            }
        }
        call.visit_children_with(self);
    }
}

pub(crate) fn collect_constructor_sensitive_values(module: &Module) -> HashSet<ValueKey> {
    let mut collector = ConstructorSensitiveUseCollector::default();
    module.visit_with(&mut collector);

    let mut sources_by_target: HashMap<ValueKey, Vec<ValueKey>> = HashMap::new();
    for (target, source) in collector.aliases {
        sources_by_target.entry(target).or_default().push(source);
    }

    // Extending a member suffix is only sound and bounded for aliases between
    // bindings (`alias = namespace`). Applying the same rewrite to member
    // aliases can grow paths indefinitely in cyclic, flow-insensitive alias
    // graphs (`a = b.x; b = a.y`). Exact member aliases are still followed by
    // the lookup below.
    let binding_sources_by_target = sources_by_target
        .iter()
        .filter_map(|(target, sources)| {
            if !target.properties.is_empty() {
                return None;
            }
            let sources = sources
                .iter()
                .filter(|source| source.properties.is_empty())
                .cloned()
                .collect::<Vec<_>>();
            (!sources.is_empty()).then(|| (target.clone(), sources))
        })
        .collect::<HashMap<_, _>>();

    let mut pending = collector.sensitive.iter().cloned().collect::<Vec<_>>();
    while let Some(target) = pending.pop() {
        if let Some(sources) = sources_by_target.get(&target) {
            for source in sources {
                if collector.sensitive.insert(source.clone()) {
                    pending.push(source.clone());
                }
            }
        }

        if target.properties.is_empty() {
            continue;
        }
        let binding = ValueKey {
            root: target.root.clone(),
            properties: Vec::new(),
        };
        let Some(sources) = binding_sources_by_target.get(&binding) else {
            continue;
        };
        for source in sources {
            let mut propagated = source.clone();
            propagated
                .properties
                .extend(target.properties.iter().cloned());
            if collector.sensitive.insert(propagated.clone()) {
                pending.push(propagated);
            }
        }
    }

    collector.sensitive
}
