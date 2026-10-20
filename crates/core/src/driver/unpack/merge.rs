//! Multi-source merge preparation and cross-input reference rewriting.
//!
//! When unpacking multiple input files at once, extracted module filenames are
//! uniqued across inputs. Relative references are updated when that uniquing
//! renames sibling outputs, and webpack `require(<id>)` / async-chunk
//! references are rewritten when the target id is unambiguous among the
//! inputs of the referencing module's build: any numeric id, and a string id
//! only where the detector saw the factory load it and found it absent from
//! its own table.

use crate::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use rayon::prelude::*;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, Mark, SourceMap, SyntaxContext, DUMMY_SP, GLOBALS};
use swc_core::ecma::ast::{
    ArrowFunctionBody, CallExpr, Callee, Expr, ExprOrSpread, Lit, MemberExpr, MemberProp, Module,
    ReturnStmt, Stmt, Str,
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
use crate::unpacker::webpack4::{fake_namespace_mode_loads_id, require_id_arg_index};
use crate::unpacker::UnpackedModule;
use crate::unpacker::{DetectedModuleFailure, DetectedModuleNote, PreparedModuleAst};
use crate::utils::paren::{strip_parens, strip_parens_mut};

pub(super) struct MultiSourceModule {
    module: UnpackedModule,
    prepared: Option<PreparedModuleAst>,
    detector_failure: Option<DetectedModuleFailure>,
    detector_note: Option<DetectedModuleNote>,
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
    /// String ids the factory loads that its own table lacks. Only a call
    /// with one of these may link to a module of another input; any other
    /// string `require` is authored or already rewritten.
    webpack_absent_string_ids: Arc<HashSet<String>>,
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
    /// The input's build identity: the chunk-loading globals it pushes into or
    /// binds as a runtime (empty when it shows none). A module-id reference only
    /// links to a module of the same build.
    chunk_loading_globals: Arc<[String]>,
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
            detector_note: None,
            implicit_commonjs_default_object: false,
            webpack_commonjs_runtime: false,
            webpack_numeric_module_id: None,
            webpack_absent_string_ids: Arc::default(),
            webpack_legacy_module_i: false,
            restore_lifted_function_boundary: true,
            allow_cross_chunk_rewrite: true,
            report_import_cycle_warnings,
            external_consumers: false,
            chunk_ids: chunk_ids.into(),
            chunk_loading_globals: Arc::default(),
            input_filename,
            input_group,
            input,
        }
    }

    pub(super) fn with_chunk_loading_globals(mut self, globals: Arc<[String]>) -> Self {
        self.chunk_loading_globals = globals;
        self
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

    pub(super) fn with_webpack_absent_string_ids(mut self, ids: Arc<HashSet<String>>) -> Self {
        self.webpack_absent_string_ids = ids;
        self
    }

    pub(super) fn with_webpack_legacy_module_i(mut self, enabled: bool) -> Self {
        self.webpack_legacy_module_i = enabled;
        self
    }

    pub(super) fn with_detector_failure(mut self, failure: Option<DetectedModuleFailure>) -> Self {
        self.detector_failure = failure;
        self
    }

    pub(super) fn with_detector_note(mut self, note: Option<DetectedModuleNote>) -> Self {
        self.detector_note = note;
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
            detector_note: None,
            implicit_commonjs_default_object: false,
            webpack_commonjs_runtime: false,
            webpack_numeric_module_id: None,
            webpack_absent_string_ids: Arc::default(),
            webpack_legacy_module_i: false,
            restore_lifted_function_boundary: false,
            allow_cross_chunk_rewrite: false,
            report_import_cycle_warnings: false,
            external_consumers: false,
            chunk_ids: Arc::default(),
            chunk_loading_globals: Arc::default(),
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
    pub(super) detector_note: Option<DetectedModuleNote>,
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
            detector_note: None,
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
            detector_note: None,
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
    build: Arc<[String]>,
    absent_string_ids: Arc<HashSet<String>>,
}

pub(super) struct FilenameRewriteModuleContext {
    original_filename: String,
    authored_from_filename: Option<String>,
    rename_map: Arc<HashMap<String, String>>,
}

/// Every module that a numeric id, or a webpack string id, names across all
/// inputs. A reference resolves only when exactly one candidate belongs to
/// the referencing module's build and that candidate is healthy.
#[derive(Default)]
pub(super) struct NumericRewritePlan {
    plain_id_targets: HashMap<usize, Vec<NumericTarget>>,
    chunk_id_targets: HashMap<(String, usize, usize), Vec<NumericTarget>>,
    string_id_targets: HashMap<String, Vec<NumericTarget>>,
    can_rewrite: bool,
}

struct NumericTarget {
    /// `None` for an opaque factory. It still occupies its id, so a healthy
    /// same-numbered module elsewhere in the build cannot capture a
    /// deliberately unresolved reference.
    filename: Option<String>,
    build: Arc<[String]>,
}

impl NumericRewritePlan {
    fn new(
        plain_id_targets: HashMap<usize, Vec<NumericTarget>>,
        chunk_id_targets: HashMap<(String, usize, usize), Vec<NumericTarget>>,
        string_id_targets: HashMap<String, Vec<NumericTarget>>,
    ) -> Self {
        // Skip the rewrite pass when no candidate could ever resolve: every
        // candidate is opaque or shares its build with another candidate.
        let resolvable = |targets: &Vec<NumericTarget>| {
            targets.iter().any(|target| {
                target.filename.is_some()
                    && resolve_numeric_target(targets, &target.build).is_some()
            })
        };
        let can_rewrite = plain_id_targets.values().any(resolvable)
            || chunk_id_targets.values().any(resolvable)
            || string_id_targets.values().any(resolvable);
        Self {
            plain_id_targets,
            chunk_id_targets,
            string_id_targets,
            can_rewrite,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        !self.can_rewrite
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
    let mut filename_maps = HashMap::<PreparedInputId, HashMap<String, String>>::default();
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
    let numeric_rewrite_plan = NumericRewritePlan::new(
        numeric_module_id_targets(&modules),
        numeric_chunk_module_id_targets(&modules),
        string_module_id_targets(&modules),
    );
    let has_rewrites = !numeric_rewrite_plan.is_empty();

    let modules = modules
        .into_iter()
        .zip(original_filenames)
        .map(|(module, original_filename)| {
            let reserved_public_path = public_paths.module_holds_reserved_path(&module);
            let numeric_rewrite = if has_rewrites
                && module.allow_cross_chunk_rewrite
                && module.detector_failure.is_none()
            {
                Some(NumericRewriteModuleContext {
                    input_group: module.input_group,
                    module_filename: module.module.filename.clone(),
                    build: module.chunk_loading_globals.clone(),
                    absent_string_ids: module.webpack_absent_string_ids.clone(),
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
                detector_note: module.detector_note,
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

fn numeric_target(module: &MultiSourceModule) -> NumericTarget {
    NumericTarget {
        filename: module
            .detector_failure
            .is_none()
            .then(|| module.module.filename.clone()),
        build: module.chunk_loading_globals.clone(),
    }
}

fn numeric_module_id_targets(modules: &[MultiSourceModule]) -> HashMap<usize, Vec<NumericTarget>> {
    let mut targets: HashMap<usize, Vec<NumericTarget>> = HashMap::default();
    for module in modules {
        if !module.allow_cross_chunk_rewrite {
            continue;
        }
        let Ok(id) = module.module.id.parse::<usize>() else {
            continue;
        };
        targets.entry(id).or_default().push(numeric_target(module));
    }
    targets
}

/// Webpack factories by the property key of their table entry, which is what
/// a string loader call looks up: a numeric key `2638` answers `"2638"`.
/// Synthetic children of a split factory carry no detector identity, and an
/// entry module may be a synthesized startup whose id is no table key; both
/// are left out.
fn string_module_id_targets(modules: &[MultiSourceModule]) -> HashMap<String, Vec<NumericTarget>> {
    let mut targets: HashMap<String, Vec<NumericTarget>> = HashMap::default();
    for module in modules {
        if !module.allow_cross_chunk_rewrite
            || !module.webpack_commonjs_runtime
            || module.module.is_entry
        {
            continue;
        }
        targets
            .entry(module.module.id.clone())
            .or_default()
            .push(numeric_target(module));
    }
    targets
}

fn numeric_chunk_module_id_targets(
    modules: &[MultiSourceModule],
) -> HashMap<(String, usize, usize), Vec<NumericTarget>> {
    let mut targets: HashMap<(String, usize, usize), Vec<NumericTarget>> = HashMap::default();
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
            targets
                .entry((module.input_group.clone(), *chunk_id, id))
                .or_default()
                .push(numeric_target(module));
        }
    }
    targets
}

/// The one healthy candidate in `build`, if the build has exactly one.
fn resolve_numeric_target<'a>(targets: &'a [NumericTarget], build: &[String]) -> Option<&'a str> {
    let mut compatible = targets
        .iter()
        .filter(|target| same_build(&target.build, build));
    let target = compatible.next()?;
    if compatible.next().is_some() {
        return None;
    }
    target.filename.as_deref()
}

/// Inputs that name no chunk-loading global (CommonJS chunks and their
/// runtime, server chunks, other formats) keep the build-agnostic match they
/// have always had among themselves; an input that names one only matches an
/// input sharing a name.
fn same_build(left: &[String], right: &[String]) -> bool {
    if left.is_empty() || right.is_empty() {
        return left.is_empty() && right.is_empty();
    }
    left.iter().any(|name| right.contains(name))
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
        build: &context.build,
        absent_string_ids: &context.absent_string_ids,
        unresolved_mark,
        plan,
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
    build: &'a [String],
    absent_string_ids: &'a HashSet<String>,
    unresolved_mark: Mark,
    plan: &'a NumericRewritePlan,
}

impl VisitMut for WebpackNumericReferenceRewriter<'_> {
    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        self.rewrite_async_chunk_load(call);
        call.visit_mut_children_with(self);
        self.rewrite_plain_require(call);
    }
}

impl WebpackNumericReferenceRewriter<'_> {
    fn rewrite_plain_require(&self, call: &mut CallExpr) {
        // `require(id)`, the bound loader `require.bind(require, id)`, and
        // the fake-namespace loader `require.t(id, mode)` /
        // `require.t.bind(require, id, mode)`.
        let Some(index) = require_id_arg_index(call, &Atom::from("require"), self.unresolved_mark)
        else {
            return;
        };
        let targets = match strip_parens(&call.args[index].expr) {
            Expr::Lit(Lit::Str(request)) => request
                .value
                .as_str()
                .filter(|id| self.absent_string_ids.contains(*id))
                .and_then(|id| self.plan.string_id_targets.get(id)),
            expr => numeric_arg_id(expr).and_then(|id| self.plan.plain_id_targets.get(&id)),
        };
        let Some(filename) =
            targets.and_then(|targets| resolve_numeric_target(targets, self.build))
        else {
            return;
        };
        rewrite_numeric_arg_to_filename(&mut call.args[index], self.module_filename, filename);
    }

    /// The module id of a lazy `import()` behind a chunk load,
    /// `R.e(chunk).then(loader)`, where `loader` is one of
    ///
    /// - `R.bind(R, id)` or `R.t.bind(R, id, mode)` (`null` for webpack 4's
    ///   receiver), or
    /// - `() => R(id)` or `() => R.t(id, mode)`, or the same with a
    ///   parameterless `function` returning the call,
    ///
    /// and `R.t` takes a module id only when `mode & 1` is set.
    fn rewrite_async_chunk_load(&self, call: &mut CallExpr) {
        let Some((runtime, chunk_id)) = self.extract_then_chunk_loader(&call.callee) else {
            return;
        };
        let Some(arg) = call.args.first_mut() else {
            return;
        };
        if arg.spread.is_some() {
            return;
        }
        let load = match strip_parens_mut(&mut arg.expr) {
            Expr::Call(bind_call) => bind_call,
            Expr::Arrow(arrow) if arrow.params.is_empty() => match &mut *arrow.body {
                ArrowFunctionBody::Expr(body) => match strip_parens_mut(body) {
                    Expr::Call(load) => load,
                    _ => return,
                },
                ArrowFunctionBody::FunctionBody(body) => {
                    let Some(load) = single_returned_call(&mut body.stmts) else {
                        return;
                    };
                    load
                }
            },
            Expr::Fn(function) if function.function.params.is_empty() => {
                let Some(body) = function.function.body.as_mut() else {
                    return;
                };
                let Some(load) = single_returned_call(&mut body.stmts) else {
                    return;
                };
                load
            }
            _ => return,
        };
        if let Some(index) = self.loader_module_arg_index(load, &runtime) {
            self.rewrite_chunk_module_arg(&mut load.args, index, chunk_id);
        }
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

    /// Which argument of `call` holds the module id when `call` loads a
    /// module through `runtime`: `R(id)`, `R.t(id, mode)`, `R.bind(T, id)`, or
    /// `R.t.bind(T, id, mode)`, with `T` the runtime itself or `null`.
    fn loader_module_arg_index(&self, call: &CallExpr, runtime: &RuntimeIdent) -> Option<usize> {
        let Callee::Expr(callee_expr) = &call.callee else {
            return None;
        };
        let is_runtime = |expr: &Expr| {
            matches!(strip_parens(expr), Expr::Ident(ident)
                if RuntimeIdent::from_ident(ident) == *runtime)
        };
        let is_runtime_t = |expr: &Expr| {
            self.extract_runtime_t_member(expr)
                .is_some_and(|member_runtime| member_runtime == *runtime)
        };
        if call.args.iter().any(|arg| arg.spread.is_some()) {
            return None;
        }
        let callee = strip_parens(callee_expr);
        if is_runtime(callee) {
            return (call.args.len() == 1).then_some(0);
        }
        if is_runtime_t(callee) {
            return (call.args.len() == 2 && fake_namespace_mode_loads_id(&call.args[1].expr))
                .then_some(0);
        }
        let Expr::Member(MemberExpr { obj, prop, .. }) = callee else {
            return None;
        };
        if !member_prop_is(prop, "bind") {
            return None;
        }
        let this_arg = call.args.first()?;
        if !is_runtime(&this_arg.expr)
            && !matches!(strip_parens(&this_arg.expr), Expr::Lit(Lit::Null(_)))
        {
            return None;
        }
        if is_runtime(obj) {
            (call.args.len() == 2).then_some(1)
        } else if is_runtime_t(obj) {
            (call.args.len() == 3 && fake_namespace_mode_loads_id(&call.args[2].expr)).then_some(1)
        } else {
            None
        }
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
        let Some(filename) = self
            .plan
            .chunk_id_targets
            .get(&(self.input_group.to_string(), chunk_id, module_id))
            .and_then(|targets| resolve_numeric_target(targets, self.build))
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

/// The call `stmts` returns when they are a single `return call;`.
fn single_returned_call(stmts: &mut [Stmt]) -> Option<&mut CallExpr> {
    let [Stmt::Return(ReturnStmt { arg: Some(arg), .. })] = stmts else {
        return None;
    };
    match strip_parens_mut(arg) {
        Expr::Call(call) => Some(call),
        _ => None,
    }
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
    fn opaque_numeric_ids_remain_reserved_without_disabling_healthy_siblings() {
        let detected = |id: &str, filename: &str, input: &str| {
            MultiSourceModule::detected(
                UnpackedModule {
                    id: id.into(),
                    filename: filename.into(),
                    ..Default::default()
                },
                HashSet::from_iter([7]),
                input.into(),
                true,
            )
        };
        for duplicate in [false, true] {
            let mut modules = vec![
                detected("100", "module-100.js", "7.js"),
                detected("200", "module-200.js", "7.js").with_detector_failure(Some(
                    DetectedModuleFailure::WebpackRuntimeParameterReuse,
                )),
            ];
            if duplicate {
                modules.push(detected("200", "module-other.js", "7.bundle.js"));
            }
            let (prepared, plan) =
                prepare_multi_source_modules(modules, &PlannedPublicPaths::default());
            let plain = |id| resolve_numeric_target(&plan.plain_id_targets[&id], &[]);
            let chunk = |id| {
                plan.chunk_id_targets
                    .iter()
                    .filter(|((_, _, key), _)| *key == id)
                    .find_map(|(_, targets)| resolve_numeric_target(targets, &[]))
            };
            assert_eq!(plain(100), Some("module-100.js"));
            assert_eq!(plain(200), None);
            assert_eq!(chunk(100), Some("module-100.js"));
            assert_eq!(chunk(200), None);
            assert!(prepared[0].numeric_rewrite.is_some());
            assert!(prepared[1].numeric_rewrite.is_none());
        }
    }

    #[test]
    fn detected_modules_share_chunk_id_storage() {
        let chunk_ids = Arc::new(HashSet::from_iter([7, 11, 13]));
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
            HashSet::default(),
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
                HashSet::default(),
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
                HashSet::default(),
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
