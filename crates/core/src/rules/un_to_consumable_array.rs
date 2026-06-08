use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrayLit, Callee, Expr, ExprOrSpread, ImportSpecifier, Module, ModuleDecl, ModuleItem,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use crate::facts::{ModuleFactsMap, TypeScriptHelperKind};

use super::helper_matcher::{binding_key, static_member_prop_name};
use super::transpiler_helper_utils::{
    is_tslib_spread_array_member, remove_helpers_without_remaining_refs, BindingKey,
    LocalHelperContext, TranspilerHelperKind, TsHelperKind,
};

/// Detects and replaces `_toConsumableArray(arr)` with `[...arr]`.
pub struct UnToConsumableArray<'a> {
    module_facts: Option<&'a ModuleFactsMap>,
}

impl UnToConsumableArray<'_> {
    pub fn new() -> Self {
        Self { module_facts: None }
    }
}

impl<'a> UnToConsumableArray<'a> {
    pub fn new_with_facts(module_facts: &'a ModuleFactsMap) -> Self {
        Self {
            module_facts: Some(module_facts),
        }
    }

    pub(crate) fn run_with_helpers(
        module: &mut Module,
        local_helpers: &LocalHelperContext,
        module_facts: Option<&ModuleFactsMap>,
    ) {
        run_un_to_consumable_array(module, local_helpers, module_facts);
    }
}

impl Default for UnToConsumableArray<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl VisitMut for UnToConsumableArray<'_> {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_to_consumable_array(module, &local_helpers, self.module_facts);
    }
}

fn run_un_to_consumable_array(
    module: &mut Module,
    local_helpers: &LocalHelperContext,
    module_facts: Option<&ModuleFactsMap>,
) {
    let helpers = local_helpers.helpers_of_kind(TranspilerHelperKind::ToConsumableArray);
    let ts_helpers = local_helpers.ts_helpers_of_kind(TsHelperKind::SpreadArray);
    let cross_module_ts_helpers = module_facts
        .map(|facts| {
            collect_cross_module_ts_helper_refs(module, facts, TypeScriptHelperKind::SpreadArray)
        })
        .unwrap_or_default();
    let tslib_namespaces = local_helpers.tslib_namespaces();
    if helpers.is_empty() {
        if ts_helpers.is_empty()
            && cross_module_ts_helpers.direct.is_empty()
            && cross_module_ts_helpers.namespaces.is_empty()
            && tslib_namespaces.is_empty()
        {
            return;
        }

        let mut replacer = ToConsumableArrayReplacer {
            helpers: &helpers,
            ts_spread_array_helpers: &ts_helpers,
            cross_module_ts_spread_array_helpers: &cross_module_ts_helpers.direct,
            cross_module_ts_spread_array_namespaces: &cross_module_ts_helpers.namespaces,
            tslib_namespaces,
        };
        module.visit_mut_with(&mut replacer);

        local_helpers.remove_unused_ts_helper_bindings(module, TsHelperKind::SpreadArray);
        return;
    }

    let mut replacer = ToConsumableArrayReplacer {
        helpers: &helpers,
        ts_spread_array_helpers: &ts_helpers,
        cross_module_ts_spread_array_helpers: &cross_module_ts_helpers.direct,
        cross_module_ts_spread_array_namespaces: &cross_module_ts_helpers.namespaces,
        tslib_namespaces,
    };
    module.visit_mut_with(&mut replacer);

    remove_helpers_without_remaining_refs(module, helpers);
    local_helpers.remove_unused_ts_helper_bindings(module, TsHelperKind::SpreadArray);
}

struct ToConsumableArrayReplacer<'a> {
    helpers: &'a HashMap<BindingKey, TranspilerHelperKind>,
    ts_spread_array_helpers: &'a HashSet<BindingKey>,
    cross_module_ts_spread_array_helpers: &'a HashSet<BindingKey>,
    cross_module_ts_spread_array_namespaces: &'a HashMap<BindingKey, HashSet<String>>,
    tslib_namespaces: &'a HashSet<BindingKey>,
}

impl VisitMut for ToConsumableArrayReplacer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        let Callee::Expr(callee) = &call.callee else {
            return;
        };

        if let Expr::Ident(id) = callee.as_ref() {
            let key = binding_key(id);
            if self.helpers.contains_key(&key) {
                // Only transform single-argument calls
                if call.args.len() != 1 {
                    return;
                }

                // _toConsumableArray(arg) -> [...arg]
                *expr = Expr::Array(ArrayLit {
                    span: DUMMY_SP,
                    elems: vec![Some(ExprOrSpread {
                        spread: Some(DUMMY_SP),
                        expr: call.args[0].expr.clone(),
                    })],
                });
                return;
            }

            if self.ts_spread_array_helpers.contains(&key)
                || self.cross_module_ts_spread_array_helpers.contains(&key)
            {
                if let Some(array) = convert_ts_spread_array_call(call) {
                    *expr = Expr::Array(array);
                }
                return;
            }
        }

        if is_tslib_spread_array_member(callee, self.tslib_namespaces) {
            if let Some(array) = convert_ts_spread_array_call(call) {
                *expr = Expr::Array(array);
            }
            return;
        }

        if is_cross_module_ts_helper_member(callee, self.cross_module_ts_spread_array_namespaces) {
            if let Some(array) = convert_ts_spread_array_call(call) {
                *expr = Expr::Array(array);
            }
        }
    }
}

#[derive(Default)]
struct CrossModuleTsHelperRefs {
    direct: HashSet<BindingKey>,
    namespaces: HashMap<BindingKey, HashSet<String>>,
}

fn collect_cross_module_ts_helper_refs(
    module: &Module,
    module_facts: &ModuleFactsMap,
    kind: TypeScriptHelperKind,
) -> CrossModuleTsHelperRefs {
    let mut refs = CrossModuleTsHelperRefs::default();

    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        let source = str_to_atom(&import.src.value);

        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Default(default) => {
                    if module_exports_ts_helper(module_facts, &source, "default", kind) {
                        refs.direct
                            .insert((default.local.sym.clone(), default.local.ctxt));
                    }
                }
                ImportSpecifier::Named(named) => {
                    let imported = named
                        .imported
                        .as_ref()
                        .map(export_name_to_atom)
                        .unwrap_or_else(|| named.local.sym.clone());
                    if module_exports_ts_helper(module_facts, &source, imported.as_ref(), kind) {
                        refs.direct
                            .insert((named.local.sym.clone(), named.local.ctxt));
                    }
                }
                ImportSpecifier::Namespace(namespace) => {
                    let exported_names = ts_helper_export_names(module_facts, &source, kind);
                    if !exported_names.is_empty() {
                        refs.namespaces.insert(
                            (namespace.local.sym.clone(), namespace.local.ctxt),
                            exported_names,
                        );
                    }
                }
            }
        }
    }

    refs
}

fn module_exports_ts_helper(
    module_facts: &ModuleFactsMap,
    source: &Atom,
    exported: &str,
    kind: TypeScriptHelperKind,
) -> bool {
    module_facts.get(source.as_ref()).is_some_and(|facts| {
        facts
            .ts_helper_exports
            .iter()
            .any(|helper| helper.exported.as_ref() == exported && helper.kind == kind)
    })
}

fn ts_helper_export_names(
    module_facts: &ModuleFactsMap,
    source: &Atom,
    kind: TypeScriptHelperKind,
) -> HashSet<String> {
    module_facts
        .get(source.as_ref())
        .map(|facts| {
            facts
                .ts_helper_exports
                .iter()
                .filter(|helper| helper.kind == kind)
                .map(|helper| helper.exported.to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn is_cross_module_ts_helper_member(
    expr: &Expr,
    namespaces: &HashMap<BindingKey, HashSet<String>>,
) -> bool {
    let Expr::Member(member) = expr else {
        return false;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };
    let Some(exported_names) = namespaces.get(&binding_key(obj)) else {
        return false;
    };
    static_member_prop_name(&member.prop).is_some_and(|name| exported_names.contains(name))
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

fn convert_ts_spread_array_call(call: &swc_core::ecma::ast::CallExpr) -> Option<ArrayLit> {
    if call.args.len() != 3 || call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }

    let mut elems = Vec::new();
    append_array_source(&mut elems, call.args[0].expr.as_ref(), true)?;
    append_array_source(&mut elems, call.args[1].expr.as_ref(), false)?;

    Some(ArrayLit {
        span: DUMMY_SP,
        elems,
    })
}

fn append_array_source(
    elems: &mut Vec<Option<ExprOrSpread>>,
    expr: &Expr,
    require_array_literal: bool,
) -> Option<()> {
    match expr {
        Expr::Array(array) => {
            elems.extend(array.elems.iter().cloned());
            Some(())
        }
        _ if !require_array_literal => {
            elems.push(Some(ExprOrSpread {
                spread: Some(DUMMY_SP),
                expr: Box::new(expr.clone()),
            }));
            Some(())
        }
        _ => None,
    }
}
