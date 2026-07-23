use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, AssignTargetPat, BinExpr, BinaryOp, CallExpr, Callee,
    Class, Expr, Ident, Lit, MemberProp, Module, NewExpr, ObjectPat, ObjectPatProp, Pat, PropName,
    SimpleAssignTarget, VarDeclarator,
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

pub(crate) fn static_prop_name(name: &PropName) -> Option<Atom> {
    match name {
        PropName::Ident(ident) => Some(ident.sym.clone()),
        PropName::Str(value) => value.value.as_str().map(Atom::from),
        PropName::Computed(computed) => match computed.expr.as_ref() {
            Expr::Lit(Lit::Str(value)) => value.value.as_str().map(Atom::from),
            _ => None,
        },
        PropName::Num(_) | PropName::BigInt(_) => None,
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

pub(crate) fn is_bind_call(call: &CallExpr) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    static_member_name(&member.prop).is_some_and(|name| name == "bind")
}

pub(crate) fn is_value_preserving_assign_op(op: AssignOp) -> bool {
    matches!(
        op,
        AssignOp::Assign | AssignOp::OrAssign | AssignOp::AndAssign | AssignOp::NullishAssign
    )
}

fn key_or_descendant_is_sensitive(
    key: &ValueKey,
    constructor_sensitive_values: &HashSet<ValueKey>,
) -> bool {
    constructor_sensitive_values.iter().any(|sensitive| {
        sensitive.root == key.root && sensitive.properties.starts_with(&key.properties)
    })
}

pub(crate) fn pat_has_constructor_sensitive_value(
    pat: &Pat,
    constructor_sensitive_values: &HashSet<ValueKey>,
) -> bool {
    match pat {
        Pat::Ident(binding) => key_or_descendant_is_sensitive(
            &ValueKey::binding(&binding.id),
            constructor_sensitive_values,
        ),
        Pat::Expr(expr) => expr_value_key(expr)
            .as_ref()
            .is_some_and(|key| key_or_descendant_is_sensitive(key, constructor_sensitive_values)),
        Pat::Assign(assign) => {
            pat_has_constructor_sensitive_value(&assign.left, constructor_sensitive_values)
        }
        Pat::Array(array) => array.elems.iter().flatten().any(|element| {
            pat_has_constructor_sensitive_value(element, constructor_sensitive_values)
        }),
        Pat::Object(object) => {
            object_pat_has_constructor_sensitive_value(object, constructor_sensitive_values)
        }
        Pat::Rest(rest) => {
            pat_has_constructor_sensitive_value(&rest.arg, constructor_sensitive_values)
        }
        Pat::Invalid(_) => false,
    }
}

pub(crate) fn object_pat_has_constructor_sensitive_value(
    object: &ObjectPat,
    constructor_sensitive_values: &HashSet<ValueKey>,
) -> bool {
    object.props.iter().any(|prop| match prop {
        ObjectPatProp::Assign(assign) => key_or_descendant_is_sensitive(
            &ValueKey::binding(&assign.key.id),
            constructor_sensitive_values,
        ),
        ObjectPatProp::KeyValue(key_value) => {
            pat_has_constructor_sensitive_value(&key_value.value, constructor_sensitive_values)
        }
        ObjectPatProp::Rest(rest) => {
            pat_has_constructor_sensitive_value(&rest.arg, constructor_sensitive_values)
        }
    })
}

pub(crate) fn assign_target_pat_has_constructor_sensitive_value(
    pat: &AssignTargetPat,
    constructor_sensitive_values: &HashSet<ValueKey>,
) -> bool {
    match pat {
        AssignTargetPat::Array(array) => array.elems.iter().flatten().any(|element| {
            pat_has_constructor_sensitive_value(element, constructor_sensitive_values)
        }),
        AssignTargetPat::Object(object) => {
            object_pat_has_constructor_sensitive_value(object, constructor_sensitive_values)
        }
        AssignTargetPat::Invalid(_) => false,
    }
}

pub(crate) fn visit_mut_pat_constructor_sensitive_defaults(
    pat: &mut Pat,
    constructor_sensitive_values: &HashSet<ValueKey>,
    visit_expr: &mut impl FnMut(&mut Expr, bool),
) {
    match pat {
        Pat::Assign(assign) => {
            visit_mut_pat_constructor_sensitive_defaults(
                &mut assign.left,
                constructor_sensitive_values,
                visit_expr,
            );
            let is_constructor_sensitive =
                pat_has_constructor_sensitive_value(&assign.left, constructor_sensitive_values);
            visit_expr(&mut assign.right, is_constructor_sensitive);
        }
        Pat::Array(array) => {
            for element in array.elems.iter_mut().flatten() {
                visit_mut_pat_constructor_sensitive_defaults(
                    element,
                    constructor_sensitive_values,
                    visit_expr,
                );
            }
        }
        Pat::Object(object) => visit_mut_object_pat_constructor_sensitive_defaults(
            object,
            constructor_sensitive_values,
            visit_expr,
        ),
        Pat::Rest(rest) => visit_mut_pat_constructor_sensitive_defaults(
            &mut rest.arg,
            constructor_sensitive_values,
            visit_expr,
        ),
        Pat::Expr(expr) => visit_expr(expr, false),
        Pat::Ident(_) | Pat::Invalid(_) => {}
    }
}

pub(crate) fn visit_mut_assign_target_pat_constructor_sensitive_defaults(
    pat: &mut AssignTargetPat,
    constructor_sensitive_values: &HashSet<ValueKey>,
    visit_expr: &mut impl FnMut(&mut Expr, bool),
) {
    match pat {
        AssignTargetPat::Array(array) => {
            for element in array.elems.iter_mut().flatten() {
                visit_mut_pat_constructor_sensitive_defaults(
                    element,
                    constructor_sensitive_values,
                    visit_expr,
                );
            }
        }
        AssignTargetPat::Object(object) => {
            visit_mut_object_pat_constructor_sensitive_defaults(
                object,
                constructor_sensitive_values,
                visit_expr,
            );
        }
        AssignTargetPat::Invalid(_) => {}
    }
}

fn visit_mut_object_pat_constructor_sensitive_defaults(
    object: &mut ObjectPat,
    constructor_sensitive_values: &HashSet<ValueKey>,
    visit_expr: &mut impl FnMut(&mut Expr, bool),
) {
    for prop in &mut object.props {
        match prop {
            ObjectPatProp::Assign(assign) => {
                if let Some(default) = &mut assign.value {
                    let binding = ValueKey::binding(&assign.key.id);
                    visit_expr(
                        default,
                        key_or_descendant_is_sensitive(&binding, constructor_sensitive_values),
                    );
                }
            }
            ObjectPatProp::KeyValue(key_value) => {
                if let PropName::Computed(computed) = &mut key_value.key {
                    visit_expr(&mut computed.expr, false);
                }
                visit_mut_pat_constructor_sensitive_defaults(
                    &mut key_value.value,
                    constructor_sensitive_values,
                    visit_expr,
                );
            }
            ObjectPatProp::Rest(rest) => {
                visit_mut_pat_constructor_sensitive_defaults(
                    &mut rest.arg,
                    constructor_sensitive_values,
                    visit_expr,
                );
            }
        }
    }
}

/// Collect the value keys an expression can evaluate to (or, for `.bind`,
/// derive its constructibility from), walking the same wrapper shapes the
/// consumers protect syntactically: parentheses, sequence results,
/// conditional/logical branches, assignment results, and `.bind` targets.
fn collect_value_sources(expr: &Expr, sources: &mut Vec<ValueKey>) {
    match expr {
        Expr::Paren(paren) => collect_value_sources(&paren.expr, sources),
        Expr::Seq(sequence) => {
            if let Some(last) = sequence.exprs.last() {
                collect_value_sources(last, sources);
            }
        }
        Expr::Cond(conditional) => {
            collect_value_sources(&conditional.cons, sources);
            collect_value_sources(&conditional.alt, sources);
        }
        Expr::Bin(binary)
            if matches!(
                binary.op,
                BinaryOp::LogicalOr | BinaryOp::LogicalAnd | BinaryOp::NullishCoalescing
            ) =>
        {
            collect_value_sources(&binary.left, sources);
            collect_value_sources(&binary.right, sources);
        }
        Expr::Assign(assign) if is_value_preserving_assign_op(assign.op) => {
            if let Some(target) = assign_target_value_key(&assign.left) {
                sources.push(target);
            }
            collect_value_sources(&assign.right, sources);
        }
        // A bound function is constructible iff its target is: `new B()` for
        // `B = f.bind(...)` constructs `f`, so `f` must stay an ordinary
        // function whenever the bound value is constructor-sensitive.
        Expr::Call(call) if is_bind_call(call) => {
            let Callee::Expr(callee) = &call.callee else {
                return;
            };
            let Expr::Member(member) = callee.as_ref() else {
                return;
            };
            collect_value_sources(&member.obj, sources);
        }
        _ => {
            if let Some(key) = expr_value_key(expr) {
                sources.push(key);
            }
        }
    }
}

fn value_sources(expr: &Expr) -> Vec<ValueKey> {
    let mut sources = Vec::new();
    collect_value_sources(expr, &mut sources);
    sources
}

#[derive(Default)]
struct ConstructorSensitiveUseCollector {
    sensitive: HashSet<ValueKey>,
    aliases: Vec<(ValueKey, ValueKey)>,
}

impl ConstructorSensitiveUseCollector {
    fn mark_expr(&mut self, expr: &Expr) {
        for key in value_sources(expr) {
            self.sensitive.insert(key);
        }
    }

    fn record_aliases(&mut self, target: ValueKey, source: &Expr) {
        for source in value_sources(source) {
            self.aliases.push((target.clone(), source));
        }
    }

    /// Record aliases for a binding pattern: `const { C } = ns` makes `C` an
    /// alias of `ns.C`, recursing through renames, nested object patterns,
    /// defaults, and rest bindings (a rest object exposes the remaining
    /// source properties, so it conservatively aliases the source itself).
    fn record_pat_aliases(&mut self, pat: &Pat, sources: &[ValueKey]) {
        match pat {
            Pat::Ident(binding) => {
                for source in sources {
                    self.aliases
                        .push((ValueKey::binding(&binding.id), source.clone()));
                }
            }
            Pat::Expr(expr) => {
                if let Some(target) = expr_value_key(expr) {
                    for source in sources {
                        self.aliases.push((target.clone(), source.clone()));
                    }
                }
            }
            Pat::Assign(assign) => {
                self.record_pat_aliases(&assign.left, sources);
                let default_sources = value_sources(&assign.right);
                self.record_pat_aliases(&assign.left, &default_sources);
            }
            Pat::Object(object) => self.record_object_pat_aliases(object, sources),
            _ => {}
        }
    }

    fn record_object_pat_aliases(&mut self, object: &ObjectPat, sources: &[ValueKey]) {
        for prop in &object.props {
            match prop {
                ObjectPatProp::Assign(assign) => {
                    let target = ValueKey::binding(&assign.key.id);
                    for source in sources {
                        self.aliases.push((
                            target.clone(),
                            source.with_property(assign.key.id.sym.clone()),
                        ));
                    }
                    if let Some(default) = &assign.value {
                        self.record_aliases(target, default);
                    }
                }
                ObjectPatProp::KeyValue(key_value) => {
                    let Some(name) = static_prop_name(&key_value.key) else {
                        continue;
                    };
                    let extended = sources
                        .iter()
                        .map(|source| source.with_property(name.clone()))
                        .collect::<Vec<_>>();
                    self.record_pat_aliases(&key_value.value, &extended);
                }
                ObjectPatProp::Rest(rest) => {
                    self.record_pat_aliases(&rest.arg, sources);
                }
            }
        }
    }
}

impl Visit for ConstructorSensitiveUseCollector {
    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        if let Some(init) = decl.init.as_deref() {
            let sources = value_sources(init);
            self.record_pat_aliases(&decl.name, &sources);
        }
        decl.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, expr: &AssignExpr) {
        match expr.op {
            AssignOp::Assign => {
                if let Some(target) = assign_target_value_key(&expr.left) {
                    self.record_aliases(target, &expr.right);
                } else if let AssignTarget::Pat(AssignTargetPat::Object(object)) = &expr.left {
                    let sources = value_sources(&expr.right);
                    self.record_object_pat_aliases(object, &sources);
                }
            }
            // A logical assignment may leave the right-hand value in the
            // target, so it aliases the same sources a plain `=` would.
            op if is_value_preserving_assign_op(op) => {
                if let Some(target) = assign_target_value_key(&expr.left) {
                    self.record_aliases(target, &expr.right);
                }
            }
            _ => {}
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
