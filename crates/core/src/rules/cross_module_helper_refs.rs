use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    Callee, Expr, ImportSpecifier, Module, ModuleDecl, ModuleItem, Pat, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::facts::{HelperKind, ModuleFactsMap, TypeScriptHelperKind};

use super::helper_matcher::{binding_key, static_member_prop_name, BindingKey};
use super::transpiler_helper_utils::TranspilerHelperKind;

#[derive(Default)]
pub(crate) struct CrossModuleHelperRefs {
    pub direct: HashMap<BindingKey, TranspilerHelperKind>,
    pub namespaces: HashMap<BindingKey, HashMap<String, TranspilerHelperKind>>,
}

#[derive(Default)]
pub(crate) struct CrossModuleTsHelperRefs {
    pub direct: HashSet<BindingKey>,
    pub namespaces: HashMap<BindingKey, HashSet<String>>,
}

impl CrossModuleTsHelperRefs {
    pub(crate) fn extend(&mut self, other: Self) {
        self.direct.extend(other.direct);
        for (namespace, names) in other.namespaces {
            self.namespaces.entry(namespace).or_default().extend(names);
        }
    }
}

pub(crate) fn collect_cross_module_helper_refs(
    module: &Module,
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
    include: impl Fn(TranspilerHelperKind) -> bool,
) -> CrossModuleHelperRefs {
    let mut refs = CrossModuleHelperRefs::default();

    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        let source = str_to_atom(&import.src.value);
        let Some(facts) = module_facts.get_from(current_filename, source.as_ref()) else {
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

pub(crate) fn collect_cross_module_ts_helper_refs(
    module: &Module,
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
    kind: TypeScriptHelperKind,
) -> CrossModuleTsHelperRefs {
    let mut refs = CrossModuleTsHelperRefs::default();
    let mut namespace_factories: HashMap<BindingKey, HashSet<String>> = HashMap::new();

    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        let source = str_to_atom(&import.src.value);
        let exported_names = ts_helper_export_names(module_facts, current_filename, &source, kind);
        let Some(source_facts) = module_facts.get_from(current_filename, source.as_ref()) else {
            continue;
        };

        for specifier in &import.specifiers {
            match specifier {
                ImportSpecifier::Default(default) => {
                    if module_exports_ts_helper(
                        module_facts,
                        current_filename,
                        &source,
                        "default",
                        kind,
                    ) {
                        refs.direct
                            .insert((default.local.sym.clone(), default.local.ctxt));
                    } else if source_facts
                        .ts_helper_namespace_factory_exports
                        .iter()
                        .any(|exported| exported.as_ref() == "default")
                    {
                        namespace_factories
                            .insert(binding_key(&default.local), exported_names.clone());
                    }
                }
                ImportSpecifier::Named(named) => {
                    let imported = named
                        .imported
                        .as_ref()
                        .map(export_name_to_atom)
                        .unwrap_or_else(|| named.local.sym.clone());
                    if module_exports_ts_helper(
                        module_facts,
                        current_filename,
                        &source,
                        imported.as_ref(),
                        kind,
                    ) {
                        refs.direct
                            .insert((named.local.sym.clone(), named.local.ctxt));
                    } else if source_facts
                        .ts_helper_namespace_factory_exports
                        .iter()
                        .any(|factory| factory == &imported)
                    {
                        namespace_factories
                            .insert(binding_key(&named.local), exported_names.clone());
                    }
                }
                ImportSpecifier::Namespace(namespace) => {
                    if !exported_names.is_empty() {
                        refs.namespaces.insert(
                            (namespace.local.sym.clone(), namespace.local.ctxt),
                            exported_names.clone(),
                        );
                    }
                }
            }
        }
    }

    if !namespace_factories.is_empty() {
        struct NamespaceFactoryUseCollector<'a, 'b> {
            namespace_factories: &'a HashMap<BindingKey, HashSet<String>>,
            refs: &'b mut CrossModuleTsHelperRefs,
        }

        impl Visit for NamespaceFactoryUseCollector<'_, '_> {
            fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
                let Pat::Ident(binding) = &declarator.name else {
                    declarator.visit_children_with(self);
                    return;
                };
                let Some(factory) = declarator.init.as_deref().and_then(zero_arg_call_ident) else {
                    declarator.visit_children_with(self);
                    return;
                };
                if let Some(exports) = self.namespace_factories.get(&binding_key(factory)) {
                    self.refs
                        .namespaces
                        .insert(binding_key(&binding.id), exports.clone());
                }
                declarator.visit_children_with(self);
            }
        }

        module.visit_with(&mut NamespaceFactoryUseCollector {
            namespace_factories: &namespace_factories,
            refs: &mut refs,
        });
    }

    refs
}

fn zero_arg_call_ident(expr: &Expr) -> Option<&swc_core::ecma::ast::Ident> {
    let Expr::Call(call) = crate::utils::paren::strip_parens(expr) else {
        return None;
    };
    if !call.args.is_empty() {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(id) = crate::utils::paren::strip_parens(callee.as_ref()) else {
        return None;
    };
    Some(id)
}

pub(crate) fn cross_module_ts_member_helper(
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

fn module_exports_ts_helper(
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
    source: &Atom,
    exported: &str,
    kind: TypeScriptHelperKind,
) -> bool {
    module_facts
        .get_from(current_filename, source.as_ref())
        .is_some_and(|facts| {
            facts
                .ts_helper_exports
                .iter()
                .any(|helper| helper.exported.as_ref() == exported && helper.kind == kind)
        })
}

fn ts_helper_export_names(
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
    source: &Atom,
    kind: TypeScriptHelperKind,
) -> HashSet<String> {
    module_facts
        .get_from(current_filename, source.as_ref())
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
