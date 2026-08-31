//! Multi-source merge preparation and cross-input reference rewriting.
//!
//! When unpacking multiple input files at once, extracted module filenames are
//! uniqued across inputs. Relative references are updated when that uniquing
//! renames sibling outputs, and numeric `require(<id>)` / async-chunk
//! references are rewritten when the target id is unambiguous across inputs.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use rayon::prelude::*;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, Mark, SourceMap, SyntaxContext, DUMMY_SP, GLOBALS};
use swc_core::ecma::ast::{
    CallExpr, Callee, Expr, ExprOrSpread, Lit, MemberExpr, MemberProp, Module, Str,
};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::super::io::{apply_fixer, parse_js, print_js};
use super::super::types::{
    PreparedInputId, PreparedModuleOutput, PreparedModuleProvenance, PreparedUnpackOutput,
    UnpackWarning, UnpackWarningKind,
};
use super::filename_recovery::{rewrite_import_sources, rewrite_import_sources_after_move};
use crate::module_path::relative_import_specifier;
use crate::unpacker::UnpackedModule;
use crate::unpacker::{DetectedModuleFailure, PreparedModuleAst};
use crate::utils::paren::{strip_parens, strip_parens_mut};

pub(super) struct MultiSourceModule {
    module: UnpackedModule,
    prepared: Option<PreparedModuleAst>,
    detector_failure: Option<DetectedModuleFailure>,
    /// Webpack initializes every factory's `module.exports` to `{}`. An empty
    /// normalized factory has no statement that can carry that runtime value
    /// into ESM recovery, so normal processing restores it explicitly.
    implicit_commonjs_default_object: bool,
    /// The module retains the exact identity of a structurally extracted
    /// webpack factory. Normal processing may therefore rely on webpack's
    /// initialized `module` / `exports` runtime bindings; raw output remains
    /// detector passthrough.
    webpack_commonjs_runtime: bool,
    /// Numeric module identity retained from the original webpack container
    /// key. `None` includes named ids and synthetic recursive children.
    webpack_numeric_module_id: Option<f64>,
    /// Whether the original container proves webpack 4's `module.i` spelling.
    webpack_legacy_module_i: bool,
    /// A structural detector lifted this code out of a callable body. If ESM
    /// recovery leaves a function-level return at module scope, the final
    /// pipeline may restore that boundary after imports have been recovered.
    restore_lifted_function_boundary: bool,
    allow_cross_chunk_rewrite: bool,
    report_import_cycle_warnings: bool,
    /// The source container registers its modules for consumption by runtimes
    /// in other physical assets (standalone lazy chunk): dead-module
    /// elimination must fail closed for this module.
    external_consumers: bool,
    chunk_ids: Arc<HashSet<usize>>,
    input_filename: String,
    input_group: String,
    input: Option<PreparedInputId>,
}

impl MultiSourceModule {
    #[cfg(test)]
    pub(super) fn detected(
        module: UnpackedModule,
        chunk_ids: impl Into<Arc<HashSet<usize>>>,
        input_filename: String,
        report_import_cycle_warnings: bool,
    ) -> Self {
        Self::detected_with_ast(
            module,
            None,
            chunk_ids,
            input_filename,
            report_import_cycle_warnings,
        )
    }

    #[cfg(test)]
    pub(super) fn detected_with_ast(
        module: UnpackedModule,
        prepared: Option<PreparedModuleAst>,
        chunk_ids: impl Into<Arc<HashSet<usize>>>,
        input_filename: String,
        report_import_cycle_warnings: bool,
    ) -> Self {
        let input_group = input_group_for_filename(&input_filename);
        Self::detected_with_ast_from_input(
            module,
            prepared,
            chunk_ids,
            input_filename,
            None,
            input_group,
            report_import_cycle_warnings,
        )
    }

    pub(super) fn detected_with_ast_from_input(
        module: UnpackedModule,
        prepared: Option<PreparedModuleAst>,
        chunk_ids: impl Into<Arc<HashSet<usize>>>,
        input_filename: String,
        input: Option<PreparedInputId>,
        input_group: String,
        report_import_cycle_warnings: bool,
    ) -> Self {
        Self {
            module,
            prepared,
            detector_failure: None,
            implicit_commonjs_default_object: false,
            webpack_commonjs_runtime: false,
            webpack_numeric_module_id: None,
            webpack_legacy_module_i: false,
            restore_lifted_function_boundary: true,
            allow_cross_chunk_rewrite: true,
            report_import_cycle_warnings,
            external_consumers: false,
            chunk_ids: chunk_ids.into(),
            input_filename,
            input_group,
            input,
        }
    }

    pub(super) fn with_implicit_commonjs_default_object(mut self, enabled: bool) -> Self {
        self.implicit_commonjs_default_object = enabled;
        self
    }

    pub(super) fn with_external_consumers(mut self, enabled: bool) -> Self {
        self.external_consumers = enabled;
        self
    }

    pub(super) fn with_webpack_commonjs_runtime(mut self, enabled: bool) -> Self {
        self.webpack_commonjs_runtime = enabled;
        self
    }

    pub(super) fn with_webpack_numeric_module_id(mut self, module_id: Option<f64>) -> Self {
        self.webpack_numeric_module_id = module_id;
        self
    }

    pub(super) fn with_webpack_legacy_module_i(mut self, enabled: bool) -> Self {
        self.webpack_legacy_module_i = enabled;
        self
    }

    pub(super) fn with_cross_chunk_rewrite(mut self, enabled: bool) -> Self {
        self.allow_cross_chunk_rewrite = enabled;
        self
    }

    pub(super) fn with_detector_failure(mut self, failure: Option<DetectedModuleFailure>) -> Self {
        self.detector_failure = failure;
        if failure.is_some() {
            // An opaque factory cannot safely participate as either caller or
            // provider in synthesized cross-chunk edges.
            self.allow_cross_chunk_rewrite = false;
        }
        self
    }

    pub(super) fn fallback_with_ast_from_input(
        module: UnpackedModule,
        prepared: Option<PreparedModuleAst>,
        input: Option<PreparedInputId>,
    ) -> Self {
        Self {
            module,
            prepared,
            detector_failure: None,
            implicit_commonjs_default_object: false,
            webpack_commonjs_runtime: false,
            webpack_numeric_module_id: None,
            webpack_legacy_module_i: false,
            restore_lifted_function_boundary: false,
            allow_cross_chunk_rewrite: false,
            report_import_cycle_warnings: false,
            external_consumers: false,
            chunk_ids: Arc::default(),
            input_filename: String::new(),
            input_group: String::new(),
            input,
        }
    }
}

pub(super) struct PreparedUnpackModule {
    pub(super) module: UnpackedModule,
    pub(super) prepared: Option<PreparedModuleAst>,
    pub(super) detector_failure: Option<DetectedModuleFailure>,
    pub(super) implicit_commonjs_default_object: bool,
    pub(super) webpack_commonjs_runtime: bool,
    pub(super) webpack_numeric_module_id: Option<f64>,
    pub(super) webpack_legacy_module_i: bool,
    pub(super) restore_lifted_function_boundary: bool,
    pub(super) numeric_rewrite: Option<NumericRewriteModuleContext>,
    pub(super) filename_rewrite: Option<FilenameRewriteModuleContext>,
    pub(super) report_import_cycle_warnings: bool,
    /// See [`MultiSourceModule::external_consumers`].
    pub(super) external_consumers: bool,
    pub(super) input: Option<PreparedInputId>,
    pub(super) reserved_public_path: bool,
}

impl PreparedUnpackModule {
    #[cfg(test)]
    pub(super) fn plain(module: UnpackedModule) -> Self {
        Self {
            module,
            prepared: None,
            detector_failure: None,
            implicit_commonjs_default_object: false,
            webpack_commonjs_runtime: false,
            webpack_numeric_module_id: None,
            webpack_legacy_module_i: false,
            restore_lifted_function_boundary: false,
            numeric_rewrite: None,
            filename_rewrite: None,
            report_import_cycle_warnings: true,
            external_consumers: false,
            input: None,
            reserved_public_path: false,
        }
    }

    #[cfg(test)]
    pub(super) fn with_cycle_warnings(
        module: UnpackedModule,
        report_import_cycle_warnings: bool,
    ) -> Self {
        Self {
            module,
            prepared: None,
            detector_failure: None,
            implicit_commonjs_default_object: false,
            webpack_commonjs_runtime: false,
            webpack_numeric_module_id: None,
            webpack_legacy_module_i: false,
            restore_lifted_function_boundary: false,
            numeric_rewrite: None,
            filename_rewrite: None,
            report_import_cycle_warnings,
            external_consumers: false,
            input: None,
            reserved_public_path: false,
        }
    }
}

pub(super) struct NumericRewriteModuleContext {
    input_group: String,
    module_filename: String,
}

pub(super) struct FilenameRewriteModuleContext {
    original_filename: String,
    authored_from_filename: Option<String>,
    rename_map: Arc<HashMap<String, String>>,
}

#[derive(Default)]
pub(super) struct NumericRewritePlan {
    plain_id_to_filename: HashMap<usize, String>,
    chunk_id_to_filename: HashMap<(String, usize, usize), String>,
}

impl NumericRewritePlan {
    pub(super) fn is_empty(&self) -> bool {
        self.plain_id_to_filename.is_empty() && self.chunk_id_to_filename.is_empty()
    }
}

/// Public output paths planned for the prepared inputs of one unpack run.
#[derive(Default)]
pub(super) struct PlannedPublicPaths {
    /// Facade candidates (esbuild-ESM / scope-hoisted inputs): reserved paths
    /// whose non-entry modules are namespaced beneath the facade.
    pub(super) facade: HashMap<PreparedInputId, String>,
    /// Every physical relative-ESM identity's public path. Plain inputs keep
    /// theirs as the provisional module filename so sibling imports between
    /// inputs stay resolvable (a basename flatten would displace same-named
    /// files). Script-loaded bundle inputs do not appear here.
    pub(super) input: HashMap<PreparedInputId, String>,
}

impl PlannedPublicPaths {
    fn module_holds_reserved_path(&self, module: &MultiSourceModule) -> bool {
        module.input.is_some_and(|input| {
            self.input
                .get(&input)
                .is_some_and(|path| path == &module.module.filename)
                && (module.module.is_entry || !self.facade.contains_key(&input))
        })
    }
}

pub(super) fn prepare_multi_source_modules(
    mut modules: Vec<MultiSourceModule>,
    public_paths: &PlannedPublicPaths,
) -> (Vec<PreparedUnpackModule>, NumericRewritePlan) {
    let span = tracing::info_span!("prepare_multi_source_modules", count = modules.len());
    let _enter = span.enter();
    let original_filenames = modules
        .iter()
        .map(|module| module.module.filename.clone())
        .collect::<Vec<_>>();
    apply_public_path_reservations(&mut modules, &public_paths.facade);
    assign_unique_module_filenames(&mut modules, public_paths);
    let mut filename_maps = HashMap::<PreparedInputId, HashMap<String, String>>::new();
    for (module, original_filename) in modules.iter().zip(&original_filenames) {
        if let Some(input) = module.input {
            filename_maps
                .entry(input)
                .or_default()
                .insert(original_filename.clone(), module.module.filename.clone());
        }
    }
    filename_maps.retain(|_, rename_map| {
        rename_map
            .iter()
            .any(|(original, final_name)| original != final_name)
    });
    let filename_maps = filename_maps
        .into_iter()
        .map(|(input, rename_map)| (input, Arc::new(rename_map)))
        .collect::<HashMap<_, _>>();
    let numeric_rewrite_plan = NumericRewritePlan {
        plain_id_to_filename: unique_numeric_module_id_map(&modules),
        chunk_id_to_filename: unique_numeric_chunk_module_id_map(&modules),
    };
    let has_rewrites = !numeric_rewrite_plan.is_empty();

    let modules = modules
        .into_iter()
        .zip(original_filenames)
        .map(|(module, original_filename)| {
            let reserved_public_path = public_paths.module_holds_reserved_path(&module);
            let numeric_rewrite = if has_rewrites && module.allow_cross_chunk_rewrite {
                Some(NumericRewriteModuleContext {
                    input_group: module.input_group,
                    module_filename: module.module.filename.clone(),
                })
            } else {
                None
            };
            let filename_rewrite = module.input.and_then(|input| {
                filename_maps
                    .get(&input)
                    .cloned()
                    .map(|rename_map| FilenameRewriteModuleContext {
                        original_filename,
                        authored_from_filename: if module.module.is_entry {
                            None
                        } else {
                            public_paths.facade.get(&input).cloned()
                        },
                        rename_map,
                    })
            });
            PreparedUnpackModule {
                reserved_public_path,
                module: module.module,
                prepared: module.prepared,
                detector_failure: module.detector_failure,
                implicit_commonjs_default_object: module.implicit_commonjs_default_object,
                webpack_commonjs_runtime: module.webpack_commonjs_runtime,
                webpack_numeric_module_id: module.webpack_numeric_module_id,
                webpack_legacy_module_i: module.webpack_legacy_module_i,
                restore_lifted_function_boundary: module.restore_lifted_function_boundary,
                numeric_rewrite,
                filename_rewrite,
                report_import_cycle_warnings: module.report_import_cycle_warnings,
                external_consumers: module.external_consumers,
                input: module.input,
            }
        })
        .collect();

    (modules, numeric_rewrite_plan)
}

pub(super) fn apply_filename_rewrites(
    module: &mut Module,
    unresolved_mark: Mark,
    context: Option<&FilenameRewriteModuleContext>,
) {
    let Some(context) = context else {
        return;
    };
    if let Some(authored_from_filename) = &context.authored_from_filename {
        rewrite_import_sources_after_move(
            module,
            &context.original_filename,
            authored_from_filename,
            &context.rename_map,
            unresolved_mark,
        );
    } else {
        rewrite_import_sources(
            module,
            &context.original_filename,
            &context.rename_map,
            unresolved_mark,
        );
    }
}

fn apply_public_path_reservations(
    modules: &mut [MultiSourceModule],
    public_paths: &HashMap<PreparedInputId, String>,
) {
    for module in modules {
        let Some(public_path) = module.input.and_then(|input| public_paths.get(&input)) else {
            continue;
        };
        if module.module.is_entry {
            module.module.filename = public_path.clone();
        } else {
            module.module.filename = super::scope_split::public_path_child_filename(
                public_path,
                &module.module.filename,
            );
        }
    }
}

fn assign_unique_module_filenames(
    modules: &mut [MultiSourceModule],
    public_paths: &PlannedPublicPaths,
) {
    let mut seen = public_paths
        .input
        .values()
        .map(|path| path.to_lowercase())
        .collect::<HashSet<_>>();
    for module in modules {
        // A module holding its input's planned public path keeps it: the
        // reserved facade entry, or the single module of a non-facade input.
        // (Facade children are namespaced beneath the facade and never carry
        // the facade path itself, so they always fall through to dedup.)
        let keeps_planned_path = public_paths.module_holds_reserved_path(module);
        if keeps_planned_path {
            continue;
        }
        module.module.filename = deduplicate_module_filename(&module.module.filename, &mut seen);
    }
}

fn deduplicate_module_filename(filename: &str, seen: &mut HashSet<String>) -> String {
    crate::unpacker::emit_esm::dedup_filename(
        filename,
        seen,
        crate::unpacker::emit_esm::FilenameDedupStyle::PathAware {
            fallback_stem: "module",
        },
    )
}

pub(super) fn input_group_for_filename(filename: &str) -> String {
    let parent = std::path::Path::new(filename)
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""));
    normalize_input_group_path(parent)
}

fn normalize_input_group_path(path: &std::path::Path) -> String {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    normalize_path_lexically(&path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub(super) fn normalize_path_lexically(path: &std::path::Path) -> std::path::PathBuf {
    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn chunk_filename_matches_id(filename: &str, chunk_id: usize) -> bool {
    let Some(name) = std::path::Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return false;
    };
    name == format!("{chunk_id}.js") || name == format!("{chunk_id}.bundle.js")
}

fn unique_numeric_module_id_map(modules: &[MultiSourceModule]) -> HashMap<usize, String> {
    let mut counts: HashMap<usize, (usize, String)> = HashMap::new();
    for module in modules {
        if !module.allow_cross_chunk_rewrite {
            continue;
        }
        let Ok(id) = module.module.id.parse::<usize>() else {
            continue;
        };
        let entry = counts
            .entry(id)
            .or_insert((0, module.module.filename.clone()));
        entry.0 += 1;
        entry.1 = module.module.filename.clone();
    }

    counts
        .into_iter()
        .filter_map(|(key, (count, filename))| (count == 1).then_some((key, filename)))
        .collect()
}

fn unique_numeric_chunk_module_id_map(
    modules: &[MultiSourceModule],
) -> HashMap<(String, usize, usize), String> {
    let mut counts: HashMap<(String, usize, usize), (usize, String)> = HashMap::new();
    for module in modules {
        if !module.allow_cross_chunk_rewrite || module.chunk_ids.is_empty() {
            continue;
        }
        let Ok(id) = module.module.id.parse::<usize>() else {
            continue;
        };
        for chunk_id in module.chunk_ids.iter() {
            if !chunk_filename_matches_id(&module.input_filename, *chunk_id) {
                continue;
            }
            let entry = counts
                .entry((module.input_group.clone(), *chunk_id, id))
                .or_insert((0, module.module.filename.clone()));
            entry.0 += 1;
            entry.1 = module.module.filename.clone();
        }
    }

    counts
        .into_iter()
        .filter_map(|(key, (count, filename))| (count == 1).then_some((key, filename)))
        .collect()
}

pub(super) fn apply_numeric_rewrites(
    module: &mut Module,
    unresolved_mark: Mark,
    context: Option<&NumericRewriteModuleContext>,
    plan: &NumericRewritePlan,
) {
    let Some(context) = context else {
        return;
    };
    if plan.is_empty() {
        return;
    }

    module.visit_mut_with(&mut WebpackNumericReferenceRewriter {
        input_group: &context.input_group,
        module_filename: &context.module_filename,
        unresolved_mark,
        plain_id_to_filename: &plan.plain_id_to_filename,
        chunk_id_to_filename: &plan.chunk_id_to_filename,
    });
}

pub(super) fn emit_raw_modules_with_numeric_rewrites(
    modules: Vec<PreparedUnpackModule>,
    numeric_rewrite_plan: NumericRewritePlan,
) -> Result<PreparedUnpackOutput> {
    if numeric_rewrite_plan.is_empty()
        && modules
            .iter()
            .all(|module| module.filename_rewrite.is_none())
    {
        return Ok(PreparedUnpackOutput {
            modules: modules
                .into_iter()
                .map(|prepared| PreparedModuleOutput {
                    filename: prepared.module.filename,
                    code: prepared.module.code,
                    source_map: None,
                    provenance: PreparedModuleProvenance {
                        input: prepared.input,
                        ranges: prepared.module.source_ranges,
                        inspection_context_ranges: prepared.module.inspection_context_ranges,
                        is_entry: prepared.module.is_entry,
                    },
                })
                .collect(),
            warnings: Vec::new(),
            detected_formats: Vec::new(),
        });
    }

    let processed = modules
        .into_par_iter()
        .map(|unpacked| {
            let provenance = PreparedModuleProvenance {
                input: unpacked.input,
                ranges: unpacked.module.source_ranges,
                inspection_context_ranges: unpacked.module.inspection_context_ranges,
                is_entry: unpacked.module.is_entry,
            };
            match GLOBALS.set(&Default::default(), || {
                let cm: Lrc<SourceMap> = Default::default();
                let mut module =
                    parse_js(&unpacked.module.code, &unpacked.module.filename, cm.clone())?;
                let unresolved_mark = Mark::new();
                let top_level_mark = Mark::new();
                module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
                apply_filename_rewrites(
                    &mut module,
                    unresolved_mark,
                    unpacked.filename_rewrite.as_ref(),
                );
                apply_numeric_rewrites(
                    &mut module,
                    unresolved_mark,
                    unpacked.numeric_rewrite.as_ref(),
                    &numeric_rewrite_plan,
                );
                apply_fixer(&mut module)?;
                print_js(&module, cm)
            }) {
                Ok(code) => (
                    PreparedModuleOutput {
                        filename: unpacked.module.filename,
                        code,
                        source_map: None,
                        provenance,
                    },
                    None,
                ),
                Err(e) => {
                    let warning = UnpackWarning::new(
                        unpacked.module.filename.clone(),
                        UnpackWarningKind::RawNormalizationFailed,
                        format!("raw reference rewrite failed, preserving unparsed code: {e}"),
                    );
                    (
                        PreparedModuleOutput {
                            filename: unpacked.module.filename,
                            code: unpacked.module.code,
                            source_map: None,
                            provenance,
                        },
                        Some(warning),
                    )
                }
            }
        })
        .collect::<Vec<_>>();

    let mut modules = Vec::new();
    let mut warnings = Vec::new();
    for (module, warning) in processed {
        modules.push(module);
        if let Some(warning) = warning {
            warnings.push(warning);
        }
    }

    Ok(PreparedUnpackOutput {
        modules,
        warnings,
        detected_formats: Vec::new(),
    })
}

struct WebpackNumericReferenceRewriter<'a> {
    input_group: &'a str,
    module_filename: &'a str,
    unresolved_mark: Mark,
    plain_id_to_filename: &'a HashMap<usize, String>,
    chunk_id_to_filename: &'a HashMap<(String, usize, usize), String>,
}

impl VisitMut for WebpackNumericReferenceRewriter<'_> {
    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        self.rewrite_async_chunk_t_bind(call);
        call.visit_mut_children_with(self);
        self.rewrite_plain_require(call);
    }
}

impl WebpackNumericReferenceRewriter<'_> {
    fn rewrite_plain_require(&self, call: &mut CallExpr) {
        let Callee::Expr(callee_expr) = &call.callee else {
            return;
        };
        let Expr::Ident(callee) = strip_parens(callee_expr) else {
            return;
        };
        if callee.sym.as_ref() != "require" || callee.ctxt.outer() != self.unresolved_mark {
            return;
        }

        let Some(module_id) = numeric_single_arg_id(call) else {
            return;
        };
        let Some(filename) = self.plain_id_to_filename.get(&module_id) else {
            return;
        };
        rewrite_numeric_arg_to_filename(&mut call.args[0], self.module_filename, filename);
    }

    fn rewrite_async_chunk_t_bind(&self, call: &mut CallExpr) {
        let Some((runtime, chunk_id)) = self.extract_then_chunk_loader(&call.callee) else {
            return;
        };
        let Some(arg) = call.args.first_mut() else {
            return;
        };
        if arg.spread.is_some() {
            return;
        }
        let Expr::Call(bind_call) = strip_parens_mut(&mut arg.expr) else {
            return;
        };
        self.rewrite_t_bind_module_arg(bind_call, &runtime, chunk_id);
    }

    fn extract_then_chunk_loader(&self, callee: &Callee) -> Option<(RuntimeIdent, usize)> {
        let Callee::Expr(callee_expr) = callee else {
            return None;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(callee_expr) else {
            return None;
        };
        if !member_prop_is(prop, "then") {
            return None;
        }

        let Expr::Call(load_call) = strip_parens(obj) else {
            return None;
        };
        self.extract_runtime_member_numeric_arg(load_call, "e", 0)
    }

    fn rewrite_t_bind_module_arg(
        &self,
        call: &mut CallExpr,
        expected_runtime: &RuntimeIdent,
        chunk_id: usize,
    ) {
        let Callee::Expr(callee_expr) = &call.callee else {
            return;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(callee_expr) else {
            return;
        };
        if !member_prop_is(prop, "bind") {
            return;
        }
        let Some(runtime) = self.extract_runtime_t_member(obj) else {
            return;
        };
        if &runtime != expected_runtime {
            return;
        }

        let Some(this_arg) = call.args.first() else {
            return;
        };
        if this_arg.spread.is_some() {
            return;
        }
        let Expr::Ident(this_ident) = strip_parens(&this_arg.expr) else {
            return;
        };
        if runtime != RuntimeIdent::from_ident(this_ident) {
            return;
        }

        self.rewrite_chunk_module_arg(&mut call.args, 1, chunk_id);
    }

    fn extract_runtime_member_numeric_arg(
        &self,
        call: &CallExpr,
        expected_prop: &str,
        arg_index: usize,
    ) -> Option<(RuntimeIdent, usize)> {
        let Callee::Expr(callee_expr) = &call.callee else {
            return None;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(callee_expr) else {
            return None;
        };
        if !member_prop_is(prop, expected_prop) {
            return None;
        }
        let Expr::Ident(runtime) = strip_parens(obj) else {
            return None;
        };
        let arg = call.args.get(arg_index)?;
        if arg.spread.is_some() {
            return None;
        }
        let module_id = numeric_arg_id(&arg.expr)?;
        Some((RuntimeIdent::from_ident(runtime), module_id))
    }

    fn extract_runtime_t_member(&self, expr: &Expr) -> Option<RuntimeIdent> {
        let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(expr) else {
            return None;
        };
        if !member_prop_is(prop, "t") {
            return None;
        }
        let Expr::Ident(runtime) = strip_parens(obj) else {
            return None;
        };
        Some(RuntimeIdent::from_ident(runtime))
    }

    fn rewrite_chunk_module_arg(&self, args: &mut [ExprOrSpread], index: usize, chunk_id: usize) {
        let Some(arg) = args.get_mut(index) else {
            return;
        };
        if arg.spread.is_some() {
            return;
        }
        let Some(module_id) = numeric_arg_id(&arg.expr) else {
            return;
        };
        let Some(filename) =
            self.chunk_id_to_filename
                .get(&(self.input_group.to_string(), chunk_id, module_id))
        else {
            return;
        };
        rewrite_numeric_arg_to_filename(arg, self.module_filename, filename);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RuntimeIdent {
    sym: Atom,
    ctxt: SyntaxContext,
}

impl RuntimeIdent {
    fn from_ident(ident: &swc_core::ecma::ast::Ident) -> Self {
        Self {
            sym: ident.sym.clone(),
            ctxt: ident.ctxt,
        }
    }
}

fn numeric_arg_id(expr: &Expr) -> Option<usize> {
    let Expr::Lit(Lit::Num(number)) = strip_parens(expr) else {
        return None;
    };
    let value = number.value;
    if value < 0.0 || value.fract() != 0.0 {
        return None;
    }
    Some(value as usize)
}

fn numeric_single_arg_id(call: &CallExpr) -> Option<usize> {
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return None;
    }
    numeric_arg_id(&call.args[0].expr)
}

fn rewrite_numeric_arg_to_filename(arg: &mut ExprOrSpread, from_filename: &str, filename: &str) {
    let path = relative_import_specifier(from_filename, filename);
    *arg.expr = Expr::Lit(Lit::Str(Str {
        span: DUMMY_SP,
        value: path.into(),
        raw: None,
    }));
}

fn member_prop_is(prop: &MemberProp, expected: &str) -> bool {
    matches!(prop, MemberProp::Ident(ident) if ident.sym.as_ref() == expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detected_modules_share_chunk_id_storage() {
        let chunk_ids = Arc::new(HashSet::from([7, 11, 13]));
        let first = MultiSourceModule::detected(
            UnpackedModule::default(),
            chunk_ids.clone(),
            "chunk-7.js".to_string(),
            true,
        );
        let second = MultiSourceModule::detected(
            UnpackedModule::default(),
            chunk_ids,
            "chunk-11.js".to_string(),
            true,
        );

        assert!(Arc::ptr_eq(&first.chunk_ids, &second.chunk_ids));
    }

    #[test]
    fn prepared_detection_keeps_typed_input_index_and_precomputed_group() {
        let module = MultiSourceModule::detected_with_ast_from_input(
            UnpackedModule::default(),
            None,
            HashSet::new(),
            "input.js".to_string(),
            Some(PreparedInputId::from_index(0)),
            "precomputed-group".to_string(),
            true,
        );

        assert_eq!(module.input_group, "precomputed-group");
        assert_eq!(module.input.map(PreparedInputId::index), Some(0));
    }

    #[test]
    fn numeric_rewrite_paths_are_relative_to_nested_module() {
        assert_eq!(
            relative_import_specifier("module-200.js", "module-100.js"),
            "./module-100.js"
        );
        assert_eq!(
            relative_import_specifier("module-11111.js", "module-11111/chunk_value.js"),
            "./module-11111/chunk_value.js"
        );
        assert_eq!(
            relative_import_specifier("module-22222/chunk_value.js", "module-44444.js"),
            "../module-44444.js"
        );
        assert_eq!(
            relative_import_specifier("module-22222/chunk_value.js", "module-22222/chunk_other.js"),
            "./chunk_other.js"
        );
        assert_eq!(
            relative_import_specifier("module-22222/chunk_value.js", "module-33333/chunk_extra.js"),
            "../module-33333/chunk_extra.js"
        );
    }

    #[test]
    fn numeric_rewrite_plan_applies_to_existing_ast_without_source_stabilization() {
        let modules = vec![
            MultiSourceModule::detected(
                UnpackedModule {
                    id: "20".to_string(),
                    is_entry: false,
                    code: "const other = require(999);".to_string(),
                    filename: "module-20.js".to_string(),
                    ..Default::default()
                },
                HashSet::new(),
                "entry.js".to_string(),
                true,
            ),
            MultiSourceModule::detected(
                UnpackedModule {
                    id: "999".to_string(),
                    is_entry: false,
                    code: "export default 1;".to_string(),
                    filename: "module-999.js".to_string(),
                    ..Default::default()
                },
                HashSet::new(),
                "chunk.js".to_string(),
                true,
            ),
        ];

        let (prepared, plan) =
            prepare_multi_source_modules(modules, &PlannedPublicPaths::default());
        assert!(
            prepared[0].module.code.contains("require(999)"),
            "prepare should keep source strings untouched"
        );

        let output = GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = parse_js(
                &prepared[0].module.code,
                &prepared[0].module.filename,
                cm.clone(),
            )
            .expect("fixture should parse");
            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
            apply_numeric_rewrites(
                &mut module,
                unresolved_mark,
                prepared[0].numeric_rewrite.as_ref(),
                &plan,
            );
            apply_fixer(&mut module).expect("fixer should not panic on fixture");
            print_js(&module, cm).expect("fixture should print")
        });

        assert!(
            output.contains(r#"require("./module-999.js")"#),
            "rewrite plan should apply to the already-parsed AST:\n{output}"
        );
    }
}
