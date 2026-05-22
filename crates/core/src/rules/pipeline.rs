use swc_core::common::Mark;
use swc_core::ecma::ast::Module;
use swc_core::ecma::visit::VisitMutWith;

use crate::facts::ModuleFactsMap;

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
    run: RuleRunner,
    enabled: RuleEnabled,
}

impl RuleDescriptor {
    const fn gated(
        id: &'static str,
        stage: RuleStage,
        run: RuleRunner,
        enabled: RuleEnabled,
    ) -> Self {
        Self {
            id,
            stage,
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

#[derive(Clone, Copy)]
struct RuleRunContext<'a> {
    unresolved_mark: Mark,
    rewrite_level: RewriteLevel,
    dead_code_elimination: bool,
    module_facts: Option<&'a ModuleFactsMap>,
}

fn always_enabled(_: RuleRunContext<'_>) -> bool {
    true
}

fn dead_code_elimination_enabled(ctx: RuleRunContext<'_>) -> bool {
    ctx.dead_code_elimination
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
    ($(
        ($id:literal, $stage:ident, $runner:ident, $enabled:ident)
    ),+ $(,)?) => {
        pub static RULE_DESCRIPTORS: &[RuleDescriptor] = &[
            $(
                RuleDescriptor::gated($id, RuleStage::$stage, $runner, $enabled),
            )+
        ];

        static RULE_NAMES: &[&str] = &[
            $(
                $id,
            )+
        ];
    };
}

runner!(run_simplify_sequence, |ctx| SimplifySequence::new(
    ctx.unresolved_mark
));
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
runner!(run_un_indirect_call, UnIndirectCall);
runner!(run_un_typeof, UnTypeof);
runner!(run_un_numeric_literal, UnNumericLiteral);
runner!(run_un_bracket_notation, UnBracketNotation);
runner!(run_un_interop_require_default, UnInteropRequireDefault);
runner!(run_un_interop_require_wildcard, UnInteropRequireWildcard);
runner!(run_un_to_consumable_array, UnToConsumableArray);

fn run_un_object_spread(module: &mut Module, ctx: RuleRunContext<'_>) {
    if let Some(facts) = ctx.module_facts {
        module.visit_mut_with(&mut UnObjectSpread::new_with_facts(facts));
    } else {
        module.visit_mut_with(&mut UnObjectSpread::new());
    }
}

runner!(run_un_object_rest, |ctx| UnObjectRest::new(
    ctx.unresolved_mark
));
runner!(run_un_sliced_to_array, UnSlicedToArray);
runner!(run_un_define_property, UnDefineProperty);
runner!(run_un_class_call_check, UnClassCallCheck);
runner!(
    run_un_possible_constructor_return,
    UnPossibleConstructorReturn
);
runner!(run_un_assert_this_initialized, UnAssertThisInitialized);
runner!(run_un_typeof_polyfill, UnTypeofPolyfill);
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
runner!(run_un_template_literal, UnTemplateLiteral);
runner!(run_un_while_loop, UnWhileLoop);
runner!(run_un_type_constructor, |ctx| UnTypeConstructor::new(
    ctx.rewrite_level
));
runner!(run_un_builtin_prototype, UnBuiltinPrototype);
runner!(run_un_argument_spread, |ctx| UnArgumentSpread::new(
    ctx.unresolved_mark,
    ctx.rewrite_level
));
runner!(run_un_array_concat_spread, UnArrayConcatSpread);
runner!(run_un_spread_array_literal, UnSpreadArrayLiteral);
runner!(run_object_assign_spread, |ctx| ObjectAssignSpread::new(
    ctx.unresolved_mark
));
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
runner!(run_un_conditionals, UnConditionals);
runner!(run_un_parameters, |ctx| UnParameters::new(
    ctx.unresolved_mark,
    ctx.rewrite_level
));
runner!(run_un_enum, UnEnum);
runner!(run_un_jsx, |ctx| UnJsx::new_with_level(
    ctx.unresolved_mark,
    ctx.rewrite_level
));
runner!(run_un_es6_class, |ctx| UnEs6Class::new(ctx.unresolved_mark));
runner!(run_un_class_fields, UnClassFields);
runner!(run_un_ts_helpers, UnTsHelpers);

fn run_un_regenerator(module: &mut Module, ctx: RuleRunContext<'_>) {
    if let Some(facts) = ctx.module_facts {
        module.visit_mut_with(&mut UnRegenerator::new_with_facts(
            ctx.unresolved_mark,
            facts,
        ));
    } else {
        module.visit_mut_with(&mut UnRegenerator::new(ctx.unresolved_mark));
    }
}

runner!(run_un_async_await, UnAsyncAwait);
runner!(run_un_then_catch, |ctx| UnThenCatch::new(
    ctx.unresolved_mark
));
runner!(run_un_undefined_init, |ctx| UnUndefinedInit::new(
    ctx.unresolved_mark
));
runner!(run_var_decl_to_let_const, VarDeclToLetConst);
runner!(run_obj_shorthand, ObjShorthand);
runner!(run_obj_method_shorthand, ObjMethodShorthand);
runner!(run_un_prototype_class, UnPrototypeClass);
runner!(run_exponent, Exponent);
runner!(run_arg_rest, |ctx| ArgRest::new(ctx.rewrite_level));
runner!(run_un_rest_array_copy, UnRestArrayCopy);
runner!(run_arrow_function, ArrowFunction);
runner!(run_arrow_return, ArrowReturn);
runner!(run_un_for_of, |ctx| UnForOf::new(ctx.rewrite_level));
runner!(run_un_webpack_define_getters, |ctx| {
    UnWebpackDefineGetters::new(ctx.unresolved_mark)
});
runner!(run_un_webpack_object_getters, |ctx| {
    UnWebpackObjectGetters::new(ctx.unresolved_mark)
});
runner!(run_import_dedup, ImportDedup);
runner!(run_un_import_rename, UnImportRename);
runner!(run_un_export_rename, UnExportRename);
runner!(run_un_destructuring, |ctx| UnDestructuring::new(
    ctx.unresolved_mark
));
runner!(run_smart_inline, |ctx| SmartInline::new(ctx.rewrite_level));
runner!(run_smart_rename, |ctx| SmartRename::new(
    ctx.unresolved_mark
));
runner!(run_dead_decls, DeadDecls);
runner!(run_dead_imports, DeadImports);
runner!(run_un_return, UnReturn);

define_rule_registry! {
    ("SimplifySequence", Syntax, run_simplify_sequence, always_enabled),
    ("FlipComparisons", Syntax, run_flip_comparisons, always_enabled),
    ("UnTypeofStrict", Syntax, run_un_typeof_strict, always_enabled),
    ("RemoveVoid", Syntax, run_remove_void, always_enabled),
    ("UnminifyBooleans", Syntax, run_unminify_booleans, always_enabled),
    ("UnDoubleNegation", Syntax, run_un_double_negation, always_enabled),
    ("UnInfinity", Syntax, run_un_infinity, always_enabled),
    ("UnIndirectCall", Syntax, run_un_indirect_call, always_enabled),
    ("UnTypeof", Syntax, run_un_typeof, always_enabled),
    ("UnNumericLiteral", Syntax, run_un_numeric_literal, always_enabled),
    ("UnBracketNotation", Syntax, run_un_bracket_notation, always_enabled),
    ("UnInteropRequireDefault", Helpers, run_un_interop_require_default, always_enabled),
    ("UnInteropRequireWildcard", Helpers, run_un_interop_require_wildcard, always_enabled),
    ("UnToConsumableArray", Helpers, run_un_to_consumable_array, always_enabled),
    ("UnObjectSpread", Helpers, run_un_object_spread, always_enabled),
    ("UnObjectRest", Helpers, run_un_object_rest, always_enabled),
    ("UnSlicedToArray", Helpers, run_un_sliced_to_array, always_enabled),
    ("UnDefineProperty", Helpers, run_un_define_property, always_enabled),
    ("UnClassCallCheck", Helpers, run_un_class_call_check, always_enabled),
    ("UnPossibleConstructorReturn", Helpers, run_un_possible_constructor_return, always_enabled),
    ("UnAssertThisInitialized", Helpers, run_un_assert_this_initialized, always_enabled),
    ("UnTypeofPolyfill", Helpers, run_un_typeof_polyfill, always_enabled),
    // UnEsm prerequisites: add braces to enable assignment splitting, remove
    // __esModule flags, strip "use strict", split chained assignments, and
    // resolve webpack interop getters. These dependencies are documented in
    // docs/rule-dependency-inventory.md.
    ("UnCurlyBraces", Helpers, run_un_curly_braces, always_enabled),
    ("UnEsmoduleFlag", Helpers, run_un_esmodule_flag, always_enabled),
    ("UnUseStrict", Helpers, run_un_use_strict, always_enabled),
    ("UnAssignmentMerging", Helpers, run_un_assignment_merging, always_enabled),
    ("UnWebpackInterop", Helpers, run_un_webpack_interop, always_enabled),
    ("UnEsm", Helpers, run_un_esm, always_enabled),
    ("UnTemplateLiteral", Structural, run_un_template_literal, always_enabled),
    ("UnWhileLoop", Structural, run_un_while_loop, always_enabled),
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
    ("UnParameters", Complex, run_un_parameters, always_enabled),
    ("UnEnum", Complex, run_un_enum, always_enabled),
    ("UnJsx", Complex, run_un_jsx, always_enabled),
    ("UnEs6Class", Complex, run_un_es6_class, always_enabled),
    ("UnClassFields", Complex, run_un_class_fields, always_enabled),
    ("UnTsHelpers", Complex, run_un_ts_helpers, always_enabled),
    ("UnRegenerator", Complex, run_un_regenerator, always_enabled),
    ("UnAsyncAwait", Complex, run_un_async_await, always_enabled),
    // Second pass: UnAsyncAwait can expose additional interop getter shapes.
    ("UnWebpackInterop2", Complex, run_un_webpack_interop, always_enabled),
    ("UnThenCatch", Modernization, run_un_then_catch, always_enabled),
    ("UnUndefinedInit", Modernization, run_un_undefined_init, always_enabled),
    ("VarDeclToLetConst", Modernization, run_var_decl_to_let_const, always_enabled),
    ("ObjShorthand", Modernization, run_obj_shorthand, always_enabled),
    ("ObjMethodShorthand", Modernization, run_obj_method_shorthand, always_enabled),
    ("UnPrototypeClass", Modernization, run_un_prototype_class, always_enabled),
    ("Exponent", Modernization, run_exponent, always_enabled),
    ("ArgRest", Modernization, run_arg_rest, always_enabled),
    ("UnRestArrayCopy", Modernization, run_un_rest_array_copy, always_enabled),
    ("ArrowFunction", Modernization, run_arrow_function, always_enabled),
    ("ArrowReturn", Modernization, run_arrow_return, always_enabled),
    ("UnForOf", Modernization, run_un_for_of, always_enabled),
    ("UnWebpackDefineGetters", Cleanup, run_un_webpack_define_getters, always_enabled),
    ("UnWebpackObjectGetters", Cleanup, run_un_webpack_object_getters, always_enabled),
    ("ImportDedup", Cleanup, run_import_dedup, always_enabled),
    ("UnImportRename", Cleanup, run_un_import_rename, always_enabled),
    ("UnExportRename", Cleanup, run_un_export_rename, always_enabled),
    // Third pass: UnEsm can convert require() bindings to imports, exposing
    // direct require.n(importBinding) helpers.
    ("UnWebpackInterop3", Cleanup, run_un_webpack_interop, always_enabled),
    ("UnDestructuring", Cleanup, run_un_destructuring, always_enabled),
    // UnDestructuring can expose param === undefined ? {} : param initializers.
    ("UnParameters2", Cleanup, run_un_parameters, always_enabled),
    ("SmartInline", Cleanup, run_smart_inline, always_enabled),
    // SmartInline can create new (() => expr)() patterns.
    ("UnIife2", Cleanup, run_un_iife, always_enabled),
    ("SmartRename", Cleanup, run_smart_rename, always_enabled),
    // SmartRename may capitalize component bindings that UnJsx intentionally
    // skipped earlier because lowercase JSX tags are HTML elements.
    ("UnJsx2", Cleanup, run_un_jsx, always_enabled),
    // DeadDecls first: removing dead helpers can leave import specifiers
    // unreferenced, which DeadImports then cleans up.
    ("DeadDecls", Cleanup, run_dead_decls, dead_code_elimination_enabled),
    ("DeadImports", Cleanup, run_dead_imports, dead_code_elimination_enabled),
    // Last pass: no downstream rule needs tail return undefined, and earlier
    // restructuring rules can introduce new ones.
    ("UnReturn", Cleanup, run_un_return, always_enabled),
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
/// Repeated passes are suffixed: "UnWebpackInterop2", "UnWebpackInterop3", "UnIife2".
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
    RULE_NAMES
}

/// Returns the ordered rule descriptors in the pipeline.
pub fn rule_descriptors() -> &'static [RuleDescriptor] {
    RULE_DESCRIPTORS
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
        RuleRunExtras::default(),
        dead_code_elimination,
        rewrite_level,
    );
}

pub(crate) fn apply_rules_between_with_level_and_facts(
    module: &mut Module,
    unresolved_mark: Mark,
    start_from: &str,
    stop_after: &str,
    dead_code_elimination: bool,
    rewrite_level: RewriteLevel,
    module_facts: &ModuleFactsMap,
) {
    apply_rules_range_impl(
        module,
        unresolved_mark,
        Some(start_from),
        Some(stop_after),
        RuleRunExtras {
            observer: None,
            module_facts: Some(module_facts),
        },
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
        RuleRunExtras {
            observer: Some(observer),
            module_facts: None,
        },
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
        RuleRunExtras {
            observer,
            module_facts: None,
        },
        dead_code_elimination,
        rewrite_level,
    );
}

#[derive(Default)]
struct RuleRunExtras<'a> {
    observer: Option<&'a mut dyn FnMut(&'static str, &Module)>,
    module_facts: Option<&'a ModuleFactsMap>,
}

fn apply_rules_range_impl(
    module: &mut Module,
    unresolved_mark: Mark,
    start_from: Option<&str>,
    stop_after: Option<&str>,
    mut extras: RuleRunExtras,
    dead_code_elimination: bool,
    rewrite_level: RewriteLevel,
) {
    let ctx = RuleRunContext {
        unresolved_mark,
        rewrite_level,
        dead_code_elimination,
        module_facts: extras.module_facts,
    };
    let mut started = start_from.is_none();

    for descriptor in RULE_DESCRIPTORS {
        if !descriptor.is_enabled(ctx) {
            continue;
        }
        if !started && start_from == Some(descriptor.id) {
            started = true;
        }
        if !started {
            continue;
        }

        let span = tracing::debug_span!("rule", name = descriptor.id);
        {
            let _enter = span.enter();
            descriptor.run(module, ctx);
        }
        if let Some(observer) = extras.observer.as_deref_mut() {
            observer(descriptor.id, module);
        }
        if stop_after == Some(descriptor.id) {
            return;
        }
    }
}
