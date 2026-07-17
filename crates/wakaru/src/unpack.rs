use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::anyhow;

use crate::decompile::diagnostic_from_core;
use crate::error::{from_core_driver_error, Error, ErrorKind, Result};
use crate::options::{
    DceMode, ModuleMode, RewriteLevel, ScopeHoistMode, UnmatchedInput, UnpackOptions,
};
use crate::output::{
    BundleFormat, Diagnostic, EntryStatus, InputAction, InputDetection, InputId, InputReceipt,
    InputReport, ModuleOutput, ModuleStatus, SourceSpan, UnpackOutput,
};
use crate::source::Source;

pub fn unpack(inputs: Vec<Source>, options: UnpackOptions) -> Result<UnpackOutput> {
    let mut job = UnpackJob::new(options)?;
    for input in inputs {
        job.push(input)?;
    }
    job.finish()
}

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
    filename: String,
}

impl UnpackJob {
    pub fn new(options: UnpackOptions) -> Result<Self> {
        if matches!(options.modules(), ModuleMode::Raw) && options.output_source_maps() {
            return Err(Error::new(
                ErrorKind::InvalidOptions,
                None,
                anyhow!("raw unpack mode does not support output source maps"),
            ));
        }
        Ok(Self {
            options,
            reports: Vec::new(),
            retained: Vec::new(),
            unmatched_error: None,
        })
    }

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
        let prepared = wakaru_core::driver::prepare_unpack_input(
            parts.filename,
            parts.code,
            self.options.scope_hoist() != ScopeHoistMode::Disabled,
            matches!(self.options.modules(), ModuleMode::Decompile(_))
                && unmatched == UnmatchedInput::Process,
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
        let mut diagnostics = Vec::new();
        if !processed.is_empty() {
            let processed_meta = processed
                .iter()
                .map(|input| ProcessedInput {
                    id: input.id,
                    filename: input.prepared.filename().to_string(),
                })
                .collect::<Vec<_>>();
            let core_output = run_core_unpack(processed, &self.options)?;
            let converted = convert_core_output(
                core_output,
                &processed_meta,
                &mut self.reports,
                matches!(self.options.modules(), ModuleMode::Raw),
            );
            modules = converted.modules;
            diagnostics = converted.diagnostics;
        }

        append_preserved_modules(&mut modules, &mut self.reports, preserved);

        Ok(UnpackOutput {
            modules,
            inputs: self.reports,
            diagnostics,
        })
    }
}

fn map_prepared_detection(
    detection: wakaru_core::driver::PreparedInputDetection,
) -> InputDetection {
    match detection {
        wakaru_core::driver::PreparedInputDetection::Bundle(format) => {
            InputDetection::Structural(map_bundle_format(format))
        }
        wakaru_core::driver::PreparedInputDetection::ScopeHoisted => {
            InputDetection::HeuristicScopeHoisted
        }
        wakaru_core::driver::PreparedInputDetection::Plain => InputDetection::Plain,
    }
}

fn map_bundle_format(format: wakaru_core::BundleFormat) -> BundleFormat {
    match format {
        wakaru_core::BundleFormat::Webpack5 => BundleFormat::Webpack5,
        wakaru_core::BundleFormat::Webpack4 => BundleFormat::Webpack4,
        wakaru_core::BundleFormat::Browserify => BundleFormat::Browserify,
        wakaru_core::BundleFormat::ClosureModuleManager => BundleFormat::ClosureModuleManager,
        wakaru_core::BundleFormat::Metro => BundleFormat::Metro,
        wakaru_core::BundleFormat::SystemJs => BundleFormat::SystemJs,
        wakaru_core::BundleFormat::Esbuild => BundleFormat::Esbuild,
        wakaru_core::BundleFormat::Amd => BundleFormat::Amd,
        wakaru_core::BundleFormat::ScopeHoisted => {
            unreachable!("scope-hoisted detection is represented separately")
        }
    }
}

fn run_core_unpack(
    inputs: Vec<RetainedInput>,
    options: &UnpackOptions,
) -> Result<wakaru_core::UnpackOutput> {
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
        heuristic_split: options.scope_hoist() != ScopeHoistMode::Disabled,
        diagnostics: !raw && options.diagnostics(),
        emit_source_map: options.output_source_maps(),
    };
    let core_inputs = inputs.into_iter().map(|input| input.prepared).collect();

    let result = wakaru_core::driver::unpack_prepared_inputs(
        core_inputs,
        core_options,
        raw,
        options.scope_hoist() == ScopeHoistMode::Recursive,
    );
    result.map_err(|error| Error::new(ErrorKind::Internal, None, error))
}

struct ConvertedOutput {
    modules: Vec<ModuleOutput>,
    diagnostics: Vec<Diagnostic>,
}

fn convert_core_output(
    output: wakaru_core::UnpackOutput,
    processed: &[ProcessedInput],
    reports: &mut [InputReport],
    raw: bool,
) -> ConvertedOutput {
    let span = tracing::info_span!("public_unpack_convert_output");
    let _enter = span.enter();
    let source_maps: HashMap<_, _> = output.source_maps.into_iter().collect();
    let provenance: HashMap<_, _> = output
        .provenance
        .into_iter()
        .map(|provenance| (provenance.filename.clone(), provenance))
        .collect();
    let input_by_name: HashMap<&str, InputId> = processed
        .iter()
        .map(|input| (input.filename.as_str(), input.id))
        .collect();
    let only_input = (processed.len() == 1).then_some(processed[0].id);
    let failed: HashSet<&str> = output
        .warnings
        .iter()
        .filter(|warning| warning.kind == wakaru_core::UnpackWarningKind::DecompileFailed)
        .map(|warning| warning.filename.as_str())
        .collect();

    let modules: Vec<_> = output
        .modules
        .into_iter()
        .enumerate()
        .map(|(index, (filename, code))| {
            let module_provenance = provenance.get(&filename);
            let provenance_input = module_provenance.and_then(|provenance| {
                if let Some(position) = wakaru_core::driver::prepared_input_index(&provenance.input)
                {
                    processed.get(position).map(|input| input.id)
                } else if provenance.input.is_empty() {
                    only_input
                } else {
                    input_by_name.get(provenance.input.as_str()).copied()
                }
            });
            let spans = module_provenance
                .map(|provenance| {
                    provenance_input
                        .into_iter()
                        .flat_map(|input| {
                            provenance
                                .ranges
                                .iter()
                                .map(move |&(start, end)| SourceSpan { input, start, end })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
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
            let entry = entry_status_from_provenance(
                module_provenance.map(|provenance| provenance.is_entry),
                &spans,
                reports,
            );
            ModuleOutput {
                source_map: source_maps.get(&filename).cloned(),
                entry,
                status: if failed.contains(filename.as_str()) {
                    ModuleStatus::DecompileFailed
                } else if raw {
                    ModuleStatus::Raw
                } else {
                    ModuleStatus::Decompiled
                },
                filename,
                code,
                provenance: spans,
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
    const METRO_FIXTURE: &str = r#"
        __d(function(global, require, importDefault, importAll, module, exports, dependencyMap) {
            module.exports = 1;
        }, 1, [], "index.js");
        __r(1);
    "#;

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
                .with_scope_hoist(ScopeHoistMode::Disabled)
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
                .with_scope_hoist(ScopeHoistMode::Disabled)
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
                .with_scope_hoist(ScopeHoistMode::Disabled)
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
                .with_scope_hoist(ScopeHoistMode::Disabled)
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
    fn duplicate_input_filenames_keep_distinct_ids_and_provenance() {
        let output = unpack(
            vec![
                Source::new("same.js", "export const first = 1;"),
                Source::new("same.js", "export const second = 2;"),
            ],
            UnpackOptions::default().with_scope_hoist(ScopeHoistMode::Disabled),
        )
        .expect("duplicate physical filenames should remain distinguishable");

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
                filename: "first.js".to_string(),
            },
            ProcessedInput {
                id: InputId::from_index(1),
                filename: "second.js".to_string(),
            },
        ];
        let mut reports = processed
            .iter()
            .map(|input| InputReport {
                id: input.id,
                filename: input.filename.clone(),
                detection: InputDetection::Plain,
                action: InputAction::Processed,
                module_indices: Vec::new(),
            })
            .collect::<Vec<_>>();
        let output = wakaru_core::UnpackOutput {
            modules: vec![("synthesized.js".to_string(), "export {};".to_string())],
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
            UnpackOptions::default().with_scope_hoist(ScopeHoistMode::Disabled),
        )
        .expect("recoverable input should produce output");

        assert!(output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == crate::DiagnosticCode::InputParseRecovered));
    }
}
