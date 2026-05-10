use swc_core::ecma::ast::{Expr, Prop, PropName};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

/// Converts `{ foo: foo }` → `{ foo }` (ES6 object property shorthand).
/// Only fires when the key is a plain identifier and the value is an identifier
/// with the same name. Computed keys, string keys, and numeric keys are skipped.
pub struct ObjShorthand;

impl VisitMut for ObjShorthand {
    fn visit_mut_prop(&mut self, prop: &mut Prop) {
        prop.visit_mut_children_with(self);

        let Prop::KeyValue(kv) = prop else {
            return;
        };

        // Key must be a plain identifier (not computed, not string, not numeric)
        let PropName::Ident(key_ident) = &kv.key else {
            return;
        };

        // Value must be an identifier with the same name
        let Expr::Ident(val_ident) = kv.value.as_ref() else {
            return;
        };

        if key_ident.sym != val_ident.sym {
            return;
        }

        // Extract the value ident (carries the binding's SyntaxContext)
        let Prop::KeyValue(kv_owned) = std::mem::replace(prop, Prop::Shorthand(Default::default()))
        else {
            unreachable!()
        };
        let Expr::Ident(val_ident) = *kv_owned.value else {
            unreachable!()
        };
        *prop = Prop::Shorthand(val_ident);
    }
}
