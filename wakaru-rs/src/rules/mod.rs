mod babel_helper_utils;
mod import_dedup;
mod arg_rest;
mod un_array_concat_spread;
mod arrow_function;
mod arrow_return;
mod exponent;
mod flip_comparisons;
mod obj_method_shorthand;
mod obj_shorthand;
mod object_assign_spread;
pub(crate) mod rename_utils;
mod remove_void;
mod simplify_sequence;
mod smart_inline;
mod smart_rename;
mod un_argument_spread;
mod un_assignment_merging;
mod un_async_await;
mod un_bracket_notation;
mod un_class_call_check;
mod un_builtin_prototype;
mod un_conditionals;
mod un_curly_braces;
mod un_enum;
mod un_es6_class;
mod un_esm;
mod un_esmodule_flag;
mod un_export_rename;
mod un_iife;
mod un_import_rename;
mod un_indirect_call;
mod un_interop_require_default;
mod un_interop_require_wildcard;
mod un_infinity;
mod un_object_spread;
mod un_sliced_to_array;
mod un_to_consumable_array;
mod un_jsx;
mod un_nullish_coalescing;
mod un_numeric_literal;
mod un_optional_chaining;
mod un_possible_constructor_return;
mod un_parameters;
mod un_rest_array_copy;
mod un_spread_array_literal;
mod un_return;
mod un_template_literal;
mod un_type_constructor;
mod un_typeof;
mod un_typeof_polyfill;
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

pub use arg_rest::ArgRest;
pub use un_array_concat_spread::UnArrayConcatSpread;
pub use import_dedup::ImportDedup;
pub use arrow_function::ArrowFunction;
pub use arrow_return::ArrowReturn;
pub use exponent::Exponent;
pub use flip_comparisons::FlipComparisons;
pub use obj_method_shorthand::ObjMethodShorthand;
pub use obj_shorthand::ObjShorthand;
pub use object_assign_spread::ObjectAssignSpread;
pub use remove_void::RemoveVoid;
pub use simplify_sequence::SimplifySequence;
pub use smart_inline::SmartInline;
pub use smart_rename::SmartRename;
pub use un_argument_spread::UnArgumentSpread;
pub use un_assignment_merging::UnAssignmentMerging;
pub use un_async_await::UnAsyncAwait;
pub use un_bracket_notation::UnBracketNotation;
pub use un_class_call_check::UnClassCallCheck;
pub use un_builtin_prototype::UnBuiltinPrototype;
pub use un_conditionals::UnConditionals;
pub use un_curly_braces::UnCurlyBraces;
pub use un_enum::UnEnum;
pub use un_es6_class::UnEs6Class;
pub use un_esm::UnEsm;
pub use un_esmodule_flag::UnEsmoduleFlag;
pub use un_export_rename::UnExportRename;
pub use un_iife::UnIife;
pub use un_import_rename::UnImportRename;
pub use un_indirect_call::UnIndirectCall;
pub use un_interop_require_default::UnInteropRequireDefault;
pub use un_interop_require_wildcard::UnInteropRequireWildcard;
pub use un_object_spread::UnObjectSpread;
pub use un_sliced_to_array::UnSlicedToArray;
pub use un_to_consumable_array::UnToConsumableArray;
pub use un_infinity::UnInfinity;
pub use un_jsx::UnJsx;
pub use un_nullish_coalescing::UnNullishCoalescing;
pub use un_numeric_literal::UnNumericLiteral;
pub use un_optional_chaining::UnOptionalChaining;
pub use un_possible_constructor_return::UnPossibleConstructorReturn;
pub use un_parameters::UnParameters;
pub use un_rest_array_copy::UnRestArrayCopy;
pub use un_spread_array_literal::UnSpreadArrayLiteral;
pub use un_return::UnReturn;
pub use un_template_literal::UnTemplateLiteral;
pub use un_type_constructor::UnTypeConstructor;
pub use un_typeof::UnTypeof;
pub use un_typeof_polyfill::UnTypeofPolyfill;
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
    module.visit_mut_with(&mut SimplifySequence::new(unresolved_mark));
    module.visit_mut_with(&mut FlipComparisons);
    if RemoveVoid::should_run(module) {
        module.visit_mut_with(&mut RemoveVoid);
    }
    module.visit_mut_with(&mut UnminifyBooleans);
    module.visit_mut_with(&mut UnInfinity);
    module.visit_mut_with(&mut UnIndirectCall);
    module.visit_mut_with(&mut UnTypeof);
    module.visit_mut_with(&mut UnNumericLiteral);
    module.visit_mut_with(&mut UnBracketNotation);
    // Babel/transpiler helper unwrapping — run early so downstream rules see clean code.
    // Needs UnIndirectCall + UnBracketNotation first (normalizes (0,x.default)() and ["default"]).
    module.visit_mut_with(&mut UnInteropRequireDefault);
    module.visit_mut_with(&mut UnInteropRequireWildcard);
    module.visit_mut_with(&mut UnToConsumableArray);
    module.visit_mut_with(&mut UnObjectSpread);
    module.visit_mut_with(&mut UnSlicedToArray);
    module.visit_mut_with(&mut UnClassCallCheck);
    module.visit_mut_with(&mut UnPossibleConstructorReturn);
    module.visit_mut_with(&mut UnTypeofPolyfill);
    module.visit_mut_with(&mut UnTemplateLiteral);
    module.visit_mut_with(&mut UnUseStrict);
    module.visit_mut_with(&mut UnWhileLoop);
    module.visit_mut_with(&mut UnCurlyBraces);
    module.visit_mut_with(&mut UnTypeConstructor);
    module.visit_mut_with(&mut UnEsmoduleFlag);
    module.visit_mut_with(&mut UnAssignmentMerging);
    module.visit_mut_with(&mut UnBuiltinPrototype);
    module.visit_mut_with(&mut UnArgumentSpread);
    module.visit_mut_with(&mut UnArrayConcatSpread);
    module.visit_mut_with(&mut UnSpreadArrayLiteral);
    module.visit_mut_with(&mut ObjectAssignSpread::new(unresolved_mark));
    module.visit_mut_with(&mut UnVariableMerging);
    module.visit_mut_with(&mut UnNullishCoalescing);
    module.visit_mut_with(&mut UnOptionalChaining);
    module.visit_mut_with(&mut UnWebpackInterop);
    module.visit_mut_with(&mut UnIife);
    module.visit_mut_with(&mut UnConditionals);
    module.visit_mut_with(&mut UnParameters);
    module.visit_mut_with(&mut UnEnum);
    module.visit_mut_with(&mut UnJsx::new(unresolved_mark));
    module.visit_mut_with(&mut UnEs6Class);
    module.visit_mut_with(&mut UnAsyncAwait);
    module.visit_mut_with(&mut UnWebpackInterop);
    module.visit_mut_with(&mut UnEsm);
    // lebab-style modernization
    module.visit_mut_with(&mut VarDeclToLetConst);
    module.visit_mut_with(&mut ObjShorthand);
    module.visit_mut_with(&mut ObjMethodShorthand);
    module.visit_mut_with(&mut Exponent);
    module.visit_mut_with(&mut ArgRest);
    module.visit_mut_with(&mut UnRestArrayCopy);
    module.visit_mut_with(&mut ArrowFunction);
    module.visit_mut_with(&mut ArrowReturn);
    module.visit_mut_with(&mut UnWebpackDefineGetters::new(unresolved_mark));
    module.visit_mut_with(&mut UnWebpackObjectGetters);
    module.visit_mut_with(&mut UnImportRename);
    module.visit_mut_with(&mut UnExportRename);
    module.visit_mut_with(&mut SmartInline);
    // Second UnIife pass: simplify any (() => expr)() patterns created by SmartInline inlining
    module.visit_mut_with(&mut UnIife);
    module.visit_mut_with(&mut SmartRename);
    // UnReturn runs last: no downstream rule needs tail `return undefined`, and earlier
    // rules (UnConditionals, SmartInline, etc.) can introduce new ones during restructuring.
    module.visit_mut_with(&mut UnReturn);
}
