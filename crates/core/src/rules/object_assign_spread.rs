use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    Callee, Expr, MemberProp, ObjectLit, Prop, PropName, PropOrSpread, SpreadElement,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

/// Converts `Object.assign({}, source1, source2, ...)` into object spread syntax.
///
/// Only fires when the **first** argument is an empty object literal `{}`.
/// In that case the semantics are identical: a fresh object is created with the
/// sources merged in order.
///
/// ```js
/// // input
/// Object.assign({}, defaults, { extra: 1 })
/// // output
/// { ...defaults, extra: 1 }
/// ```
///
/// When the first argument is NOT `{}` (e.g. `Object.assign(target, src)`) the
/// call is left unchanged because it mutates `target` in place, which is
/// semantically different from spread.
pub struct ObjectAssignSpread {
    unresolved_mark: Mark,
}

impl ObjectAssignSpread {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }
}

impl VisitMut for ObjectAssignSpread {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        // Bottom-up: transform inner Object.assign calls first so that their
        // results (plain object literals) can be inlined by an outer transform.
        expr.visit_mut_children_with(self);

        if let Some(new_expr) = transform_object_assign(expr, self.unresolved_mark) {
            *expr = new_expr;
        }
    }
}

fn transform_object_assign(expr: &Expr, unresolved_mark: Mark) -> Option<Expr> {
    let Expr::Call(call) = expr else {
        return None;
    };

    // Callee must be `Object.assign`
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee_expr.as_ref() else {
        return None;
    };
    let Expr::Ident(obj_ident) = member.obj.as_ref() else {
        return None;
    };
    if obj_ident.sym != "Object" || obj_ident.ctxt.outer() != unresolved_mark {
        return None;
    }
    if !matches!(&member.prop, MemberProp::Ident(i) if i.sym == "assign") {
        return None;
    }

    // Need at least one argument (the target)
    if call.args.is_empty() {
        return None;
    }

    // First argument must be an empty object literal `{}`
    let first_arg = &call.args[0];
    if first_arg.spread.is_some() {
        return None;
    }
    let Expr::Object(first_obj) = first_arg.expr.as_ref() else {
        return None;
    };
    if !first_obj.props.is_empty() {
        return None;
    }

    // Build the spread properties from the remaining arguments.
    //
    // Inline only plain data properties. Accessors, methods, and bare
    // `__proto__` entries are kept behind a spread because directly placing them
    // in the output object literal would change semantics.
    let mut props: Vec<PropOrSpread> = Vec::new();
    for arg in &call.args[1..] {
        if arg.spread.is_some() {
            // Can't handle a spread argument in call position — bail out.
            return None;
        }
        if let Expr::Object(obj) = arg.expr.as_ref() {
            if is_safe_to_inline_props(&obj.props) {
                props.extend(obj.props.clone());
                continue;
            }
        }

        props.push(PropOrSpread::Spread(SpreadElement {
            dot3_token: DUMMY_SP,
            expr: arg.expr.clone(),
        }));
    }

    Some(Expr::Object(ObjectLit {
        span: call.span,
        props,
    }))
}

fn is_safe_to_inline_props(props: &[PropOrSpread]) -> bool {
    props.iter().all(is_safe_to_inline_prop)
}

fn is_safe_to_inline_prop(prop: &PropOrSpread) -> bool {
    match prop {
        PropOrSpread::Spread(_) => true,
        PropOrSpread::Prop(prop) => match prop.as_ref() {
            Prop::Shorthand(ident) => ident.sym != "__proto__",
            Prop::KeyValue(kv) => !is_bare_proto_name(&kv.key),
            Prop::Assign(assign) => assign.key.sym != "__proto__",
            Prop::Getter(_) | Prop::Setter(_) | Prop::Method(_) => false,
        },
    }
}

fn is_bare_proto_name(name: &PropName) -> bool {
    match name {
        PropName::Ident(ident) => ident.sym == "__proto__",
        PropName::Str(value) => value.value == "__proto__",
        PropName::Num(_) | PropName::BigInt(_) | PropName::Computed(_) => false,
    }
}
