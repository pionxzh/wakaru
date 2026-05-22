use std::collections::HashMap;

use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    Callee, Expr, ImportSpecifier, Module, ModuleDecl, ModuleItem, ObjectLit, Prop, PropName,
    PropOrSpread, SpreadElement,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use crate::facts::{HelperKind, ModuleFactsMap};

use super::babel_helper_utils::{
    collect_helpers, helpers_with_remaining_refs, remove_helper_declarations, BabelHelperKind,
    BindingKey,
};

/// Detects and replaces `_extends` and `_objectSpread2` helper calls with
/// object spread syntax.
///
/// Both `_extends` and `_objectSpread2` mutate and return their first argument
/// (like Object.assign). Only transform when the first arg is a safe fresh object
/// literal target, which guarantees no mutation/identity side effects:
///   `_extends({}, obj1, obj2)` → `{ ...obj1, ...obj2 }`
///   `_extends({ a: 1 }, obj1)` → `{ a: 1, ...obj1 }`
///   `_objectSpread2({}, y)` → `{ ...y }`
///   `_extends(target, source)` → left as-is (mutation semantics)
///   `_objectSpread2(existing, {a: 1})` → left as-is (mutation semantics)
pub struct UnObjectSpread<'a> {
    module_facts: Option<&'a ModuleFactsMap>,
}

impl UnObjectSpread<'_> {
    pub fn new() -> Self {
        Self { module_facts: None }
    }
}

impl<'a> UnObjectSpread<'a> {
    pub fn new_with_facts(module_facts: &'a ModuleFactsMap) -> Self {
        Self {
            module_facts: Some(module_facts),
        }
    }
}

impl VisitMut for UnObjectSpread<'_> {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let all_helpers = collect_helpers(module);
        let local_helpers: HashMap<BindingKey, BabelHelperKind> = all_helpers
            .into_iter()
            .filter(|(_, kind)| {
                *kind == BabelHelperKind::Extends
                    || *kind == BabelHelperKind::ObjectSpread
                    || *kind == BabelHelperKind::HelperDependency
            })
            .collect();
        let mut helpers = local_helpers.clone();
        if let Some(module_facts) = self.module_facts {
            helpers.extend(collect_cross_module_object_spread_helpers(
                module,
                module_facts,
            ));
        }
        if helpers.is_empty() {
            return;
        }

        let mut replacer = SpreadReplacer { helpers: &helpers };
        module.visit_mut_with(&mut replacer);

        // Only remove declaration if no untransformed calls remain
        let remaining = helpers_with_remaining_refs(module, &local_helpers);
        let safe_to_remove: HashMap<BindingKey, BabelHelperKind> = helpers
            .into_iter()
            .filter(|(key, _)| local_helpers.contains_key(key) && !remaining.contains(key))
            .collect();
        if !safe_to_remove.is_empty() {
            remove_helper_declarations(&mut module.body, &safe_to_remove);
        }
    }
}

impl Default for UnObjectSpread<'_> {
    fn default() -> Self {
        Self::new()
    }
}

fn collect_cross_module_object_spread_helpers(
    module: &Module,
    module_facts: &ModuleFactsMap,
) -> HashMap<BindingKey, BabelHelperKind> {
    let mut helpers = HashMap::new();

    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        let source = str_to_atom(&import.src.value);

        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Default(default) => {
                    if let Some(kind) = module_helper_export_kind(module_facts, &source, "default")
                    {
                        helpers.insert((default.local.sym.clone(), default.local.ctxt), kind);
                    }
                }
                ImportSpecifier::Named(named) => {
                    let imported = named
                        .imported
                        .as_ref()
                        .map(export_name_to_atom)
                        .unwrap_or_else(|| named.local.sym.clone());
                    if let Some(kind) =
                        module_helper_export_kind(module_facts, &source, imported.as_ref())
                    {
                        helpers.insert((named.local.sym.clone(), named.local.ctxt), kind);
                    }
                }
                ImportSpecifier::Namespace(_) => {}
            }
        }
    }

    helpers
}

fn module_helper_export_kind(
    module_facts: &ModuleFactsMap,
    source: &Atom,
    exported: &str,
) -> Option<BabelHelperKind> {
    module_facts.get(source.as_ref()).and_then(|facts| {
        facts
            .helper_exports
            .iter()
            .find(|helper| helper.exported.as_ref() == exported)
            .and_then(|helper| helper_kind_to_babel(helper.kind))
    })
}

fn helper_kind_to_babel(kind: HelperKind) -> Option<BabelHelperKind> {
    match kind {
        HelperKind::Extends => Some(BabelHelperKind::Extends),
        HelperKind::ObjectSpread => Some(BabelHelperKind::ObjectSpread),
        _ => None,
    }
}

struct SpreadReplacer<'a> {
    helpers: &'a HashMap<BindingKey, BabelHelperKind>,
}

impl VisitMut for SpreadReplacer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Ident(id) = callee.as_ref() else {
            return;
        };

        let key = (id.sym.clone(), id.ctxt);
        let Some(kind) = self.helpers.get(&key) else {
            return;
        };
        if !matches!(
            kind,
            BabelHelperKind::Extends | BabelHelperKind::ObjectSpread
        ) {
            return;
        };

        if call.args.is_empty() {
            return;
        }

        // Both _extends and _objectSpread2 mutate their first argument.
        // Only transform when the first arg is a safe fresh object literal
        // target, otherwise mutation/identity semantics are lost.
        let Expr::Object(first_obj) = call.args[0].expr.as_ref() else {
            return;
        };
        if call.args[0].spread.is_some() || !is_safe_to_inline_props(&first_obj.props) {
            return;
        }

        // Merge all arguments into a single object expression.
        // - Object literal args: flatten their properties
        // - Everything else: wrap as spread element
        let mut properties: Vec<PropOrSpread> = first_obj.props.clone();

        for arg in &call.args[1..] {
            if arg.spread.is_some() {
                properties.push(PropOrSpread::Spread(SpreadElement {
                    dot3_token: DUMMY_SP,
                    expr: arg.expr.clone(),
                }));
                continue;
            }

            match arg.expr.as_ref() {
                Expr::Object(obj) if is_safe_to_inline_props(&obj.props) => {
                    properties.extend(obj.props.iter().cloned());
                }
                _ => {
                    properties.push(PropOrSpread::Spread(SpreadElement {
                        dot3_token: DUMMY_SP,
                        expr: arg.expr.clone(),
                    }));
                }
            }
        }

        *expr = Expr::Object(ObjectLit {
            span: DUMMY_SP,
            props: properties,
        });
    }
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

fn export_name_to_atom(name: &swc_core::ecma::ast::ModuleExportName) -> Atom {
    match name {
        swc_core::ecma::ast::ModuleExportName::Ident(id) => id.sym.clone(),
        swc_core::ecma::ast::ModuleExportName::Str(s) => str_to_atom(&s.value),
    }
}

fn str_to_atom(value: &swc_core::atoms::Wtf8Atom) -> Atom {
    Atom::from(value.as_str().unwrap_or(""))
}
