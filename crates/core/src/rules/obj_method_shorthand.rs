use std::collections::HashSet;

use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, BinaryOp, Expr, MethodProp, Module, ObjectLit, Prop, PropName,
    PropOrSpread, VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::constructor_sensitivity::{
    assign_target_pat_has_constructor_sensitive_value, assign_target_value_key,
    collect_constructor_sensitive_values, is_value_preserving_assign_op,
    pat_has_constructor_sensitive_value, pat_value_key, static_prop_name,
    visit_mut_assign_target_pat_constructor_sensitive_defaults,
    visit_mut_pat_constructor_sensitive_defaults, ValueKey,
};
use super::decl_utils::has_duplicate_param_names;

pub struct ObjMethodShorthand;

impl VisitMut for ObjMethodShorthand {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let constructor_sensitive_values = collect_constructor_sensitive_values(module);
        module.visit_mut_with(&mut ObjMethodShorthandConverter {
            constructor_sensitive_values: &constructor_sensitive_values,
        });
    }
}

struct ObjMethodShorthandConverter<'a> {
    constructor_sensitive_values: &'a HashSet<ValueKey>,
}

impl VisitMut for ObjMethodShorthandConverter<'_> {
    fn visit_mut_var_declarator(&mut self, decl: &mut VarDeclarator) {
        let constructor_sensitive_values = self.constructor_sensitive_values;
        visit_mut_pat_constructor_sensitive_defaults(
            &mut decl.name,
            constructor_sensitive_values,
            &mut |expr, is_constructor_sensitive| {
                if is_constructor_sensitive {
                    visit_mut_value_expr(expr, None, true, self);
                } else {
                    expr.visit_mut_with(self);
                }
            },
        );
        let Some(init) = &mut decl.init else {
            return;
        };
        if let Some(key) = pat_value_key(&decl.name) {
            visit_mut_value_expr(init, Some(&key), false, self);
        } else if pat_has_constructor_sensitive_value(&decl.name, self.constructor_sensitive_values)
        {
            visit_mut_value_expr(init, None, true, self);
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
                            visit_mut_value_expr(expr, None, true, self);
                        } else {
                            expr.visit_mut_with(self);
                        }
                    },
                );
            }
        }
        if is_value_preserving_assign_op(expr.op) {
            if let Some(key) = assign_target_value_key(&expr.left) {
                visit_mut_value_expr(&mut expr.right, Some(&key), false, self);
                return;
            }
            if pattern_is_constructor_sensitive {
                visit_mut_value_expr(&mut expr.right, None, true, self);
                return;
            }
        }
        expr.right.visit_mut_with(self);
    }

    fn visit_mut_prop(&mut self, prop: &mut Prop) {
        prop.visit_mut_children_with(self);
        try_convert_prop(prop, false);
    }
}

fn visit_mut_value_expr(
    expr: &mut Expr,
    key: Option<&ValueKey>,
    force_constructor_sensitive: bool,
    converter: &mut ObjMethodShorthandConverter<'_>,
) {
    match expr {
        Expr::Paren(paren) => {
            visit_mut_value_expr(&mut paren.expr, key, force_constructor_sensitive, converter)
        }
        Expr::Seq(sequence) => {
            if let Some((last, prefix)) = sequence.exprs.split_last_mut() {
                for expr in prefix {
                    expr.visit_mut_with(converter);
                }
                visit_mut_value_expr(last, key, force_constructor_sensitive, converter);
            }
        }
        Expr::Cond(conditional) => {
            conditional.test.visit_mut_with(converter);
            visit_mut_value_expr(
                &mut conditional.cons,
                key,
                force_constructor_sensitive,
                converter,
            );
            visit_mut_value_expr(
                &mut conditional.alt,
                key,
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
                key,
                force_constructor_sensitive,
                converter,
            );
            visit_mut_value_expr(
                &mut binary.right,
                key,
                force_constructor_sensitive,
                converter,
            );
        }
        Expr::Object(object) => {
            visit_mut_object_value(object, key, force_constructor_sensitive, converter)
        }
        _ => expr.visit_mut_with(converter),
    }
}

fn visit_mut_object_value(
    object: &mut ObjectLit,
    key: Option<&ValueKey>,
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
                key,
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
        let value_key = key.map(|key| key.with_property(property));
        visit_mut_value_expr(
            &mut key_value.value,
            value_key.as_ref(),
            force_constructor_sensitive,
            converter,
        );
        let constructor_sensitive = force_constructor_sensitive
            || value_key
                .as_ref()
                .is_some_and(|key| converter.constructor_sensitive_values.contains(key));
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
    if !matches!(kv.key, PropName::Ident(_)) {
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
