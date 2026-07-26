use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::anyhow;

use crate::decompile::diagnostic_from_core;
use crate::error::{from_core_driver_error, Error, ErrorKind, Result};
use crate::options::{
    DceMode, ModuleMode, RewriteLevel, UnmatchedInput, UnpackMode, UnpackOptions,
};
use crate::output::{
    BundleFormat, Diagnostic, EntryStatus, InputAction, InputDetection, InputId, InputReceipt,
    InputReport, ModuleOutput, ModuleStatus, OutputSafety, SourceSpan, UnpackOutput,
};
use crate::source::Source;

/// Split one or more bundle/chunk inputs into modules and process each.
///
/// The inputs are treated as one logical bundle/chunk set: under
/// `ModuleMode::Decompile`, modules selected for processing participate in
/// the same cross-module fact graph.
///
/// This function is semantically equivalent to pushing the sources into one
/// [`UnpackJob`] in order and finishing it; the contract does not require a
/// literally serial push loop. Each physical input is detected at most once
/// per call, and a compatible plain JavaScript input is parsed at most once
/// before rules.
pub fn unpack(inputs: Vec<Source>, options: UnpackOptions) -> Result<UnpackOutput> {
    let mut job = UnpackJob::new(options)?;
    for input in inputs {
        job.push(input)?;
    }
    job.finish()
}

/// Incremental intake form of [`unpack`] for directory walkers and other
/// producers that should not retain every candidate source simultaneously.
///
/// [`push`](UnpackJob::push) detects and prepares one input immediately; a
/// skipped plain input's source is released before `push` returns, so peak
/// intake memory is bounded by retained detected/processed inputs rather
/// than every file visited by a walk. No separate boolean detection preflight
/// is needed.
pub struct UnpackJob {
    options: UnpackOptions,
    reports: Vec<InputReport>,
    retained: Vec<RetainedInput>,
    unmatched_error: Option<String>,
}

struct RetainedInput {
    id: InputId,
    prepared: wakaru_core::driver::PreparedUnpackInput,
    preserve: bool,
}

impl std::fmt::Debug for UnpackJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnpackJob")
            .field("options", &self.options)
            .field("reports", &self.reports)
            .field("retained_count", &self.retained.len())
            .field("unmatched_error", &self.unmatched_error)
            .finish()
    }
}

struct ProcessedInput {
    id: InputId,
}

impl UnpackJob {
    /// Create a job, validating option combinations (for example, raw mode
    /// with requested output source maps is `ErrorKind::InvalidOptions`).
    pub fn new(options: UnpackOptions) -> Result<Self> {
        if matches!(options.modules(), ModuleMode::Raw) && options.output_source_maps() {
            return Err(Error::new(
                ErrorKind::InvalidOptions,
                None,
                anyhow!("raw unpack mode does not support output source maps"),
            ));
        }
        if matches!(options.modules(), ModuleMode::Raw) && options.recovery().angular_components() {
            return Err(Error::new(
                ErrorKind::InvalidOptions,
                None,
                anyhow!("raw unpack mode does not support component recovery"),
            ));
        }
        Ok(Self {
            options,
            reports: Vec::new(),
            retained: Vec::new(),
            unmatched_error: None,
        })
    }

    /// Detect and prepare one input immediately.
    ///
    /// A skipped plain input's source is released before this method returns.
    /// A failed push does not add an input or consume an [`InputId`]; the job
    /// remains usable. A successful push returns the assigned ID and
    /// detection result so a walker can report progress before `finish`.
    pub fn push(&mut self, input: Source) -> Result<InputReceipt> {
        self.push_with_unmatched(input, self.options.unmatched())
    }

    /// Detects and prepares one input using an input-specific plain-source
    /// policy while retaining every other job option.
    ///
    /// This lets a single cross-module job process explicit files while
    /// treating directory-walk candidates as detection-only inputs.
    pub fn push_with_unmatched(
        &mut self,
        input: Source,
        unmatched: UnmatchedInput,
    ) -> Result<InputReceipt> {
        if input.source_map().is_some() {
            return Err(Error::new(
                ErrorKind::InvalidOptions,
                Some(input.filename().to_string()),
                anyhow!("input source maps are not supported by unpack"),
            ));
        }

        let parts = input.into_parts();
        let input_filename = parts.filename.clone();
        let prepared = wakaru_core::driver::prepare_unpack_input_with_policy(
            parts.filename,
            parts.code,
            matches!(self.options.modules(), ModuleMode::Decompile(_))
                && unmatched == UnmatchedInput::Process,
            core_scope_hoist_policy(&self.options),
        )
        .map_err(|error| {
            let kind = from_core_driver_error(error.kind());
            Error::new(kind, Some(input_filename), error.into_inner())
        })?;
        let detection = map_prepared_detection(prepared.detection());
        let id = InputId::from_index(self.reports.len());
        let (action, retain, preserve) = match detection {
            InputDetection::Structural(_) | InputDetection::HeuristicScopeHoisted => {
                (InputAction::Unpacked, true, false)
            }
            InputDetection::Plain => match unmatched {
                UnmatchedInput::Skip => (InputAction::Skipped, false, false),
                UnmatchedInput::Process => (InputAction::Processed, true, false),
                UnmatchedInput::Preserve => (InputAction::Preserved, true, true),
                UnmatchedInput::Error => {
                    self.unmatched_error
                        .get_or_insert_with(|| prepared.filename().to_string());
                    (InputAction::Processed, false, false)
                }
            },
        };

        self.reports.push(InputReport {
            id,
            filename: prepared.filename().to_string(),
            detection,
            action,
            module_indices: Vec::new(),
        });
        if retain {
            self.retained.push(RetainedInput {
                id,
                prepared,
                preserve,
            });
        }

        Ok(InputReceipt { id, detection })
    }

    /// Run the shared cross-module phases and materialize final output.
    ///
    /// Finishing a job with no successfully pushed inputs returns
    /// `ErrorKind::InvalidInput`. Finishing a non-empty job whose inputs were
    /// all skipped returns `Ok(UnpackOutput)` with zero modules and one
    /// skipped [`InputReport`] per input. If the job's
    /// [`UnmatchedInput::Error`] policy was violated by any pushed input,
    /// this returns `ErrorKind::InvalidInput`. All output ordering is
    /// deterministic regardless of thread scheduling.
    pub fn finish(mut self) -> Result<UnpackOutput> {
        if self.reports.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                None,
                anyhow!("at least one input is required"),
            ));
        }
        if let Some(filename) = self.unmatched_error {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                Some(filename.clone()),
                anyhow!("input {filename:?} is not a recognized bundle"),
            ));
        }

        let mut processed = Vec::new();
        let mut preserved = Vec::new();
        for input in self.retained {
            if input.preserve {
                preserved.push(input);
            } else {
                processed.push(input);
            }
        }

        let mut modules = Vec::new();
        let mut pre_rewrite_modules = Vec::new();
        let mut diagnostics = Vec::new();
        if !processed.is_empty() {
            let processed_meta = processed
                .iter()
                .map(|input| ProcessedInput { id: input.id })
                .collect::<Vec<_>>();
            let core_output = run_core_unpack(processed, &self.options)?;
            let converted = convert_core_output(
                core_output,
                &processed_meta,
                &mut self.reports,
                matches!(self.options.modules(), ModuleMode::Raw),
            );
            modules = converted.modules;
            pre_rewrite_modules = converted.pre_rewrite_modules;
            diagnostics = converted.diagnostics;
        }

        append_preserved_modules(&mut modules, &mut self.reports, preserved);
        let (artifacts, recovery_diagnostics) = crate::artifacts::recover_artifacts(
            &modules,
            &pre_rewrite_modules,
            self.options.recovery(),
            self.options.diagnostics(),
        );
        diagnostics.extend(recovery_diagnostics);

        Ok(UnpackOutput {
            modules,
            artifacts,
            inputs: self.reports,
            diagnostics,
            safety: if self.options.mode() == UnpackMode::Inspect {
                OutputSafety::InspectionOnly
            } else {
                OutputSafety::Normal
            },
        })
    }
}

fn map_prepared_detection(
    detection: wakaru_core::driver::PreparedInputDetection,
) -> InputDetection {
    match detection {
        wakaru_core::driver::PreparedInputDetection::Bundle(format) => map_bundle_detection(format),
        wakaru_core::driver::PreparedInputDetection::ScopeHoisted => {
            InputDetection::HeuristicScopeHoisted
        }
        wakaru_core::driver::PreparedInputDetection::Plain => InputDetection::Plain,
    }
}

fn map_bundle_detection(format: wakaru_core::BundleFormat) -> InputDetection {
    match format {
        wakaru_core::BundleFormat::Webpack5 => InputDetection::Structural(BundleFormat::Webpack5),
        wakaru_core::BundleFormat::Webpack4 => InputDetection::Structural(BundleFormat::Webpack4),
        wakaru_core::BundleFormat::Browserify => {
            InputDetection::Structural(BundleFormat::Browserify)
        }
        wakaru_core::BundleFormat::ClosureModuleManager => {
            InputDetection::Structural(BundleFormat::ClosureModuleManager)
        }
        wakaru_core::BundleFormat::Metro => InputDetection::Structural(BundleFormat::Metro),
        wakaru_core::BundleFormat::SystemJs => InputDetection::Structural(BundleFormat::SystemJs),
        wakaru_core::BundleFormat::Esbuild => InputDetection::Structural(BundleFormat::Esbuild),
        wakaru_core::BundleFormat::Amd => InputDetection::Structural(BundleFormat::Amd),
        wakaru_core::BundleFormat::ScopeHoisted => InputDetection::HeuristicScopeHoisted,
    }
}

fn run_core_unpack(
    inputs: Vec<RetainedInput>,
    options: &UnpackOptions,
) -> Result<wakaru_core::driver::CapturedUnpackOutput> {
    let span = tracing::info_span!("public_unpack_core");
    let _enter = span.enter();
    let (level, dce_mode, raw) = match options.modules() {
        ModuleMode::Raw => (RewriteLevel::Standard, DceMode::Off, true),
        ModuleMode::Decompile(rewrite) => (rewrite.level(), rewrite.dce(), false),
    };
    let core_options = wakaru_core::DecompileOptions {
        filename: inputs[0].prepared.filename().to_string(),
        sourcemap: None,
        dce_mode: dce_mode.into_core(),
        level: level.into_core(),
        heuristic_split: options.mode() != UnpackMode::Strict,
        diagnostics: !raw && options.diagnostics(),
        emit_source_map: options.output_source_maps(),
    };
    let core_inputs = inputs.into_iter().map(|input| input.prepared).collect();

    let result = wakaru_core::driver::unpack_prepared_inputs_with_policy_and_capture(
        core_inputs,
        core_options,
        raw,
        core_scope_hoist_policy(options),
        !raw && options.recovery().angular_components(),
    );
    result.map_err(|error| Error::new(ErrorKind::Internal, None, error))
}

fn core_scope_hoist_policy(options: &UnpackOptions) -> wakaru_core::driver::ScopeHoistPolicy {
    match options.mode() {
        UnpackMode::Strict => wakaru_core::driver::ScopeHoistPolicy::Disabled,
        UnpackMode::Inspect => wakaru_core::driver::ScopeHoistPolicy::Inspect,
        UnpackMode::Auto => match options.modules() {
            ModuleMode::Decompile(rewrite) if rewrite.level() == RewriteLevel::Aggressive => {
                wakaru_core::driver::ScopeHoistPolicy::Recursive
            }
            ModuleMode::Raw | ModuleMode::Decompile(_) => {
                wakaru_core::driver::ScopeHoistPolicy::Fallback
            }
        },
    }
}

struct ConvertedOutput {
    modules: Vec<ModuleOutput>,
    pre_rewrite_modules: Vec<(String, String)>,
    diagnostics: Vec<Diagnostic>,
}

fn convert_core_output(
    captured: wakaru_core::driver::CapturedUnpackOutput,
    processed: &[ProcessedInput],
    reports: &mut [InputReport],
    raw: bool,
) -> ConvertedOutput {
    let span = tracing::info_span!("public_unpack_convert_output");
    let _enter = span.enter();
    let wakaru_core::driver::CapturedUnpackOutput {
        output,
        pre_rewrite_modules,
    } = captured;
    let only_input = (processed.len() == 1).then_some(processed[0].id);
    let failed: HashSet<&str> = output
        .warnings
        .iter()
        .filter(|warning| {
            matches!(
                warning.kind,
                wakaru_core::UnpackWarningKind::DecompileFailed
                    | wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
            )
        })
        .map(|warning| warning.filename.as_str())
        .collect();

    let modules: Vec<_> = output
        .modules
        .into_iter()
        .enumerate()
        .map(|(index, module)| {
            let provenance_input = module
                .provenance
                .input
                .and_then(|input| processed.get(input.index()).map(|input| input.id))
                .or(only_input);
            let spans = provenance_input
                .into_iter()
                .flat_map(|input| {
                    module
                        .provenance
                        .ranges
                        .iter()
                        .map(move |&(start, end)| SourceSpan { input, start, end })
                })
                .collect::<Vec<_>>();
            let inspection_context = provenance_input
                .into_iter()
                .flat_map(|input| {
                    module
                        .provenance
                        .inspection_context_ranges
                        .iter()
                        .map(move |&(start, end)| SourceSpan { input, start, end })
                })
                .collect::<Vec<_>>();
            let mut associated: HashSet<_> = spans.iter().map(|span| span.input).collect();
            if associated.is_empty() {
                if let Some(input) = provenance_input {
                    associated.insert(input);
                } else {
                    associated.extend(processed.iter().map(|input| input.id));
                }
            }
            for input in associated {
                reports[input.get() as usize].module_indices.push(index);
            }
            let entry =
                entry_status_from_provenance(Some(module.provenance.is_entry), &spans, reports);
            ModuleOutput {
                source_map: module.source_map,
                entry,
                status: if failed.contains(module.filename.as_str()) {
                    ModuleStatus::DecompileFailed
                } else if raw {
                    ModuleStatus::Raw
                } else {
                    ModuleStatus::Decompiled
                },
                filename: module.filename,
                code: module.code,
                provenance: spans,
                inspection_context,
            }
        })
        .collect();

    let module_by_filename: HashMap<&str, usize> = modules
        .iter()
        .enumerate()
        .map(|(index, module)| (module.filename.as_str(), index))
        .collect();
    let diagnostics = output
        .warnings
        .into_iter()
        .map(|warning| {
            let module = module_by_filename.get(warning.filename.as_str()).copied();
            let mut diagnostic = diagnostic_from_core(warning, module);
            diagnostic.input = module.and_then(|index| {
                let mut inputs = modules[index].provenance.iter().map(|span| span.input);
                let first = inputs.next()?;
                inputs.all(|input| input == first).then_some(first)
            });
            diagnostic
        })
        .collect();

    ConvertedOutput {
        modules,
        pre_rewrite_modules,
        diagnostics,
    }
}

fn entry_status_from_provenance(
    is_entry: Option<bool>,
    spans: &[SourceSpan],
    reports: &[InputReport],
) -> EntryStatus {
    let Some(is_entry) = is_entry else {
        return EntryStatus::Unknown;
    };
    let mut associated = spans.iter().filter_map(|span| {
        reports
            .get(span.input.get() as usize)
            .map(|report| report.detection)
    });
    let Some(first) = associated.next() else {
        return EntryStatus::Unknown;
    };
    if !entry_status_is_definitive(first) || !associated.all(entry_status_is_definitive) {
        return EntryStatus::Unknown;
    }
    if is_entry {
        EntryStatus::Entry
    } else {
        EntryStatus::NonEntry
    }
}

fn entry_status_is_definitive(detection: InputDetection) -> bool {
    matches!(
        detection,
        InputDetection::Structural(
            BundleFormat::Webpack5
                | BundleFormat::Webpack4
                | BundleFormat::Browserify
                | BundleFormat::ClosureModuleManager
                | BundleFormat::Metro
        )
    )
}

fn append_preserved_modules(
    modules: &mut Vec<ModuleOutput>,
    reports: &mut [InputReport],
    preserved: Vec<RetainedInput>,
) {
    let mut seen: HashSet<String> = modules
        .iter()
        .map(|module| module.filename.to_lowercase())
        .collect();
    for input in preserved {
        let (source_filename, source_code) = input
            .prepared
            .into_plain_source()
            .expect("only plain inputs can be preserved");
        let filename = unique_preserved_filename(&source_filename, &mut seen);
        let source_len = source_code.len() as u32;
        let index = modules.len();
        reports[input.id.get() as usize].module_indices.push(index);
        modules.push(ModuleOutput {
            filename,
            code: source_code,
            source_map: None,
            provenance: vec![SourceSpan {
                input: input.id,
                start: 0,
                end: source_len,
            }],
            inspection_context: Vec::new(),
            entry: EntryStatus::Unknown,
            status: ModuleStatus::Preserved,
        });
    }
}

fn unique_preserved_filename(filename: &str, seen: &mut HashSet<String>) -> String {
    let basename = Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("module.js");
    let mut candidate = basename.to_string();
    let mut suffix = 2;
    while !seen.insert(candidate.to_lowercase()) {
        let path = Path::new(basename);
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("module");
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("js");
        candidate = format!("{stem}_{suffix}.{extension}");
        suffix += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOSURE_MODULE_MANAGER_FIXTURE: &str =
        include_str!("../../core/tests/bundles/closure-module-manager/synthetic.js");
    const ANGULAR_PRODUCTION_RUNTIME: &str =
        include_str!("../../core/tests/bundles/angular-ivy-gen/dist/runtime.js");
    const ANGULAR_PRODUCTION_MAIN: &str =
        include_str!("../../core/tests/bundles/angular-ivy-gen/dist/main.js");
    const ANGULAR_PRODUCTION_LAZY: &str =
        include_str!("../../core/tests/bundles/angular-ivy-gen/dist/lazy.js");
    const CLOSURE_ANGULAR_COMPONENT_FIXTURE: &str = r#"
        "use strict";
        this.localSuite = this.localSuite || {};
        (function(shared) {
          var window = this;

          /*_M:runtime*/
          try {
            shared.before("runtime");
            shared._ModuleManager_initialize(
              "runtime/component:0",
              ["runtime", "component"]
            );
            shared.define = function(value) {
              return shared.noSideEffects(() => Object.assign({}, shared.baseDefinition, {
                type: value.type,
                selectors: value.selectors,
                template: value.template,
                dependencies: value.dependencies,
                styles: value.styles
              }));
            };
            shared.element = function() {
              return shared.element;
            };
            shared.publicRuntime = {
              "ɵɵelement": shared.element
            };
            shared.after();
          } catch (error) {
            shared._DumpException(error);
          }

          /*_M:component*/
          try {
            shared.before("component");
            shared.Card = class CardComponent {
              title = "Local";
            };
            shared.Card.definition = shared.define({
              type: shared.Card,
              selectors: [["local-card"]],
              template: function(renderFlags) {
                if (renderFlags & 1) {
                  shared.element(0, "article");
                }
              },
              dependencies: [],
              styles: []
            });
            shared.after();
          } catch (error) {
            shared._DumpException(error);
          }
        }).call(this, this.localSuite);
    "#;
    const METRO_FIXTURE: &str = r#"
        __d(function(global, require, importDefault, importAll, module, exports, dependencyMap) {
            module.exports = 1;
        }, 1, [], "index.js");
        __r(1);
    "#;

    #[test]
    fn bundle_level_scope_hoisted_detection_is_mapped_without_panicking() {
        assert_eq!(
            map_prepared_detection(wakaru_core::driver::PreparedInputDetection::Bundle(
                wakaru_core::BundleFormat::ScopeHoisted,
            )),
            InputDetection::HeuristicScopeHoisted
        );
    }

    #[test]
    fn inspect_mode_exposes_fine_grained_clusters_and_marks_output() {
        let source = r#"
            class A {}
            const x1 = 1; function f1() { return x1; }
            const x2 = 2; function f2() { return x2; }
            const x3 = 3; function f3() { return x3; }
            const x4 = 4; function f4() { return x4; }
            function make() { return new A(); }
            const result = make();
            console.log(result, f1(), f2(), f3(), f4());
            export { result };
        "#;
        let options = UnpackOptions::default()
            .with_modules(ModuleMode::Raw)
            .with_mode(UnpackMode::Inspect);

        let output = unpack(vec![Source::new("bundle.js", source)], options)
            .expect("inspection split should unpack");

        assert_eq!(output.modules.len(), 6);
        assert_eq!(output.safety, OutputSafety::InspectionOnly);
        assert!(output
            .modules
            .iter()
            .any(|module| module.code.contains("from \"./entry.js\"")));
    }

    #[test]
    fn inspect_mode_exposes_shared_coarse_context_for_split_write_components() {
        let mut source = String::new();
        for owner in 0..8 {
            source.push_str(&format!(
                "var state{owner} = 0; function read{owner}() {{ return state{owner}; }}\n"
            ));
        }
        source.push_str(
            r#"
                function spacer0() { return 0; }
                function spacer1() { return spacer0() + 1; }
                function spacer2() { return spacer1() + 1; }
                function spacer3() { return spacer2() + 1; }
                function mutateAll() {
            "#,
        );
        for owner in 0..8 {
            source.push_str(&format!("state{owner}++;\n"));
        }
        source.push_str("} console.log(mutateAll(), spacer3());");

        let inspect = unpack(
            vec![Source::new("bundle.js", source.clone())],
            UnpackOptions::default()
                .with_modules(ModuleMode::Raw)
                .with_mode(UnpackMode::Inspect),
        )
        .expect("inspection split should unpack");
        let contexts = inspect
            .modules
            .iter()
            .filter(|module| !module.inspection_context.is_empty())
            .map(|module| &module.inspection_context)
            .collect::<Vec<_>>();
        assert!(contexts.len() > 1, "fine siblings should expose context");
        assert!(contexts.iter().all(|context| *context == contexts[0]));
        assert!(contexts[0].iter().all(|span| span.input.get() == 0));

        let normal = unpack(
            vec![Source::new("bundle.js", source)],
            UnpackOptions::default().with_modules(ModuleMode::Raw),
        )
        .expect("normal split should unpack");
        assert!(normal
            .modules
            .iter()
            .all(|module| module.inspection_context.is_empty()));
    }

    #[test]
    fn unpack_profiles_map_to_valid_internal_policies() {
        assert_eq!(
            core_scope_hoist_policy(&UnpackOptions::default()),
            wakaru_core::driver::ScopeHoistPolicy::Fallback
        );
        assert_eq!(
            core_scope_hoist_policy(
                &UnpackOptions::default().with_modules(ModuleMode::Decompile(
                    crate::RewriteOptions::default().with_level(RewriteLevel::Aggressive),
                )),
            ),
            wakaru_core::driver::ScopeHoistPolicy::Recursive
        );
        assert_eq!(
            core_scope_hoist_policy(&UnpackOptions::default().with_mode(UnpackMode::Strict)),
            wakaru_core::driver::ScopeHoistPolicy::Disabled
        );
        assert_eq!(
            core_scope_hoist_policy(&UnpackOptions::default().with_mode(UnpackMode::Inspect)),
            wakaru_core::driver::ScopeHoistPolicy::Inspect
        );
    }

    #[test]
    fn reports_closure_module_manager_detection() {
        let mut job = UnpackJob::new(UnpackOptions::default().with_modules(ModuleMode::Raw))
            .expect("options should be valid");
        let receipt = job
            .push(Source::new(
                "closure-bundle.js",
                CLOSURE_MODULE_MANAGER_FIXTURE,
            ))
            .expect("Closure bundle should be detected");

        assert_eq!(
            receipt.detection,
            InputDetection::Structural(BundleFormat::ClosureModuleManager)
        );
        let output = job.finish().expect("Closure bundle should unpack");
        assert!(!output.modules.is_empty());
        assert_eq!(output.inputs[0].detection, receipt.detection);
    }

    #[test]
    fn closure_unpack_and_angular_recovery_remain_separate_root_phases() {
        let options = UnpackOptions::default()
            .with_recovery(crate::RecoveryOptions::default().with_angular_components(true));
        let output = unpack(
            vec![Source::new(
                "local-closure-bundle.js",
                CLOSURE_ANGULAR_COMPONENT_FIXTURE,
            )],
            options,
        )
        .expect("synthetic Closure bundle should unpack and recover artifacts");

        assert_eq!(
            output.inputs[0].detection,
            InputDetection::Structural(BundleFormat::ClosureModuleManager)
        );
        assert_eq!(
            output.artifacts.len(),
            1,
            "modules: {:#?}\ndiagnostics: {:#?}",
            output.modules,
            output.diagnostics
        );
        let artifact = &output.artifacts[0];
        assert_eq!(artifact.kind, crate::ArtifactKind::AngularComponent);
        assert_eq!(artifact.status, crate::ArtifactStatus::Complete);
        assert_eq!(artifact.filename, "local-card.component.ts");
        assert_eq!(artifact.module_indices.len(), 1);
        assert!(artifact.code.contains("<article></article>"));
        assert!(!artifact.code.contains("shared."));
    }

    #[test]
    fn angular_cli_production_chunks_recover_through_the_root_operation() {
        let output = unpack(
            vec![
                Source::new("runtime.js", ANGULAR_PRODUCTION_RUNTIME),
                Source::new("main.js", ANGULAR_PRODUCTION_MAIN),
                Source::new("lazy.js", ANGULAR_PRODUCTION_LAZY),
            ],
            UnpackOptions::default()
                .with_mode(UnpackMode::Strict)
                .with_unmatched(UnmatchedInput::Process)
                .with_recovery(crate::RecoveryOptions::default().with_angular_components(true)),
        )
        .expect("generated Angular chunks should decompile and recover artifacts");

        assert_eq!(output.modules.len(), 3);
        assert_eq!(output.artifacts.len(), 3);
        assert!(output.artifacts.iter().any(|artifact| {
            artifact.status == crate::ArtifactStatus::Complete
                && artifact.code.contains("selector: \"fixture-card\"")
                && artifact.code.contains("(click)=\"select()\"")
                && artifact.code.contains("[disabled]=\"disabled\"")
        }));
        assert!(output
            .artifacts
            .iter()
            .any(|artifact| artifact.code.contains("selector: \"fixture-lazy-card\"")));
    }

    #[test]
    fn reports_metro_detection() {
        let mut job = UnpackJob::new(UnpackOptions::default().with_modules(ModuleMode::Raw))
            .expect("options should be valid");
        let receipt = job
            .push(Source::new("metro-bundle.js", METRO_FIXTURE))
            .expect("Metro bundle should be detected");

        assert_eq!(
            receipt.detection,
            InputDetection::Structural(BundleFormat::Metro)
        );
        assert_eq!(BundleFormat::Metro.as_str(), "metro");
        let output = job.finish().expect("Metro bundle should unpack");
        assert!(!output.modules.is_empty());
        assert!(
            output
                .modules
                .iter()
                .any(|module| module.entry == EntryStatus::Entry),
            "Metro run statements establish entry status"
        );
        assert_eq!(output.inputs[0].detection, receipt.detection);
    }

    #[test]
    fn entry_status_is_unknown_without_definitive_detector_knowledge() {
        let span = SourceSpan {
            input: InputId::from_index(0),
            start: 0,
            end: 1,
        };
        let report = |format| InputReport {
            id: InputId::from_index(0),
            filename: "bundle.js".to_string(),
            detection: InputDetection::Structural(format),
            action: InputAction::Unpacked,
            module_indices: Vec::new(),
        };

        assert_eq!(
            entry_status_from_provenance(
                Some(false),
                std::slice::from_ref(&span),
                &[report(BundleFormat::Webpack5)],
            ),
            EntryStatus::NonEntry
        );
        assert_eq!(
            entry_status_from_provenance(
                Some(false),
                std::slice::from_ref(&span),
                &[report(BundleFormat::Esbuild)],
            ),
            EntryStatus::Unknown
        );
        assert_eq!(
            entry_status_from_provenance(
                Some(true),
                std::slice::from_ref(&span),
                &[report(BundleFormat::Amd)],
            ),
            EntryStatus::Unknown
        );
        assert_eq!(
            entry_status_from_provenance(Some(true), &[], &[report(BundleFormat::Metro)]),
            EntryStatus::Unknown
        );
    }

    #[test]
    fn per_push_unmatched_policy_composes_explicit_and_candidate_inputs() {
        let mut job = UnpackJob::new(
            UnpackOptions::default()
                .with_mode(UnpackMode::Strict)
                .with_unmatched(UnmatchedInput::Process),
        )
        .expect("options should be valid");

        job.push(Source::new("explicit.js", "const explicit = 1;"))
            .expect("explicit source should be processed");
        job.push_with_unmatched(
            Source::new("candidate.js", "const candidate = 2;"),
            UnmatchedInput::Skip,
        )
        .expect("plain candidate should be skipped");

        let output = job.finish().expect("mixed intake should finish");
        assert_eq!(output.inputs[0].action, InputAction::Processed);
        assert_eq!(output.inputs[1].action, InputAction::Skipped);
        assert_eq!(output.inputs[0].module_indices, vec![0]);
        assert!(output.inputs[1].module_indices.is_empty());
        assert_eq!(output.modules.len(), 1);
        assert!(output.modules[0].code.contains("explicit"));
    }

    #[test]
    fn all_skipped_inputs_return_an_empty_successful_output() {
        let mut job = UnpackJob::new(
            UnpackOptions::default()
                .with_mode(UnpackMode::Strict)
                .with_unmatched(UnmatchedInput::Skip),
        )
        .expect("options should be valid");
        let receipt = job
            .push(Source::new("plain.js", "const value = 1;"))
            .expect("plain input should be accepted");
        assert_eq!(receipt.id.get(), 0);
        assert_eq!(receipt.detection, InputDetection::Plain);

        let output = job.finish().expect("all-skipped is a valid result");
        assert!(output.modules.is_empty());
        assert_eq!(output.inputs.len(), 1);
        assert_eq!(output.inputs[0].action, InputAction::Skipped);
    }

    #[test]
    fn unmatched_error_is_deferred_until_finish() {
        let mut job = UnpackJob::new(
            UnpackOptions::default()
                .with_mode(UnpackMode::Strict)
                .with_unmatched(UnmatchedInput::Error),
        )
        .expect("options should be valid");
        let first = job
            .push(Source::new("plain.js", "const value = 1;"))
            .expect("plain detection should not fail push");
        let second = job
            .push(Source::new("also-plain.js", "const other = 2;"))
            .expect("job remains usable");
        assert_eq!(first.id.get(), 0);
        assert_eq!(second.id.get(), 1);

        let error = job.finish().expect_err("plain input should fail the job");
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
        assert_eq!(error.input_filename(), Some("plain.js"));
    }

    #[test]
    fn unrecoverable_input_parse_is_a_typed_push_error() {
        let mut job =
            UnpackJob::new(UnpackOptions::default()).expect("default options should be valid");
        let error = job
            .push(Source::new("broken.js", "function ("))
            .expect_err("invalid input should fail during push");

        assert_eq!(error.kind(), ErrorKind::Parse);
        assert_eq!(error.input_filename(), Some("broken.js"));
    }

    #[test]
    fn preserved_plain_input_is_returned_without_rewriting() {
        let source = "var untouched = 1;";
        let output = unpack(
            vec![Source::new("plain.js", source)],
            UnpackOptions::default()
                .with_mode(UnpackMode::Strict)
                .with_unmatched(UnmatchedInput::Preserve),
        )
        .expect("preserve should succeed");

        assert_eq!(output.modules.len(), 1);
        assert_eq!(output.modules[0].code, source);
        assert_eq!(output.modules[0].status, ModuleStatus::Preserved);
        assert_eq!(output.inputs[0].module_indices, vec![0]);
    }

    #[test]
    fn raw_output_maps_are_rejected_during_job_creation() {
        let error = UnpackJob::new(
            UnpackOptions::default()
                .with_modules(ModuleMode::Raw)
                .with_output_source_maps(true),
        )
        .expect_err("invalid combination should fail");
        assert_eq!(error.kind(), ErrorKind::InvalidOptions);
    }

    #[test]
    fn raw_output_rejects_component_recovery() {
        let error = UnpackJob::new(
            UnpackOptions::default()
                .with_modules(ModuleMode::Raw)
                .with_recovery(crate::RecoveryOptions::default().with_angular_components(true)),
        )
        .expect_err("raw mode should not run component recovery");
        assert_eq!(error.kind(), ErrorKind::InvalidOptions);
    }

    #[test]
    fn duplicate_input_filenames_fail_as_ambiguous_public_paths() {
        let error = unpack(
            vec![
                Source::new("same.js", "export const first = 1;"),
                Source::new("same.js", "export const second = 2;"),
            ],
            UnpackOptions::default().with_mode(UnpackMode::Strict),
        )
        .expect_err("duplicate physical identities must fail before emission");

        assert!(
            error.to_string().contains("ambiguous public module path"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn distinct_input_paths_keep_distinct_ids_and_provenance() {
        let output = unpack(
            vec![
                Source::new("first/same.js", "export const first = 1;"),
                Source::new("second/same.js", "export const second = 2;"),
            ],
            UnpackOptions::default().with_mode(UnpackMode::Strict),
        )
        .expect("distinct physical filenames should remain distinguishable");

        assert_eq!(output.inputs.len(), 2);
        assert_eq!(output.inputs[0].id.get(), 0);
        assert_eq!(output.inputs[1].id.get(), 1);
        assert_eq!(output.inputs[0].module_indices.len(), 1);
        assert_eq!(output.inputs[1].module_indices.len(), 1);
        assert_ne!(
            output.inputs[0].module_indices,
            output.inputs[1].module_indices
        );
        let first_module = output.inputs[0].module_indices[0];
        let second_module = output.inputs[1].module_indices[0];
        assert_eq!(output.modules[first_module].provenance[0].input.get(), 0);
        assert_eq!(output.modules[second_module].provenance[0].input.get(), 1);
    }

    #[test]
    fn provenance_less_synthesized_module_is_associated_with_all_processed_inputs() {
        let processed = vec![
            ProcessedInput {
                id: InputId::from_index(0),
            },
            ProcessedInput {
                id: InputId::from_index(1),
            },
        ];
        let mut reports = ["first.js", "second.js"]
            .into_iter()
            .zip(&processed)
            .map(|(filename, input)| InputReport {
                id: input.id,
                filename: filename.to_string(),
                detection: InputDetection::Plain,
                action: InputAction::Processed,
                module_indices: Vec::new(),
            })
            .collect::<Vec<_>>();
        let output = wakaru_core::driver::CapturedUnpackOutput {
            output: wakaru_core::driver::PreparedUnpackOutput {
                modules: vec![wakaru_core::driver::PreparedModuleOutput {
                    filename: "synthesized.js".to_string(),
                    code: "export {};".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        let converted = convert_core_output(output, &processed, &mut reports, false);

        assert_eq!(converted.modules.len(), 1);
        assert!(converted.modules[0].provenance.is_empty());
        assert_eq!(reports[0].module_indices, vec![0]);
        assert_eq!(reports[1].module_indices, vec![0]);
    }

    #[test]
    fn parse_recovery_is_reported_without_optional_diagnostics() {
        let output = unpack(
            vec![Source::new(
                "duplicate-label.js",
                "label: label: break label;",
            )],
            UnpackOptions::default().with_mode(UnpackMode::Strict),
        )
        .expect("recoverable input should produce output");

        assert!(output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == crate::DiagnosticCode::InputParseRecovered));
    }

    #[test]
    fn opaque_webpack_factory_has_failed_status_and_stable_diagnostic() {
        let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      if (globalThis.useAlternate) load = globalThis.alternateLoader;
      module.exports = load;
    }),
    1: ((module) => { module.exports = "stable"; })
  });
  var cache = {};
  function __nccwpck_require__(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, __nccwpck_require__);
    return module.exports;
  }
  module.exports = __nccwpck_require__(0);
})();
"#;

        let output = unpack(
            vec![Source::new("webpack5-opaque-factory.js", source)],
            UnpackOptions::default().with_mode(UnpackMode::Strict),
        )
        .expect("partial structural recovery should remain successful");

        let failed = output
            .modules
            .iter()
            .find(|module| module.filename == "module-0.js")
            .expect("opaque module should be present");
        assert_eq!(failed.status, ModuleStatus::DecompileFailed);
        assert_eq!(failed.entry, EntryStatus::Entry);
        assert!(output.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .module
                .is_some_and(|index| output.modules[index].filename == "module-0.js")
                && diagnostic.code == crate::DiagnosticCode::WebpackFactoryRecoveryFailed
                && diagnostic.code.as_str() == "webpack_factory_recovery_failed"
        }));
        assert_eq!(
            output
                .modules
                .iter()
                .find(|module| module.filename == "module-1.js")
                .map(|module| module.status),
            Some(ModuleStatus::Decompiled)
        );
    }
}
