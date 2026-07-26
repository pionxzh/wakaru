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

    match recover_angular_components(modules, pre_rewrite_modules) {
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
                message: format!("Angular component recovery was skipped: {error}"),
                input: None,
                module: None,
                span: None,
            }],
        ),
    }
}

fn recover_angular_components(
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
        components,
        stats,
        unknown_runtime_call_shapes,
        ..
    } = report;
    let artifacts = components
        .into_iter()
        .map(|component| {
            let (module_index, module) = eligible
                .get(component.module_index)
                .copied()
                .ok_or_else(|| anyhow::anyhow!("component source module index is out of bounds"))?;
            let filename = angular_artifact_filename(
                &module.filename,
                &component.name,
                &component.selector,
                &mut seen,
            );
            Ok(ArtifactOutput {
                filename,
                code: component.source,
                kind: ArtifactKind::AngularComponent,
                status: match component.completeness {
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

fn angular_artifact_filename(
    module_filename: &str,
    component_name: &str,
    selector: &str,
    seen: &mut HashSet<String>,
) -> String {
    let module_path =
        wakaru_core::safe_relative_module_path(module_filename).unwrap_or_else(|_| {
            Path::new(module_filename)
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("module.js"))
        });
    let parent = module_path.parent().filter(|path| *path != Path::new(""));
    let stem = safe_selector_stem(selector)
        .or_else(|| component_name_stem(component_name))
        .unwrap_or_else(|| "component".to_string());
    let filename = format!("{stem}.component.ts");
    let candidate = parent
        .map(|parent| parent.join(&filename))
        .unwrap_or_else(|| PathBuf::from(filename));
    deduplicate_component_path(&candidate, seen)
        .to_string_lossy()
        .replace('\\', "/")
}

fn deduplicate_component_path(path: &Path, seen: &mut HashSet<String>) -> PathBuf {
    if seen.insert(path.to_string_lossy().to_lowercase()) {
        return path.to_path_buf();
    }

    let filename = path
        .file_name()
        .and_then(|filename| filename.to_str())
        .unwrap_or("component.component.ts");
    let stem = filename
        .strip_suffix(".component.ts")
        .unwrap_or("component");
    let parent = path.parent().unwrap_or(Path::new(""));
    let mut suffix = 2;
    loop {
        let candidate = parent.join(format!("{stem}_{suffix}.component.ts"));
        if seen.insert(candidate.to_string_lossy().to_lowercase()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn safe_selector_stem(selector: &str) -> Option<String> {
    (!selector.is_empty()
        && selector
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_')))
    .then(|| selector.replace('_', "-").to_ascii_lowercase())
}

fn component_name_stem(name: &str) -> Option<String> {
    let name = name.strip_suffix("Component").unwrap_or(name);
    let mut stem = String::new();
    for (index, character) in name.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 && !stem.ends_with('-') {
                stem.push('-');
            }
            stem.push(character.to_ascii_lowercase());
        } else if character.is_ascii_alphanumeric() {
            stem.push(character.to_ascii_lowercase());
        } else if !stem.ends_with('-') {
            stem.push('-');
        }
    }
    let stem = stem.trim_matches('-').to_string();
    (!stem.is_empty()).then_some(stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_names_are_relative_and_deduplicated_case_insensitively() {
        let mut seen = HashSet::new();
        assert_eq!(
            angular_artifact_filename("/tmp/input.js", "DemoCardComponent", "demo-card", &mut seen,),
            "demo-card.component.ts"
        );
        assert_eq!(
            angular_artifact_filename("src/other.js", "OtherComponent", "DEMO-CARD", &mut seen,),
            "src/demo-card.component.ts"
        );
        assert_eq!(
            angular_artifact_filename("src/third.js", "ThirdComponent", "demo-card", &mut seen,),
            "src/demo-card_2.component.ts"
        );
    }

    #[test]
    fn unsafe_selector_falls_back_to_component_name() {
        let mut seen = HashSet::new();
        assert_eq!(
            angular_artifact_filename(
                "src/input.js",
                "DemoCardComponent",
                "[demo-card]",
                &mut seen,
            ),
            "src/demo-card.component.ts"
        );
    }
}
