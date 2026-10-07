use crate::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::Mark;
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, BinaryOp, CallExpr, Callee, Expr, MethodProp, Module, ObjectLit,
    Prop, PropName, PropOrSpread, VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::constructor_sensitivity::{
    assign_target_pat_has_constructor_sensitive_value, assign_target_value_key,
    collect_constructor_sensitive_values, expr_value_key, is_value_preserving_assign_op,
    pat_has_constructor_sensitive_value, pat_value_key, static_prop_name,
    visit_mut_assign_target_pat_constructor_sensitive_defaults,
    visit_mut_pat_constructor_sensitive_defaults, CreateClassHelpers, ValueKey,
};
use super::decl_utils::has_duplicate_param_names;
use super::transpiler_helper_utils::LocalHelperContext;

pub struct ObjMethodShorthand {
    unresolved_mark: Mark,
}

impl ObjMethodShorthand {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }

    pub(crate) fn run_with_helpers(
        module: &mut Module,
        unresolved_mark: Mark,
        local_helpers: &LocalHelperContext,
    ) {
        let create_class = CreateClassHelpers::collect(module, unresolved_mark, local_helpers);
        let constructor_sensitive_values =
            collect_constructor_sensitive_values(module, &create_class);
        let constructed_suffixes = constructor_sensitive_values
            .iter()
            .filter_map(ValueKey::property_suffix)
            .filter(|(parent, _)| parent.as_ref() != "prototype")
            .map(|(parent, property)| (parent.clone(), property.clone()))
            .collect();
        module.visit_mut_with(&mut ObjMethodShorthandConverter {
            constructor_sensitive_values: &constructor_sensitive_values,
            constructed_suffixes: &constructed_suffixes,
        });
    }
}

impl VisitMut for ObjMethodShorthand {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect_with_mark(module, self.unresolved_mark);
        Self::run_with_helpers(module, self.unresolved_mark, &local_helpers);
    }
}

struct ObjMethodShorthandConverter<'a> {
    constructor_sensitive_values: &'a HashSet<ValueKey>,
    /// `(parent, property)` name suffixes of constructor-sensitive keys,
    /// without a `prototype` parent. The resolver keys miss a namespace
    /// object reached through a different binding in a sibling scope (UMD
    /// IIFEs linked only through a global, a wrapper parameter bound to a
    /// `require` result), so `new C.algo.HMAC.init()` also protects
    /// `X.HMAC = Base.extend({ init: function () {} })`. A false match only
    /// keeps a function expression.
    constructed_suffixes: &'a HashSet<(Atom, Atom)>,
}

impl ObjMethodShorthandConverter<'_> {
    fn is_constructor_sensitive(&self, key: &ValueKey) -> bool {
        self.constructor_sensitive_values.contains(key)
            || key.property_suffix().is_some_and(|(parent, property)| {
                self.constructed_suffixes
                    .contains(&(parent.clone(), property.clone()))
            })
    }
}

impl VisitMut for ObjMethodShorthandConverter<'_> {
    fn visit_mut_var_declarator(&mut self, decl: &mut VarDeclarator) {
        let constructor_sensitive_values = self.constructor_sensitive_values;
        visit_mut_pat_constructor_sensitive_defaults(
            &mut decl.name,
            constructor_sensitive_values,
            &mut |expr, is_constructor_sensitive| {
                if is_constructor_sensitive {
                    visit_mut_value_expr(expr, &[], true, self);
                } else {
                    expr.visit_mut_with(self);
                }
            },
        );
        let Some(init) = &mut decl.init else {
            return;
        };
        if let Some(key) = pat_value_key(&decl.name) {
            visit_mut_value_expr(init, std::slice::from_ref(&key), false, self);
        } else if pat_has_constructor_sensitive_value(&decl.name, self.constructor_sensitive_values)
        {
            visit_mut_value_expr(init, &[], true, self);
        } else {
            init.visit_mut_with(self);
        }
    }

    fn visit_mut_assign_expr(&mut self, expr: &mut AssignExpr) {
        let pattern_is_constructor_sensitive = match &expr.left {
            AssignTarget::Pat(pat) => assign_target_pat_has_constructor_sensitive_value(
                pat,
                self.constructor_sensitive_values,
            ),
            AssignTarget::Simple(_) => false,
        };
        match &mut expr.left {
            AssignTarget::Simple(target) => target.visit_mut_with(self),
            AssignTarget::Pat(pat) => {
                let constructor_sensitive_values = self.constructor_sensitive_values;
                visit_mut_assign_target_pat_constructor_sensitive_defaults(
                    pat,
                    constructor_sensitive_values,
                    &mut |expr, is_constructor_sensitive| {
                        if is_constructor_sensitive {
                            visit_mut_value_expr(expr, &[], true, self);
                        } else {
                            expr.visit_mut_with(self);
                        }
                    },
                );
            }
        }
        if is_value_preserving_assign_op(expr.op) {
            if let Some(key) = assign_target_value_key(&expr.left) {
                visit_mut_value_expr(&mut expr.right, std::slice::from_ref(&key), false, self);
                return;
            }
            if pattern_is_constructor_sensitive {
                visit_mut_value_expr(&mut expr.right, &[], true, self);
                return;
            }
        }
        expr.right.visit_mut_with(self);
    }

    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        visit_mut_call(call, &[], self);
    }

    fn visit_mut_prop(&mut self, prop: &mut Prop) {
        prop.visit_mut_children_with(self);
        try_convert_prop(prop, false);
    }
}

fn visit_mut_value_expr(
    expr: &mut Expr,
    keys: &[ValueKey],
    force_constructor_sensitive: bool,
    converter: &mut ObjMethodShorthandConverter<'_>,
) {
    match expr {
        Expr::Paren(paren) => visit_mut_value_expr(
            &mut paren.expr,
            keys,
            force_constructor_sensitive,
            converter,
        ),
        Expr::Seq(sequence) => {
            if let Some((last, prefix)) = sequence.exprs.split_last_mut() {
                for expr in prefix {
                    expr.visit_mut_with(converter);
                }
                visit_mut_value_expr(last, keys, force_constructor_sensitive, converter);
            }
        }
        Expr::Cond(conditional) => {
            conditional.test.visit_mut_with(converter);
            visit_mut_value_expr(
                &mut conditional.cons,
                keys,
                force_constructor_sensitive,
                converter,
            );
            visit_mut_value_expr(
                &mut conditional.alt,
                keys,
                force_constructor_sensitive,
                converter,
            );
        }
        Expr::Bin(binary)
            if matches!(
                binary.op,
                BinaryOp::LogicalOr | BinaryOp::LogicalAnd | BinaryOp::NullishCoalescing
            ) =>
        {
            visit_mut_value_expr(
                &mut binary.left,
                keys,
                force_constructor_sensitive,
                converter,
            );
            visit_mut_value_expr(
                &mut binary.right,
                keys,
                force_constructor_sensitive,
                converter,
            );
        }
        Expr::Object(object) => {
            visit_mut_object_value(object, keys, force_constructor_sensitive, converter)
        }
        Expr::Call(call) => visit_mut_call(call, keys, converter),
        // `S.fn = S.prototype = { init: function () {} }` exposes the object
        // under both targets.
        Expr::Assign(assign)
            if is_value_preserving_assign_op(assign.op)
                && matches!(assign.left, AssignTarget::Simple(_)) =>
        {
            assign.left.visit_mut_with(converter);
            let mut chained = keys.to_vec();
            chained.extend(assign_target_value_key(&assign.left));
            visit_mut_value_expr(
                &mut assign.right,
                &chained,
                force_constructor_sensitive,
                converter,
            );
        }
        _ => expr.visit_mut_with(converter),
    }
}

/// call_result_exposes_argument_properties: a call may copy the properties of
/// an argument object onto its result (`Word = extend({ init })` exposes
/// `Word.init`) or onto its receiver (`Lib.mixin({ make })` exposes
/// `Lib.make`). Argument objects therefore inherit the call result's keys and
/// the receiver's key, but not `force_constructor_sensitive`. A wrong
/// assumption only skips shorthand; it does not invent a TypeError. Only an
/// inline object argument is linked: a spread argument is not, and neither is
/// an argument binding (`extend(props)` does not protect `props.init`).
/// Construction of the property in another module is out of scope.
fn visit_mut_call(
    call: &mut CallExpr,
    result_keys: &[ValueKey],
    converter: &mut ObjMethodShorthandConverter<'_>,
) {
    call.callee.visit_mut_with(converter);
    let mut keys = result_keys.to_vec();
    if call
        .args
        .iter()
        .any(|arg| arg.spread.is_none() && may_hold_object(&arg.expr))
    {
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Member(member) = callee.as_ref() {
                keys.extend(expr_value_key(&member.obj));
            }
        }
    }
    for arg in &mut call.args {
        if arg.spread.is_some() || keys.is_empty() {
            arg.visit_mut_with(converter);
            continue;
        }
        visit_mut_value_expr(&mut arg.expr, &keys, false, converter);
    }
    call.type_args.visit_mut_with(converter);
}

/// Value shapes `visit_mut_value_expr` can follow to an object literal.
fn may_hold_object(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Object(_)
            | Expr::Paren(_)
            | Expr::Seq(_)
            | Expr::Cond(_)
            | Expr::Bin(_)
            | Expr::Call(_)
    )
}

fn visit_mut_object_value(
    object: &mut ObjectLit,
    keys: &[ValueKey],
    force_constructor_sensitive: bool,
    converter: &mut ObjMethodShorthandConverter<'_>,
) {
    for prop in &mut object.props {
        let PropOrSpread::Prop(prop) = prop else {
            let PropOrSpread::Spread(spread) = prop else {
                unreachable!();
            };
            visit_mut_value_expr(
                &mut spread.expr,
                keys,
                force_constructor_sensitive,
                converter,
            );
            continue;
        };

        let Prop::KeyValue(key_value) = prop.as_mut() else {
            prop.visit_mut_with(converter);
            continue;
        };
        key_value.key.visit_mut_with(converter);
        let Some(property) = static_prop_name(&key_value.key) else {
            key_value.value.visit_mut_with(converter);
            try_convert_prop(prop, false);
            continue;
        };
        let value_keys = keys
            .iter()
            .map(|key| key.with_property(property.clone()))
            .collect::<Vec<_>>();
        visit_mut_value_expr(
            &mut key_value.value,
            &value_keys,
            force_constructor_sensitive,
            converter,
        );
        let constructor_sensitive = force_constructor_sensitive
            || value_keys
                .iter()
                .any(|key| converter.is_constructor_sensitive(key));
        try_convert_prop(prop, constructor_sensitive);
    }
}

fn try_convert_prop(prop: &mut Prop, constructor_sensitive: bool) {
    if constructor_sensitive {
        return;
    }

    let Prop::KeyValue(kv) = prop else {
        return;
    };

    // Only convert plain identifier keys — string, numeric, and computed
    // keys cannot use method shorthand syntax
    let PropName::Ident(key) = &kv.key else {
        return;
    };

    // A `constructor` function is usually constructed where this module
    // cannot see it: class-system helpers such as `extend(Base, { constructor })`
    // return it as the class, and `new this.constructor()` reads it from a
    // prototype object. Method shorthand would drop its [[Construct]].
    if key.sym == "constructor" {
        return;
    }

    // Value must be a function expression
    let Expr::Fn(fn_expr) = kv.value.as_ref() else {
        return;
    };

    // Don't convert named function expressions — the internal name may be
    // used for self-reference inside the body, and dropping it changes semantics
    if fn_expr.ident.is_some() {
        return;
    }

    // Don't convert generator functions
    if fn_expr.function.is_generator {
        return;
    }

    // Don't convert async functions (keep safe for now)
    if fn_expr.function.is_async {
        return;
    }

    // Method parameter lists require unique names; a sloppy-mode function
    // expression may carry duplicates.
    if has_duplicate_param_names(&fn_expr.function.params) {
        return;
    }

    // Take ownership to build the method
    let Prop::KeyValue(kv_owned) = std::mem::replace(prop, Prop::Shorthand(Default::default()))
    else {
        unreachable!()
    };

    let key = kv_owned.key;
    let Expr::Fn(fn_expr) = *kv_owned.value else {
        unreachable!()
    };

    *prop = Prop::Method(MethodProp {
        key,
        function: fn_expr.function,
    });
}
