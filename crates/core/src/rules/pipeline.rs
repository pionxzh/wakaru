use std::cell::RefCell;
use std::rc::Rc;

use swc_core::common::Mark;
use swc_core::ecma::ast::Module;
use swc_core::ecma::visit::VisitMutWith;

use crate::facts::ModuleFactsMap;
use crate::DceMode;

use super::dead_decls::compute_pre_dead_decl_spans;
use super::dead_imports::{compute_pre_dead_import_spans, compute_pre_existing_import_spans};
use super::transpiler_helper_utils::{LocalHelperContext, TranspilerHelperKind};
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleStage {
    Syntax,
    Helpers,
    Structural,
    Complex,
    Modernization,
    Cleanup,
}

type RuleRunner = for<'a> fn(&mut Module, RuleRunContext<'a>);
type RuleEnabled = for<'a> fn(RuleRunContext<'a>) -> bool;

#[derive(Clone, Copy)]
pub struct RuleDescriptor {
    pub id: &'static str,
    pub stage: RuleStage,
    pub requires: &'static [&'static str],
    run: RuleRunner,
    enabled: RuleEnabled,
}

impl RuleDescriptor {
    const fn gated(
        id: &'static str,
        stage: RuleStage,
        run: RuleRunner,
        enabled: RuleEnabled,
        requires: &'static [&'static str],
    ) -> Self {
        Self {
            id,
            stage,
            requires,
            run,
            enabled,
        }
    }

    fn is_enabled(self, ctx: RuleRunContext<'_>) -> bool {
        (self.enabled)(ctx)
    }

    fn run(self, module: &mut Module, ctx: RuleRunContext<'_>) {
        (self.run)(module, ctx);
    }
}

#[derive(Clone)]
struct RuleRunContext<'a> {
    unresolved_mark: Mark,
    rewrite_level: RewriteLevel,
    dce_mode: DceMode,
    source_import_reads_are_observable: bool,
    module_facts: Option<&'a ModuleFactsMap>,
    current_filename: Option<&'a str>,
    local_helpers: Rc<RefCell<Option<Rc<LocalHelperContext>>>>,
    extracted_function_names: SharedExtractedFunctionNames,
    pre_dead: Option<Rc<PreDeadSet>>,
}

pub(super) struct PreDeadSet {
    pub decl_spans:
        std::collections::HashSet<(swc_core::common::BytePos, swc_core::common::BytePos)>,
    pub preserved_import_spans:
        std::collections::HashSet<(swc_core::common::BytePos, swc_core::common::BytePos)>,
}

impl RuleRunContext<'_> {
    fn local_helpers(&self, module: &Module) -> Rc<LocalHelperContext> {
        if let Some(local_helpers) = self.local_helpers.borrow().as_ref() {
            return Rc::clone(local_helpers);
        }

        let local_helpers = Rc::new(LocalHelperContext::collect_with_mark(
            module,
            self.unresolved_mark,
        ));
        *self.local_helpers.borrow_mut() = Some(Rc::clone(&local_helpers));
        local_helpers
    }

    fn invalidate_local_helpers(&self) {
        *self.local_helpers.borrow_mut() = None;
    }
}

fn always_enabled(_: RuleRunContext<'_>) -> bool {
    true
}

fn dead_code_elimination_enabled(ctx: RuleRunContext<'_>) -> bool {
    ctx.dce_mode.is_enabled()
}

fn standard_or_above(ctx: RuleRunContext<'_>) -> bool {
    ctx.rewrite_level >= RewriteLevel::Standard
}

macro_rules! runner {
    ($name:ident, |$ctx:ident| $rule:expr) => {
        fn $name(module: &mut Module, $ctx: RuleRunContext<'_>) {
            module.visit_mut_with(&mut $rule);
        }
    };
    ($name:ident, $rule:expr) => {
        fn $name(module: &mut Module, _: RuleRunContext<'_>) {
            module.visit_mut_with(&mut $rule);
        }
    };
}

macro_rules! define_rule_registry {
    (@requires) => {
        &[]
    };
    (@requires $($requires:literal),+ $(,)?) => {
        &[$($requires),+]
    };
    ($(
        ($id:literal, $stage:ident, $runner:ident, $enabled:ident $(, requires: [$($requires:literal),* $(,)?])?)
    ),+ $(,)?) => {
        pub static RULE_DESCRIPTORS: &[RuleDescriptor] = &[
            $(
                RuleDescriptor::gated(
                    $id,
                    RuleStage::$stage,
                    $runner,
                    $enabled,
                    define_rule_registry!(@requires $($($requires),*)?),
                ),
            )+
        ];

        static RULE_NAMES: &[&str] = &[
            $(
                $id,
            )+
        ];
    };
}

runner!(run_simplify_sequence, |ctx| {
    SimplifySequence::new_with_import_semantics(
        ctx.unresolved_mark,
        ctx.rewrite_level,
        ctx.source_import_reads_are_observable,
    )
});
runner!(run_flip_comparisons, |ctx| FlipComparisons::new(
    ctx.unresolved_mark
));
runner!(run_un_typeof_strict, UnTypeofStrict);

fn run_remove_void(module: &mut Module, ctx: RuleRunContext<'_>) {
    if RemoveVoid::should_run(module) {
        module.visit_mut_with(&mut RemoveVoid::new(ctx.unresolved_mark));
    }
}

runner!(run_unminify_booleans, UnminifyBooleans);
runner!(run_un_double_negation, UnDoubleNegation);
runner!(run_un_infinity, UnInfinity);
runner!(run_un_indirect_call, |ctx| UnIndirectCall::new(
    ctx.rewrite_level
));
runner!(run_un_typeof, UnTypeof);
runner!(run_un_numeric_literal, UnNumericLiteral);
runner!(run_un_bracket_notation, UnBracketNotation);
fn run_un_interop_require_default(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    UnInteropRequireDefault::run_with_helpers(module, local_helpers.as_ref());
    // Unwrapping `_interopRequireDefault(require("@babel/runtime/helpers/..."))` can
    // expose new runtime-path helpers (e.g. interopRequireWildcard) that were hidden
    // behind the default wrapper. Rebuild the cache so the next rule sees them.
    ctx.invalidate_local_helpers();
}

fn run_un_interop_require_wildcard(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    UnInteropRequireWildcard::run_with_helpers(module, local_helpers.as_ref());
}

fn run_un_to_consumable_array(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    UnToConsumableArray::run_with_helpers(
        module,
        ctx.unresolved_mark,
        local_helpers.as_ref(),
        ctx.module_facts,
        ctx.current_filename,
    );
}

fn run_un_object_spread(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    UnObjectSpread::run_with_helpers(
        module,
        ctx.unresolved_mark,
        local_helpers.as_ref(),
        ctx.module_facts,
        ctx.current_filename,
    );
}

fn run_un_object_spread_late(module: &mut Module, ctx: RuleRunContext<'_>) {
    run_un_object_spread(module, ctx.clone());
}

fn run_un_object_rest(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    UnObjectRest::run_with_helpers(
        module,
        ctx.unresolved_mark,
        local_helpers.as_ref(),
        ctx.module_facts,
        ctx.current_filename,
    );
}

fn run_un_object_rest_late(module: &mut Module, ctx: RuleRunContext<'_>) {
    run_un_object_rest(module, ctx.clone());
}

fn run_un_object_rest_after_async(module: &mut Module, ctx: RuleRunContext<'_>) {
    run_un_object_rest(module, ctx.clone());
    ctx.invalidate_local_helpers();
}

fn run_un_sliced_to_array(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    UnSlicedToArray::run_with_helpers(
        module,
        ctx.unresolved_mark,
        local_helpers.as_ref(),
        ctx.module_facts,
        ctx.current_filename,
        ctx.rewrite_level,
    );
}

fn run_un_define_property(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    UnDefineProperty::run_with_helpers(module, local_helpers.as_ref());
}
fn run_un_class_call_check(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    UnClassCallCheck::run_with_helpers(module, local_helpers.as_ref());
}

fn run_un_possible_constructor_return(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    UnPossibleConstructorReturn::run_with_helpers(module, local_helpers.as_ref());
}
fn run_un_assert_this_initialized(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    UnAssertThisInitialized::run_with_helpers(module, local_helpers.as_ref());
}
fn run_un_typeof_polyfill(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    UnTypeofPolyfill::run_with_helpers(module, local_helpers.as_ref());
}
runner!(run_un_curly_braces, UnCurlyBraces);
runner!(run_un_esmodule_flag, |ctx| UnEsmoduleFlag::new(
    ctx.unresolved_mark
));
runner!(run_un_use_strict, UnUseStrict);
runner!(run_un_assignment_merging, UnAssignmentMerging);
runner!(run_un_webpack_interop, |ctx| UnWebpackInterop::new(
    ctx.unresolved_mark
));
runner!(run_un_esm, |ctx| UnEsm::new(
    ctx.unresolved_mark,
    ctx.rewrite_level
));
fn run_un_template_literal(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    let mut rule = if let Some(module_facts) = ctx.module_facts {
        UnTemplateLiteral::new_with_facts(ctx.rewrite_level, module_facts)
    } else {
        UnTemplateLiteral::new_with_level(ctx.rewrite_level)
    };
    rule.set_current_filename(ctx.current_filename);
    rule.run_with_helpers(module, local_helpers.as_ref());
}
runner!(run_un_while_loop, UnWhileLoop);
runner!(run_un_type_constructor, |ctx| UnTypeConstructor::new(
    ctx.rewrite_level
));
runner!(run_un_builtin_prototype, UnBuiltinPrototype);
runner!(run_un_argument_spread, |ctx| UnArgumentSpread::new(
    ctx.unresolved_mark,
    ctx.rewrite_level
));
runner!(run_un_array_concat_spread, |ctx| {
    UnArrayConcatSpread::new_with_level(ctx.rewrite_level)
});
runner!(run_un_spread_array_literal, UnSpreadArrayLiteral);
runner!(run_object_assign_spread, |ctx| ObjectAssignSpread::new(
    ctx.unresolved_mark
));
runner!(
    run_un_variable_merging_decls_only,
    UnVariableMergingDeclsOnly
);
fn run_un_builtin_aliases(module: &mut Module, ctx: RuleRunContext<'_>) {
    let mut rule = UnBuiltinAliases::new(ctx.unresolved_mark);
    if rule.run(module) {
        ctx.invalidate_local_helpers();
    }
}
runner!(run_un_variable_merging, UnVariableMerging);
runner!(run_un_nullish_coalescing, |ctx| UnNullishCoalescing::new(
    ctx.unresolved_mark,
    ctx.rewrite_level
));
runner!(run_un_optional_chaining, |ctx| UnOptionalChaining::new(
    ctx.unresolved_mark,
    ctx.rewrite_level
));
runner!(run_un_iife, |ctx| UnIife::new(ctx.rewrite_level));
runner!(run_extract_inlined_function, |ctx| {
    ExtractInlinedFunction::new_with_extracted_function_names(
        ctx.rewrite_level,
        Rc::clone(&ctx.extracted_function_names),
    )
});
fn run_un_conditionals(module: &mut Module, ctx: RuleRunContext<'_>) {
    module.visit_mut_with(&mut UnConditionals);
    // UnConditionals rewrites ternary helper bodies (e.g. _defineProperty with
    // _toPropertyKey) into if/else form. This changes helper shapes, so rebuild
    // the cache so UnClassFields and UnDefineProperty see the expanded bodies.
    ctx.invalidate_local_helpers();
}
runner!(run_un_parameters, |ctx| UnParameters::new(
    ctx.unresolved_mark,
    ctx.rewrite_level
));
runner!(run_un_enum, UnEnum);
runner!(run_un_jsx, |ctx| UnJsx::new_with_level(
    ctx.unresolved_mark,
    ctx.rewrite_level
));
fn run_un_es6_class(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    UnEs6Class::run_with_helpers(
        module,
        ctx.unresolved_mark,
        ctx.rewrite_level,
        local_helpers.as_ref(),
    );
}
fn run_un_class_fields(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    UnClassFields::new_with_mark(ctx.unresolved_mark, ctx.rewrite_level)
        .run_with_helpers(module, local_helpers.as_ref());
}
fn run_un_regenerator(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    UnRegenerator::run_with_helpers(
        module,
        ctx.unresolved_mark,
        ctx.module_facts,
        ctx.current_filename,
        local_helpers.as_ref(),
    );
}

fn run_un_async_await(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    UnAsyncAwait::run_with_helpers(
        module,
        ctx.unresolved_mark,
        local_helpers.as_ref(),
        ctx.module_facts,
        ctx.current_filename,
    );
}
runner!(run_un_then_catch, |ctx| UnThenCatch::new(
    ctx.unresolved_mark
));
runner!(run_un_undefined_init, |ctx| UnUndefinedInit::new(
    ctx.unresolved_mark
));
runner!(run_merge_declaration_init, |ctx| MergeDeclarationInit::new(
    ctx.rewrite_level
));
runner!(run_var_decl_to_let_const, |ctx| {
    VarDeclToLetConst::new_with_level(ctx.rewrite_level)
});
runner!(
    run_class_expression_to_declaration,
    ClassExpressionToDeclaration
);
runner!(run_obj_shorthand, ObjShorthand);
runner!(run_obj_method_shorthand, ObjMethodShorthand);
runner!(run_un_prototype_class, UnPrototypeClass);
runner!(run_exponent, Exponent);
runner!(run_arg_rest, |ctx| ArgRest::new(ctx.rewrite_level));
runner!(run_un_rest_array_copy, UnRestArrayCopy);
runner!(run_arrow_function, ArrowFunction);
runner!(run_arrow_return, ArrowReturn);
fn run_un_for_of(module: &mut Module, ctx: RuleRunContext<'_>) {
    if !UnForOf::should_run_with_level(ctx.rewrite_level, module) {
        return;
    }
    let local_helpers = ctx.local_helpers(module);
    let mut rule = if let Some(module_facts) = ctx.module_facts {
        UnForOf::new_with_mark_and_facts(ctx.unresolved_mark, ctx.rewrite_level, module_facts)
    } else {
        UnForOf::new_with_mark(ctx.unresolved_mark, ctx.rewrite_level)
    };
    rule.set_current_filename(ctx.current_filename);
    rule.run_with_helpers(module, local_helpers.as_ref());
}
runner!(run_un_webpack_define_getters, |ctx| {
    UnWebpackDefineGetters::new(ctx.unresolved_mark)
});
runner!(run_un_webpack_object_getters, |ctx| {
    UnWebpackObjectGetters::new(ctx.unresolved_mark)
});
runner!(run_import_dedup, ImportDedup);
runner!(run_un_import_rename, |ctx| UnImportRename::new(
    ctx.unresolved_mark
));
runner!(run_un_export_rename, UnExportRename);
fn run_un_destructuring(module: &mut Module, ctx: RuleRunContext<'_>) {
    let local_helpers = ctx.local_helpers(module);
    let mut rule = UnDestructuring::new_with_helpers(
        ctx.unresolved_mark,
        ctx.rewrite_level,
        local_helpers.as_ref(),
    );
    module.visit_mut_with(&mut rule);

    let consumed_helpers = rule.consumed_sliced_to_array_helpers();
    if consumed_helpers.is_empty() {
        return;
    }

    local_helpers.remove_helpers_with_dependencies(
        module,
        consumed_helpers
            .into_iter()
            .map(|key| (key, TranspilerHelperKind::SlicedToArray))
            .collect(),
    );
    ctx.invalidate_local_helpers();
}
runner!(run_un_to_array, |ctx| UnToArray::new_with_mark(
    ctx.unresolved_mark
));
runner!(run_smart_inline, |ctx| SmartInline::new_with_mark(
    ctx.rewrite_level,
    ctx.unresolved_mark
));
runner!(run_un_esbuild_cjs_wrapper, |ctx| {
    UnEsbuildCjsWrapper::new(ctx.unresolved_mark)
});
runner!(run_smart_rename, |ctx| SmartRename::new(
    ctx.unresolved_mark
));
runner!(run_smart_rename_second_pass, |ctx| {
    SmartRenameSecondPass::new_with_extracted_function_names(
        ctx.unresolved_mark,
        Rc::clone(&ctx.extracted_function_names),
    )
});
runner!(run_dead_uninitialized_decls, DeadUninitializedDecls);

fn run_dead_decls(module: &mut Module, ctx: RuleRunContext<'_>) {
    match ctx.dce_mode {
        DceMode::Full => module.visit_mut_with(&mut DeadDecls::full()),
        DceMode::TransformOnly => {
            if let Some(pre_dead) = ctx.pre_dead.as_ref() {
                module.visit_mut_with(&mut DeadDecls::delta(&pre_dead.decl_spans));
            }
        }
        DceMode::Off => {}
    }
}

fn run_dead_imports(module: &mut Module, ctx: RuleRunContext<'_>) {
    match ctx.dce_mode {
        DceMode::Full => module.visit_mut_with(&mut DeadImports::full()),
        DceMode::TransformOnly => {
            if let Some(pre_dead) = ctx.pre_dead.as_ref() {
                module.visit_mut_with(&mut DeadImports::delta(&pre_dead.preserved_import_spans));
            }
        }
        DceMode::Off => {}
    }
}
runner!(run_un_return, |ctx| UnReturn::new(ctx.unresolved_mark));

define_rule_registry! {
    ("SimplifySequence", Syntax, run_simplify_sequence, always_enabled),
    ("FlipComparisons", Syntax, run_flip_comparisons, always_enabled),
    ("UnTypeofStrict", Syntax, run_un_typeof_strict, always_enabled),
    ("RemoveVoid", Syntax, run_remove_void, always_enabled, requires: [
        "SimplifySequence"
    ]),
    ("UnminifyBooleans", Syntax, run_unminify_booleans, always_enabled),
    ("UnDoubleNegation", Syntax, run_un_double_negation, always_enabled),
    ("UnInfinity", Syntax, run_un_infinity, always_enabled),
    ("UnIndirectCall", Syntax, run_un_indirect_call, always_enabled),
    ("UnTypeof", Syntax, run_un_typeof, always_enabled),
    ("UnNumericLiteral", Syntax, run_un_numeric_literal, always_enabled),
    ("UnBracketNotation", Syntax, run_un_bracket_notation, always_enabled),
    ("UnInteropRequireDefault", Helpers, run_un_interop_require_default, always_enabled, requires: [
        "UnIndirectCall",
        "UnBracketNotation"
    ]),
    ("UnInteropRequireWildcard", Helpers, run_un_interop_require_wildcard, always_enabled, requires: [
        "UnIndirectCall",
        "UnBracketNotation"
    ]),
    ("UnToConsumableArray", Helpers, run_un_to_consumable_array, always_enabled),
    ("UnObjectSpread", Helpers, run_un_object_spread, always_enabled),
    ("UnObjectRest", Helpers, run_un_object_rest, always_enabled, requires: [
        "UnBracketNotation"
    ]),
    ("UnSlicedToArray", Helpers, run_un_sliced_to_array, always_enabled),
    ("UnClassCallCheck", Helpers, run_un_class_call_check, always_enabled),
    ("UnPossibleConstructorReturn", Helpers, run_un_possible_constructor_return, always_enabled),
    ("UnTypeofPolyfill", Helpers, run_un_typeof_polyfill, always_enabled),
    // UnEsm prerequisites: add braces to enable assignment splitting, remove
    // __esModule flags, strip "use strict", split chained assignments, and
    // resolve webpack interop getters. These dependencies are documented in
    // docs/rule-dependency-inventory.md.
    ("UnCurlyBraces", Helpers, run_un_curly_braces, always_enabled),
    ("SimplifySequence2", Helpers, run_simplify_sequence, always_enabled, requires: [
        "UnCurlyBraces"
    ]),
    ("UnEsmoduleFlag", Helpers, run_un_esmodule_flag, always_enabled),
    ("UnUseStrict", Helpers, run_un_use_strict, standard_or_above),
    ("UnAssignmentMerging", Helpers, run_un_assignment_merging, always_enabled, requires: [
        "UnCurlyBraces"
    ]),
    ("UnVariableMergingDeclsOnly", Helpers, run_un_variable_merging_decls_only, always_enabled, requires: [
        "UnAssignmentMerging"
    ]),
    ("UnBuiltinAliases", Helpers, run_un_builtin_aliases, standard_or_above, requires: [
        "UnVariableMergingDeclsOnly"
    ]),
    ("UnWebpackInterop", Helpers, run_un_webpack_interop, always_enabled, requires: [
        "UnBracketNotation",
        "UnEsmoduleFlag"
    ]),
    ("UnEsm", Helpers, run_un_esm, always_enabled, requires: [
        "UnCurlyBraces",
        "UnEsmoduleFlag",
        "UnUseStrict",
        "UnAssignmentMerging",
        "UnVariableMergingDeclsOnly",
        "UnWebpackInterop"
    ]),
    ("UnObjectSpread2", Helpers, run_un_object_spread_late, always_enabled, requires: [
        "UnEsm"
    ]),
    ("UnObjectRest2", Helpers, run_un_object_rest_late, always_enabled, requires: [
        "UnObjectSpread2"
    ]),
    ("UnSlicedToArray2", Helpers, run_un_sliced_to_array, always_enabled, requires: [
        "UnObjectRest2"
    ]),
    ("UnTemplateLiteral", Structural, run_un_template_literal, always_enabled),
    ("UnTypeConstructor", Structural, run_un_type_constructor, always_enabled),
    ("UnBuiltinPrototype", Structural, run_un_builtin_prototype, always_enabled),
    ("UnArgumentSpread", Structural, run_un_argument_spread, always_enabled),
    ("UnArrayConcatSpread", Structural, run_un_array_concat_spread, always_enabled),
    ("UnSpreadArrayLiteral", Structural, run_un_spread_array_literal, always_enabled),
    ("ObjectAssignSpread", Structural, run_object_assign_spread, always_enabled),
    ("UnVariableMerging", Structural, run_un_variable_merging, always_enabled),
    ("UnNullishCoalescing", Structural, run_un_nullish_coalescing, always_enabled),
    ("UnOptionalChaining", Structural, run_un_optional_chaining, always_enabled),
    ("UnIife", Complex, run_un_iife, always_enabled),
    ("UnConditionals", Complex, run_un_conditionals, always_enabled),
    ("UnOptionalChaining2", Complex, run_un_optional_chaining, always_enabled, requires: [
        "UnConditionals"
    ]),
    ("UnParameters", Complex, run_un_parameters, always_enabled, requires: [
        "FlipComparisons",
        "RemoveVoid"
    ]),
    // UnParameters can remove a for-loop initializer, exposing `for(; test;)`.
    ("UnWhileLoop", Complex, run_un_while_loop, always_enabled, requires: [
        "UnParameters"
    ]),
    ("UnEnum", Complex, run_un_enum, always_enabled),
    ("UnJsx", Complex, run_un_jsx, standard_or_above),
    ("UnEs6Class", Complex, run_un_es6_class, always_enabled),
    // UnEs6Class can expose nested _assertThisInitialized(this) calls after
    // constructor recovery.
    ("UnAssertThisInitialized", Complex, run_un_assert_this_initialized, always_enabled, requires: [
        "UnEs6Class"
    ]),
    ("UnClassFields", Complex, run_un_class_fields, always_enabled),
    // UnConditionals expands compact Babel _defineProperty helpers into the
    // if/else shape recognized by UnDefineProperty. Run after UnClassFields so
    // Babel class-field helper calls can still prove class field provenance.
    ("UnDefineProperty", Complex, run_un_define_property, always_enabled, requires: [
        "UnConditionals",
        "UnClassFields"
    ]),
    ("UnRegenerator", Complex, run_un_regenerator, always_enabled),
    ("UnAsyncAwait", Complex, run_un_async_await, always_enabled),
    // Async/regenerator recovery can expose assignment-form object rest.
    ("UnObjectRest3", Complex, run_un_object_rest_after_async, always_enabled, requires: [
        "UnAsyncAwait"
    ]),
    // Async recovery can expose memoized `.apply(...)` argument-spread shapes.
    ("UnArgumentSpread2", Complex, run_un_argument_spread, always_enabled, requires: [
        "UnAsyncAwait"
    ]),
    // Second pass: UnAsyncAwait can expose additional interop getter shapes.
    ("UnWebpackInterop2", Complex, run_un_webpack_interop, always_enabled, requires: [
        "UnObjectRest3"
    ]),
    ("UnThenCatch", Modernization, run_un_then_catch, always_enabled),
    ("UnUndefinedInit", Modernization, run_un_undefined_init, always_enabled),
    ("VarDeclToLetConst", Modernization, run_var_decl_to_let_const, always_enabled),
    ("ClassExpressionToDeclaration", Modernization, run_class_expression_to_declaration, always_enabled, requires: [
        "VarDeclToLetConst"
    ]),
    ("ObjShorthand", Modernization, run_obj_shorthand, always_enabled),
    ("ObjMethodShorthand", Modernization, run_obj_method_shorthand, always_enabled),
    ("UnPrototypeClass", Modernization, run_un_prototype_class, always_enabled),
    ("Exponent", Modernization, run_exponent, always_enabled),
    ("ArgRest", Modernization, run_arg_rest, always_enabled),
    ("UnRestArrayCopy", Modernization, run_un_rest_array_copy, always_enabled),
    ("ArrowFunction", Modernization, run_arrow_function, standard_or_above),
    ("ArrowReturn", Modernization, run_arrow_return, always_enabled),
    ("UnForOf", Modernization, run_un_for_of, always_enabled),
    ("UnWebpackDefineGetters", Cleanup, run_un_webpack_define_getters, always_enabled),
    ("UnWebpackObjectGetters", Cleanup, run_un_webpack_object_getters, always_enabled),
    ("ImportDedup", Cleanup, run_import_dedup, always_enabled),
    ("UnExportRename", Cleanup, run_un_export_rename, always_enabled),
    ("UnImportRename", Cleanup, run_un_import_rename, always_enabled, requires: [
        "UnExportRename"
    ]),
    // Third pass: UnEsm can convert require() bindings to imports, exposing
    // direct require.n(importBinding) helpers.
    ("UnWebpackInterop3", Cleanup, run_un_webpack_interop, always_enabled, requires: [
        "UnEsm"
    ]),
    ("UnDestructuring", Cleanup, run_un_destructuring, standard_or_above, requires: [
        "UnImportRename",
        "UnExportRename"
    ]),
    // Async recovery can expose nullish ternaries, but keep this after
    // UnDestructuring so temp/index reads remain available for pattern recovery.
    ("UnNullishCoalescing2", Cleanup, run_un_nullish_coalescing, always_enabled, requires: [
        "UnDestructuring"
    ]),
    // Strip the `toArray` helper around a recovered array-rest destructuring
    // source (`[a, ...b] = _toArray(x)` -> `[a, ...b] = x`). Runs after
    // UnDestructuring, which builds the `[a, ...b]` pattern this rule keys on.
    ("UnToArray", Cleanup, run_un_to_array, standard_or_above, requires: [
        "UnNullishCoalescing2"
    ]),
    // UnDestructuring can expose param === undefined ? {} : param initializers.
    ("UnParameters2", Cleanup, run_un_parameters, always_enabled, requires: [
        "UnDestructuring"
    ]),
    ("SmartInline", Cleanup, run_smart_inline, always_enabled, requires: [
        "UnDestructuring"
    ]),
    // Fold hoisted `let x; … x = e` (from async/regenerator lowering) back into
    // `let x = e`. Runs after UnDestructuring/SmartInline so it does not disturb
    // the assignment-form temps those rules rely on.
    ("MergeDeclarationInit", Cleanup, run_merge_declaration_init, standard_or_above, requires: [
        "SmartInline"
    ]),
    ("SmartRename", Cleanup, run_smart_rename, standard_or_above, requires: [
        "SmartInline"
    ]),
    // SmartRename can make destructured aliases readable enough to fold into
    // parameters, e.g. `{ theme: t } = opts` -> `{ theme } = opts`.
    ("UnParameters3", Cleanup, run_un_parameters, always_enabled, requires: [
        "SmartRename"
    ]),
    // UnParameters3 can remove every non-return statement from an arrow body.
    ("ArrowReturn2", Cleanup, run_arrow_return, always_enabled, requires: [
        "UnParameters3"
    ]),
    // SmartRename can free minified export target names that were occupied
    // when the first UnExportRename pass ran.
    ("UnExportRename2", Cleanup, run_un_export_rename, always_enabled, requires: [
        "SmartRename"
    ]),
    // SmartRename can free minified import alias names (e.g. P_2 → P_1,
    // Z_2 → Z) that were occupied when the first UnImportRename pass ran.
    ("UnImportRename2", Cleanup, run_un_import_rename, always_enabled, requires: [
        "SmartRename",
        "UnExportRename2"
    ]),
    // SmartRename can recover argument names that make IIFE params readable.
    ("UnIife2", Cleanup, run_un_iife, always_enabled, requires: [
        "SmartRename"
    ]),
    ("ExtractInlinedFunction", Cleanup, run_extract_inlined_function, always_enabled, requires: [
        "UnIife2"
    ]),
    // SmartRename may capitalize component bindings that UnJsx intentionally
    // skipped earlier because lowercase JSX tags are HTML elements.
    ("UnJsx2", Cleanup, run_un_jsx, standard_or_above, requires: [
        "SmartRename",
        "ExtractInlinedFunction"
    ]),
    // UnJsx2 can expose component aliases and value-position hints in JSX.
    // Only the JSX-aware sub-rules need to re-run; the non-JSX sub-rules
    // and recursive function/arrow descent were fully handled by SmartRename.
    ("SmartRename2", Cleanup, run_smart_rename_second_pass, always_enabled, requires: [
        "UnJsx2"
    ]),
    // Late structural rewrites can consume uninitialized temps used by lowered
    // optional chaining / nullish coalescing. Keep this after name-recovery
    // passes that can use empty declarations as hints.
    ("DeadUninitializedDecls", Cleanup, run_dead_uninitialized_decls, always_enabled, requires: [
        "SmartRename2"
    ]),
    ("UnEsbuildCjsWrapper", Cleanup, run_un_esbuild_cjs_wrapper, standard_or_above, requires: [
        "DeadUninitializedDecls"
    ]),
    // DeadDecls first: removing dead helpers can leave import specifiers
    // unreferenced, which DeadImports then cleans up.
    ("DeadDecls", Cleanup, run_dead_decls, dead_code_elimination_enabled),
    ("DeadImports", Cleanup, run_dead_imports, dead_code_elimination_enabled, requires: [
        "DeadDecls"
    ]),
    // Last pass: no downstream rule needs tail return undefined, and earlier
    // restructuring rules can introduce new ones.
    ("UnReturn", Cleanup, run_un_return, always_enabled),
    // Late rules (SmartInline, ArrowFunction, UnReturn) can create or expose
    // conditional patterns (return ternaries, short-circuit expression
    // statements) that the first UnConditionals pass could not see.
    ("UnConditionals2", Cleanup, run_un_conditionals, always_enabled, requires: [
        "UnReturn"
    ]),
}

#[derive(Debug, Clone, Copy)]
pub struct RulePipelineOptions<'a> {
    pub start_from: Option<&'a str>,
    pub stop_after: Option<&'a str>,
    pub dce_mode: DceMode,
    pub rewrite_level: RewriteLevel,
    pub module_facts: Option<&'a ModuleFactsMap>,
    pub current_filename: Option<&'a str>,
}

impl Default for RulePipelineOptions<'_> {
    fn default() -> Self {
        Self {
            start_from: None,
            stop_after: None,
            dce_mode: DceMode::Full,
            rewrite_level: RewriteLevel::Standard,
            module_facts: None,
            current_filename: None,
        }
    }
}

impl<'a> RulePipelineOptions<'a> {
    pub fn until(stop_after: &'a str) -> Self {
        Self {
            stop_after: Some(stop_after),
            ..Default::default()
        }
    }

    pub fn between(start_from: &'a str, stop_after: &'a str) -> Self {
        Self {
            start_from: Some(start_from),
            stop_after: Some(stop_after),
            ..Default::default()
        }
    }

    pub fn with_dce_mode(mut self, dce_mode: DceMode) -> Self {
        self.dce_mode = dce_mode;
        self
    }

    pub fn with_rewrite_level(mut self, rewrite_level: RewriteLevel) -> Self {
        self.rewrite_level = rewrite_level;
        self
    }

    pub fn with_module_facts(mut self, module_facts: &'a ModuleFactsMap) -> Self {
        self.module_facts = Some(module_facts);
        self
    }

    pub fn with_current_filename(mut self, current_filename: &'a str) -> Self {
        self.current_filename = Some(current_filename);
        self
    }
}

pub fn apply_rules(module: &mut Module, unresolved_mark: Mark, options: RulePipelineOptions<'_>) {
    apply_rules_impl(module, unresolved_mark, options, None, true);
}

pub(crate) fn apply_rules_to_recovered_module(
    module: &mut Module,
    unresolved_mark: Mark,
    options: RulePipelineOptions<'_>,
) {
    apply_rules_impl(module, unresolved_mark, options, None, false);
}

pub(crate) fn apply_rules_with_observer(
    module: &mut Module,
    unresolved_mark: Mark,
    options: RulePipelineOptions<'_>,
    observer: &mut dyn FnMut(&'static str, &Module),
) {
    apply_rules_impl(module, unresolved_mark, options, Some(observer), true);
}

/// Returns the ordered list of rule names in the pipeline.
pub fn rule_names() -> &'static [&'static str] {
    RULE_NAMES
}

/// Returns the ordered rule descriptors in the pipeline.
pub fn rule_descriptors() -> &'static [RuleDescriptor] {
    RULE_DESCRIPTORS
}

fn apply_rules_impl(
    module: &mut Module,
    unresolved_mark: Mark,
    options: RulePipelineOptions<'_>,
    mut observer: Option<&mut dyn FnMut(&'static str, &Module)>,
    preserve_input_import_link_checks: bool,
) {
    let pre_dead = if options.dce_mode == DceMode::TransformOnly {
        Some(Rc::new(PreDeadSet {
            decl_spans: compute_pre_dead_decl_spans(module),
            preserved_import_spans: if preserve_input_import_link_checks {
                compute_pre_existing_import_spans(module)
            } else {
                compute_pre_dead_import_spans(module)
            },
        }))
    } else {
        None
    };
    let ctx = RuleRunContext {
        unresolved_mark,
        rewrite_level: options.rewrite_level,
        dce_mode: options.dce_mode,
        source_import_reads_are_observable: preserve_input_import_link_checks,
        module_facts: options.module_facts,
        current_filename: options.current_filename,
        local_helpers: Rc::new(RefCell::new(None)),
        extracted_function_names: Rc::new(RefCell::new(ExtractedFunctionNames::new())),
        pre_dead,
    };
    let mut started = options.start_from.is_none();

    for descriptor in RULE_DESCRIPTORS {
        if !descriptor.is_enabled(ctx.clone()) {
            continue;
        }
        if !started && options.start_from == Some(descriptor.id) {
            started = true;
        }
        if !started {
            continue;
        }

        let span = tracing::debug_span!("rule", name = descriptor.id);
        {
            let _enter = span.enter();
            descriptor.run(module, ctx.clone());
        }
        if let Some(observer) = observer.as_deref_mut() {
            observer(descriptor.id, module);
        }
        if options.stop_after == Some(descriptor.id) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use swc_core::common::{DUMMY_SP, GLOBALS};
    use swc_core::ecma::ast::{BlockStmt, ModuleItem, Stmt, TryStmt};

    use super::super::transpiler_helper_utils::{
        collect_transpiler_helpers_call_count, reset_collect_transpiler_helpers_call_count,
        LocalHelperContext,
    };
    use super::*;

    fn module_with_try_stmt() -> Module {
        Module {
            span: DUMMY_SP,
            body: vec![ModuleItem::Stmt(Stmt::Try(Box::new(TryStmt {
                span: DUMMY_SP,
                block: BlockStmt {
                    span: DUMMY_SP,
                    ctxt: Default::default(),
                    stmts: Vec::new(),
                },
                handler: None,
                finalizer: None,
            })))],
            shebang: None,
        }
    }

    #[test]
    fn reuses_local_helper_context_within_stable_rule_spans() {
        GLOBALS.set(&Default::default(), || {
            let mut module = Module {
                span: DUMMY_SP,
                body: Vec::new(),
                shebang: None,
            };
            let unresolved_mark = Mark::new();

            reset_collect_transpiler_helpers_call_count();
            apply_rules(
                &mut module,
                unresolved_mark,
                RulePipelineOptions::between("UnInteropRequireDefault", "UnRegenerator"),
            );

            // The context is rebuilt twice: once after UnInteropRequireDefault
            // (unwrapping can expose new runtime-path helpers) and once after
            // UnConditionals (ternary→if/else rewrites change helper body shapes
            // for UnClassFields/UnDefineProperty).
            assert_eq!(collect_transpiler_helpers_call_count(), 3);
        });
    }

    #[test]
    fn un_for_of_runner_uses_precomputed_local_helper_context() {
        GLOBALS.set(&Default::default(), || {
            let mut module = module_with_try_stmt();
            let unresolved_mark = Mark::new();
            let ctx = RuleRunContext {
                unresolved_mark,
                rewrite_level: RewriteLevel::Standard,
                dce_mode: DceMode::Full,
                source_import_reads_are_observable: true,
                module_facts: None,
                current_filename: None,
                local_helpers: Rc::new(RefCell::new(Some(Rc::new(LocalHelperContext::default())))),
                extracted_function_names: Default::default(),
                pre_dead: None,
            };

            reset_collect_transpiler_helpers_call_count();
            run_un_for_of(&mut module, ctx);

            assert_eq!(collect_transpiler_helpers_call_count(), 0);
        });
    }
}
