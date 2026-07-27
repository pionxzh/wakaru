use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::output::{
    ArtifactKind, ArtifactOutput, ArtifactStatus, Diagnostic, DiagnosticCode, DiagnosticSeverity,
    ModuleOutput, ModuleStatus,
};

pub(crate) fn recover_artifacts(
    modules: &[ModuleOutput],
    pre_rewrite_modules: &[(String, String)],
    recovery: crate::RecoveryOptions,
    diagnostics: bool,
) -> (Vec<ArtifactOutput>, Vec<Diagnostic>) {
    if !recovery.angular_components() {
        return (Vec::new(), Vec::new());
    }

    match recover_angular_modules(modules, pre_rewrite_modules) {
        Ok((artifacts, stats, unknown_runtime_call_shapes)) => {
            let unknown_shape_summary =
                format_unknown_runtime_call_shapes(&unknown_runtime_call_shapes);
            let recovery_diagnostics = diagnostics
                .then(|| Diagnostic {
                    severity: if stats.partial_components > 0
                        || stats.rejected_component_candidates > 0
                    {
                        DiagnosticSeverity::Warning
                    } else {
                        DiagnosticSeverity::Info
                    },
                    code: DiagnosticCode::ArtifactRecoveryReport,
                    message: format!(
                        "Angular recovery emitted {}/{} component candidates \
                         ({} complete, {} partial, {} rejected); rendered {}/{} runtime calls \
                         ({} unsupported, {} malformed){}",
                        stats.recovered_components,
                        stats.component_candidates,
                        stats.complete_components,
                        stats.partial_components,
                        stats.rejected_component_candidates,
                        stats.rendered_instruction_calls,
                        stats.runtime_calls_observed,
                        stats.unsupported_runtime_calls,
                        stats.malformed_instruction_calls,
                        unknown_shape_summary,
                    ),
                    input: None,
                    module: None,
                    span: None,
                })
                .into_iter()
                .collect();
            (artifacts, recovery_diagnostics)
        }
        Err(error) => (
            Vec::new(),
            vec![Diagnostic {
                severity: DiagnosticSeverity::Warning,
                code: DiagnosticCode::ArtifactRecoveryFailed,
                message: format!("Angular module recovery was skipped: {error}"),
                input: None,
                module: None,
                span: None,
            }],
        ),
    }
}

fn recover_angular_modules(
    modules: &[ModuleOutput],
    pre_rewrite_modules: &[(String, String)],
) -> anyhow::Result<(
    Vec<ArtifactOutput>,
    wakaru_core::AngularRecoveryStats,
    Vec<wakaru_core::AngularUnknownRuntimeCallShape>,
)> {
    let eligible = modules
        .iter()
        .enumerate()
        .filter(|(_, module)| module.status == ModuleStatus::Decompiled)
        .collect::<Vec<_>>();
    if eligible.is_empty() {
        return Ok((
            Vec::new(),
            wakaru_core::AngularRecoveryStats::default(),
            Vec::new(),
        ));
    }

    let evidence_by_filename = pre_rewrite_modules
        .iter()
        .map(|(filename, source)| (filename.as_str(), source.as_str()))
        .collect::<std::collections::HashMap<_, _>>();
    let views = eligible
        .iter()
        .map(|(_, module)| wakaru_core::AngularModuleView {
            filename: module.filename.as_str(),
            evidence_source: evidence_by_filename
                .get(module.filename.as_str())
                .copied()
                .unwrap_or(module.code.as_str()),
            readable_source: module.code.as_str(),
        })
        .collect::<Vec<_>>();
    let report = wakaru_core::analyze_angular_components_from_module_views(
        &views,
        wakaru_core::AngularRecoveryOptions::default(),
    )?;

    let mut seen = modules
        .iter()
        .filter_map(|module| wakaru_core::safe_relative_module_path(&module.filename).ok())
        .map(|path| path.to_string_lossy().to_lowercase())
        .collect::<HashSet<_>>();
    let wakaru_core::AngularRecoveryReport {
        modules: recovered_modules,
        stats,
        unknown_runtime_call_shapes,
        ..
    } = report;
    let artifacts = recovered_modules
        .into_iter()
        .map(|recovered_module| {
            let (module_index, module) = eligible
                .get(recovered_module.module_index)
                .copied()
                .ok_or_else(|| anyhow::anyhow!("Angular source module index is out of bounds"))?;
            let filename = angular_module_artifact_filename(&module.filename, &mut seen);
            Ok(ArtifactOutput {
                filename,
                code: recovered_module.source,
                kind: ArtifactKind::AngularModule,
                status: match recovered_module.completeness {
                    wakaru_core::AngularRecoveryCompleteness::Complete => ArtifactStatus::Complete,
                    wakaru_core::AngularRecoveryCompleteness::Partial => ArtifactStatus::Partial,
                    _ => ArtifactStatus::Partial,
                },
                module_indices: vec![module_index],
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok((artifacts, stats, unknown_runtime_call_shapes))
}

fn format_unknown_runtime_call_shapes(
    shapes: &[wakaru_core::AngularUnknownRuntimeCallShape],
) -> String {
    if shapes.is_empty() {
        return String::new();
    }

    let mut shapes = shapes.iter().collect::<Vec<_>>();
    shapes.sort_by(|left, right| {
        right
            .runtime_calls
            .cmp(&left.runtime_calls)
            .then_with(|| right.occurrences.cmp(&left.occurrences))
            .then_with(|| left.phase.cmp(&right.phase))
            .then_with(|| left.argument_counts.cmp(&right.argument_counts))
    });
    let visible = shapes
        .iter()
        .take(5)
        .map(|shape| {
            let phase = match shape.phase {
                wakaru_core::AngularTemplatePhase::Creation => "creation",
                wakaru_core::AngularTemplatePhase::Update => "update",
                wakaru_core::AngularTemplatePhase::OutsideRender => "outside-render",
                _ => "unknown-phase",
            };
            let argument_counts = shape
                .argument_counts
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            let occurrence_label = if shape.occurrences == 1 {
                "occurrence"
            } else {
                "occurrences"
            };
            let call_label = if shape.runtime_calls == 1 {
                "call"
            } else {
                "calls"
            };
            format!(
                "{phase} [{argument_counts}] ({} {occurrence_label}/{} {call_label})",
                shape.occurrences, shape.runtime_calls,
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let omitted = shapes.len().saturating_sub(5);
    if omitted == 0 {
        format!("; unknown call shapes: {visible}")
    } else {
        format!("; unknown call shapes: {visible}, +{omitted} more")
    }
}

fn angular_module_artifact_filename(module_filename: &str, seen: &mut HashSet<String>) -> String {
    let module_path =
        wakaru_core::safe_relative_module_path(module_filename).unwrap_or_else(|_| {
            Path::new(module_filename)
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("module.js"))
        });
    let parent = module_path.parent().filter(|path| *path != Path::new(""));
    let stem = module_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("module");
    let filename = format!("{stem}.angular.ts");
    let candidate = parent
        .map(|parent| parent.join(&filename))
        .unwrap_or_else(|| PathBuf::from(filename));
    deduplicate_angular_module_path(&candidate, seen)
        .to_string_lossy()
        .replace('\\', "/")
}

fn deduplicate_angular_module_path(path: &Path, seen: &mut HashSet<String>) -> PathBuf {
    if seen.insert(path.to_string_lossy().to_lowercase()) {
        return path.to_path_buf();
    }

    let filename = path
        .file_name()
        .and_then(|filename| filename.to_str())
        .unwrap_or("module.angular.ts");
    let stem = filename.strip_suffix(".angular.ts").unwrap_or("module");
    let parent = path.parent().unwrap_or(Path::new(""));
    let mut suffix = 2;
    loop {
        let candidate = parent.join(format!("{stem}_{suffix}.angular.ts"));
        if seen.insert(candidate.to_string_lossy().to_lowercase()) {
            return candidate;
        }
        suffix += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_artifact_names_are_relative_and_deduplicated_case_insensitively() {
        let mut seen = HashSet::new();
        assert_eq!(
            angular_module_artifact_filename("/tmp/input.js", &mut seen),
            "input.angular.ts"
        );
        assert_eq!(
            angular_module_artifact_filename("src/feature.js", &mut seen),
            "src/feature.angular.ts"
        );
        assert_eq!(
            angular_module_artifact_filename("src/FEATURE.mjs", &mut seen),
            "src/FEATURE_2.angular.ts"
        );
    }
}
