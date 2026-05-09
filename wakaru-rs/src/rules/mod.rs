mod arg_rest;
mod arrow_function;
mod arrow_return;
mod babel_helper_utils;
mod dead_decls;
mod dead_imports;
mod exponent;
mod flip_comparisons;
mod import_dedup;
mod obj_method_shorthand;
mod obj_shorthand;
mod object_assign_spread;
mod remove_void;
pub(crate) mod rename_utils;
mod simplify_sequence;
mod smart_inline;
mod smart_rename;
mod un_argument_spread;
mod un_array_concat_spread;
mod un_assert_this_initialized;
mod un_assignment_merging;
mod un_async_await;
mod un_bracket_notation;
mod un_builtin_prototype;
mod un_class_call_check;
mod un_class_fields;
mod un_conditionals;
mod un_curly_braces;
mod un_define_property;
mod un_destructuring;
mod un_double_negation;
mod un_enum;
mod un_es6_class;
mod un_esm;
mod un_esmodule_flag;
mod un_export_rename;
mod un_for_of;
mod un_iife;
mod un_import_rename;
mod un_indirect_call;
mod un_infinity;
mod un_interop_require_default;
mod un_interop_require_wildcard;
mod un_jsx;
mod un_nullish_coalescing;
mod un_numeric_literal;
mod un_object_rest;
mod un_object_spread;
mod un_optional_chaining;
mod un_parameters;
mod un_possible_constructor_return;
mod un_prototype_class;
mod un_rest_array_copy;
mod un_return;
mod un_sliced_to_array;
mod un_spread_array_literal;
mod un_template_literal;
mod un_then_catch;
mod un_to_consumable_array;
mod un_ts_helpers;
mod un_type_constructor;
mod un_typeof;
mod un_typeof_polyfill;
mod un_typeof_strict;
mod un_undefined_init;
mod un_use_strict;
mod un_variable_merging;
mod un_webpack_define_getters;
mod un_webpack_interop;
mod un_webpack_object_getters;
mod un_while_loop;
mod unminify_booleans;
mod var_decl_to_let_const;

use swc_core::common::Mark;
use swc_core::ecma::ast::Module;
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum RewriteLevel {
    Minimal,
    #[default]
    Standard,
    Aggressive,
}

pub use arg_rest::ArgRest;
pub use arrow_function::ArrowFunction;
pub use arrow_return::ArrowReturn;
pub use dead_decls::DeadDecls;
pub use dead_imports::DeadImports;
pub use exponent::Exponent;
pub use flip_comparisons::FlipComparisons;
pub use import_dedup::ImportDedup;
pub use obj_method_shorthand::ObjMethodShorthand;
pub use obj_shorthand::ObjShorthand;
pub use object_assign_spread::ObjectAssignSpread;
pub use remove_void::RemoveVoid;
pub use simplify_sequence::SimplifySequence;
pub use smart_inline::SmartInline;
pub use smart_rename::SmartRename;
pub use un_argument_spread::UnArgumentSpread;
pub use un_array_concat_spread::UnArrayConcatSpread;
pub use un_assert_this_initialized::UnAssertThisInitialized;
pub use un_assignment_merging::UnAssignmentMerging;
pub use un_async_await::UnAsyncAwait;
pub use un_bracket_notation::UnBracketNotation;
pub use un_builtin_prototype::UnBuiltinPrototype;
pub use un_class_call_check::UnClassCallCheck;
pub use un_class_fields::UnClassFields;
pub use un_conditionals::UnConditionals;
pub use un_curly_braces::UnCurlyBraces;
pub use un_define_property::UnDefineProperty;
pub use un_destructuring::UnDestructuring;
pub use un_double_negation::UnDoubleNegation;
pub use un_enum::UnEnum;
pub use un_es6_class::UnEs6Class;
pub use un_esm::UnEsm;
pub use un_esmodule_flag::UnEsmoduleFlag;
pub use un_export_rename::UnExportRename;
pub use un_for_of::UnForOf;
pub use un_iife::UnIife;
pub use un_import_rename::UnImportRename;
pub use un_indirect_call::UnIndirectCall;
pub use un_infinity::UnInfinity;
pub use un_interop_require_default::UnInteropRequireDefault;
pub use un_interop_require_wildcard::UnInteropRequireWildcard;
pub use un_jsx::UnJsx;
pub use un_nullish_coalescing::UnNullishCoalescing;
pub use un_numeric_literal::UnNumericLiteral;
pub use un_object_rest::UnObjectRest;
pub use un_object_spread::UnObjectSpread;
pub use un_optional_chaining::UnOptionalChaining;
pub use un_parameters::UnParameters;
pub use un_possible_constructor_return::UnPossibleConstructorReturn;
pub use un_prototype_class::UnPrototypeClass;
pub use un_rest_array_copy::UnRestArrayCopy;
pub use un_return::UnReturn;
pub use un_sliced_to_array::UnSlicedToArray;
pub use un_spread_array_literal::UnSpreadArrayLiteral;
pub use un_template_literal::UnTemplateLiteral;
pub use un_then_catch::UnThenCatch;
pub use un_to_consumable_array::UnToConsumableArray;
pub use un_ts_helpers::UnTsHelpers;
pub use un_type_constructor::UnTypeConstructor;
pub use un_typeof::UnTypeof;
pub use un_typeof_polyfill::UnTypeofPolyfill;
pub use un_typeof_strict::UnTypeofStrict;
pub use un_undefined_init::UnUndefinedInit;
pub use un_use_strict::UnUseStrict;
pub use un_variable_merging::UnVariableMerging;
pub use un_webpack_define_getters::UnWebpackDefineGetters;
pub use un_webpack_interop::UnWebpackInterop;
pub use un_webpack_object_getters::UnWebpackObjectGetters;
pub use un_while_loop::UnWhileLoop;
pub use unminify_booleans::UnminifyBooleans;
pub use var_decl_to_let_const::VarDeclToLetConst;

pub trait Rule: VisitMut {
    fn name(&self) -> &'static str;
}

#[derive(Default)]
pub struct NoopRule;

impl VisitMut for NoopRule {}

impl Rule for NoopRule {
    fn name(&self) -> &'static str {
        "noop"
    }
}

pub fn apply_default_rules(module: &mut Module, unresolved_mark: Mark) {
    apply_default_rules_with_level(module, unresolved_mark, true, RewriteLevel::Standard);
}

pub fn apply_default_rules_with_options(
    module: &mut Module,
    unresolved_mark: Mark,
    dead_code_elimination: bool,
) {
    apply_default_rules_with_level(
        module,
        unresolved_mark,
        dead_code_elimination,
        RewriteLevel::Standard,
    );
}

pub fn apply_default_rules_with_level(
    module: &mut Module,
    unresolved_mark: Mark,
    dead_code_elimination: bool,
    rewrite_level: RewriteLevel,
) {
    apply_rules_impl(
        module,
        unresolved_mark,
        None,
        None,
        dead_code_elimination,
        rewrite_level,
    );
}

/// Run the decompile pipeline, stopping immediately after `stop_after` completes.
/// Rule names match their struct names (e.g. "SmartInline", "UnEsm").
/// Second passes are suffixed: "UnWebpackInterop2", "UnIife2".
pub fn apply_rules_until(module: &mut Module, unresolved_mark: Mark, stop_after: &str) {
    apply_rules_until_with_level(
        module,
        unresolved_mark,
        stop_after,
        true,
        RewriteLevel::Standard,
    );
}

pub fn apply_rules_until_with_options(
    module: &mut Module,
    unresolved_mark: Mark,
    stop_after: &str,
    dead_code_elimination: bool,
) {
    apply_rules_until_with_level(
        module,
        unresolved_mark,
        stop_after,
        dead_code_elimination,
        RewriteLevel::Standard,
    );
}

pub fn apply_rules_until_with_level(
    module: &mut Module,
    unresolved_mark: Mark,
    stop_after: &str,
    dead_code_elimination: bool,
    rewrite_level: RewriteLevel,
) {
    apply_rules_impl(
        module,
        unresolved_mark,
        Some(stop_after),
        None,
        dead_code_elimination,
        rewrite_level,
    );
}

/// Returns the ordered list of rule names in the pipeline.
pub fn rule_names() -> &'static [&'static str] {
    &[
        "SimplifySequence",
        "FlipComparisons",
        "UnTypeofStrict",
        "RemoveVoid",
        "UnminifyBooleans",
        "UnDoubleNegation",
        "UnInfinity",
        "UnIndirectCall",
        "UnTypeof",
        "UnNumericLiteral",
        "UnBracketNotation",
        "UnInteropRequireDefault",
        "UnInteropRequireWildcard",
        "UnToConsumableArray",
        "UnObjectSpread",
        "UnObjectRest",
        "UnSlicedToArray",
        "UnDefineProperty",
        "UnClassCallCheck",
        "UnPossibleConstructorReturn",
        "UnAssertThisInitialized",
        "UnTypeofPolyfill",
        "UnCurlyBraces",
        "UnEsmoduleFlag",
        "UnUseStrict",
        "UnAssignmentMerging",
        "UnWebpackInterop",
        "UnEsm",
        "UnTemplateLiteral",
        "UnWhileLoop",
        "UnTypeConstructor",
        "UnBuiltinPrototype",
        "UnArgumentSpread",
        "UnArrayConcatSpread",
        "UnSpreadArrayLiteral",
        "ObjectAssignSpread",
        "UnVariableMerging",
        "UnNullishCoalescing",
        "UnOptionalChaining",
        "UnIife",
        "UnConditionals",
        "UnParameters",
        "UnEnum",
        "UnJsx",
        "UnEs6Class",
        "UnClassFields",
        "UnTsHelpers",
        "UnAsyncAwait",
        "UnWebpackInterop2",
        "UnThenCatch",
        "UnUndefinedInit",
        "VarDeclToLetConst",
        "ObjShorthand",
        "ObjMethodShorthand",
        "UnPrototypeClass",
        "Exponent",
        "ArgRest",
        "UnRestArrayCopy",
        "ArrowFunction",
        "ArrowReturn",
        "UnForOf",
        "UnWebpackDefineGetters",
        "UnWebpackObjectGetters",
        "UnImportRename",
        "UnExportRename",
        "UnDestructuring",
        "SmartInline",
        "UnIife2",
        "SmartRename",
        "DeadImports",
        "DeadDecls",
        "UnReturn",
    ]
}

/// Run only the rules from `start_from` through `stop_after` (inclusive on both ends).
/// Useful for testing a rule's behavior given realistic intermediate pipeline state.
pub fn apply_rules_between(
    module: &mut Module,
    unresolved_mark: Mark,
    start_from: &str,
    stop_after: &str,
) {
    apply_rules_between_with_options(module, unresolved_mark, start_from, stop_after, true);
}

pub fn apply_rules_between_with_options(
    module: &mut Module,
    unresolved_mark: Mark,
    start_from: &str,
    stop_after: &str,
    dead_code_elimination: bool,
) {
    apply_rules_between_with_level(
        module,
        unresolved_mark,
        start_from,
        stop_after,
        dead_code_elimination,
        RewriteLevel::Standard,
    );
}

pub fn apply_rules_between_with_level(
    module: &mut Module,
    unresolved_mark: Mark,
    start_from: &str,
    stop_after: &str,
    dead_code_elimination: bool,
    rewrite_level: RewriteLevel,
) {
    apply_rules_range_impl(
        module,
        unresolved_mark,
        Some(start_from),
        Some(stop_after),
        None,
        dead_code_elimination,
        rewrite_level,
    );
}

pub(crate) fn apply_rules_range_with_observer_with_level(
    module: &mut Module,
    unresolved_mark: Mark,
    start_from: Option<&str>,
    stop_after: Option<&str>,
    observer: &mut dyn FnMut(&'static str, &Module),
    dead_code_elimination: bool,
    rewrite_level: RewriteLevel,
) {
    apply_rules_range_impl(
        module,
        unresolved_mark,
        start_from,
        stop_after,
        Some(observer),
        dead_code_elimination,
        rewrite_level,
    );
}

fn apply_rules_impl(
    module: &mut Module,
    unresolved_mark: Mark,
    stop_after: Option<&str>,
    observer: Option<&mut dyn FnMut(&'static str, &Module)>,
    dead_code_elimination: bool,
    rewrite_level: RewriteLevel,
) {
    apply_rules_range_impl(
        module,
        unresolved_mark,
        None,
        stop_after,
        observer,
        dead_code_elimination,
        rewrite_level,
    );
}

fn apply_rules_range_impl(
    module: &mut Module,
    unresolved_mark: Mark,
    start_from: Option<&str>,
    stop_after: Option<&str>,
    mut observer: Option<&mut dyn FnMut(&'static str, &Module)>,
    dead_code_elimination: bool,
    rewrite_level: RewriteLevel,
) {
    let mut started = start_from.is_none();
    macro_rules! run {
        ($rule:expr, $name:expr) => {
            if !started {
                if start_from == Some($name) {
                    started = true;
                }
            }
            if started {
                module.visit_mut_with(&mut $rule);
                if let Some(observer) = observer.as_deref_mut() {
                    observer($name, module);
                }
                if stop_after == Some($name) {
                    return;
                }
            }
        };
    }

    // Stage 1: Syntax normalization
    run!(SimplifySequence::new(unresolved_mark), "SimplifySequence");
    run!(FlipComparisons::new(unresolved_mark), "FlipComparisons");
    run!(UnTypeofStrict, "UnTypeofStrict");
    if !started && start_from == Some("RemoveVoid") {
        started = true;
    }
    if started && RemoveVoid::should_run(module) {
        module.visit_mut_with(&mut RemoveVoid);
        if let Some(observer) = observer.as_deref_mut() {
            observer("RemoveVoid", module);
        }
    }
    if started && stop_after == Some("RemoveVoid") {
        return;
    }
    run!(UnminifyBooleans, "UnminifyBooleans");
    run!(UnDoubleNegation, "UnDoubleNegation");
    run!(UnInfinity, "UnInfinity");
    run!(UnIndirectCall, "UnIndirectCall");
    run!(UnTypeof, "UnTypeof");
    run!(UnNumericLiteral, "UnNumericLiteral");
    run!(UnBracketNotation, "UnBracketNotation");

    // Stage 2: Transpiler helper unwrapping + module-system reconstruction.
    // Needs UnIndirectCall + UnBracketNotation first (normalizes (0,x.default)() and ["default"]).
    run!(UnInteropRequireDefault, "UnInteropRequireDefault");
    run!(UnInteropRequireWildcard, "UnInteropRequireWildcard");
    run!(UnToConsumableArray, "UnToConsumableArray");
    run!(UnObjectSpread, "UnObjectSpread");
    run!(UnObjectRest::new(unresolved_mark), "UnObjectRest");
    run!(UnSlicedToArray, "UnSlicedToArray");
    run!(UnDefineProperty, "UnDefineProperty");
    run!(UnClassCallCheck, "UnClassCallCheck");
    run!(UnPossibleConstructorReturn, "UnPossibleConstructorReturn");
    run!(UnAssertThisInitialized, "UnAssertThisInitialized");
    run!(UnTypeofPolyfill, "UnTypeofPolyfill");
    // UnEsm prerequisites: add braces (enables assignment splitting), remove __esModule
    // flag, strip "use strict", split chained assignments, resolve webpack interop
    // getters — confirmed experimentally (see rule-dependency-inventory.md).
    run!(UnCurlyBraces, "UnCurlyBraces");
    run!(UnEsmoduleFlag, "UnEsmoduleFlag");
    run!(UnUseStrict, "UnUseStrict");
    run!(UnAssignmentMerging, "UnAssignmentMerging");
    run!(UnWebpackInterop, "UnWebpackInterop");
    run!(UnEsm::new(unresolved_mark, rewrite_level), "UnEsm");

    // Stage 3: Structural restoration
    run!(UnTemplateLiteral, "UnTemplateLiteral");
    run!(UnWhileLoop, "UnWhileLoop");
    run!(UnTypeConstructor::new(rewrite_level), "UnTypeConstructor");
    run!(UnBuiltinPrototype, "UnBuiltinPrototype");
    run!(UnArgumentSpread::new(rewrite_level), "UnArgumentSpread");
    run!(UnArrayConcatSpread, "UnArrayConcatSpread");
    run!(UnSpreadArrayLiteral, "UnSpreadArrayLiteral");
    run!(
        ObjectAssignSpread::new(unresolved_mark),
        "ObjectAssignSpread"
    );
    run!(UnVariableMerging, "UnVariableMerging");
    run!(UnNullishCoalescing, "UnNullishCoalescing");
    run!(UnOptionalChaining::new(rewrite_level), "UnOptionalChaining");

    // Stage 4: Complex pattern restoration
    run!(UnIife::new(rewrite_level), "UnIife");
    run!(UnConditionals, "UnConditionals");
    run!(
        UnParameters::new(unresolved_mark, rewrite_level),
        "UnParameters"
    );
    run!(UnEnum, "UnEnum");
    run!(
        UnJsx::new_with_level(unresolved_mark, rewrite_level),
        "UnJsx"
    );
    run!(UnEs6Class, "UnEs6Class");
    run!(UnClassFields, "UnClassFields");
    run!(UnTsHelpers, "UnTsHelpers");
    run!(UnAsyncAwait, "UnAsyncAwait");
    // Second pass: catches interop getters exposed by UnAsyncAwait.
    run!(UnWebpackInterop, "UnWebpackInterop2");

    // Stage 5: Modernization
    run!(UnThenCatch, "UnThenCatch");
    run!(UnUndefinedInit, "UnUndefinedInit");
    run!(VarDeclToLetConst, "VarDeclToLetConst");
    run!(ObjShorthand, "ObjShorthand");
    run!(ObjMethodShorthand, "ObjMethodShorthand");
    run!(UnPrototypeClass, "UnPrototypeClass");
    run!(Exponent, "Exponent");
    run!(ArgRest::new(rewrite_level), "ArgRest");
    run!(UnRestArrayCopy, "UnRestArrayCopy");
    run!(ArrowFunction, "ArrowFunction");
    run!(ArrowReturn, "ArrowReturn");
    run!(UnForOf::new(rewrite_level), "UnForOf");

    // Stage 7: Cleanup and renaming
    run!(
        UnWebpackDefineGetters::new(unresolved_mark),
        "UnWebpackDefineGetters"
    );
    run!(UnWebpackObjectGetters, "UnWebpackObjectGetters");
    run!(UnImportRename, "UnImportRename");
    run!(UnExportRename, "UnExportRename");
    run!(UnDestructuring, "UnDestructuring");
    // UnDestructuring can expose `param === undefined ? {} : param` initializers.
    run!(
        UnParameters::new(unresolved_mark, rewrite_level),
        "UnParameters2"
    );
    run!(SmartInline::new(rewrite_level), "SmartInline");
    // Second UnIife pass: simplify any (() => expr)() patterns created by SmartInline inlining
    run!(UnIife::new(rewrite_level), "UnIife2");
    run!(SmartRename, "SmartRename");
    // Optional final DCE pass. Tests that focus on structural restoration can
    // disable this to avoid coupling fixture baselines to late cleanup.
    if dead_code_elimination {
        // DeadImports runs after all rewrites that might remove usages (JSX,
        // SmartInline, SmartRename). Strips unreferenced import specifiers;
        // keeps the import as a side-effect-only declaration when all
        // specifiers go.
        run!(DeadImports, "DeadImports");
        run!(DeadDecls, "DeadDecls");
    }
    // UnReturn runs last: no downstream rule needs tail `return undefined`, and earlier
    // rules (UnConditionals, SmartInline, etc.) can introduce new ones during restructuring.
    run!(UnReturn, "UnReturn");
}
