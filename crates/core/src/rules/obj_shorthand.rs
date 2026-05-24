use swc_core::ecma::ast::{AssignPatProp, Expr, ObjectPat, ObjectPatProp, Pat, Prop, PropName};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

/// Converts `{ foo: foo }` → `{ foo }` (ES6 object property shorthand).
/// Only fires when the key is a plain identifier and the value is an identifier
/// with the same name. Computed keys, string keys, and numeric keys are skipped.
pub struct ObjShorthand;

impl VisitMut for ObjShorthand {
    fn visit_mut_object_pat(&mut self, obj: &mut ObjectPat) {
        obj.visit_mut_children_with(self);

        obj.props = obj
            .props
            .drain(..)
            .map(|prop| match prop {
                ObjectPatProp::KeyValue(kv) => {
                    let PropName::Ident(key_ident) = &kv.key else {
                        return ObjectPatProp::KeyValue(kv);
                    };

                    match *kv.value {
                        Pat::Ident(binding) if key_ident.sym == binding.id.sym => {
                            ObjectPatProp::Assign(AssignPatProp {
                                span: binding.id.span,
                                key: binding,
                                value: None,
                            })
                        }
                        Pat::Assign(assign)
                            if matches!(
                                assign.left.as_ref(),
                                Pat::Ident(binding) if key_ident.sym == binding.id.sym
                            ) =>
                        {
                            let Pat::Ident(binding) = *assign.left else {
                                unreachable!()
                            };
                            ObjectPatProp::Assign(AssignPatProp {
                                span: binding.id.span,
                                key: binding,
                                value: Some(assign.right),
                            })
                        }
                        value => ObjectPatProp::KeyValue(swc_core::ecma::ast::KeyValuePatProp {
                            key: kv.key,
                            value: Box::new(value),
                        }),
                    }
                }
                other => other,
            })
            .collect();
    }

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
