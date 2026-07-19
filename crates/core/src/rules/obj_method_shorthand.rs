use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, BinaryOp, Expr, Lit, MethodProp, Module, ObjectLit, Prop, PropName,
    PropOrSpread, VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::constructor_sensitivity::{
    assign_target_value_key, collect_constructor_sensitive_values, pat_value_key, ValueKey,
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
        decl.name.visit_mut_with(self);
        let Some(init) = &mut decl.init else {
            return;
        };
        if let Some(key) = pat_value_key(&decl.name) {
            visit_mut_value_expr(init, &key, self);
        } else {
            init.visit_mut_with(self);
        }
    }

    fn visit_mut_assign_expr(&mut self, expr: &mut AssignExpr) {
        expr.left.visit_mut_with(self);
        if expr.op == AssignOp::Assign {
            if let Some(key) = assign_target_value_key(&expr.left) {
                visit_mut_value_expr(&mut expr.right, &key, self);
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
    key: &ValueKey,
    converter: &mut ObjMethodShorthandConverter<'_>,
) {
    match expr {
        Expr::Paren(paren) => visit_mut_value_expr(&mut paren.expr, key, converter),
        Expr::Seq(sequence) => {
            if let Some((last, prefix)) = sequence.exprs.split_last_mut() {
                for expr in prefix {
                    expr.visit_mut_with(converter);
                }
                visit_mut_value_expr(last, key, converter);
            }
        }
        Expr::Cond(conditional) => {
            conditional.test.visit_mut_with(converter);
            visit_mut_value_expr(&mut conditional.cons, key, converter);
            visit_mut_value_expr(&mut conditional.alt, key, converter);
        }
        Expr::Bin(binary)
            if matches!(
                binary.op,
                BinaryOp::LogicalOr | BinaryOp::LogicalAnd | BinaryOp::NullishCoalescing
            ) =>
        {
            visit_mut_value_expr(&mut binary.left, key, converter);
            visit_mut_value_expr(&mut binary.right, key, converter);
        }
        Expr::Object(object) => visit_mut_object_value(object, key, converter),
        _ => expr.visit_mut_with(converter),
    }
}

fn visit_mut_object_value(
    object: &mut ObjectLit,
    key: &ValueKey,
    converter: &mut ObjMethodShorthandConverter<'_>,
) {
    for prop in &mut object.props {
        let PropOrSpread::Prop(prop) = prop else {
            let PropOrSpread::Spread(spread) = prop else {
                unreachable!();
            };
            visit_mut_value_expr(&mut spread.expr, key, converter);
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
        let value_key = key.with_property(property);
        visit_mut_value_expr(&mut key_value.value, &value_key, converter);
        let constructor_sensitive = converter.constructor_sensitive_values.contains(&value_key);
        try_convert_prop(prop, constructor_sensitive);
    }
}

fn static_prop_name(name: &PropName) -> Option<Atom> {
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
