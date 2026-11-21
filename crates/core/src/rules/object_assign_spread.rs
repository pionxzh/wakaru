use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    Callee, Expr, MemberProp, Module, ObjectLit, Prop, PropName, PropOrSpread, SpreadElement,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::RewriteLevel;

/// Converts `Object.assign({}, source1, source2, ...)` into object spread syntax.
///
/// Only fires when the **first** argument is a safe object literal target, so
/// the call builds a fresh object instead of mutating one:
///
/// ```js
/// // input
/// Object.assign({}, defaults, { extra: 1 })
/// // output (aggressive)
/// { ...defaults, extra: 1 }
/// ```
///
/// The two are not identical. `Object.assign` writes each key with [[Set]],
/// spread defines it: a source with an own `__proto__` key (parsed JSON) sets
/// the result's prototype under `Object.assign` and becomes an own property
/// under spread. Below `aggressive` the call converts only when every source
/// is an object literal of plain data properties, which can have no such key;
/// a dynamic source is a common hand-written merge or clone, so its
/// conversion relies on `assign_sources_without_proto_key` and needs
/// `aggressive`.
///
/// When the first argument is not a safe object literal target (e.g.
/// `Object.assign(target, src)`) the call is left unchanged because it mutates
/// `target` in place, which is semantically different from spread.
pub struct ObjectAssignSpread {
    unresolved_mark: Mark,
    level: RewriteLevel,
}

impl ObjectAssignSpread {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self::new_with_level(unresolved_mark, RewriteLevel::Aggressive)
    }

    pub fn new_with_level(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            unresolved_mark,
            level,
        }
    }
}

impl VisitMut for ObjectAssignSpread {
    fn visit_mut_module(&mut self, module: &mut Module) {
        // `Object` is read as the builtin; a `with` statement or a direct eval
        // anywhere in the module can rebind it, so the module is left as is
        // (docs/rewrite-assumptions.md, dynamic-scope skip).
        if super::eval_utils::has_dynamic_scope_construct(module) {
            return;
        }
        module.visit_mut_children_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        // Bottom-up: transform inner Object.assign calls first so that their
        // results (plain object literals) can be inlined by an outer transform.
        expr.visit_mut_children_with(self);

        if let Some(new_expr) = transform_object_assign(expr, self.unresolved_mark, self.level) {
            *expr = new_expr;
        }
    }
}

fn transform_object_assign(
    expr: &Expr,
    unresolved_mark: Mark,
    level: RewriteLevel,
) -> Option<Expr> {
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

    // First argument must be a safe fresh object literal target.
    let first_arg = &call.args[0];
    if first_arg.spread.is_some() {
        return None;
    }
    let Expr::Object(first_obj) = first_arg.expr.as_ref() else {
        return None;
    };
    if !is_safe_to_inline_props(&first_obj.props) {
        return None;
    }
    if level < RewriteLevel::Aggressive
        && !call.args[1..].iter().all(|arg| {
            arg.spread.is_none()
                && matches!(arg.expr.as_ref(), Expr::Object(obj) if is_static_data_object(obj))
        })
    {
        return None;
    }

    // Build the spread properties from the remaining arguments.
    //
    // Inline only plain data properties. Accessors, methods, and bare
    // `__proto__` entries are kept behind a spread because directly placing them
    // in the output object literal would change semantics.
    let mut props: Vec<PropOrSpread> = first_obj.props.clone();
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

/// An object literal whose own keys are all known plain data properties: no
/// spread, accessor, method, computed key, or `__proto__` key.
fn is_static_data_object(obj: &ObjectLit) -> bool {
    obj.props.iter().all(|prop| match prop {
        PropOrSpread::Spread(_) => false,
        PropOrSpread::Prop(prop) => match prop.as_ref() {
            Prop::Shorthand(ident) => ident.sym != "__proto__",
            Prop::KeyValue(kv) => {
                !matches!(kv.key, PropName::Computed(_)) && !is_bare_proto_name(&kv.key)
            }
            Prop::Assign(_) | Prop::Getter(_) | Prop::Setter(_) | Prop::Method(_) => false,
        },
    })
}

fn is_bare_proto_name(name: &PropName) -> bool {
    match name {
        PropName::Ident(ident) => ident.sym == "__proto__",
        PropName::Str(value) => value.value == "__proto__",
        PropName::Num(_) | PropName::BigInt(_) | PropName::Computed(_) => false,
    }
}
