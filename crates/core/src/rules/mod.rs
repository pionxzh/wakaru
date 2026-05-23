mod arg_rest;
mod arrow_function;
mod arrow_return;
pub(crate) mod babel_helper_utils;
pub(crate) mod binding_facts;
mod dead_decls;
mod dead_imports;
pub(crate) mod decl_utils;
mod exponent;
pub(crate) mod expr_utils;
mod flip_comparisons;
mod import_dedup;
pub(crate) mod match_context;
mod obj_method_shorthand;
mod obj_shorthand;
mod object_assign_spread;
mod pipeline;
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
mod un_regenerator;
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

use swc_core::ecma::visit::VisitMut;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum RewriteLevel {
    Minimal,
    #[default]
    Standard,
    Aggressive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RewriteAssumptions {
    pub no_document_all: bool,
    pub pure_getters: bool,
}

impl RewriteAssumptions {
    pub fn from_level(level: RewriteLevel) -> Self {
        match level {
            RewriteLevel::Minimal => Self {
                no_document_all: false,
                pure_getters: false,
            },
            RewriteLevel::Standard => Self {
                no_document_all: true,
                pure_getters: false,
            },
            RewriteLevel::Aggressive => Self {
                no_document_all: true,
                pure_getters: true,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RewritePolicy {
    pub level: RewriteLevel,
    pub assumptions: RewriteAssumptions,
}

impl RewritePolicy {
    pub fn from_level(level: RewriteLevel) -> Self {
        Self {
            level,
            assumptions: RewriteAssumptions::from_level(level),
        }
    }
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
pub(crate) use pipeline::{apply_default_rules, apply_rules_with_observer};
pub use pipeline::{
    apply_rules, rule_descriptors, rule_names, RuleDescriptor, RulePipelineOptions, RuleStage,
};
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
pub use un_regenerator::UnRegenerator;
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
