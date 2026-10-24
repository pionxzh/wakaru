//! Two-phase multi-module pipeline: fact collection (Phase 1) and output
//! decompilation with the cross-module late pass (Phase 2).

use std::cell::RefCell;
use std::sync::Arc;

use anyhow::{bail, Result};
use swc_core::common::{sync::Lrc, Globals, Mark, SourceMap, SyntaxContext, DUMMY_SP, GLOBALS};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, Expr, ExprStmt, Ident, IdentName, MemberExpr, MemberProp,
    Module, ModuleDecl, ModuleItem, ObjectLit, SimpleAssignTarget, Stmt,
};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};

use super::super::diagnostics::{
    collect_commonjs_export_residual_warnings, collect_commonjs_exports_escape_warnings,
    collect_commonjs_require_of_esm_warnings, collect_commonjs_require_residual_warnings,
    collect_es_source_kept_commonjs_warnings, collect_input_parse_warnings,
    collect_output_diagnostics, collect_webpack_runtime_residual_warnings,
};
use super::super::io::{
    apply_fixer, build_output_sourcemap, parse_js, parse_js_with_recovery, print_js,
    print_js_with_srcmap, ParseDiagnostic,
};
use super::super::output_finalize::strip_redundant_module_use_strict;
use super::super::types::{
    DecompileOptions, PreparedInputId, PreparedModuleOutput, PreparedModuleProvenance,
    PreparedUnpackOutput, UnpackWarning, UnpackWarningKind,
};
use super::super::unpack_cycles::collect_import_cycle_warnings;
use super::dead_module::{collect_import_report, eliminate_dead_helper_modules, ImportReport};
use super::filename_recovery::{
    build_rename_map, harvest_suggested_filename, rewrite_import_sources,
};
use super::interop_report::import_forms;
use super::link_check::{collect_link_report, collect_missing_imported_name_warnings, LinkReport};
use super::merge::{
    apply_filename_rewrites, apply_numeric_rewrites, NumericRewritePlan, PreparedUnpackModule,
};
use super::schedule::par_map_largest_first;
use super::source_index::{build_composed_output_sourcemap, InputOrigin};
use super::webpack_commonjs_runtime::normalize_webpack_commonjs_runtime;
use super::{recover_late_esm_from_factory_iifes, LateEsmRecoveryOptions};
use crate::facts::{
    collect_commonjs_default_attached_properties, collect_commonjs_default_object,
    collect_commonjs_value_is_default, collect_default_object_tail, collect_input_is_es_module,
    collect_marks_es_module, collect_module_facts, collect_module_facts_with_helpers,
    collect_replaces_module_exports, collect_require_returns_exports_object,
    is_normalized_commonjs_default, HelperFacts, ModuleFactsMap,
};
use crate::interface_plan::{
    exports_from_requires, exports_only_whole_value, passthrough_source, required_whole_value,
    InterfacePlan, PlanModule,
};
use crate::kept_require::run_kept_require_default_reads;
use crate::namespace_decomposition::run_namespace_decomposition_with_tags;
use crate::provider_helpers::ProviderHelpers;
use crate::provider_import_repair::run_provider_import_repair;
use crate::provider_namespace_repair::run_provider_namespace_repair;
use crate::require_edges::{
    declarator_require_calls, tag_declarator_imports, ImportTags, RequireEdges,
};
use crate::rules::cross_module_interop_import::{
    import_unproven_helper_requires, run_cross_module_interop_imports,
};
use crate::rules::eval_utils::DirectEvalAnalyzer;
use crate::rules::expr_utils::is_unresolved_ident;
use crate::rules::is_webpack_harmony_module;
use crate::rules::lowered_dynamic_import::run_cross_module_lowered_dynamic_imports;
use crate::rules::transpiler_helper_utils::LocalHelperContext;
use crate::rules::webpack_lazy_import::run_cross_module_webpack_lazy_imports;
use crate::rules::{
    apply_rules, apply_rules_to_recovered_module, contains_local_self_require,
    lowers_to_commonjs_namespace, ImportDedup, PipelineMode, RewriteLevel, RulePipelineOptions,
    SimplifySequence, UnAssignmentMerging, UnConditionals, UnConditionalsAssignmentOnly,
    UnImportRename, UnOptionalChaining,
};
use crate::sourcemap_rename::{apply_sourcemap_renames, parse_sourcemap};
use crate::synthetic_import_cleanup::downgrade_unused_synthetic_imports;
use crate::unpacker::{
    arrow_iife_call, module_stmts_have_function_level_special_bindings,
    stmts_have_function_level_return, stmts_have_function_level_special_bindings,
    DetectedModuleFailure, DetectedModuleNote, InputOffsets, TurbopackContextUse,
};

#[derive(Debug, PartialEq, Eq)]
enum BoundaryRestoration {
    /// Restoration was disabled or the module has no function-level return
    /// at module scope; the module is already valid ESM at this boundary.
    NotNeeded,
    /// The lifted statements were wrapped back into a callable boundary.
    Restored,
    /// A function-level return remains at module scope but the module cannot
    /// be wrapped (recovered exports would move outside the guarded flow).
    /// Emitting this module as ESM would not parse.
    Declined,
}

fn restore_lifted_function_boundary(module: &mut Module, enabled: bool) -> BoundaryRestoration {
    if !enabled {
        return BoundaryRestoration::NotNeeded;
    }
    let statements = module
        .body
        .iter()
        .filter_map(|item| match item {
            ModuleItem::Stmt(statement) => Some(statement.clone()),
            ModuleItem::ModuleDecl(_) => None,
        })
        .collect::<Vec<_>>();
    if !stmts_have_function_level_return(&statements) {
        return BoundaryRestoration::NotNeeded;
    }
    if module.body.iter().any(|item| {
        matches!(
            item,
            ModuleItem::ModuleDecl(decl) if !matches!(decl, ModuleDecl::Import(_))
        )
    }) {
        return BoundaryRestoration::Declined;
    }
    // The restored arrow boundary inherits module-scope `this` (undefined),
    // has no `arguments`, and cannot provide a valid `new.target`. A lifted
    // body observing any of them would break at parse time or runtime. Fail
    // closed to the extracted-source fallback instead.
    if stmts_have_function_level_special_bindings(&statements) {
        return BoundaryRestoration::Declined;
    }

    let mut restored = std::mem::take(&mut module.body)
        .into_iter()
        .filter(|item| matches!(item, ModuleItem::ModuleDecl(ModuleDecl::Import(_))))
        .collect::<Vec<_>>();
    restored.push(ModuleItem::Stmt(Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(arrow_iife_call(statements)),
    })));
    module.body = restored;
    BoundaryRestoration::Restored
}

fn detector_failure_warning(filename: &str, failure: DetectedModuleFailure) -> UnpackWarning {
    match failure {
        DetectedModuleFailure::WebpackRuntimeParameterReuse => UnpackWarning::new(
            filename,
            UnpackWarningKind::WebpackFactoryRecoveryFailed,
            "webpack factory runtime-parameter reuse could not be normalized; preserving the opaque factory body",
        ),
        DetectedModuleFailure::WebpackFreeNameCapture => UnpackWarning::new(
            filename,
            UnpackWarningKind::WebpackFactoryRecoveryFailed,
            "webpack factory could not take the `module`/`exports`/`require` parameter names without capturing a free reference; preserving the opaque factory body",
        ),
        DetectedModuleFailure::TranslatedFactoryNormalization => UnpackWarning::new(
            filename,
            UnpackWarningKind::WebpackFactoryRecoveryFailed,
            "translated factory could not be normalized; preserving the opaque factory body",
        ),
        DetectedModuleFailure::TurbopackUnsupportedRuntime(context_use) => UnpackWarning::new(
            filename,
            UnpackWarningKind::DecompileFailed,
            match context_use {
                TurbopackContextUse::Member(member) => format!(
                    "Turbopack factory uses runtime context member `{member}` without a known translation; preserving the opaque factory body"
                ),
                TurbopackContextUse::Dirname => "Turbopack factory reads its `__dirname` context binding; preserving the opaque factory body".to_string(),
                TurbopackContextUse::Other => "Turbopack factory uses its runtime context without a known translation; preserving the opaque factory body".to_string(),
            },
        ),
    }
}

fn detector_note_warning(filename: &str, note: DetectedModuleNote) -> UnpackWarning {
    match note {
        DetectedModuleNote::TurbopackRuntimeResidual(letter) => UnpackWarning::new(
            filename,
            UnpackWarningKind::RuntimeResidual,
            format!(
                "Turbopack runtime member `{letter}` is kept as a call through `__turbopack_context__`, which the split output does not define"
            ),
        ),
    }
}

struct Phase1PreparedModule {
    globals: Globals,
    module: Module,
    /// `module` after the rules Phase 2 runs before its late recovery, as
    /// the Phase 1b prediction ran them without the facts. Kept for a module
    /// `UnEsm` did not settle that has no imports; Phase 2 adopts it instead
    /// of running those rules again when its passes leave `module` as it was
    /// and `range_ignores_facts` holds.
    ranged: Option<Module>,
    unresolved_mark: Mark,
    /// The import specifiers `UnEsm` made from requires in this AST.
    import_tags: ImportTags,
}

/// A module past Phase 1a: its AST where the rule range reaches `UnEsm`,
/// and what Phase 1a read before the rules ran.
struct Phase1aModule {
    filename: String,
    /// The extracted source length, for the largest-first schedule.
    size: usize,
    globals: Globals,
    module: Module,
    unresolved_mark: Mark,
    raw: RawCommonJsFacts,
    require_edges: RequireEdges,
    /// Requiring the module returns its CommonJS `exports` object: it is
    /// neither ESM nor a webpack harmony module where `UnEsm` will see it.
    exports_object_writable: bool,
    /// The interop helpers the module exports, on the AST `UnEsm` will see.
    helpers: Option<ProviderHelpers>,
    /// The module's only export is one top-level `module.exports = v`, read
    /// on the input.
    exports_whole_value: bool,
    /// Requiring the module returns that `v`, its default export, read on
    /// the input (`required_whole_value`).
    required_whole_value: bool,
    /// The module is a CommonJS passthrough of this source, on the AST
    /// `UnEsm` will see.
    passthrough: Option<swc_core::atoms::Atom>,
    /// The sources whose required value may become `module.exports`, on
    /// the AST `UnEsm` will see.
    exports_from_requires: Vec<swc_core::atoms::Atom>,
    warning: Option<UnpackWarning>,
    input_parse_warnings: Vec<UnpackWarning>,
    suggested_filename: Option<String>,
}

enum Phase1a {
    /// No rule runs on the module: its Phase 1 result is final.
    Done(Box<Phase1Module>),
    Pending(Box<Phase1aModule>),
}

/// The CommonJS provider facts read on the resolved input, before any rule.
struct RawCommonJsFacts {
    commonjs_default_object: Option<crate::facts::CommonJsDefaultObjectFact>,
    commonjs_default_attached_properties: Vec<swc_core::atoms::Atom>,
    require_returns_exports_object: bool,
    input_is_es_module: bool,
    commonjs_value_is_default: bool,
    replaces_module_exports: bool,
    es_source_evidence: Vec<&'static str>,
    marks_es_module: bool,
}

impl RawCommonJsFacts {
    fn apply(self, facts: &mut crate::facts::ModuleFacts) {
        facts.commonjs_default_object = self.commonjs_default_object;
        facts.commonjs_default_attached_properties = self.commonjs_default_attached_properties;
        facts.require_returns_exports_object = self.require_returns_exports_object;
        facts.input_is_es_module = self.input_is_es_module;
        facts.commonjs_value_is_default = self.commonjs_value_is_default;
        facts.replaces_module_exports = self.replaces_module_exports;
        facts.es_source_evidence = self.es_source_evidence;
        facts.marks_es_module = self.marks_es_module;
    }
}

/// Phase 1a's rule range: every rule before `UnEsm`. Returns the module's
/// require edges: each call's form and interop, read on the input, and its
/// uses, read on the AST `UnEsm` will see.
fn run_rules_before_un_esm(
    module: &mut Module,
    unresolved_mark: Mark,
    level: RewriteLevel,
    filename: &str,
) -> RequireEdges {
    let mut edges = RequireEdges::collect(module, unresolved_mark);
    apply_rules(
        module,
        unresolved_mark,
        RulePipelineOptions::before("UnEsm")
            .with_mode(PipelineMode::UnpackPhase1)
            .with_rewrite_level(level)
            .with_current_filename(filename),
    );
    if edges.iter().next().is_some() {
        let local_helpers = LocalHelperContext::collect_with_mark(module, unresolved_mark);
        edges.add_uses(module, unresolved_mark, &local_helpers);
    }
    edges
}

/// Phase 1b's rule range: `UnEsm` alone, on the AST Phase 1a left. Returns
/// the import specifiers it made from requires.
fn run_un_esm_range(
    module: &mut Module,
    unresolved_mark: Mark,
    level: RewriteLevel,
    filename: &str,
    plan: Option<&InterfacePlan>,
) -> ImportTags {
    let import_tags = RefCell::new(ImportTags::default());
    apply_rules(
        module,
        unresolved_mark,
        RulePipelineOptions::between("UnEsm", "UnEsm")
            .with_mode(PipelineMode::UnpackPhase1)
            .with_rewrite_level(level)
            .with_current_filename(filename)
            .with_import_tags(&import_tags)
            .with_interface_plan(plan),
    );
    import_tags.into_inner()
}

/// What Phase 2's rule range reads from the other modules.
#[derive(Clone, Copy)]
struct LateRangeFacts<'a> {
    module_facts: &'a ModuleFactsMap,
    call_required_plan: &'a crate::rules::CallRequiredPlan,
}

/// The rules Phase 2 runs after its cross-module passes and before its late
/// ESM recovery: the registry range after `UnEsm` through `UnReturn`, then
/// the syntax cleanup the late recovery needs. Phase 1b runs the same steps
/// on its prediction clone, without the facts, so that the late recovery
/// there sees the shapes these rules expose (a `require` hoisted out of a
/// `for` head, a TypeScript enum IIFE expanded) and the module facts predict
/// the format Phase 2 reaches.
fn run_rules_before_late_recovery(
    module: &mut Module,
    unresolved_mark: Mark,
    level: RewriteLevel,
    dce_mode: crate::DceMode,
    filename: &str,
    mode: PipelineMode,
    facts: Option<LateRangeFacts<'_>>,
) {
    let mut pipeline_options = RulePipelineOptions::between("UnObjectSpread2", "UnReturn")
        .with_mode(mode)
        .with_dce_mode(dce_mode)
        .with_rewrite_level(level)
        .with_current_filename(filename);
    if let Some(facts) = facts {
        pipeline_options = pipeline_options
            .with_module_facts(facts.module_facts)
            .with_call_required_plan(facts.call_required_plan);
    }
    apply_rules_to_recovered_module(module, unresolved_mark, pipeline_options);
    // Later rules can expose sequence expressions. The old unpack
    // path cleaned those by running a second full module pipeline;
    // keep only the syntax cleanup needed after the split.
    module.visit_mut_with(&mut SimplifySequence::new_with_level(
        unresolved_mark,
        level,
    ));
    module.visit_mut_with(&mut UnAssignmentMerging::new(unresolved_mark));
}

/// Whether [`run_rules_before_late_recovery`] does the same on `module`
/// with `facts` as without them, so that Phase 2 can adopt the run the
/// Phase 1b prediction made on the same AST.
///
/// The range reads other modules in two places only. The transpiler-helper
/// rules (`UnObjectSpread2`, `UnObjectRest2`/`3`, `UnSlicedToArray2`,
/// `UnTemplateLiteral`, `UnRegenerator`, `UnAsyncAwait`, `UnForOf`) look up
/// the module named by an import source or a `require("…")` argument and
/// read only its helper facts (`helper_exports`,
/// `default_object_helper_exports`, `ts_helper_exports`,
/// `ts_helper_namespace_factory_exports`); with all of them empty, a lookup
/// finds what a run without facts finds. `UnEs6Class` and
/// `UnPrototypeClass` read this module's `CallRequiredPlan` pins. No rule in
/// the range reads the pipeline mode. A rule added to the range that reads
/// facts another way must be covered here.
fn range_ignores_facts(module: &Module, facts: LateRangeFacts<'_>, filename: &str) -> bool {
    struct ProviderSources<'a> {
        facts: &'a ModuleFactsMap,
        filename: &'a str,
        helper_provider: bool,
    }
    impl ProviderSources<'_> {
        fn check(&mut self, source: &swc_core::ecma::ast::Str) {
            // The rules read a source that is not valid UTF-8 as "".
            let source = source.value.as_str().unwrap_or("");
            self.helper_provider |= self
                .facts
                .get_from(Some(self.filename), source)
                .is_some_and(|provider| {
                    !provider.helper_exports.is_empty()
                        || !provider.default_object_helper_exports.is_empty()
                        || !provider.ts_helper_exports.is_empty()
                        || !provider.ts_helper_namespace_factory_exports.is_empty()
                });
        }
    }
    impl Visit for ProviderSources<'_> {
        fn visit_import_decl(&mut self, import: &swc_core::ecma::ast::ImportDecl) {
            self.check(&import.src);
        }
        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            for arg in &call.args {
                if let Expr::Lit(swc_core::ecma::ast::Lit::Str(source)) = arg.expr.as_ref() {
                    self.check(source);
                }
            }
            call.visit_children_with(self);
        }
    }
    if facts
        .call_required_plan
        .pinned_exports(filename)
        .is_some_and(|pins| !pins.is_empty())
    {
        return false;
    }
    let mut sources = ProviderSources {
        facts: facts.module_facts,
        filename,
        helper_provider: false,
    };
    module.visit_with(&mut sources);
    !sources.helper_provider
}

/// Debug builds check every adoption that `range_ignores_facts` allows:
/// Phase 2's own run of the range, with the facts, on the module its passes
/// left must print the same as the adopted run made without them. A
/// mismatch means a rule in the range reads facts in a way the check does
/// not cover.
#[cfg(debug_assertions)]
#[allow(clippy::too_many_arguments)]
fn assert_adopted_range_matches(
    adopted: &Module,
    mut module: Module,
    unresolved_mark: Mark,
    level: RewriteLevel,
    dce_mode: crate::DceMode,
    filename: &str,
    facts: LateRangeFacts<'_>,
) {
    run_rules_before_late_recovery(
        &mut module,
        unresolved_mark,
        level,
        dce_mode,
        filename,
        PipelineMode::UnpackPhase2,
        Some(facts),
    );
    let print = |module: &Module| print_js(module, Lrc::<SourceMap>::default()).ok();
    assert!(
        print(&module) == print(adopted),
        "{filename}: Phase 2 adopted the rule range Phase 1b ran without facts, but the run \
         with facts differs: a rule after `UnEsm` reads facts that `range_ignores_facts` does \
         not cover (docs/fact-system.md, \"Adding a new fact-reading rule\")"
    );
}

/// Whether `module` is an ES module with no CommonJS left that the late ESM
/// recovery can convert: it reads no free `module` or `exports`, and uses
/// the free `require` only as `require(<number>)` and as webpack's
/// namespace-object helpers on a local object (`require.r(ns)`,
/// `require.d(ns, …)`). `UnEsm` makes imports from string specifiers only,
/// and treats `require.r` / `require.d` as exports only on `exports`; no
/// rule turns a number into a string or a local into `exports`. So the late
/// recovery has nothing to convert in such a module, whatever rules run
/// before it. A numeric `require` names a module outside the bundle (an
/// external of a chunk); webpack's module concatenation builds a namespace
/// object with `require.r` / `require.d`.
fn is_settled_es_module(module: &Module, unresolved_mark: Mark) -> bool {
    struct CommonJsGlobals {
        unresolved_mark: Mark,
        found: bool,
    }
    impl CommonJsGlobals {
        fn is_free(&self, expr: &Expr, name: &str) -> bool {
            matches!(expr, Expr::Ident(ident)
                if ident.sym.as_ref() == name && ident.ctxt.outer() == self.unresolved_mark)
        }

        /// A `require` call the late recovery leaves alone, apart from
        /// what its arguments contain.
        fn is_inert_require_call(&self, call: &swc_core::ecma::ast::CallExpr) -> bool {
            use swc_core::ecma::ast::{Callee, Lit};
            let Callee::Expr(callee) = &call.callee else {
                return false;
            };
            if call.args.iter().any(|arg| arg.spread.is_some()) {
                return false;
            }
            if self.is_free(callee, "require") {
                return matches!(call.args.as_slice(), [arg]
                    if matches!(arg.expr.as_ref(), Expr::Lit(Lit::Num(_))));
            }
            let Expr::Member(member) = callee.as_ref() else {
                return false;
            };
            self.is_free(&member.obj, "require")
                && member
                    .prop
                    .as_ident()
                    .is_some_and(|prop| matches!(prop.sym.as_ref(), "r" | "d"))
                && call.args.first().is_some_and(|target| {
                    matches!(target.expr.as_ref(), Expr::Ident(local)
                        if local.ctxt.outer() != self.unresolved_mark)
                })
        }
    }
    impl Visit for CommonJsGlobals {
        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if self.is_inert_require_call(call) {
                call.args.visit_with(self);
            } else {
                call.visit_children_with(self);
            }
        }
        fn visit_ident(&mut self, ident: &Ident) {
            self.found |= ident.ctxt.outer() == self.unresolved_mark
                && matches!(ident.sym.as_ref(), "module" | "exports" | "require");
        }
    }
    if !module
        .body
        .iter()
        .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
    {
        return false;
    }
    let mut globals = CommonJsGlobals {
        unresolved_mark,
        found: false,
    };
    module.visit_with(&mut globals);
    !globals.found
}

struct Phase1Module {
    filename: String,
    facts: crate::facts::ModuleFacts,
    prepared: Option<Phase1PreparedModule>,
    warning: Option<UnpackWarning>,
    input_parse_warnings: Vec<UnpackWarning>,
    /// Original source filename recovered from provenance markers (Sentry
    /// `data-sentry-source-file`), if any. Used at the barrier to rename the
    /// module's output file and rewrite importers' references.
    suggested_filename: Option<String>,
}

/// Restore the default object that webpack creates before invoking a CommonJS
/// factory. Empty factories carry this detector fact directly. A non-empty
/// self-requiring factory may use the same fact only when its complete body
/// cannot observe the original wrapper-created object through a runtime alias.
/// Raw output never calls this normal-pipeline helper.
fn restore_webpack_factory_default(
    module: &mut Module,
    unresolved_mark: Mark,
    implicit_empty_default: bool,
    webpack_commonjs_runtime: bool,
    current_filename: &str,
) {
    let restore_empty = implicit_empty_default && module.body.is_empty();
    let restore_self_required = webpack_commonjs_runtime
        && !module.body.is_empty()
        && !module
            .body
            .iter()
            .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
        && contains_local_self_require(module, unresolved_mark, Some(current_filename))
        && !observes_webpack_factory_environment(module, unresolved_mark);
    if !restore_empty && !restore_self_required {
        return;
    }

    let insert_at = module
        .body
        .iter()
        .take_while(|item| is_directive_prologue_item(item))
        .count();
    let module_ident = Ident::new(
        "module".into(),
        DUMMY_SP,
        SyntaxContext::empty().apply_mark(unresolved_mark),
    );
    module.body.insert(
        insert_at,
        ModuleItem::Stmt(Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(Expr::Assign(AssignExpr {
                span: DUMMY_SP,
                op: AssignOp::Assign,
                left: AssignTarget::Simple(SimpleAssignTarget::Member(MemberExpr {
                    span: DUMMY_SP,
                    obj: Box::new(Expr::Ident(module_ident)),
                    prop: MemberProp::Ident(IdentName::new("exports".into(), DUMMY_SP)),
                })),
                right: Box::new(Expr::Object(ObjectLit {
                    span: DUMMY_SP,
                    props: Vec::new(),
                })),
            })),
        })),
    );
}

fn is_directive_prologue_item(item: &ModuleItem) -> bool {
    matches!(item, ModuleItem::Stmt(Stmt::Expr(statement))
        if matches!(statement.expr.as_ref(), Expr::Lit(swc_core::ecma::ast::Lit::Str(_))))
}

/// Replacing webpack's initial exports object at the first executable
/// statement is unobservable unless the factory body retained an alias to the
/// original object. Reject every explicit wrapper binding and the dynamic
/// mechanisms that can recover one.
fn observes_webpack_factory_environment(module: &Module, unresolved_mark: Mark) -> bool {
    let mut observer = WebpackFactoryEnvironmentObserver {
        unresolved_mark,
        observed: false,
    };
    module.visit_with(&mut observer);
    if observer.observed {
        return true;
    }
    if module_stmts_have_function_level_special_bindings(module) {
        return true;
    }

    let mut eval = DirectEvalAnalyzer::default();
    module.visit_with(&mut eval);
    eval.unknown_direct_eval || !eval.known_direct_eval_sources.is_empty()
}

struct WebpackFactoryEnvironmentObserver {
    unresolved_mark: Mark,
    observed: bool,
}

impl Visit for WebpackFactoryEnvironmentObserver {
    fn visit_ident(&mut self, ident: &Ident) {
        if is_unresolved_ident(ident, "module", self.unresolved_mark)
            || is_unresolved_ident(ident, "exports", self.unresolved_mark)
        {
            self.observed = true;
        }
    }
}

/// Multi-module unpack with cross-module late pass.
///
/// Phase 1: obtain a resolved AST + through-UnEsm range + fact recovery/collection
/// Phase 2: resume the retained AST + late pass + UnTemplateLiteral-through-UnReturn range
///
/// Normal unpack retains each Phase 1 AST together with its `Globals` and
/// unresolved mark across the facts barrier. Webpack5 detectors can provide an
/// already-resolved, bundler-normalized AST, avoiding their intermediate emit
/// and this phase's parse/resolver. Source-map mode deliberately materializes
/// detector ASTs and uses the parser path so output mappings keep parser-owned
/// source coordinates.
///
/// # Best-effort semantics
///
/// Individual extracted modules that fail to parse are preserved as raw code
/// rather than aborting the entire unpack. The extraction process can
/// produce module bodies that are not valid standalone JS (e.g. incomplete
/// slicing, runtime wrapper residue). Hard-failing on those would discard
/// all other successfully extracted modules, which is worse for both
/// interactive and automated users. Failures are reported via
/// `PreparedUnpackOutput::warnings` so callers can surface them without silent
/// swallowing.
///
/// Both phases run via rayon. On targets without threading support, Rayon falls
/// back to sequential execution.
#[cfg(test)]
pub(super) fn unpack_multi_module(
    modules: Vec<crate::unpacker::UnpackedModule>,
    options: DecompileOptions,
) -> Result<PreparedUnpackOutput> {
    let modules = modules
        .into_iter()
        .map(PreparedUnpackModule::plain)
        .collect();
    unpack_multi_module_with_plan(modules, NumericRewritePlan::default(), options, &[], None)
}

pub(super) fn unpack_multi_module_with_plan(
    mut modules: Vec<PreparedUnpackModule>,
    numeric_rewrite_plan: NumericRewritePlan,
    options: DecompileOptions,
    origins: &[Option<Arc<InputOrigin>>],
    interop: Option<&super::InteropSink>,
) -> Result<PreparedUnpackOutput> {
    if options.sourcemap.is_some() {
        bail!(
            "input source maps are not supported with unpacking because extracted module coordinates differ from bundle coordinates; use --emit-source-map for output maps"
        );
    }
    let span = tracing::info_span!("unpack_multi_module", count = modules.len());
    let _enter = span.enter();
    let report_import_cycle_warnings = modules
        .iter()
        .all(|module| module.report_import_cycle_warnings);
    let reserved_public_paths = modules
        .iter()
        .filter(|module| module.reserved_public_path)
        .map(|module| module.module.filename.clone())
        .collect::<crate::collections::HashSet<_>>();

    // Stash per-module provenance (byte ranges into the original input)
    // keyed by provisional filename. Final provenance is built after dead
    // module elimination and filename recovery, so only surviving modules
    // appear with their final names.
    let provenance_by_provisional: crate::collections::HashMap<
        String,
        (
            Option<PreparedInputId>,
            Vec<(u32, u32)>,
            Vec<(u32, u32)>,
            bool,
        ),
    > = modules
        .iter()
        .map(|prepared| {
            (
                prepared.module.filename.clone(),
                (
                    prepared.input,
                    prepared.module.source_ranges.clone(),
                    prepared.module.inspection_context_ranges.clone(),
                    prepared.module.is_entry,
                ),
            )
        })
        .collect();

    // Parse the sourcemap once before the loop.
    let parsed_sourcemap = options
        .sourcemap
        .as_deref()
        .map(parse_sourcemap)
        .transpose()?;
    let can_reuse_phase1_ast = parsed_sourcemap.is_none() && !options.emit_source_map;
    // Filename recovery from provenance markers is a readability rewrite, gated
    // to standard+ like other speculative recovery.
    let recover_filenames = !matches!(options.level, RewriteLevel::Minimal);
    // Dead helper-module elimination is dead-code cleanup: it relies on the
    // binding->side-effect import downgrade that only runs when DCE is on, and
    // dropping a module is structural, so gate it to standard+ as well.
    let eliminate_dead_modules =
        options.dce_mode.is_enabled() && !matches!(options.level, RewriteLevel::Minimal);

    // Phase 1: collect facts, in two halves. Phase 1a runs the rule range up
    // to `UnEsm` on each module and reads its require edges; Phase 1b runs
    // `UnEsm` and extracts the import/export facts. Every module finishes
    // Phase 1a before any runs `UnEsm` (barrier 0). For normal unpacking,
    // keep the normalized AST so Phase 2 can resume after the facts barrier.
    // Source-map mode still reparses in Phase 2 because sourcemap renaming
    // depends on the original parser SourceMap.
    let prepare_module = |unpacked: &mut PreparedUnpackModule| -> Phase1a {
        if let Some(failure) = unpacked.detector_failure {
            // The factory loader's later lifetime is unproven. Running any
            // require-aware transform or fact collector over this body could
            // turn an ordinary local call into a fabricated module edge.
            unpacked.prepared = None;
            return Phase1a::Done(Box::new(Phase1Module {
                filename: unpacked.module.filename.clone(),
                facts: crate::facts::ModuleFacts::default(),
                prepared: None,
                warning: Some(detector_failure_warning(&unpacked.module.filename, failure)),
                input_parse_warnings: Vec::new(),
                suggested_filename: None,
            }));
        }
        let (globals, prepared_input, input_parse_warnings) = match unpacked.prepared.take() {
            Some(prepared) => {
                let filename = unpacked.module.filename.clone();
                let errors = prepared
                    .recoverable_parse_errors
                    .into_iter()
                    .map(|error| ParseDiagnostic {
                        filename: filename.clone(),
                        line: error.line,
                        column: error.column,
                        message: error.message,
                    })
                    .collect::<Vec<_>>();
                let warnings = collect_input_parse_warnings(&errors);
                (
                    prepared.globals,
                    Some((prepared.module, prepared.unresolved_mark)),
                    warnings,
                )
            }
            None => (Globals::new(), None, Vec::new()),
        };
        let prepared = GLOBALS.set(&globals, || {
            let (mut module, unresolved_mark) = match prepared_input {
                Some((mut module, _detector_mark)) => {
                    // The detector's contexts describe its own surgery; give
                    // the rules resolver-derived ones.
                    let span = tracing::info_span!("phase1: reresolve prepared");
                    let _enter = span.enter();
                    let unresolved_mark = crate::unpacker::resolve_prepared_module(&mut module);
                    (module, unresolved_mark)
                }
                None => {
                    let cm: Lrc<SourceMap> = Default::default();
                    let mut module = {
                        let span = tracing::info_span!("phase1: parse");
                        let _enter = span.enter();
                        match parse_js(
                            &unpacked.module.code,
                            &unpacked.module.filename,
                            cm.clone(),
                        ) {
                            Ok(module) => module,
                            Err(e) => {
                                return Err(UnpackWarning::new(
                                    unpacked.module.filename.clone(),
                                    UnpackWarningKind::FactCollectionParseFailed,
                                    format!(
                                        "parse failed during fact collection, using empty facts: {e}"
                                    ),
                                ));
                            }
                        }
                    };
                    let unresolved_mark = {
                        let span = tracing::info_span!("phase1: resolver");
                        let _enter = span.enter();
                        let unresolved_mark = Mark::new();
                        let top_level_mark = Mark::new();
                        module.visit_mut_with(&mut resolver(
                            unresolved_mark,
                            top_level_mark,
                            false,
                        ));
                        unresolved_mark
                    };
                    (module, unresolved_mark)
                }
            };
            // Harvest the original filename from provenance markers before any
            // rule mutates the AST. The marker is still a props-object property
            // here (UnJsx has not run), so this does not depend on JSX recovery.
            let suggested_filename = if recover_filenames {
                harvest_suggested_filename(&module)
            } else {
                None
            };
            apply_filename_rewrites(
                &mut module,
                unresolved_mark,
                unpacked.filename_rewrite.as_ref(),
            );
            let residual_warning = apply_numeric_rewrites(
                &mut module,
                unresolved_mark,
                unpacked.numeric_rewrite.as_ref(),
                &numeric_rewrite_plan,
            );
            normalize_webpack_commonjs_runtime(
                &mut module,
                unresolved_mark,
                unpacked.webpack_commonjs_runtime,
                unpacked.webpack_numeric_module_id,
                unpacked.webpack_legacy_module_i,
            );
            restore_webpack_factory_default(
                &mut module,
                unresolved_mark,
                unpacked.implicit_commonjs_default_object,
                unpacked.webpack_commonjs_runtime,
                &unpacked.module.filename,
            );
            let commonjs_default_object =
                collect_commonjs_default_object(&module, unresolved_mark);
            let commonjs_default_attached_properties =
                collect_commonjs_default_attached_properties(&module, unresolved_mark);
            let require_returns_exports_object =
                collect_require_returns_exports_object(&module, unresolved_mark);
            let replaces_module_exports =
                collect_replaces_module_exports(&module, unresolved_mark);
            let commonjs_value_is_default =
                collect_commonjs_value_is_default(&module, unresolved_mark)
                    || (unpacked.webpack_commonjs_runtime
                        && is_normalized_commonjs_default(&module))
                    || collect_default_object_tail(&module, unresolved_mark);
            let input_is_es_module =
                collect_input_is_es_module(&module, unresolved_mark, commonjs_value_is_default);
            let es_source_evidence = if options.diagnostics {
                crate::kept_require::es_source_evidence(&module, unresolved_mark)
            } else {
                Vec::new()
            };
            let marks_es_module = collect_marks_es_module(&module, unresolved_mark);
            let exports_whole_value = exports_only_whole_value(&module, unresolved_mark);
            let required_whole_value = required_whole_value(&module, unresolved_mark);
            let raw = RawCommonJsFacts {
                commonjs_default_object,
                commonjs_default_attached_properties,
                require_returns_exports_object,
                input_is_es_module,
                commonjs_value_is_default,
                replaces_module_exports,
                es_source_evidence,
                marks_es_module,
            };
            let require_edges = {
                let span = tracing::info_span!("phase1: rules");
                let _enter = span.enter();
                run_rules_before_un_esm(
                    &mut module,
                    unresolved_mark,
                    options.level,
                    &unpacked.module.filename,
                )
            };
            let exports_object_writable = !module
                .body
                .iter()
                .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
                && !is_webpack_harmony_module(&module, unresolved_mark);
            let helpers = {
                let span = tracing::info_span!("phase1: provider helpers");
                let _enter = span.enter();
                ProviderHelpers::collect(&module, unresolved_mark)
            };
            // `UnEsm` lowers esbuild's `module.exports = __toCommonJS(ns)` to
            // the namespace's names, so the module does not export the
            // assigned value as its default.
            let lowers_namespace = lowers_to_commonjs_namespace(&module, unresolved_mark);
            let exports_whole_value = exports_whole_value && !lowers_namespace;
            let required_whole_value = required_whole_value && !lowers_namespace;
            let passthrough = exports_object_writable
                .then(|| passthrough_source(&module, unresolved_mark))
                .flatten();
            let exports_from_requires = if exports_object_writable {
                exports_from_requires(&module, unresolved_mark)
            } else {
                Vec::new()
            };
            Ok((
                module,
                unresolved_mark,
                raw,
                require_edges,
                exports_object_writable,
                helpers,
                exports_whole_value,
                required_whole_value,
                passthrough,
                exports_from_requires,
                residual_warning,
                suggested_filename,
            ))
        });
        match prepared {
            Ok((
                module,
                unresolved_mark,
                raw,
                require_edges,
                exports_object_writable,
                helpers,
                exports_whole_value,
                required_whole_value,
                passthrough,
                exports_from_requires,
                warning,
                suggested_filename,
            )) => Phase1a::Pending(Box::new(Phase1aModule {
                filename: unpacked.module.filename.clone(),
                size: unpacked.module.code.len(),
                globals,
                module,
                unresolved_mark,
                raw,
                require_edges,
                exports_object_writable,
                helpers,
                exports_whole_value,
                required_whole_value,
                passthrough,
                exports_from_requires,
                warning,
                input_parse_warnings,
                suggested_filename,
            })),
            Err(warning) => Phase1a::Done(Box::new(Phase1Module {
                filename: unpacked.module.filename.clone(),
                facts: crate::facts::ModuleFacts::default(),
                prepared: None,
                warning: Some(warning),
                input_parse_warnings,
                suggested_filename: None,
            })),
        }
    };

    // Phase 1b: `UnEsm`, then the module facts. Each module's AST resumes
    // where Phase 1a left it.
    let finish_facts = |pending: Box<Phase1aModule>, plan: &InterfacePlan| -> Phase1Module {
        let Phase1aModule {
            filename,
            size: _,
            globals,
            mut module,
            unresolved_mark,
            raw,
            require_edges,
            exports_object_writable: _,
            helpers: _,
            exports_whole_value: _,
            required_whole_value: _,
            passthrough: _,
            exports_from_requires: _,
            warning,
            input_parse_warnings,
            suggested_filename,
        } = *pending;
        let (facts, prepared_parts) = GLOBALS.set(&globals, || {
            let import_tags = {
                let span = tracing::info_span!("phase1: UnEsm");
                let _enter = span.enter();
                run_un_esm_range(
                    &mut module,
                    unresolved_mark,
                    options.level,
                    &filename,
                    Some(plan),
                )
            };
            // A written provider may become ESM only in `UnEsm` above, which
            // read the plan: one it left CommonJS stays so through the late
            // recovery, here and in Phase 2.
            let keep_commonjs = plan.is_written(&filename)
                && !module
                    .body
                    .iter()
                    .any(|item| matches!(item, ModuleItem::ModuleDecl(_)));
            let settled = is_settled_es_module(&module, unresolved_mark);
            let recover_late = |module: &mut Module| {
                recover_late_esm_from_factory_iifes(
                    module,
                    unresolved_mark,
                    options.level,
                    &filename,
                    PipelineMode::UnpackLatePrediction,
                    LateEsmRecoveryOptions::default()
                        .with_interface_plan(plan)
                        .with_keep_commonjs(keep_commonjs),
                );
            };
            // The facts predict the module Phase 2 ends with. Its late ESM
            // recovery (UnIife, factory-IIFE rewrites, renames) exposes the
            // import/export declarations the facts read. A module `UnEsm`
            // left CommonJS, in whole or in part, can convert there after
            // rules that expose the shape the recovery converts, so its
            // prediction runs those rules first; a settled ES module has
            // nothing left to convert. The rule
            // range rewrites helper bodies (`UnConditionals`,
            // `ArrowFunction`, DCE) into shapes the helper detectors do not
            // read, so the helper facts come from the late recovery alone.
            //
            // Phase 2 resumes from the through-UnEsm barrier state and runs
            // those steps itself at `options.level`, so it needs the
            // unmodified `module`. When the AST will be reused (no-sourcemap
            // path), clone before recovering for facts, and keep the AST the
            // rule range left (`keep_ranged`): Phase 2 adopts it when its
            // passes leave the module unchanged and its own run of the range
            // would match (`range_ignores_facts`). A module with imports is
            // left out: the provider repairs usually rewrite them, and the
            // kept AST would only add to peak memory. When the AST won't be
            // reused (sourcemap path discards `module`), recover in place.
            let predict = |module: &mut Module, keep_ranged: bool| {
                let span = tracing::info_span!("phase1: fact recovery");
                let _enter = span.enter();
                if settled {
                    recover_late(module);
                    return (collect_module_facts(module), None);
                }
                // The helper facts and the rule range work on separate
                // copies of the barrier AST, so they run in parallel: when
                // the largest module is unsettled, its prediction is the
                // longest task of Phase 1b. The range, the longer of the
                // two, starts on this thread; another worker takes the
                // helper facts when one is idle.
                let mut helper_module = module.clone();
                let ((), helpers) = GLOBALS.with(|globals| {
                    rayon::join(
                        || {
                            GLOBALS.set(globals, || {
                                run_rules_before_late_recovery(
                                    module,
                                    unresolved_mark,
                                    options.level,
                                    options.dce_mode,
                                    &filename,
                                    PipelineMode::UnpackLatePrediction,
                                    None,
                                )
                            })
                        },
                        || {
                            GLOBALS.set(globals, || {
                                recover_late(&mut helper_module);
                                HelperFacts::collect(&helper_module)
                            })
                        },
                    )
                });
                let ranged = keep_ranged.then(|| module.clone());
                recover_late(module);
                (collect_module_facts_with_helpers(module, helpers), ranged)
            };
            let (mut facts, prepared) = if can_reuse_phase1_ast {
                let keep_ranged = !module
                    .body
                    .iter()
                    .any(|item| matches!(item, ModuleItem::ModuleDecl(ModuleDecl::Import(_))));
                let (mut facts, ranged) = predict(&mut module.clone(), keep_ranged);
                // Class recovery runs on this pre-late AST. Probing the
                // late-renamed clone misses IIFEs Phase 2 still converts.
                facts.import_call_edges = crate::rules::collect_import_call_edges(
                    &module,
                    unresolved_mark,
                    options.level,
                );
                (facts, Some((module, ranged, unresolved_mark, import_tags)))
            } else {
                // Edges read the pre-late AST, as above. Collect them before
                // recovering in place.
                let import_call_edges = crate::rules::collect_import_call_edges(
                    &module,
                    unresolved_mark,
                    options.level,
                );
                let (mut facts, _) = predict(&mut module, false);
                facts.import_call_edges = import_call_edges;
                (facts, None)
            };
            raw.apply(&mut facts);
            // `UnEsm` read each require of a passthrough as a require of the
            // module its chain ends at; so do the passes after the barrier.
            let mut require_edges = require_edges;
            require_edges.alias_sources(&plan.passthrough_sources(&filename));
            facts.require_edges = require_edges;
            (facts, prepared)
        });
        let prepared = prepared_parts.map(|(module, ranged, unresolved_mark, import_tags)| {
            Phase1PreparedModule {
                globals,
                module,
                ranged,
                import_tags,
                unresolved_mark,
            }
        });
        Phase1Module {
            filename,
            facts,
            prepared,
            warning,
            input_parse_warnings,
            suggested_filename,
        }
    };

    let phase1a: Vec<Phase1a> = {
        let span = tracing::info_span!("phase1a_rules_before_un_esm");
        let _enter = span.enter();
        par_map_largest_first(
            modules.iter_mut().collect(),
            |module: &&mut PreparedUnpackModule| module.module.code.len(),
            prepare_module,
        )
    };

    // Barrier 0: the plan from every module's require edges.
    let interface_plan = {
        let span = tracing::info_span!("barrier0_interface_plan");
        let _enter = span.enter();
        InterfacePlan::build(phase1a.iter().filter_map(|module| match module {
            Phase1a::Done(_) => None,
            Phase1a::Pending(pending) => Some(PlanModule {
                filename: &pending.filename,
                edges: &pending.require_edges,
                exports_object_writable: pending.exports_object_writable,
                helpers: pending.helpers.as_ref(),
                exports_whole_value: pending.exports_whole_value,
                required_whole_value: pending.required_whole_value,
                passthrough: pending.passthrough.clone(),
                exports_from_requires: pending.exports_from_requires.clone(),
            }),
        }))
    };

    let phase1: Vec<Phase1Module> = {
        let span = tracing::info_span!("phase1b_collect_facts");
        let _enter = span.enter();
        par_map_largest_first(
            phase1a,
            |module: &Phase1a| match module {
                Phase1a::Done(_) => 0,
                Phase1a::Pending(pending) => pending.size,
            },
            |module| match module {
                Phase1a::Done(done) => *done,
                Phase1a::Pending(pending) => finish_facts(pending, &interface_plan),
            },
        )
    };

    let mut module_facts = ModuleFactsMap::new();
    let mut prepared_modules = Vec::with_capacity(phase1.len());
    let mut prepared_parse_warnings = Vec::with_capacity(phase1.len());
    let mut warnings = Vec::new();
    let mut rename_entries = Vec::with_capacity(phase1.len());
    for phase1_module in phase1 {
        rename_entries.push((
            phase1_module.filename.clone(),
            phase1_module.suggested_filename,
        ));
        module_facts.insert(&phase1_module.filename, phase1_module.facts);
        prepared_modules.push(phase1_module.prepared);
        prepared_parse_warnings.push(phase1_module.input_parse_warnings);
        if let Some(w) = phase1_module.warning {
            warnings.push(w);
        }
    }

    let call_required_plan = crate::rules::CallRequiredPlan::build(&module_facts);

    // Cross-module barrier: resolve recovered filenames into a final rename
    // table. Kept separate from the fact map so the pipeline (facts, numeric
    // rewrites, namespace decomposition) keeps operating on provisional names;
    // only the final emit step swaps names and rewrites import sources.
    let rename_map = if recover_filenames {
        build_rename_map(&rename_entries, &reserved_public_paths)
    } else {
        crate::collections::HashMap::default()
    };

    // Phase 2: output pipeline with late pass. Each module is parsed from
    // the original source only when Phase 1 failed to prepare an AST; otherwise
    // it continues from the Phase 1 normalized AST after the facts barrier.
    let facts_ref = &module_facts;
    let call_required_plan_ref = &call_required_plan;
    let plan_ref = &interface_plan;
    let sm_ref = &parsed_sourcemap;
    let rename_ref = &rename_map;
    let phase2_inputs: Vec<_> = modules
        .into_iter()
        .zip(prepared_modules)
        .zip(prepared_parse_warnings)
        .map(|((module, prepared), warnings)| (module, prepared, warnings))
        .collect();

    let decompile_module = |(unpacked, prepared, prepared_parse_warnings): (
        PreparedUnpackModule,
        Option<Phase1PreparedModule>,
        Vec<UnpackWarning>,
    )|
     -> (
        String,
        String,
        Vec<UnpackWarning>,
        Option<ImportReport>,
        Option<String>,
        Option<LinkReport>,
    ) {
        if unpacked.detector_failure.is_some() {
            return (
                unpacked.module.filename,
                unpacked.module.code,
                Vec::new(),
                None,
                None,
                None,
            );
        }
        let run_phase2_tail = |mut module: Module,
                               ranged: Option<Module>,
                               cm: Lrc<SourceMap>,
                               unresolved_mark: Mark,
                               mut import_tags: ImportTags,
                               input_parse_warnings: Vec<UnpackWarning>|
         -> Result<(
            String,
            Option<String>,
            Vec<UnpackWarning>,
            Option<ImportReport>,
            LinkReport,
        )> {
            let rules_span = tracing::info_span!("phase2: rules");
            let rules_enter = rules_span.enter();
            // The imports `UnEsm` made, before the barrier passes repair them.
            let mut un_esm_imports = interop.map(|_| import_forms(&module, &import_tags));
            // A copy of the barrier AST, made only while the prediction's run
            // of the rule range can stand in for this one: the passes below
            // must then leave the module as it was.
            let ranged = ranged.filter(|_| {
                range_ignores_facts(
                    &module,
                    LateRangeFacts {
                        module_facts: facts_ref,
                        call_required_plan: call_required_plan_ref,
                    },
                    &unpacked.module.filename,
                )
            });
            let barrier = ranged.as_ref().map(|_| module.clone());
            // Late pass at the barrier
            run_provider_import_repair(
                &mut module,
                facts_ref,
                Some(&unpacked.module.filename),
                &mut import_tags,
            );
            run_provider_namespace_repair(
                &mut module,
                facts_ref,
                Some(&unpacked.module.filename),
                unresolved_mark,
                true,
                &import_tags,
            );
            run_cross_module_lowered_dynamic_imports(
                &mut module,
                facts_ref,
                Some(&unpacked.module.filename),
                unresolved_mark,
            );
            run_cross_module_webpack_lazy_imports(
                &mut module,
                facts_ref,
                Some(&unpacked.module.filename),
                unresolved_mark,
            );
            // This pass turns `x = helper(require("./p"))` declarators into
            // imports: tag them as `UnEsm` tags its own.
            let declarator_requires = declarator_require_calls(&module, unresolved_mark);
            run_cross_module_interop_imports(
                &mut module,
                facts_ref,
                Some(&unpacked.module.filename),
                unresolved_mark,
            );
            tag_declarator_imports(&module, &declarator_requires, &mut import_tags);
            import_unproven_helper_requires(
                &mut module,
                facts_ref,
                Some(&unpacked.module.filename),
                unresolved_mark,
            );
            run_namespace_decomposition_with_tags(
                &mut module,
                facts_ref,
                Some(&unpacked.module.filename),
                unresolved_mark,
                &mut import_tags,
            );
            downgrade_unused_synthetic_imports(&mut module, &import_tags);
            if let Some(interop) = interop {
                interop.record(
                    &unpacked.module.filename,
                    facts_ref.get(&unpacked.module.filename),
                    plan_ref,
                    &module,
                    &import_tags,
                    un_esm_imports
                        .take()
                        .unwrap_or_else(|| import_forms(&module, &import_tags)),
                );
            }
            // Late helper-through-UnReturn range. The prediction ran it on
            // the same barrier AST; when the passes above left that AST
            // unchanged and the range reads no facts here, its result is
            // the one this run would produce.
            match (ranged, barrier) {
                (Some(ranged), Some(barrier)) if barrier == module => {
                    #[cfg(debug_assertions)]
                    assert_adopted_range_matches(
                        &ranged,
                        barrier,
                        unresolved_mark,
                        options.level,
                        options.dce_mode,
                        &unpacked.module.filename,
                        LateRangeFacts {
                            module_facts: facts_ref,
                            call_required_plan: call_required_plan_ref,
                        },
                    );
                    module = ranged;
                }
                _ => run_rules_before_late_recovery(
                    &mut module,
                    unresolved_mark,
                    options.level,
                    options.dce_mode,
                    &unpacked.module.filename,
                    PipelineMode::UnpackPhase2,
                    Some(LateRangeFacts {
                        module_facts: facts_ref,
                        call_required_plan: call_required_plan_ref,
                    }),
                ),
            }
            // UnIife2 can expose webpack export helpers that were hidden in
            // factory wrappers at the Stage 2 barrier. Recover just that ESM
            // shape without restoring the old full second pass.
            recover_late_esm_from_factory_iifes(
                &mut module,
                unresolved_mark,
                options.level,
                &unpacked.module.filename,
                PipelineMode::UnpackPhase2,
                LateEsmRecoveryOptions::default()
                    .with_interface_plan(plan_ref)
                    .with_keep_commonjs(
                        plan_ref.is_written(&unpacked.module.filename)
                            && facts_ref
                                .get(&unpacked.module.filename)
                                .is_some_and(|facts| facts.stays_commonjs),
                    ),
            );
            module.visit_mut_with(&mut UnOptionalChaining::new(unresolved_mark, options.level));
            module.visit_mut_with(&mut UnConditionalsAssignmentOnly);
            // Unpack counterpart of the pipeline's UnConditionals2 cleanup pass.
            module.visit_mut_with(&mut UnConditionals::with_nested_actions());

            // Source-map-enhanced passes
            if let Some(sm) = sm_ref {
                module.visit_mut_with(&mut ImportDedup);
                apply_sourcemap_renames(&mut module, sm, &cm, unresolved_mark);
                module.visit_mut_with(&mut UnImportRename::new(unresolved_mark));
            }

            // A module that still carries a function-level return at module
            // scope after a declined restoration would not parse as ESM.
            // Preserve the extracted body and report the module as failed
            // instead of claiming a successful unpack over invalid output.
            if restore_lifted_function_boundary(
                &mut module,
                unpacked.restore_lifted_function_boundary,
            ) == BoundaryRestoration::Declined
            {
                let mut fallback_warnings = input_parse_warnings.clone();
                fallback_warnings.push(UnpackWarning::new(
                    &unpacked.module.filename,
                    UnpackWarningKind::DecompileFailed,
                    "lifted function boundary could not be restored around a module-scope return; preserving the extracted body",
                ));
                return Ok((
                    unpacked.module.code.clone(),
                    None,
                    fallback_warnings,
                    None,
                    LinkReport::default(),
                ));
            }
            drop(rules_enter);
            drop(rules_span);

            let mut diag_warnings = input_parse_warnings;
            // Import sources are still provisional here, so they resolve
            // against the same fact keys the plan was built from.
            for (source, imported) in call_required_plan_ref.mispredicted_calls(
                facts_ref,
                &unpacked.module.filename,
                &module,
            ) {
                diag_warnings.push(UnpackWarning::new(
                    &unpacked.module.filename,
                    UnpackWarningKind::CrossModuleClassCall,
                    format!(
                        "`{imported}` from `{source}` is still invoked with .call/.apply, but it was expected to become super(); the provider may now be a class that cannot be called without `new`"
                    ),
                ));
            }

            // After every rule: a later recovery can still turn the module
            // into ESM, which reads the provider through an import instead.
            let kept_requires_of_esm = run_kept_require_default_reads(
                &mut module,
                facts_ref,
                Some(&unpacked.module.filename),
                unresolved_mark,
            );

            diag_warnings.extend(collect_commonjs_export_residual_warnings(
                &module,
                unresolved_mark,
                &unpacked.module.filename,
            ));
            diag_warnings.extend(collect_commonjs_require_residual_warnings(
                &module,
                unresolved_mark,
                &unpacked.module.filename,
            ));
            diag_warnings.extend(collect_webpack_runtime_residual_warnings(
                &module,
                unresolved_mark,
                &unpacked.module.filename,
            ));
            diag_warnings.extend(collect_commonjs_require_of_esm_warnings(
                kept_requires_of_esm,
                &module,
                unresolved_mark,
                &unpacked.module.filename,
            ));
            diag_warnings.extend(collect_commonjs_exports_escape_warnings(
                facts_ref,
                plan_ref,
                &unpacked.module.filename,
            ));
            if options.diagnostics {
                if let Some(own) = facts_ref.get(&unpacked.module.filename) {
                    diag_warnings.extend(collect_es_source_kept_commonjs_warnings(
                        &module,
                        unresolved_mark,
                        &own.es_source_evidence,
                        &unpacked.module.filename,
                    ));
                }
            }

            // Final, isolated remap: rewrite import-source strings that point
            // at modules renamed via recovered filenames. Runs after every
            // fact-driven pass so the fact map stays keyed by provisional names.
            if !rename_ref.is_empty() {
                rewrite_import_sources(
                    &mut module,
                    &unpacked.module.filename,
                    rename_ref,
                    unresolved_mark,
                );
            }

            let final_filename = rename_ref
                .get(&unpacked.module.filename)
                .map(|s| s.as_str())
                .unwrap_or(&unpacked.module.filename);
            if !matches!(options.level, RewriteLevel::Minimal) {
                crate::rules::strip_redundant_sentry_source_file(&mut module, final_filename);
            }
            // Dead-helper DCE may still prune bare side-effect imports after
            // this parallel phase. Do not use such an import as the only
            // Module proof when that later mutation is enabled.
            strip_redundant_module_use_strict(&mut module, final_filename, !eliminate_dead_modules);

            // Collect the dead-module-elimination report after output
            // finalization so redundant module directives do not make an
            // otherwise pure generated helper appear side-effectful.
            let report = if eliminate_dead_modules {
                let is_helper = facts_ref
                    .get(&unpacked.module.filename)
                    .is_some_and(|facts| facts.is_helper_module);
                Some(collect_import_report(
                    &module,
                    unpacked.module.is_entry,
                    is_helper,
                    unpacked.external_consumers,
                ))
            } else {
                None
            };
            let link = collect_link_report(&module);

            {
                let span = tracing::info_span!("phase2: fixer");
                let _enter = span.enter();
                apply_fixer(&mut module)?;
            }
            let (code, srcmap_json) = if options.emit_source_map {
                let (code, srcmap_buf) = {
                    let span = tracing::info_span!("phase2: emit");
                    let _enter = span.enter();
                    print_js_with_srcmap(&module, cm.clone())?
                };
                let origin = unpacked
                    .input
                    .and_then(|input| origins.get(input.index())?.as_deref());
                let map_json = match origin {
                    // A module without input offsets (fully synthesized) still
                    // maps into its origin: every position is left unmapped.
                    Some(origin) => build_composed_output_sourcemap(
                        &srcmap_buf,
                        &cm,
                        final_filename,
                        InputOffsets::of(&unpacked.module).unwrap_or(InputOffsets::Printed(&[])),
                        origin,
                    )?,
                    None => build_output_sourcemap(&srcmap_buf, &cm, final_filename)?,
                };
                (code, Some(map_json))
            } else {
                let code = {
                    let span = tracing::info_span!("phase2: emit");
                    let _enter = span.enter();
                    print_js(&module, cm)?
                };
                (code, None)
            };

            if options.diagnostics {
                diag_warnings.extend(collect_output_diagnostics(&code, &unpacked.module.filename));
            }

            Ok((code, srcmap_json, diag_warnings, report, link))
        };

        let result = if let Some(prepared) = prepared {
            let Phase1PreparedModule {
                globals,
                module,
                ranged,
                unresolved_mark,
                import_tags,
            } = prepared;
            GLOBALS.set(&globals, || {
                let cm: Lrc<SourceMap> = Default::default();
                run_phase2_tail(
                    module,
                    ranged,
                    cm,
                    unresolved_mark,
                    import_tags,
                    prepared_parse_warnings,
                )
            })
        } else {
            GLOBALS.set(&Default::default(), || {
                let cm: Lrc<SourceMap> = Default::default();
                let parsed = {
                    let span = tracing::info_span!("phase2: parse");
                    let _enter = span.enter();
                    parse_js_with_recovery(
                        &unpacked.module.code,
                        &unpacked.module.filename,
                        cm.clone(),
                    )?
                };
                let mut module = parsed.module;
                let unresolved_mark = {
                    let span = tracing::info_span!("phase2: resolver");
                    let _enter = span.enter();
                    let unresolved_mark = Mark::new();
                    let top_level_mark = Mark::new();
                    module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
                    unresolved_mark
                };
                apply_filename_rewrites(
                    &mut module,
                    unresolved_mark,
                    unpacked.filename_rewrite.as_ref(),
                );
                // Phase 1 already reported the calls this keeps.
                let _ = apply_numeric_rewrites(
                    &mut module,
                    unresolved_mark,
                    unpacked.numeric_rewrite.as_ref(),
                    &numeric_rewrite_plan,
                );
                normalize_webpack_commonjs_runtime(
                    &mut module,
                    unresolved_mark,
                    unpacked.webpack_commonjs_runtime,
                    unpacked.webpack_numeric_module_id,
                    unpacked.webpack_legacy_module_i,
                );
                restore_webpack_factory_default(
                    &mut module,
                    unresolved_mark,
                    unpacked.implicit_commonjs_default_object,
                    unpacked.webpack_commonjs_runtime,
                    &unpacked.module.filename,
                );

                // Through-UnEsm range, which records its own import tags on
                // this AST, as Phase 1 did on its own.
                let import_tags = {
                    let span = tracing::info_span!("phase2: early rules");
                    let _enter = span.enter();
                    apply_rules(
                        &mut module,
                        unresolved_mark,
                        RulePipelineOptions::before("UnEsm")
                            .with_mode(PipelineMode::UnpackPhase1)
                            .with_rewrite_level(options.level)
                            .with_current_filename(&unpacked.module.filename),
                    );
                    run_un_esm_range(
                        &mut module,
                        unresolved_mark,
                        options.level,
                        &unpacked.module.filename,
                        Some(plan_ref),
                    )
                };

                let input_parse_warnings = collect_input_parse_warnings(&parsed.recoverable_errors);
                run_phase2_tail(
                    module,
                    None,
                    cm,
                    unresolved_mark,
                    import_tags,
                    input_parse_warnings,
                )
            })
        };

        match result {
            Ok((code, srcmap_json, mut diag_warnings, report, link)) => {
                let out_filename = rename_ref
                    .get(&unpacked.module.filename)
                    .cloned()
                    .unwrap_or(unpacked.module.filename);
                if let Some(note) = unpacked.detector_note {
                    diag_warnings.push(detector_note_warning(&out_filename, note));
                }
                (
                    out_filename,
                    code,
                    diag_warnings,
                    report,
                    srcmap_json,
                    Some(link),
                )
            }
            Err(e) => (
                unpacked.module.filename.clone(),
                unpacked.module.code,
                vec![UnpackWarning::new(
                    unpacked.module.filename,
                    UnpackWarningKind::DecompileFailed,
                    format!("decompile failed, preserving raw code: {e}"),
                )],
                None,
                None,
                None,
            ),
        }
    };

    let triples: Vec<_> = {
        let span = tracing::info_span!("phase2_decompile_modules");
        let _enter = span.enter();
        par_map_largest_first(
            phase2_inputs,
            |(unpacked, _, _): &(
                PreparedUnpackModule,
                Option<Phase1PreparedModule>,
                Vec<UnpackWarning>,
            )| unpacked.module.code.len(),
            decompile_module,
        )
    };

    // Separate source maps from the tuples before dead-module elimination.
    let mut srcmap_by_filename: crate::collections::HashMap<String, String> =
        crate::collections::HashMap::default();
    let mut link_by_filename: crate::collections::HashMap<String, LinkReport> =
        crate::collections::HashMap::default();
    let triples_for_dead: Vec<(String, String, Vec<UnpackWarning>, Option<ImportReport>)> = triples
        .into_iter()
        .map(|(filename, code, warns, report, srcmap, link)| {
            if let Some(map_json) = srcmap {
                srcmap_by_filename.insert(filename.clone(), map_json);
            }
            if let Some(link) = link {
                link_by_filename.insert(filename.clone(), link);
            }
            (filename, code, warns, report)
        })
        .collect();

    let mut modules = Vec::with_capacity(triples_for_dead.len());
    if eliminate_dead_modules {
        let (kept, module_warnings) = eliminate_dead_helper_modules(triples_for_dead);
        modules = kept;
        warnings.extend(module_warnings);
    } else {
        for (filename, code, module_warnings, _report) in triples_for_dead {
            modules.push((filename, code));
            warnings.extend(module_warnings);
        }
    }
    // Only modules that survived dead-module elimination link.
    let link_reports: Vec<(&str, &LinkReport)> = modules
        .iter()
        .filter_map(|(filename, _)| {
            link_by_filename
                .get(filename)
                .map(|report| (filename.as_str(), report))
        })
        .collect();
    warnings.extend(collect_missing_imported_name_warnings(&link_reports));
    if options.diagnostics && report_import_cycle_warnings {
        warnings.extend(collect_import_cycle_warnings(&modules));
    }

    // Build final provenance from the surviving output modules, mapping
    // provisional filenames to their recovered names.  Dead helper modules
    // that were eliminated above are excluded.
    let reverse_rename: crate::collections::HashMap<&str, &str> = rename_ref
        .iter()
        .map(|(prov, renamed)| (renamed.as_str(), prov.as_str()))
        .collect();
    let modules = modules
        .into_iter()
        .map(|(final_filename, code)| {
            let provisional = reverse_rename
                .get(final_filename.as_str())
                .copied()
                .unwrap_or(final_filename.as_str());
            let (input, ranges, inspection_context_ranges, is_entry) = provenance_by_provisional
                .get(provisional)
                .expect("every surviving module retains its provenance");
            let source_map = srcmap_by_filename.remove(&final_filename);
            PreparedModuleOutput {
                filename: final_filename,
                code,
                source_map,
                provenance: PreparedModuleProvenance {
                    input: *input,
                    ranges: ranges.clone(),
                    inspection_context_ranges: inspection_context_ranges.clone(),
                    is_entry: *is_entry,
                },
            }
        })
        .collect();

    Ok(PreparedUnpackOutput {
        modules,
        warnings,
        detected_formats: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_tracing::record_spans;
    use crate::unpacker::UnpackedModule;
    use crate::{validate_output_modules, DceMode, OutputFindingKind};

    /// The require edges, with their uses, and the import tags of one module
    /// run through the range as Phase 1 runs it (`second_parse == false`) or
    /// as the source-map path reruns it in Phase 2.
    fn edges_through_un_esm(
        code: &str,
        second_parse: bool,
    ) -> (RequireEdges, Vec<crate::require_edges::EdgeKey>) {
        GLOBALS.set(&Globals::new(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = if second_parse {
                parse_js_with_recovery(code, "consumer.js", cm)
                    .unwrap()
                    .module
            } else {
                parse_js(code, "consumer.js", cm).unwrap()
            };
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
            let level = RewriteLevel::Standard;
            let edges = run_rules_before_un_esm(&mut module, unresolved_mark, level, "consumer.js");
            let tags = run_un_esm_range(&mut module, unresolved_mark, level, "consumer.js", None);
            let mut keys: Vec<_> = tags
                .iter()
                .flat_map(|(_, keys)| keys.iter().copied())
                .collect();
            keys.sort();
            (edges, keys)
        })
    }

    #[test]
    fn a_settled_es_module_reads_no_commonjs_global() {
        let settled = |code: &str| {
            GLOBALS.set(&Globals::new(), || {
                let cm: Lrc<SourceMap> = Default::default();
                let mut module = parse_js(code, "m.js", cm).unwrap();
                let unresolved_mark = Mark::new();
                module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
                is_settled_es_module(&module, unresolved_mark)
            })
        };
        assert!(settled("import n from \"./n.js\";\nexport const x = n;"));
        // CommonJS left beside the declarations: the late recovery can
        // still convert it.
        assert!(!settled("import n from \"./n.js\";\nexports.x = n;"));
        assert!(!settled("export const x = () => require(\"./n.js\");"));
        assert!(!settled("export default module.id;"));
        // No declaration at all.
        assert!(!settled("const x = 1;"));
        // A local named like a CommonJS global is not one.
        assert!(settled("export function f(exports) { return exports.x; }"));
        // `require` of a numeric module id, and webpack's namespace-object
        // helpers on a local: nothing the late recovery converts.
        assert!(settled(
            "const ext = require(999);\nexport const x = ext.go;"
        ));
        assert!(settled(
            "const ns = {};\nrequire.r(ns);\nrequire.d(ns, { a: () => a });\nvar a = 1;\nexport { ns };"
        ));
        // Their arguments are still read.
        assert!(!settled(
            "const ns = {};\nrequire.d(ns, { a: () => require(\"./a.js\") });\nexport { ns };"
        ));
        // A string specifier, or one a rule could inline into a string.
        assert!(!settled("const n = require(\"./n.js\");\nexport { n };"));
        assert!(!settled(
            "const id = \"./n.js\";\nexport const n = require(id);"
        ));
        assert!(!settled("export const n = require(999, 1);"));
        assert!(!settled("export const n = require(...ids);"));
        // The helpers on `exports`, on an undeclared object, or any other
        // runtime helper.
        assert!(!settled("export const x = 1;\nrequire.r(exports);"));
        assert!(!settled("export const x = 1;\nrequire.r(ns);"));
        assert!(!settled("export const x = require.e(777);"));
        // `require` as a value.
        assert!(!settled("export const r = require.bind(require, 999);"));
        assert!(!settled("export const r = require;"));
    }

    fn resolved_module(code: &str) -> (Module, Mark, Lrc<SourceMap>) {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_js(code, "m.js", cm.clone()).unwrap();
        let unresolved_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
        (module, unresolved_mark, cm)
    }

    /// `helpers.js` exports an object-spread helper as its default; `plain.js`
    /// exports none.
    fn helper_provider_facts() -> ModuleFactsMap {
        use crate::facts::{HelperExportFact, HelperKind, ModuleFacts};
        let mut facts = ModuleFactsMap::new();
        facts.insert(
            "helpers.js",
            ModuleFacts {
                helper_exports: vec![HelperExportFact {
                    exported: "default".into(),
                    local: None,
                    kind: HelperKind::ObjectSpread,
                }],
                ..Default::default()
            },
        );
        facts.insert("plain.js", ModuleFacts::default());
        facts
    }

    #[test]
    fn the_rule_range_ignores_facts_without_a_helper_provider_or_pins() {
        GLOBALS.set(&Globals::new(), || {
            let facts = helper_provider_facts();
            let plan = crate::rules::CallRequiredPlan::build(&facts);
            let ignores = |code: &str, filename: &str| {
                let (module, _, _) = resolved_module(code);
                range_ignores_facts(
                    &module,
                    LateRangeFacts {
                        module_facts: &facts,
                        call_required_plan: &plan,
                    },
                    filename,
                )
            };
            assert!(ignores(
                "import p from \"./plain.js\";\nconst q = require(\"./plain.js\");\nexport const x = [p, q];",
                "m.js"
            ));
            // The helper rules read a helper provider's facts through an
            // import or a `require` argument.
            assert!(!ignores(
                "import s from \"./helpers.js\";\nexport const x = s({}, a);",
                "m.js"
            ));
            assert!(!ignores(
                "const s = interop(require(\"./helpers.js\"));\nmodule.exports = s.default({}, a);",
                "m.js"
            ));

            // `base.js` exports `Foo`, which `child.js` still calls: the
            // class rules read that pin while they run on `base.js`.
            let mut pinned = ModuleFactsMap::new();
            pinned.insert(
                "base.js",
                crate::facts::ModuleFacts {
                    exports: vec![crate::facts::ExportFact {
                        exported: "Foo".into(),
                        local: Some("Foo".into()),
                        kind: crate::facts::ExportKind::Named,
                    }],
                    ..Default::default()
                },
            );
            pinned.insert(
                "child.js",
                crate::facts::ModuleFacts {
                    import_call_edges: vec![crate::facts::ImportCallEdge {
                        source: "./base.js".into(),
                        imported: "Foo".into(),
                        consumed_by_exports: None,
                    }],
                    ..Default::default()
                },
            );
            let pinned_plan = crate::rules::CallRequiredPlan::build(&pinned);
            let (module, _, _) = resolved_module("export function Foo() {}");
            let pinned_facts = LateRangeFacts {
                module_facts: &pinned,
                call_required_plan: &pinned_plan,
            };
            assert!(!range_ignores_facts(&module, pinned_facts, "base.js"));
            assert!(range_ignores_facts(&module, pinned_facts, "child.js"));
        });
    }

    #[test]
    fn the_rule_range_runs_alike_with_facts_it_ignores() {
        // Phase 2 adopts the prediction's run of the range, made without
        // facts, when `range_ignores_facts` holds. The run with facts must
        // then print the same; with a helper provider it does not.
        let run = |code: &str, with_facts: bool| {
            GLOBALS.set(&Globals::new(), || {
                let facts = helper_provider_facts();
                let plan = crate::rules::CallRequiredPlan::build(&facts);
                let (mut module, unresolved_mark, cm) = resolved_module(code);
                let late_facts = LateRangeFacts {
                    module_facts: &facts,
                    call_required_plan: &plan,
                };
                let ignored = range_ignores_facts(&module, late_facts, "m.js");
                run_rules_before_late_recovery(
                    &mut module,
                    unresolved_mark,
                    RewriteLevel::Standard,
                    crate::DceMode::TransformOnly,
                    "m.js",
                    if with_facts {
                        PipelineMode::UnpackPhase2
                    } else {
                        PipelineMode::UnpackLatePrediction
                    },
                    with_facts.then_some(late_facts),
                );
                (ignored, print_js(&module, cm).unwrap())
            })
        };
        let plain = "import s from \"./plain.js\";\nexport const x = s({}, a, { b: 1 });";
        let (ignored, with_facts) = run(plain, true);
        assert!(ignored);
        assert_eq!(with_facts, run(plain, false).1);

        let helper = "import s from \"./helpers.js\";\nexport const x = s({}, a, { b: 1 });";
        let (ignored, with_facts) = run(helper, true);
        assert!(!ignored);
        assert!(with_facts.contains("...a"), "{with_facts}");
        assert_ne!(with_facts, run(helper, false).1);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "reads facts that `range_ignores_facts` does not cover")]
    fn an_adopted_range_that_facts_would_change_fails_the_debug_check() {
        // A gate that let a helper provider's consumer through: the run
        // without facts leaves the helper call, the run with them spreads
        // it, and the check catches the difference.
        GLOBALS.set(&Globals::new(), || {
            let facts = helper_provider_facts();
            let plan = crate::rules::CallRequiredPlan::build(&facts);
            let (module, unresolved_mark, _) = resolved_module(
                "import s from \"./helpers.js\";\nexport const x = s({}, a, { b: 1 });",
            );
            let mut adopted = module.clone();
            run_rules_before_late_recovery(
                &mut adopted,
                unresolved_mark,
                RewriteLevel::Standard,
                crate::DceMode::TransformOnly,
                "m.js",
                PipelineMode::UnpackLatePrediction,
                None,
            );
            assert_adopted_range_matches(
                &adopted,
                module,
                unresolved_mark,
                RewriteLevel::Standard,
                crate::DceMode::TransformOnly,
                "m.js",
                LateRangeFacts {
                    module_facts: &facts,
                    call_required_plan: &plan,
                },
            );
        });
    }

    #[test]
    #[cfg(debug_assertions)]
    fn an_adopted_range_that_ignores_facts_passes_the_debug_check() {
        GLOBALS.set(&Globals::new(), || {
            let facts = helper_provider_facts();
            let plan = crate::rules::CallRequiredPlan::build(&facts);
            let (module, unresolved_mark, _) = resolved_module(
                "import s from \"./plain.js\";\nexport const x = s({}, a, { b: 1 });",
            );
            let mut adopted = module.clone();
            run_rules_before_late_recovery(
                &mut adopted,
                unresolved_mark,
                RewriteLevel::Standard,
                crate::DceMode::TransformOnly,
                "m.js",
                PipelineMode::UnpackLatePrediction,
                None,
            );
            assert_adopted_range_matches(
                &adopted,
                module,
                unresolved_mark,
                RewriteLevel::Standard,
                crate::DceMode::TransformOnly,
                "m.js",
                LateRangeFacts {
                    module_facts: &facts,
                    call_required_plan: &plan,
                },
            );
        });
    }

    #[test]
    fn edges_get_their_uses_where_the_range_reaches_un_esm() {
        // The interop is read on the input, where the `require.n` getter is
        // still there; the uses are read on the AST `UnEsm` will see, after
        // the comma sequence is split, before `UnEsm` runs.
        let code = r#"var p = require("./p.js"), n = require.n(p);
use(p.alpha), p.beta = 1, n().gamma;"#;
        let (edges, _) = edges_through_un_esm(code, false);
        let edge = edges.iter().next().expect("one edge");
        assert_eq!(edge.interop, crate::require_edges::Interop::Default);
        let uses = edge.uses.as_ref().expect("the call reached UnEsm");
        assert!(uses.member_reads.iter().any(|name| name == "alpha"));
        assert_eq!(
            uses.member_writes,
            vec![swc_core::atoms::Atom::from("beta")]
        );
    }

    #[test]
    fn source_map_rerun_records_the_same_edges_and_tags_as_phase1() {
        let code = r#"var alpha = require("./alpha.js");
var beta = require("./beta.js").gamma;
alpha.delta = beta;
module.exports = function () { return require("./epsilon.js"); };"#;
        let (phase1_edges, phase1_tags) = edges_through_un_esm(code, false);
        let (rerun_edges, rerun_tags) = edges_through_un_esm(code, true);
        assert_eq!(phase1_edges.iter().count(), 3);
        assert!(phase1_edges.iter().all(|edge| edge.uses.is_some()));
        assert_eq!(phase1_edges, rerun_edges);
        assert_eq!(phase1_tags, rerun_tags);
    }

    fn validate_prepared_output(output: &PreparedUnpackOutput) -> Vec<crate::OutputFinding> {
        let modules = output
            .modules
            .iter()
            .map(|module| (module.filename.clone(), module.code.clone()))
            .collect::<Vec<_>>();
        validate_output_modules(&modules)
    }

    #[test]
    fn webpack_self_default_restoration_requires_an_unobserved_factory_object() {
        fn restore(source: &str) -> String {
            GLOBALS.set(&Default::default(), || {
                let cm: Lrc<SourceMap> = Default::default();
                let mut module = parse_js(source, "provider.js", cm.clone())
                    .expect("webpack factory fixture should parse");
                let unresolved_mark = Mark::new();
                module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
                restore_webpack_factory_default(
                    &mut module,
                    unresolved_mark,
                    false,
                    true,
                    "provider.js",
                );
                print_js(&module, cm).expect("restored fixture should print")
            })
        }

        let restored = restore(
            r#""use strict";
var current = require("./provider.js");
for (var key in current) globalThis[key] = current[key];"#,
        );
        let directive = restored
            .find("\"use strict\";")
            .expect("expected directive");
        let default = restored
            .find("module.exports = {};")
            .expect("expected detector-owned default restoration");
        let self_require = restored
            .find("require(\"./provider.js\")")
            .expect("expected self require");
        assert!(directive < default && default < self_require, "{restored}");

        for source in [
            r#"var current = require("./provider.js"); consume(module, current);"#,
            r#"var current = require("./provider.js"); consume(exports, current);"#,
            r#"var current = require("./provider.js"); consume(this, current);"#,
            r#"var current = require("./provider.js"); consume(arguments, current);"#,
            r#"var current = require("./provider.js"); eval("exports");"#,
            r#"export const marker = 1; var current = require("./provider.js");"#,
        ] {
            let output = restore(source);
            assert!(
                !output.contains("module.exports = {};"),
                "an observable factory environment must fail closed:\n{output}"
            );
        }
    }

    #[test]
    fn lifted_return_boundary_keeps_imports_at_module_scope() {
        let code = GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = parse_js(
                r#"import { value } from "./dep.js"; if (skip) return; consume(value);"#,
                "entry.js",
                cm.clone(),
            )
            .expect("recoverable top-level return should produce an AST");
            assert_eq!(
                restore_lifted_function_boundary(&mut module, true),
                BoundaryRestoration::Restored
            );
            apply_fixer(&mut module).expect("restored module should fix");
            print_js(&module, cm).expect("restored module should print")
        });

        assert!(
            code.starts_with("import ") && code.contains("return;") && code.contains("(()=>"),
            "imports should remain outside the restored callable boundary:\n{code}"
        );
        assert_eq!(
            validate_output_modules(&[
                ("entry.js".to_string(), code),
                ("dep.js".to_string(), "export const value = 1;".to_string()),
            ]),
            vec![]
        );
    }

    #[test]
    fn lifted_return_boundary_rejects_unguarded_exports() {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = parse_js(
                "if (skip) return; export const value = initialize();",
                "entry.js",
                cm,
            )
            .expect("recoverable top-level return should produce an AST");

            assert_eq!(
                restore_lifted_function_boundary(&mut module, true),
                BoundaryRestoration::Declined,
                "an export initializer cannot be moved outside the restored return boundary"
            );
        });
    }

    #[test]
    fn lifted_async_return_boundary_preserves_top_level_await() {
        let code = GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = parse_js(
                "if (skip) return; await initialize();",
                "entry.js",
                cm.clone(),
            )
            .expect("recoverable top-level return should produce an AST");
            assert_eq!(
                restore_lifted_function_boundary(&mut module, true),
                BoundaryRestoration::Restored
            );
            apply_fixer(&mut module).expect("restored async module should fix");
            print_js(&module, cm).expect("restored async module should print")
        });

        assert!(
            code.contains("async")
                && code.contains("await initialize()")
                && code.contains("return;"),
            "an async lifted body must regain an async callable boundary:\n{code}"
        );
        assert_eq!(
            validate_output_modules(&[("entry.js".to_string(), code)]),
            vec![]
        );
    }

    #[test]
    fn lifted_return_boundary_ignores_nested_function_returns() {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = parse_js(
                "function read() { return 1; } consume(read());",
                "entry.js",
                cm,
            )
            .expect("fixture should parse");

            assert_eq!(
                restore_lifted_function_boundary(&mut module, true),
                BoundaryRestoration::NotNeeded
            );
        });
    }

    #[test]
    fn lifted_boundary_declines_bodies_observing_this_or_arguments() {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = parse_js(
                "if (skip) return; consume(this.config);",
                "entry.js",
                cm.clone(),
            )
            .expect("recoverable top-level return should produce an AST");
            assert_eq!(
                restore_lifted_function_boundary(&mut module, true),
                BoundaryRestoration::Declined,
                "an arrow boundary would rebind the lifted `this`"
            );

            let mut with_arguments = parse_js(
                "if (skip) return; consume(arguments[0]);",
                "entry.js",
                cm.clone(),
            )
            .expect("recoverable top-level return should produce an AST");
            assert_eq!(
                restore_lifted_function_boundary(&mut with_arguments, true),
                BoundaryRestoration::Declined,
                "an arrow boundary has no `arguments`"
            );

            let mut nested = parse_js(
                "if (skip) return; function tail() { return arguments[0]; } consume(tail(1));",
                "entry.js",
                cm,
            )
            .expect("recoverable top-level return should produce an AST");
            assert_eq!(
                restore_lifted_function_boundary(&mut nested, true),
                BoundaryRestoration::Restored,
                "nested-function `arguments` belongs to that function"
            );
        });
    }

    #[test]
    fn profiler_reports_phase_operation_boundaries() {
        let modules = vec![UnpackedModule {
            id: "entry".to_string(),
            is_entry: true,
            code: "export const value = 1;".to_string(),
            filename: "entry.js".to_string(),
            ..Default::default()
        }];

        let (output, spans) = record_spans(|| {
            unpack_multi_module(modules, DecompileOptions::default())
                .expect("profiled fixture should decompile")
        });

        assert_eq!(output.modules.len(), 1);
        for expected in [
            "phase1: parse",
            "phase1: resolver",
            "phase1: rules",
            "phase1: fact recovery",
            "phase2: rules",
            "phase2: fixer",
            "phase2: emit",
        ] {
            assert!(
                spans.iter().any(|name| name == expected),
                "missing {expected:?} in {spans:?}"
            );
        }
        assert!(
            !spans.iter().any(|name| name == "phase2: parse"),
            "normal unpack should reuse the Phase 1 AST"
        );
    }

    #[test]
    fn an_unsettled_helper_provider_predicts_alike_on_any_worker_count() {
        // The kept require in the `for` head leaves `helpers.js` unsettled,
        // so its prediction runs the rule range and the recovery for its
        // helper facts in parallel. The consumer still reads the helper,
        // and the output does not depend on whether a second worker took
        // the helper facts.
        let modules = || {
            [
                (
                    "helpers.js",
                    r#"function helper(source, excluded) {
    var target = {};
    for (var key in source) {
        if (Object.prototype.hasOwnProperty.call(source, key) && excluded.indexOf(key) < 0) {
            target[key] = source[key];
        }
    }
    if (source != null && typeof Object.getOwnPropertySymbols === "function") {
        for (var i = 0, key = Object.getOwnPropertySymbols(source); i < key.length; i++) {
            if (excluded.indexOf(key[i]) < 0 && Object.prototype.propertyIsEnumerable.call(source, key[i])) {
                target[key[i]] = source[key[i]];
            }
        }
    }
    return target;
}
export { helper as __rest };
for (var r = require("./side.js"), n = 0; n < 1; n++) r.go(n);
"#,
                ),
                (
                    "consumer.js",
                    r#"import { __rest } from "./helpers.js";
var label = props.label, rest = __rest(props, ["label"]);
use(label, rest);
"#,
                ),
                ("side.js", "module.exports = { go: function () {} };\n"),
            ]
            .into_iter()
            .map(|(filename, code)| UnpackedModule {
                id: filename.to_string(),
                is_entry: filename == "consumer.js",
                code: code.to_string(),
                filename: filename.to_string(),
                ..Default::default()
            })
            .collect::<Vec<_>>()
        };
        let unpack_on = |threads: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| unpack_multi_module(modules(), DecompileOptions::default()))
                .expect("modules should decompile")
                .modules
                .into_iter()
                .map(|module| (module.filename, module.code))
                .collect::<Vec<_>>()
        };
        let one = unpack_on(1);
        let consumer = &one
            .iter()
            .find(|(filename, _)| filename == "consumer.js")
            .expect("consumer output")
            .1;
        assert!(consumer.contains("...rest"), "{consumer}");
        assert_eq!(one, unpack_on(4));
    }

    #[test]
    fn export_of_a_block_nested_var_survives_unpack() {
        // `var p` inside the block is a module-scope binding, so the export
        // names a declared local.
        let modules = vec![UnpackedModule {
            id: "entry".to_string(),
            is_entry: true,
            code: r#"if (globalThis.flag) {
    var p = 1;
}
export { p as t };
"#
            .to_string(),
            filename: "entry.js".to_string(),
            ..Default::default()
        }];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("fixture should decompile");
        let code = &output.modules[0].code;
        assert!(
            code.contains("export { p as t };"),
            "the export should be kept:\n{code}"
        );
    }

    #[test]
    fn late_cleanup_preserves_pre_existing_dead_recovered_import_specifier() {
        let modules = vec![UnpackedModule {
            id: "entry".to_string(),
            is_entry: true,
            code: r#"import { alreadyDead } from "./module.js";"#.to_string(),
            filename: "entry.js".to_string(),
            ..Default::default()
        }];

        let output = unpack_multi_module(
            modules,
            DecompileOptions {
                dce_mode: DceMode::TransformOnly,
                ..Default::default()
            },
        )
        .expect("fixture should decompile");
        assert_eq!(
            output.modules[0].code,
            "import { alreadyDead } from \"./module.js\";\n"
        );
    }

    #[test]
    fn scope_split_cycles_do_not_emit_diagnostic_warnings() {
        let modules = vec![
            PreparedUnpackModule::with_cycle_warnings(
                UnpackedModule {
                    id: "a".to_string(),
                    is_entry: true,
                    code: r#"import { b } from "./b.js"; export const a = b + 1;"#.to_string(),
                    filename: "entry.js".to_string(),
                    ..Default::default()
                },
                false,
            ),
            PreparedUnpackModule::with_cycle_warnings(
                UnpackedModule {
                    id: "b".to_string(),
                    is_entry: false,
                    code: r#"import { a } from "./entry.js"; export const b = a + 1;"#.to_string(),
                    filename: "b.js".to_string(),
                    ..Default::default()
                },
                false,
            ),
        ];

        let output = unpack_multi_module_with_plan(
            modules,
            NumericRewritePlan::default(),
            DecompileOptions {
                diagnostics: true,
                ..Default::default()
            },
            &[],
            None,
        )
        .expect("scope split cycle should decompile");

        assert!(
            output.warnings.is_empty(),
            "native ESM cycles from scope splits should not produce stderr warnings: {:?}",
            output.warnings
        );
    }

    #[test]
    fn multi_module_split_sequence_uses_member_name_for_assignment_temp() {
        let modules = vec![UnpackedModule {
            id: "1".to_string(),
            is_entry: false,
            code: r#"var i, a, o;
module.exports = (a = (i = require("./module-2.js")).lib, o = a.WordArray, i.SHA1);
"#
            .to_string(),
            filename: "module-1.js".to_string(),
            ..Default::default()
        }];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("fixture should decompile");
        let code = &output.modules[0].code;
        assert!(
            code.contains("const lib ="),
            "expected temp binding to use member name:\n{code}"
        );
        assert!(
            !code.contains("const _a ="),
            "should not synthesize the fallback assignment name:\n{code}"
        );
    }

    #[test]
    fn multi_module_preserves_lowered_interop_binding_read_until_import_recovery() {
        let modules = vec![UnpackedModule {
            id: "1".to_string(),
            is_entry: false,
            code: r#""use strict";
Object.defineProperty(exports, "__esModule", {
    value: true
});
var a = require("./module-2.js"), o = (r(a), r(require("./module-3.js")));
function r(e) {
    return e && e.__esModule ? e : {
        default: e
    };
}
class l extends a.Component {}
exports.default = o.default(l);
"#
            .to_string(),
            filename: "module-1.js".to_string(),
            ..Default::default()
        }];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("fixture should decompile");
        let code = &output.modules[0].code;
        assert!(
            code.contains("import a from \"./module-2.js\";"),
            "expected require binding to become an import:\n{code}"
        );
        assert!(
            code.contains("import o from \"./module-3.js\";"),
            "expected interop require to become an import:\n{code}"
        );
        assert!(
            code.contains("a;\nclass l extends a.Component"),
            "expected lowered interop binding read to survive until import recovery:\n{code}"
        );
    }

    #[test]
    fn lowered_interop_residue_does_not_block_named_import_recovery() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.transform = function(value) { return value + 1; };".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
function interopRequireDefault(value) {
    return value && value.__esModule ? value : { default: value };
}
var provider = require("./provider.js");
interopRequireDefault(provider);
module.exports = function(value) { return provider.transform(value); };
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("interop residue fixture should decompile");
        let findings = validate_prepared_output(&output);
        assert_eq!(findings, vec![]);

        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import { transform }") && !consumer.contains("provider;"),
            "an inert lowered interop read must not retain a missing default import:\n{consumer}"
        );
    }

    #[test]
    fn named_only_provider_repairs_synthetic_default_to_namespace_import() {
        let modules = || {
            vec![
                UnpackedModule {
                    id: "provider".to_string(),
                    code: r#"
exports.alpha = 1;
exports.beta = 2;
"#
                    .to_string(),
                    filename: "provider.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "consumer".to_string(),
                    is_entry: true,
                    code: r#"
var provider = require("./provider.js");
function read(alpha, beta) {
    return provider.alpha + provider.beta + alpha + beta;
}
module.exports = read;
"#
                    .to_string(),
                    filename: "consumer.js".to_string(),
                    ..Default::default()
                },
            ]
        };

        let output = unpack_multi_module(modules(), DecompileOptions::default())
            .expect("named-only provider fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import * as provider from \"./provider.js\";"),
            "colliding local names should retain a valid namespace fallback:\n{consumer}"
        );

        let source_map_output = unpack_multi_module(
            modules(),
            DecompileOptions {
                emit_source_map: true,
                ..Default::default()
            },
        )
        .expect("namespace repair should survive the source-map path");
        assert_eq!(validate_prepared_output(&source_map_output), vec![]);
        assert!(
            source_map_output
                .modules
                .iter()
                .all(|module| module.source_map.is_some()),
            "namespace repair must preserve emitted source maps"
        );
    }

    #[test]
    fn named_only_provider_repairs_a_transparent_synthetic_import_alias() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1; exports.beta = 2;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var imported = require("./provider.js");
let provider = imported;
const value = provider.alpha + provider.beta;
provider = { alpha: 3 };
module.exports = value + provider.alpha;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("a transparent alias should preserve namespace semantics");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import * as imported from \"./provider.js\";"),
            "the synthesized edge should become a namespace import:\n{consumer}"
        );
        assert!(
            consumer.contains("provider = {") && consumer.contains("provider.alpha"),
            "the reassigned alias must keep its later, non-namespace lifetime:\n{consumer}"
        );
    }

    #[test]
    fn named_only_provider_keeps_namespace_member_mutation_fail_closed() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var imported = require("./provider.js");
let provider = imported;
provider.alpha = 2;
module.exports = provider.alpha;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("a namespace mutation should remain printable");
        let findings = validate_prepared_output(&output);
        assert!(findings.iter().any(|finding| {
            finding.kind == OutputFindingKind::MissingImportedName
                && finding.filename == "consumer.js"
        }));
    }

    #[test]
    fn named_only_provider_keeps_conditional_alias_replacement_fail_closed() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var imported = require("./provider.js");
let provider = imported;
if (replaceProvider) provider = { alpha: 2 };
module.exports = provider.alpha;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("a conditional alias replacement should remain printable");
        let findings = validate_prepared_output(&output);
        assert!(findings.iter().any(|finding| {
            finding.kind == OutputFindingKind::MissingImportedName
                && finding.filename == "consumer.js"
        }));
    }

    #[test]
    fn named_only_provider_namespace_supports_enumeration_and_copy_sources() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1; exports.beta = 2;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var provider = require("./provider.js");
var copy = {};
Object.assign(copy, provider);
module.exports = Object.keys(provider).join(",") + copy.alpha;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("namespace enumeration fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import * as provider from \"./provider.js\";"),
            "whole-namespace enumeration and copy must use a namespace import:\n{consumer}"
        );
    }

    #[test]
    fn named_only_provider_repairs_an_escaped_synthetic_default() {
        // Without a default export the default import cannot link, so a
        // namespace passed on as a value keeps the only meaning left.
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1; exports.beta = 2;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var provider = require("./provider.js");
register(provider);
exports.alpha = provider.alpha;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("escaped namespace fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import * as provider from \"./provider.js\";")
                && consumer.contains("register(provider)"),
            "an escaped namespace must not keep a default import that cannot link:\n{consumer}"
        );
    }

    fn decompiled_consumer(provider: &str, consumer: &str) -> String {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: provider.to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: consumer.to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];
        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("provider fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.clone())
            .expect("expected consumer module")
    }

    #[test]
    fn exports_object_provider_with_a_default_repairs_a_whole_use_to_a_namespace() {
        // Requiring the provider returns its `exports` object, which holds
        // `default` as one property among the others: the namespace of the
        // recovered module, not its default export.
        for provider in [
            "exports.default = D; exports.alpha = 1; function D() { return 2; }",
            "require.r(exports); require.d(exports, { default: () => D, alpha: () => alpha }); \
             function D() { return 2; } var alpha = 1;",
        ] {
            let consumer = decompiled_consumer(
                provider,
                "var provider = require(\"./provider.js\"); register(provider); \
                 exports.value = provider.default() + provider.alpha;",
            );
            assert!(
                consumer.contains("import * as provider from \"./provider.js\";")
                    && consumer.contains("register(provider)"),
                "the whole `exports` object is the namespace:\n{consumer}"
            );
        }
    }

    #[test]
    fn provider_declaring_its_default_before_un_esm_keeps_the_default_import() {
        // The webpack runtime normalizers write a replaced `module.exports`
        // as `export default` before the rules run; that default is the whole
        // required value.
        let consumer = decompiled_consumer(
            "var _webpackDefault = function D() {}; _webpackDefault.alpha = 1; \
             export default _webpackDefault; exports.beta = 2;",
            "var provider = require(\"./provider.js\"); register(provider); \
             exports.value = provider.alpha;",
        );
        assert!(
            !consumer.contains("import * as provider"),
            "a declared default is the whole required value:\n{consumer}"
        );
    }

    #[test]
    fn interop_default_of_an_exports_object_provider_keeps_the_default_import() {
        // webpack's `require.n` getter, inline or as the runtime call, reads
        // the default export of a provider marked ESM. The getter is replaced
        // by the plain binding before the repair runs, so the binding then
        // means the default export, not the namespace.
        let provider = "Object.defineProperty(exports, \"__esModule\", { value: true }); \
                        exports.default = { run() { return 2; } }; exports.alpha = 1;";
        for consumer in [
            "var provider = require(\"./provider.js\"), \
             get = () => provider && provider.__esModule ? provider.default : provider; \
             exports.value = get().run();",
            "var provider = require(\"./provider.js\"), get = require.n(provider); \
             exports.value = get().run();",
            // `UnEsm` merges both bindings of the source into one default
            // import, so the whole use alone does not make it a namespace.
            "function _interopRequireDefault(obj) { \
               return obj && obj.__esModule ? obj : { default: obj }; } \
             var whole = require(\"./provider.js\"); \
             var provider = _interopRequireDefault(require(\"./provider.js\")); \
             register(whole); exports.value = provider.default.run();",
        ] {
            let consumer = decompiled_consumer(provider, consumer);
            assert!(
                !consumer.contains("import * as"),
                "an unwrapped interop default must keep the default import:\n{consumer}"
            );
        }
    }

    /// swc's `_interop_require_wildcard`, inlined by Terser at its only call
    /// site and applied to `require("./provider.js")`.
    /// shape: producer swc@1.16.2 module=commonjs, then @rspack/core@1.7.12
    /// mode=production.
    const INLINE_WILDCARD_REQUIRE: &str = "function(e) { \
        if (e && e.__esModule) return e; \
        if (null === e || \"object\" != typeof e && \"function\" != typeof e) return { default: e }; \
        var n = { __proto__: null }, u = Object.defineProperty && Object.getOwnPropertyDescriptor; \
        for (var o in e) if (\"default\" !== o && Object.prototype.hasOwnProperty.call(e, o)) { \
          var a = u ? Object.getOwnPropertyDescriptor(e, o) : null; \
          a && (a.get || a.set) ? Object.defineProperty(n, o, a) : n[o] = e[o]; } \
        return n.default = e, n; }(require(\"./provider.js\"))";

    #[test]
    fn inline_wildcard_of_a_marked_provider_repairs_to_a_namespace() {
        // A wildcard interop returns a provider marked `__esModule` unchanged,
        // so the binding is the whole `exports` object: the namespace.
        let consumer = decompiled_consumer(
            "Object.defineProperty(exports, \"__esModule\", { value: !0 }); \
             exports.default = D; exports.alpha = 1; function D() { return 2; }",
            &format!(
                "var provider = {INLINE_WILDCARD_REQUIRE}; register(provider); \
                 exports.value = provider.default() + provider.alpha;"
            ),
        );
        assert!(
            consumer.contains("import * as provider from \"./provider.js\";")
                && consumer.contains("register(provider)"),
            "the wildcard of a marked provider is its namespace:\n{consumer}"
        );
    }

    #[test]
    fn inline_wildcard_of_an_unmarked_provider_keeps_the_default_import() {
        // Without the marker the wildcard builds a copy whose `default` is the
        // whole `exports` object, not the recovered default export.
        let consumer = decompiled_consumer(
            "exports.default = D; exports.alpha = 1; function D() { return 2; }",
            &format!(
                "var provider = {INLINE_WILDCARD_REQUIRE}; register(provider); \
                 exports.value = provider.default.alpha;"
            ),
        );
        assert!(
            !consumer.contains("import * as provider"),
            "an unmarked provider must keep the default import:\n{consumer}"
        );
    }

    #[test]
    fn replaced_module_exports_provider_keeps_the_default_import_of_a_whole_use() {
        // `module.exports = D` makes `D` the whole required value; its
        // properties are recovered as named exports, but the default import
        // is the value the consumer used.
        let consumer = decompiled_consumer(
            "module.exports = D; module.exports.alpha = 1; function D() { return 2; }",
            "var provider = require(\"./provider.js\"); register(provider); \
             exports.value = provider() + provider.alpha;",
        );
        assert!(
            consumer.contains("import provider from \"./provider.js\";"),
            "a replaced `module.exports` is the default export:\n{consumer}"
        );
    }

    #[test]
    fn webpack_namespace_object_of_a_commonjs_import_becomes_a_namespace_import() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1; exports.beta = 2;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var ns;
var provider = require("./provider.js");
register(ns || (ns = require.t(provider, 2)));
exports.alpha = provider.alpha;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("require.t namespace fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import * as provider from \"./provider.js\";")
                && consumer.contains("register(provider)")
                && !consumer.contains("require.t"),
            "webpack's namespace object of a whole import is the namespace import:\n{consumer}"
        );
    }

    #[test]
    fn recovered_export_star_surface_repairs_downstream_namespace_copy() {
        let modules = vec![
            UnpackedModule {
                id: "source".to_string(),
                code: "exports.alpha = 1; exports.beta = 2;".to_string(),
                filename: "source.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "facade".to_string(),
                code: r#"
var source = require("./source.js");
Object.keys(source).forEach(function(key) {
    key !== "default" && key !== "__esModule" &&
        (key in exports && exports[key] === source[key] ||
            (exports[key] = source[key]));
});
"#
                .to_string(),
                filename: "facade.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var facade = require("./facade.js");
var copy = {};
Object.assign(copy, facade);
module.exports = copy.alpha;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("recovered export-star fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let facade = output
            .modules
            .iter()
            .find(|module| module.filename == "facade.js")
            .map(|module| module.code.as_str())
            .expect("expected facade module");
        assert_eq!(facade, "export * from \"./source.js\";\n");
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import * as facade from \"./facade.js\";"),
            "the downstream copy must consume the recovered namespace surface:\n{consumer}"
        );
    }

    #[test]
    fn export_star_provider_supports_synthetic_namespace_require() {
        let modules = vec![
            UnpackedModule {
                id: "source".to_string(),
                code: "exports.alpha = 1;".to_string(),
                filename: "source.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "provider".to_string(),
                code: "export * from \"./source.js\";".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var provider = require("./provider.js");
module.exports = Object.keys(provider);
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("export-star namespace fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import * as provider from \"./provider.js\";"),
            "an export-star surface is valid through a namespace import:\n{consumer}"
        );
    }

    #[test]
    fn authored_default_import_is_not_provider_repaired() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
import provider from "./provider.js";
console.log(Object.keys(provider));
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("authored import fixture should decompile");
        let findings = validate_prepared_output(&output);
        assert!(
            findings.iter().any(|finding| {
                finding.kind == OutputFindingKind::MissingImportedName
                    && finding.filename == "consumer.js"
            }),
            "provider repair must not reinterpret authored default imports: {findings:#?}"
        );
    }

    #[test]
    fn provider_namespace_repair_rejects_esmodule_meta_observation() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var provider = require("./provider.js");
module.exports = provider.__esModule;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("namespace metadata fixture should decompile");
        let findings = validate_prepared_output(&output);
        assert!(
            findings.iter().any(|finding| {
                finding.kind == OutputFindingKind::MissingImportedName
                    && finding.filename == "consumer.js"
            }),
            "observing CommonJS interop metadata must keep namespace repair fail closed: {findings:#?}"
        );
    }

    #[test]
    fn getter_map_default_provider_supports_synthetic_default_import() {
        let modules = || {
            vec![
                UnpackedModule {
                    id: "provider".to_string(),
                    code: r#"
((target, getters) => {
    for (const key in getters) {
        Object.defineProperty(target, key, {
            enumerable: true,
            get: getters[key]
        });
    }
})(exports, {
    dim() { return dim; },
    default() { return logger; }
});
function dim(value) { return value; }
const logger = {
    warn(value) { console.warn(value); }
};
if ((typeof exports.default === "function" || typeof exports.default === "object" && exports.default !== null) && exports.default.__esModule === undefined) {
    Object.defineProperty(exports.default, "__esModule", { value: true });
    Object.assign(exports.default, exports);
    module.exports = exports.default;
}
"#
                    .to_string(),
                    filename: "provider.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "consumer".to_string(),
                    is_entry: true,
                    code: r#"
var logger = require("./provider.js");
module.exports = function(value) { return logger.warn(value); };
"#
                    .to_string(),
                    filename: "consumer.js".to_string(),
                    ..Default::default()
                },
            ]
        };

        let output = unpack_multi_module(modules(), DecompileOptions::default())
            .expect("getter-map default fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let provider = output
            .modules
            .iter()
            .find(|module| module.filename == "provider.js")
            .map(|module| module.code.as_str())
            .expect("expected provider module");
        assert!(
            provider.contains("export { logger as default };")
                && !provider.contains("Object.defineProperty"),
            "the provider should expose the recovered live default:\n{provider}"
        );
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        // The compatibility block makes the default the whole required value.
        assert!(
            consumer.contains("import logger from \"./provider.js\";"),
            "the proven provider default should satisfy the synthetic require:\n{consumer}"
        );

        let source_map_output = unpack_multi_module(
            modules(),
            DecompileOptions {
                emit_source_map: true,
                ..Default::default()
            },
        )
        .expect("getter-map default should survive the source-map path");
        assert_eq!(validate_prepared_output(&source_map_output), vec![]);
        assert!(
            source_map_output
                .modules
                .iter()
                .all(|module| module.source_map.is_some()),
            "getter-map default recovery must preserve emitted source maps"
        );
    }

    #[test]
    fn unused_synthetic_require_becomes_side_effect_import() {
        let modules = || {
            vec![
                UnpackedModule {
                    id: "provider".to_string(),
                    code: "exports.alpha = 1;".to_string(),
                    filename: "provider.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "consumer".to_string(),
                    is_entry: true,
                    code: r#"
before();
var unused = require("./provider.js");
after();
module.exports = 42;
"#
                    .to_string(),
                    filename: "consumer.js".to_string(),
                    ..Default::default()
                },
            ]
        };

        let output = unpack_multi_module(modules(), DecompileOptions::default())
            .expect("unused require fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import \"./provider.js\";"),
            "provider evaluation must survive as a side-effect import:\n{consumer}"
        );
        assert!(
            !consumer.contains("import unused") && !consumer.contains("var unused"),
            "the unused guessed default binding should be removed:\n{consumer}"
        );

        let source_map_output = unpack_multi_module(
            modules(),
            DecompileOptions {
                emit_source_map: true,
                ..Default::default()
            },
        )
        .expect("unused require cleanup should survive the source-map path");
        assert_eq!(validate_prepared_output(&source_map_output), vec![]);
        assert!(
            source_map_output
                .modules
                .iter()
                .all(|module| module.source_map.is_some()),
            "side-effect import recovery must preserve emitted source maps"
        );
    }

    #[test]
    fn unused_synthetic_default_is_removed_from_mixed_import() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var unused = require("./provider.js");
var alpha = require("./provider.js").alpha;
module.exports = alpha;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("mixed import fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import { alpha }") && !consumer.contains("import unused"),
            "only the used named binding should remain:\n{consumer}"
        );
    }

    #[test]
    fn unused_authored_import_is_not_synthetic_cleanup() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
import missing from "./provider.js";
export const result = 1;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("authored unused import fixture should decompile");
        let findings = validate_prepared_output(&output);
        assert!(
            findings.iter().any(|finding| {
                finding.kind == OutputFindingKind::MissingImportedName
                    && finding.filename == "consumer.js"
            }),
            "cleanup must not reinterpret an authored dead import: {findings:#?}"
        );
    }

    #[test]
    fn substantive_namespace_use_still_blocks_named_import_recovery() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.transform = function(value) { return value + 1; };".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
function interopRequireDefault(value) {
    return value && value.__esModule ? value : { default: value };
}
var provider = require("./provider.js");
interopRequireDefault(provider);
observe(provider);
module.exports = function(value) { return provider.transform(value); };
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("substantive namespace fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import * as provider from \"./provider.js\";")
                && consumer.contains("observe(provider)")
                && consumer.contains("provider.transform(value)")
                && !consumer.contains("import { transform }"),
            "a substantive whole-object use must keep the whole namespace binding:\n{consumer}"
        );
    }

    #[test]
    fn provider_facts_capture_missing_properties_from_proven_empty_default_objects() {
        let modules = || {
            vec![
                UnpackedModule {
                    id: "empty-stub".to_string(),
                    code: "module.exports = {};".to_string(),
                    filename: "empty-stub.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "consumer".to_string(),
                    is_entry: true,
                    code: r#"
before();
var existsSync = require("./empty-stub.js").existsSync;
after();
module.exports = function(path) {
    return Boolean(existsSync) && existsSync(path);
};
"#
                    .to_string(),
                    filename: "consumer.js".to_string(),
                    ..Default::default()
                },
            ]
        };

        for emit_source_map in [false, true] {
            let output = unpack_multi_module(
                modules(),
                DecompileOptions {
                    emit_source_map,
                    ..Default::default()
                },
            )
            .expect("proven empty default-object fixture should decompile");
            assert_eq!(validate_prepared_output(&output), vec![]);

            let consumer = output
                .modules
                .iter()
                .find(|module| module.filename == "consumer.js")
                .map(|module| module.code.as_str())
                .expect("expected consumer module");
            assert!(
                !consumer.contains("import { existsSync"),
                "a CommonJS property read must not remain a named import:\n{consumer}"
            );
            assert!(
                consumer.contains(".existsSync"),
                "the property must be captured from the proven default object:\n{consumer}"
            );
            let before = consumer
                .find("before();")
                .expect("leading effect should remain");
            let capture = consumer[before..]
                .find(".existsSync")
                .map(|offset| before + offset)
                .expect("property capture should remain");
            let after = consumer
                .find("after();")
                .expect("trailing effect should remain");
            assert!(
                before < capture && capture < after,
                "the property read must remain at its original require position:\n{consumer}"
            );
            if emit_source_map {
                assert!(
                    output
                        .modules
                        .iter()
                        .all(|module| module.source_map.is_some()),
                    "provider repair must preserve emitted source maps"
                );
            }
        }
    }

    #[test]
    fn provider_facts_capture_proven_callable_default_properties() {
        let modules = || {
            vec![
                UnpackedModule {
                    id: "provider".to_string(),
                    code: r#"
function api(value) { return value; }
api.parse = function(value) { return value.length; };
api.Rule = class Rule {};
module.exports = api;
api.default = api;
"#
                    .to_string(),
                    filename: "provider.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "consumer".to_string(),
                    is_entry: true,
                    code: r#"
before();
var parse = require("./provider.js").parse;
var Rule = require("./provider.js").Rule;
after();
module.exports = [parse("value"), new Rule()];
"#
                    .to_string(),
                    filename: "consumer.js".to_string(),
                    ..Default::default()
                },
            ]
        };

        for emit_source_map in [false, true] {
            let output = unpack_multi_module(
                modules(),
                DecompileOptions {
                    emit_source_map,
                    ..Default::default()
                },
            )
            .expect("proven callable-property fixture should decompile");
            assert_eq!(validate_prepared_output(&output), vec![]);

            let consumer = output
                .modules
                .iter()
                .find(|module| module.filename == "consumer.js")
                .map(|module| module.code.as_str())
                .expect("expected consumer module");
            assert!(
                !consumer.contains("import { parse") && !consumer.contains("Rule } from"),
                "attached CommonJS properties must not remain guessed named imports:\n{consumer}"
            );
            assert!(
                (consumer.contains(".parse") && consumer.contains(".Rule"))
                    || consumer.contains("{ parse, Rule }"),
                "attached properties must be captured from the callable default:\n{consumer}"
            );
            let before = consumer
                .find("before();")
                .expect("leading effect should remain");
            let parse_capture = consumer
                .find(".parse")
                .or_else(|| consumer.find("{ parse"))
                .expect("parse capture should remain");
            let rule_capture = consumer[parse_capture..]
                .find("Rule")
                .map(|offset| parse_capture + offset)
                .expect("Rule capture should remain");
            let after = consumer
                .find("after();")
                .expect("trailing effect should remain");
            assert!(
                before < parse_capture && parse_capture < rule_capture && rule_capture < after,
                "callable property reads must stay at their original require positions:\n{consumer}"
            );
            if emit_source_map {
                assert!(
                    output
                        .modules
                        .iter()
                        .all(|module| module.source_map.is_some()),
                    "callable property repair must preserve emitted source maps"
                );
            }
        }
    }

    #[test]
    fn provider_facts_reject_a_conditionally_rewritten_callable_default() {
        let modules = || {
            vec![
                UnpackedModule {
                    id: "provider".to_string(),
                    code: r#"
function api(value) { return value; }
api.parse = function(value) { return value.length; };
module.exports = api;
if (globalThis.legacy) { module.exports = globalThis.wrap(api); }
"#
                    .to_string(),
                    filename: "provider.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "consumer".to_string(),
                    is_entry: true,
                    code: r#"
var parse = require("./provider.js").parse;
module.exports = function(value) { return parse(value); };
"#
                    .to_string(),
                    filename: "consumer.js".to_string(),
                    ..Default::default()
                },
            ]
        };

        for emit_source_map in [false, true] {
            let output = unpack_multi_module(
                modules(),
                DecompileOptions {
                    emit_source_map,
                    ..Default::default()
                },
            )
            .expect("conditional callable-default fixture should decompile");
            let findings = validate_prepared_output(&output);
            assert!(
                findings.iter().any(|finding| {
                    finding.kind == OutputFindingKind::MissingImportedName
                        && finding.filename == "consumer.js"
                }),
                "an unproven callable surface must remain visible to validation: {findings:#?}"
            );

            let consumer = output
                .modules
                .iter()
                .find(|module| module.filename == "consumer.js")
                .map(|module| module.code.as_str())
                .expect("expected consumer module");
            assert!(
                consumer.contains("import { parse } from \"./provider.js\""),
                "an unproven property import must not be rewritten through the default export:\n{consumer}"
            );
        }
    }

    #[test]
    fn a_conditional_property_of_a_whole_value_callable_is_read_through_its_default() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: r#"
function api(value) { return value; }
var local = module.exports = api;
if (globalThis.enableOptional) { local.optional = function() { return true; }; }
"#
                .to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var optional = require("./provider.js").optional;
module.exports = optional;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        // The provider facts do not prove `optional`, but the plan proves
        // the provider's only export is `module.exports = api`, so `UnEsm`
        // reads the property from the default import, present or not.
        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("conditional alias-property fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import provider from \"./provider.js\""),
            "{consumer}"
        );
        assert!(
            consumer.contains("const optional = provider.optional"),
            "{consumer}"
        );
    }

    #[test]
    fn an_absent_property_of_a_whole_value_callable_is_read_through_its_default() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: r#"
function api(value) { return value; }
api.parse = function(value) { return value.length; };
module.exports = api;
"#
                .to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var missing = require("./provider.js").missing;
module.exports = function() { return missing; };
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        // Callable facts prove only observed properties, but the plan
        // proves the provider's only export is `module.exports = api`:
        // CommonJS reads `undefined`, and so does the default import.
        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("absent callable-property fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import provider from \"./provider.js\""),
            "{consumer}"
        );
        assert!(
            consumer.contains("const missing = provider.missing"),
            "{consumer}"
        );
    }

    #[test]
    fn provider_property_capture_preserves_same_declaration_order() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: r#"
module.exports = {
    capability: function() { return true; },
    secondary: function() { return true; }
};
"#
                .to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
let first = sideEffect(),
    { capability, secondary } = require("./provider.js"),
    available = Boolean(capability && secondary);
observe(first, available);
module.exports = available;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("same-declaration capture fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        let effect = consumer
            .find("sideEffect()")
            .expect("leading declarator should remain");
        let capture = consumer
            .find(".capability")
            .or_else(|| consumer.find("{ capability"))
            .expect("property capture should remain");
        let secondary_capture = consumer[capture..]
            .find("secondary")
            .map(|offset| capture + offset)
            .expect("second property capture should remain");
        let feature_check = consumer
            .find("Boolean(")
            .expect("later declarator should remain");
        assert!(
            effect < capture && capture < secondary_capture && secondary_capture < feature_check,
            "captures must remain ordered between surrounding declarators:\n{consumer}"
        );
    }

    #[test]
    fn provider_facts_repair_recovered_commonjs_import_shapes() {
        let modules = || {
            vec![
                UnpackedModule {
                    id: "default-provider".to_string(),
                    code: r#"
var methods = {
    map: function(value) { return value + 1; },
    filter: function(value) { return value > 0; }
};
module.exports = methods;
"#
                    .to_string(),
                    filename: "default-provider.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "named-consumer".to_string(),
                    is_entry: true,
                    code: r#"
before();
var map = require("./default-provider.js").map;
between();
var filter = require("./default-provider.js").filter;
map = replacement;
module.exports = function(value) { return filter(map(value)); };
"#
                    .to_string(),
                    filename: "named-consumer.js".to_string(),
                    ..Default::default()
                },
            ]
        };

        let output = unpack_multi_module(modules(), DecompileOptions::default())
            .expect("provider-aware fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);

        let named_consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "named-consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected named consumer");
        assert!(
            !named_consumer.contains("import { map") && !named_consumer.contains("filter } from"),
            "properties of a proven default object must be captured from its default import:\n{named_consumer}"
        );
        assert!(
            (named_consumer.contains("{ map:") && named_consumer.contains("filter }"))
                || (named_consumer.contains(".map") && named_consumer.contains(".filter")),
            "property captures must come from the proven default object:\n{named_consumer}"
        );
        assert!(
            named_consumer.contains("let map") || named_consumer.contains("let { map"),
            "a reassigned property local must remain mutable instead of becoming an import binding:\n{named_consumer}"
        );
        let before = named_consumer
            .find("before();")
            .expect("leading effect should remain");
        let map_capture = named_consumer[before..]
            .find("map")
            .map(|offset| before + offset)
            .expect("map capture should remain");
        let between = named_consumer
            .find("between();")
            .expect("middle effect should remain");
        let filter_capture = named_consumer[between..]
            .find("filter")
            .map(|offset| between + offset)
            .expect("filter capture should remain");
        assert!(
            before < map_capture && map_capture < between && between < filter_capture,
            "property captures must stay at their original require positions around effects:\n{named_consumer}"
        );

        let source_map_output = unpack_multi_module(
            modules(),
            DecompileOptions {
                emit_source_map: true,
                ..Default::default()
            },
        )
        .expect("provider-aware fixture should decompile through the source-map path");
        assert_eq!(validate_prepared_output(&source_map_output), vec![]);
        assert!(
            source_map_output
                .modules
                .iter()
                .all(|module| module.source_map.is_some()),
            "provider repair must survive the source-map path's second parse"
        );
    }

    #[test]
    fn a_property_of_an_unknown_whole_value_is_read_through_its_default() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "module.exports = makeProvider();".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var missing = require("./provider.js").missing;
module.exports = function() { return missing; };
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        // The provider facts know nothing of the call's value, but the plan
        // proves it is the provider's only export, so the consumer reads
        // `missing` from it as CommonJS did.
        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("unproven provider fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import provider from \"./provider.js\""),
            "{consumer}"
        );
        assert!(
            consumer.contains("const missing = provider.missing"),
            "{consumer}"
        );
    }

    #[test]
    fn a_hand_written_import_of_a_whole_value_provider_is_left_as_written() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "module.exports = makeProvider();".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
import { name } from "./provider.js";
var other = require("./provider.js").other;
export default function() { return [name, other]; }
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("mixed consumer fixture should decompile");
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        // The authored named import stays named, beside the default import
        // the require became; its missing export is the input's own.
        assert!(
            consumer.contains("import provider, { name } from \"./provider.js\""),
            "{consumer}"
        );
        assert!(
            consumer.contains("const other = provider.other"),
            "{consumer}"
        );
    }

    #[test]
    fn a_mutated_commonjs_exports_object_keeps_its_provider_commonjs() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.value = 1;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var provider = require("./provider.js");
provider.value = 2;
module.exports = provider;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("mutable namespace fixture should decompile");
        // The consumer writes to the provider's `exports` object, which an
        // ES module does not have, so the provider stays CommonJS and the
        // consumer's default import is that object.
        let findings = validate_prepared_output(&output);
        assert!(findings.is_empty(), "{findings:#?}");
        let code = |name: &str| {
            output
                .modules
                .iter()
                .find(|module| module.filename == name)
                .map(|module| module.code.clone())
                .unwrap()
        };
        let provider = code("provider.js");
        assert!(provider.contains("exports.value = 1"), "{provider}");
        let consumer = code("consumer.js");
        assert!(
            consumer.contains("import provider from \"./provider.js\""),
            "{consumer}"
        );
    }

    #[test]
    fn provider_facts_leave_export_star_surfaces_unresolved() {
        let modules = vec![
            UnpackedModule {
                id: "star-source".to_string(),
                code: "exports.other = 1;".to_string(),
                filename: "star-source.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "provider".to_string(),
                code: r#"
module.exports = { value: 1 };
export * from "./star-source.js";
"#
                .to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var value = require("./provider.js").value;
module.exports = value;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("export-star provider fixture should decompile");
        let findings = validate_prepared_output(&output);
        assert!(
            findings.iter().any(|finding| {
                finding.kind == OutputFindingKind::MissingImportedName
                    && finding.filename == "consumer.js"
            }),
            "an open export-star surface must fail closed instead of treating local absence as proof: {findings:#?}"
        );
    }

    #[test]
    fn provider_facts_do_not_rewrite_authored_esm_imports() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "export default { value: 1 };".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"import { value } from "./provider.js"; console.log(value);"#.to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("authored ESM fixture should decompile");
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected ESM consumer");
        assert!(
            consumer.contains("import { value }"),
            "authored ESM imports must remain outside CommonJS repair:\n{consumer}"
        );
    }

    #[test]
    fn unpack_keeps_exported_builtin_alias_declared() {
        // `export { create }` names the alias binding, so the alias inliner
        // leaves the declaration alone instead of removing it and relying on
        // stale-export pruning to drop the specifier (which would also drop
        // the export from every consumer's point of view).
        let modules = vec![UnpackedModule {
            id: "helper".to_string(),
            is_entry: false,
            code: r#"
var create = Object.create;
function wrap(value) {
    return create(value);
}
export { create, wrap };
"#
            .to_string(),
            filename: "helper.js".to_string(),
            ..Default::default()
        }];

        let output = unpack_multi_module(
            modules,
            DecompileOptions {
                level: RewriteLevel::Standard,
                ..Default::default()
            },
        )
        .expect("module should decompile");
        let code = &output.modules[0].code;

        assert!(
            code.contains("= Object.create;"),
            "exported alias declaration should survive:\n{code}"
        );
        assert!(
            code.contains("create") && code.contains("wrap"),
            "both exports should be preserved:\n{code}"
        );
        let findings = crate::validate_output_modules(&[("helper.js".to_string(), code.clone())]);
        assert!(findings.is_empty(), "{findings:#?}");
    }

    #[test]
    fn normal_unpack_phase_preserves_helper_declaration_order() {
        let modules = vec![UnpackedModule {
            id: "entry".to_string(),
            is_entry: true,
            code: r#"
setup();
const { defineProperty } = Object;
var helper = (target) => defineProperty({}, "x", { value: target });
function setup() {
    return helper;
}
export { helper };
"#
            .to_string(),
            filename: "entry.js".to_string(),
            ..Default::default()
        }];

        let output = unpack_multi_module(
            modules,
            DecompileOptions {
                level: RewriteLevel::Minimal,
                ..Default::default()
            },
        )
        .expect("module should decompile");
        let code = &output.modules[0].code;
        let setup_call = code.find("setup()").expect("setup call should remain");
        let define_property = code
            .find("defineProperty } = Object")
            .expect("Object destructuring helper should remain");
        let helper = code.find("helper =").expect("helper binding should remain");

        assert!(
            setup_call < define_property && define_property < helper,
            "normal unpack should preserve declaration order; raw runnable cleanup owns helper hoisting:\n{code}"
        );
    }

    #[test]
    fn unpack_emit_source_map_uses_phase2_parser_source_map() {
        let modules = vec![UnpackedModule {
            id: "entry".to_string(),
            is_entry: true,
            code: "const value = input + 1;\nexport { value };".to_string(),
            filename: "entry.js".to_string(),
            ..Default::default()
        }];

        let output = unpack_multi_module(
            modules,
            DecompileOptions {
                emit_source_map: true,
                ..Default::default()
            },
        )
        .expect("module should decompile with source maps");

        let source_map = output.modules[0]
            .source_map
            .as_deref()
            .expect("unpack should emit one source map per kept module");
        let sm = sourcemap::SourceMap::from_reader(source_map.as_bytes())
            .expect("source map should parse");
        assert_eq!(sm.get_file(), Some("entry.js"));
        assert!(
            sm.get_token_count() > 0,
            "source map should contain generated-to-input mappings"
        );
    }

    #[test]
    fn unpack_rejects_bundle_level_input_source_map() {
        let modules = vec![UnpackedModule {
            id: "entry".to_string(),
            is_entry: true,
            code: "export const value = 1;".to_string(),
            filename: "entry.js".to_string(),
            ..Default::default()
        }];

        let error = unpack_multi_module(
            modules,
            DecompileOptions {
                sourcemap: Some(br#"{"version":3,"sources":[],"names":[],"mappings":""}"#.to_vec()),
                ..Default::default()
            },
        )
        .expect_err("bundle-level input maps must be rejected before module renaming");
        assert!(error.to_string().contains(
            "input source maps are not supported with unpacking because extracted module coordinates differ from bundle coordinates"
        ));
    }
}
