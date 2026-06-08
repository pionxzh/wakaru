use std::collections::HashMap;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{Expr, ImportSpecifier, Module, ModuleDecl, ModuleItem};

use crate::facts::{HelperKind, ModuleFactsMap};

use super::helper_matcher::{binding_key, static_member_prop_name, BindingKey};
use super::transpiler_helper_utils::TranspilerHelperKind;

#[derive(Default)]
pub(crate) struct CrossModuleHelperRefs {
    pub direct: HashMap<BindingKey, TranspilerHelperKind>,
    pub namespaces: HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
}

pub(crate) fn collect_cross_module_helper_refs(
    module: &Module,
    module_facts: &ModuleFactsMap,
    include: impl Fn(TranspilerHelperKind) -> bool,
) -> CrossModuleHelperRefs {
    let mut refs = CrossModuleHelperRefs::default();

    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        let source = str_to_atom(&import.src.value);
        let Some(facts) = module_facts.get(source.as_ref()) else {
            continue;
        };

        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Default(default) => {
                    collect_default_ref(
                        &mut refs,
                        (default.local.sym.clone(), default.local.ctxt),
                        facts,
                        &include,
                    );
                }
                ImportSpecifier::Named(named) => {
                    let imported = named
                        .imported
                        .as_ref()
                        .map(export_name_to_atom)
                        .unwrap_or_else(|| named.local.sym.clone());
                    if let Some(kind) = facts
                        .helper_exports
                        .iter()
                        .find(|helper| helper.exported.as_ref() == imported.as_ref())
                        .and_then(|helper| helper_kind_to_transpiler(helper.kind))
                        .filter(|kind| include(*kind))
                    {
                        refs.direct
                            .insert((named.local.sym.clone(), named.local.ctxt), kind);
                    }
                }
                ImportSpecifier::Namespace(namespace_import) => {
                    collect_namespace_ref(
                        &mut refs,
                        (
                            namespace_import.local.sym.clone(),
                            namespace_import.local.ctxt,
                        ),
                        facts,
                        &include,
                    );
                }
            }
        }
    }

    refs
}

fn collect_default_ref(
    refs: &mut CrossModuleHelperRefs,
    local: BindingKey,
    facts: &crate::facts::ModuleFacts,
    include: &impl Fn(TranspilerHelperKind) -> bool,
) {
    if let Some(kind) = facts
        .helper_exports
        .iter()
        .find(|helper| helper.exported.as_ref() == "default")
        .and_then(|helper| helper_kind_to_transpiler(helper.kind))
        .filter(|kind| include(*kind))
    {
        refs.direct.insert(local.clone(), kind);
    }

    let namespace = filtered_helper_members(
        facts
            .default_object_helper_exports
            .iter()
            .map(|helper| (helper.exported.to_string(), helper.kind)),
        include,
    );
    if !namespace.is_empty() {
        refs.namespaces.insert(local, namespace);
    }
}

fn collect_namespace_ref(
    refs: &mut CrossModuleHelperRefs,
    local: BindingKey,
    facts: &crate::facts::ModuleFacts,
    include: &impl Fn(TranspilerHelperKind) -> bool,
) {
    let namespace = filtered_helper_members(
        facts
            .helper_exports
            .iter()
            .chain(facts.default_object_helper_exports.iter())
            .map(|helper| (helper.exported.to_string(), helper.kind)),
        include,
    );
    if !namespace.is_empty() {
        refs.namespaces.insert(local, namespace);
    }
}

pub(crate) fn cross_module_member_helper_kind(
    expr: &Expr,
    namespaces: &HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
) -> Option<TranspilerHelperKind> {
    let Expr::Member(member) = expr else {
        return None;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return None;
    };
    let exported = static_member_prop_name(&member.prop)?;
    namespaces
        .get(&binding_key(obj))
        .and_then(|helpers| helpers.get(exported))
        .copied()
}

fn filtered_helper_members(
    helpers: impl Iterator<Item = (String, HelperKind)>,
    include: &impl Fn(TranspilerHelperKind) -> bool,
) -> HashMap<String, TranspilerHelperKind> {
    helpers
        .filter_map(|(exported, kind)| {
            let kind = helper_kind_to_transpiler(kind)?;
            include(kind).then_some((exported, kind))
        })
        .collect()
}

fn helper_kind_to_transpiler(kind: HelperKind) -> Option<TranspilerHelperKind> {
    match kind {
        HelperKind::InteropRequireDefault => Some(TranspilerHelperKind::InteropRequireDefault),
        HelperKind::InteropRequireWildcard => Some(TranspilerHelperKind::InteropRequireWildcard),
        HelperKind::ToConsumableArray => Some(TranspilerHelperKind::ToConsumableArray),
        HelperKind::Extends => Some(TranspilerHelperKind::Extends),
        HelperKind::ObjectSpread => Some(TranspilerHelperKind::ObjectSpread),
        HelperKind::SlicedToArray => Some(TranspilerHelperKind::SlicedToArray),
        HelperKind::ClassCallCheck => Some(TranspilerHelperKind::ClassCallCheck),
        HelperKind::PossibleConstructorReturn => {
            Some(TranspilerHelperKind::PossibleConstructorReturn)
        }
        HelperKind::AssertThisInitialized => Some(TranspilerHelperKind::AssertThisInitialized),
        HelperKind::ObjectWithoutProperties => Some(TranspilerHelperKind::ObjectWithoutProperties),
        HelperKind::Inherits => Some(TranspilerHelperKind::Inherits),
        HelperKind::CallSuper => Some(TranspilerHelperKind::CallSuper),
        HelperKind::AsyncToGenerator => Some(TranspilerHelperKind::AsyncToGenerator),
        HelperKind::TaggedTemplateLiteral => Some(TranspilerHelperKind::TaggedTemplateLiteral),
        HelperKind::RegeneratorRuntime => None,
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
