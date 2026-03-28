use swc_core::ecma::ast::{Expr, FnExpr, MethodProp, Prop, PropName};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct ObjMethodShorthand;

impl VisitMut for ObjMethodShorthand {
    fn visit_mut_prop(&mut self, prop: &mut Prop) {
        // Recurse into children first
        prop.visit_mut_children_with(self);

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
}
