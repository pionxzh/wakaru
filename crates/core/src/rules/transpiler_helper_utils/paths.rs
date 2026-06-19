//! Helper import-path constants and path/name-based helper classification.
//!
//! Babel runtime paths and SWC external-helper paths are grouped together per
//! helper kind: detection is by *what the helper is*, not which transpiler
//! emitted it.

use swc_core::atoms::Atom;
use swc_core::ecma::ast::ModuleExportName;

use super::TranspilerHelperKind;

const INTEROP_DEFAULT_PATHS: &[&str] = &[
    "@babel/runtime/helpers/interopRequireDefault",
    "@babel/runtime/helpers/esm/interopRequireDefault",
    "@swc/helpers/_/_interop_require_default",
];

const INTEROP_WILDCARD_PATHS: &[&str] = &[
    "@babel/runtime/helpers/interopRequireWildcard",
    "@babel/runtime/helpers/esm/interopRequireWildcard",
    "@swc/helpers/_/_interop_require_wildcard",
];

const TO_CONSUMABLE_ARRAY_PATHS: &[&str] = &[
    "@babel/runtime/helpers/toConsumableArray",
    "@babel/runtime/helpers/esm/toConsumableArray",
    "@swc/helpers/_/_to_consumable_array",
];

const EXTENDS_PATHS: &[&str] = &[
    "@babel/runtime/helpers/extends",
    "@babel/runtime/helpers/esm/extends",
    "@swc/helpers/_/_extends",
];

const OBJECT_SPREAD_PATHS: &[&str] = &[
    "@babel/runtime/helpers/objectSpread2",
    "@babel/runtime/helpers/esm/objectSpread2",
    "@babel/runtime/helpers/objectSpread",
    "@babel/runtime/helpers/esm/objectSpread",
    "@swc/helpers/_/_object_spread",
    "@swc/helpers/_/_object_spread_props",
];

const SLICED_TO_ARRAY_PATHS: &[&str] = &[
    "@babel/runtime/helpers/slicedToArray",
    "@babel/runtime/helpers/esm/slicedToArray",
    "@swc/helpers/_/_sliced_to_array",
];

const OBJECT_WITHOUT_PROPERTIES_PATHS: &[&str] = &[
    "@babel/runtime/helpers/objectWithoutProperties",
    "@babel/runtime/helpers/esm/objectWithoutProperties",
    "@babel/runtime/helpers/objectWithoutPropertiesLoose",
    "@babel/runtime/helpers/esm/objectWithoutPropertiesLoose",
    "@swc/helpers/_/_object_without_properties",
    "@swc/helpers/_/_object_without_properties_loose",
];

const INHERITS_PATHS: &[&str] = &[
    "@babel/runtime/helpers/inherits",
    "@babel/runtime/helpers/esm/inherits",
    "@swc/helpers/_/_inherits",
];

const ASYNC_TO_GENERATOR_PATHS: &[&str] = &[
    "@babel/runtime/helpers/asyncToGenerator",
    "@babel/runtime/helpers/esm/asyncToGenerator",
    "@swc/helpers/_/_async_to_generator",
];

const DEFINE_PROPERTY_PATHS: &[&str] = &[
    "@babel/runtime/helpers/defineProperty",
    "@babel/runtime/helpers/esm/defineProperty",
    "@swc/helpers/_/_define_property",
];

const CLASS_CALL_CHECK_PATHS: &[&str] = &[
    "@babel/runtime/helpers/classCallCheck",
    "@babel/runtime/helpers/esm/classCallCheck",
    "@swc/helpers/_/_class_call_check",
];

const POSSIBLE_CONSTRUCTOR_RETURN_PATHS: &[&str] = &[
    "@babel/runtime/helpers/possibleConstructorReturn",
    "@babel/runtime/helpers/esm/possibleConstructorReturn",
    "@swc/helpers/_/_possible_constructor_return",
];

const ASSERT_THIS_INITIALIZED_PATHS: &[&str] = &[
    "@babel/runtime/helpers/assertThisInitialized",
    "@babel/runtime/helpers/esm/assertThisInitialized",
    "@swc/helpers/_/_assert_this_initialized",
];

const CALL_SUPER_PATHS: &[&str] = &["@swc/helpers/_/_call_super"];

const CREATE_CLASS_PATHS: &[&str] = &[
    "@babel/runtime/helpers/createClass",
    "@babel/runtime/helpers/esm/createClass",
    "@swc/helpers/_/_create_class",
];

const TAGGED_TEMPLATE_LITERAL_PATHS: &[&str] = &["@swc/helpers/_/_tagged_template_literal"];

pub(crate) fn detect_helper_from_path(path: &str) -> Option<TranspilerHelperKind> {
    if INTEROP_DEFAULT_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::InteropRequireDefault);
    }
    if INTEROP_WILDCARD_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::InteropRequireWildcard);
    }
    if TO_CONSUMABLE_ARRAY_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::ToConsumableArray);
    }
    if EXTENDS_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::Extends);
    }
    if OBJECT_SPREAD_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::ObjectSpread);
    }
    if SLICED_TO_ARRAY_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::SlicedToArray);
    }
    if OBJECT_WITHOUT_PROPERTIES_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::ObjectWithoutProperties);
    }
    if INHERITS_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::Inherits);
    }
    if ASYNC_TO_GENERATOR_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::AsyncToGenerator);
    }
    if DEFINE_PROPERTY_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::DefineProperty);
    }
    if TAGGED_TEMPLATE_LITERAL_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::TaggedTemplateLiteral);
    }
    if CLASS_CALL_CHECK_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::ClassCallCheck);
    }
    if POSSIBLE_CONSTRUCTOR_RETURN_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::PossibleConstructorReturn);
    }
    if ASSERT_THIS_INITIALIZED_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::AssertThisInitialized);
    }
    if CALL_SUPER_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::CallSuper);
    }
    if CREATE_CLASS_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::CreateClass);
    }
    if path == "@swc/helpers/_/_array_with_holes" || path == "@swc/helpers/_/_set_prototype_of" {
        return Some(TranspilerHelperKind::HelperDependency);
    }
    None
}

fn export_name_is(name: &swc_core::ecma::ast::ModuleExportName, expected: &str) -> bool {
    match name {
        swc_core::ecma::ast::ModuleExportName::Ident(id) => id.sym.as_ref() == expected,
        swc_core::ecma::ast::ModuleExportName::Str(s) => s.value.as_str() == Some(expected),
    }
}

pub(super) fn export_name_to_atom(name: &ModuleExportName) -> Atom {
    match name {
        ModuleExportName::Ident(id) => id.sym.clone(),
        ModuleExportName::Str(s) => Atom::from(s.value.as_str().unwrap_or("")),
    }
}

pub(super) fn named_import_is_helper(
    path: &str,
    named: &swc_core::ecma::ast::ImportNamedSpecifier,
) -> bool {
    named
        .imported
        .as_ref()
        .is_some_and(|imported| export_name_is(imported, "default"))
        || (is_swc_helper_path(path)
            && named
                .imported
                .as_ref()
                .map_or(named.local.sym.as_ref() == "_", |imported| {
                    export_name_is(imported, "_")
                }))
}

fn is_swc_helper_path(path: &str) -> bool {
    path.starts_with("@swc/helpers/_/_")
}
