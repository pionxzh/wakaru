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

// NOTE: @swc/helpers/_/_sliced_to_array_loose exists in the package but no
// current SWC transform emits it — `loose: true` destructuring skips the
// helper entirely (direct index access). Same for its sub-helper
// _iterable_to_array_limit_loose. Verified 2026-06-19 against swc main.
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
    "@babel/runtime/helpers/inheritsLoose",
    "@babel/runtime/helpers/esm/inheritsLoose",
    "@swc/helpers/_/_inherits",
    "@swc/helpers/_/_inherits_loose",
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

const CALL_SUPER_PATHS: &[&str] = &[
    "@babel/runtime/helpers/callSuper",
    "@babel/runtime/helpers/esm/callSuper",
    "@swc/helpers/_/_call_super",
];

const CREATE_CLASS_PATHS: &[&str] = &[
    "@babel/runtime/helpers/createClass",
    "@babel/runtime/helpers/esm/createClass",
    "@swc/helpers/_/_create_class",
];

const TAGGED_TEMPLATE_LITERAL_PATHS: &[&str] = &[
    "@babel/runtime/helpers/taggedTemplateLiteral",
    "@babel/runtime/helpers/esm/taggedTemplateLiteral",
    "@babel/runtime/helpers/taggedTemplateLiteralLoose",
    "@babel/runtime/helpers/esm/taggedTemplateLiteralLoose",
    "@swc/helpers/_/_tagged_template_literal",
    "@swc/helpers/_/_tagged_template_literal_loose",
];

const TYPEOF_PATHS: &[&str] = &[
    "@babel/runtime/helpers/typeof",
    "@babel/runtime/helpers/esm/typeof",
    "@swc/helpers/_/_type_of",
];

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
    if TYPEOF_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::Typeof);
    }
    if HELPER_DEPENDENCY_PATHS.contains(&path) {
        return Some(TranspilerHelperKind::HelperDependency);
    }
    None
}

/// Sub-helper paths: internal dependencies of parent helpers that have no
/// standalone semantic meaning. When a parent is unwrapped, these orphaned
/// imports should be removed.
const HELPER_DEPENDENCY_PATHS: &[&str] = &[
    // --- array destructuring (from slicedToArray) ---
    "@babel/runtime/helpers/arrayWithHoles",
    "@babel/runtime/helpers/esm/arrayWithHoles",
    "@swc/helpers/_/_array_with_holes",
    "@babel/runtime/helpers/iterableToArrayLimit",
    "@babel/runtime/helpers/esm/iterableToArrayLimit",
    "@swc/helpers/_/_iterable_to_array_limit",
    "@babel/runtime/helpers/nonIterableRest",
    "@babel/runtime/helpers/esm/nonIterableRest",
    "@swc/helpers/_/_non_iterable_rest",
    // --- array spread (from toConsumableArray) ---
    "@babel/runtime/helpers/arrayWithoutHoles",
    "@babel/runtime/helpers/esm/arrayWithoutHoles",
    "@swc/helpers/_/_array_without_holes",
    "@babel/runtime/helpers/iterableToArray",
    "@babel/runtime/helpers/esm/iterableToArray",
    "@swc/helpers/_/_iterable_to_array",
    "@babel/runtime/helpers/nonIterableSpread",
    "@babel/runtime/helpers/esm/nonIterableSpread",
    "@swc/helpers/_/_non_iterable_spread",
    // --- shared array helpers (from slicedToArray + toConsumableArray) ---
    "@babel/runtime/helpers/unsupportedIterableToArray",
    "@babel/runtime/helpers/esm/unsupportedIterableToArray",
    "@swc/helpers/_/_unsupported_iterable_to_array",
    "@babel/runtime/helpers/arrayLikeToArray",
    "@babel/runtime/helpers/esm/arrayLikeToArray",
    "@swc/helpers/_/_array_like_to_array",
    // --- inheritance (from inherits, callSuper) ---
    "@babel/runtime/helpers/setPrototypeOf",
    "@babel/runtime/helpers/esm/setPrototypeOf",
    "@swc/helpers/_/_set_prototype_of",
    "@babel/runtime/helpers/getPrototypeOf",
    "@babel/runtime/helpers/esm/getPrototypeOf",
    "@swc/helpers/_/_get_prototype_of",
    "@babel/runtime/helpers/isNativeReflectConstruct",
    "@babel/runtime/helpers/esm/isNativeReflectConstruct",
    "@swc/helpers/_/_is_native_reflect_construct",
    // --- for-of iteration (from UnForOf shape-match; cleanup only) ---
    "@babel/runtime/helpers/createForOfIteratorHelper",
    "@babel/runtime/helpers/esm/createForOfIteratorHelper",
    "@babel/runtime/helpers/createForOfIteratorHelperLoose",
    "@babel/runtime/helpers/esm/createForOfIteratorHelperLoose",
    "@swc/helpers/_/_create_for_of_iterator_helper_loose",
    // --- property keys (from createClass, defineProperty) ---
    "@babel/runtime/helpers/toPropertyKey",
    "@babel/runtime/helpers/esm/toPropertyKey",
    "@swc/helpers/_/_to_property_key",
    "@babel/runtime/helpers/toPrimitive",
    "@babel/runtime/helpers/esm/toPrimitive",
    "@swc/helpers/_/_to_primitive",
];

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_runtime_typeof_imports() {
        for path in [
            "@babel/runtime/helpers/typeof",
            "@babel/runtime/helpers/esm/typeof",
            "@swc/helpers/_/_type_of",
        ] {
            assert_eq!(
                detect_helper_from_path(path),
                Some(TranspilerHelperKind::Typeof),
                "{path}"
            );
        }
    }

    #[test]
    fn classifies_babel_runtime_call_super_imports() {
        for path in [
            "@babel/runtime/helpers/callSuper",
            "@babel/runtime/helpers/esm/callSuper",
        ] {
            assert_eq!(
                detect_helper_from_path(path),
                Some(TranspilerHelperKind::CallSuper),
                "{path}"
            );
        }
    }

    #[test]
    fn classifies_runtime_tagged_template_imports() {
        for path in [
            "@babel/runtime/helpers/taggedTemplateLiteral",
            "@babel/runtime/helpers/esm/taggedTemplateLiteral",
            "@babel/runtime/helpers/taggedTemplateLiteralLoose",
            "@babel/runtime/helpers/esm/taggedTemplateLiteralLoose",
            "@swc/helpers/_/_tagged_template_literal",
            "@swc/helpers/_/_tagged_template_literal_loose",
        ] {
            assert_eq!(
                detect_helper_from_path(path),
                Some(TranspilerHelperKind::TaggedTemplateLiteral),
                "{path}"
            );
        }
    }

    #[test]
    fn classifies_inherits_loose_paths() {
        for path in [
            "@babel/runtime/helpers/inheritsLoose",
            "@babel/runtime/helpers/esm/inheritsLoose",
            "@swc/helpers/_/_inherits_loose",
        ] {
            assert_eq!(
                detect_helper_from_path(path),
                Some(TranspilerHelperKind::Inherits),
                "{path}"
            );
        }
    }

    #[test]
    fn classifies_sub_helper_dependency_paths() {
        let paths = [
            // array destructuring
            "@babel/runtime/helpers/arrayWithHoles",
            "@babel/runtime/helpers/esm/arrayWithHoles",
            "@swc/helpers/_/_array_with_holes",
            "@babel/runtime/helpers/iterableToArrayLimit",
            "@swc/helpers/_/_iterable_to_array_limit",
            "@babel/runtime/helpers/nonIterableRest",
            "@swc/helpers/_/_non_iterable_rest",
            // array spread
            "@babel/runtime/helpers/arrayWithoutHoles",
            "@swc/helpers/_/_array_without_holes",
            "@babel/runtime/helpers/iterableToArray",
            "@swc/helpers/_/_iterable_to_array",
            "@babel/runtime/helpers/nonIterableSpread",
            "@swc/helpers/_/_non_iterable_spread",
            // shared array
            "@babel/runtime/helpers/unsupportedIterableToArray",
            "@swc/helpers/_/_unsupported_iterable_to_array",
            "@babel/runtime/helpers/arrayLikeToArray",
            "@swc/helpers/_/_array_like_to_array",
            // inheritance
            "@babel/runtime/helpers/setPrototypeOf",
            "@swc/helpers/_/_set_prototype_of",
            "@babel/runtime/helpers/getPrototypeOf",
            "@swc/helpers/_/_get_prototype_of",
            "@babel/runtime/helpers/isNativeReflectConstruct",
            "@swc/helpers/_/_is_native_reflect_construct",
            // property keys
            "@babel/runtime/helpers/toPropertyKey",
            "@swc/helpers/_/_to_property_key",
            "@babel/runtime/helpers/toPrimitive",
            "@swc/helpers/_/_to_primitive",
            // for-of iteration
            "@babel/runtime/helpers/createForOfIteratorHelper",
            "@babel/runtime/helpers/createForOfIteratorHelperLoose",
            "@swc/helpers/_/_create_for_of_iterator_helper_loose",
        ];
        for path in paths {
            assert_eq!(
                detect_helper_from_path(path),
                Some(TranspilerHelperKind::HelperDependency),
                "{path}"
            );
        }
    }
}
