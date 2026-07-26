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
) -> (Vec<ArtifactOutput>, Vec<Diagnostic>) {
    if !recovery.angular_components() {
        return (Vec::new(), Vec::new());
    }

    match recover_angular_components(modules, pre_rewrite_modules) {
        Ok(artifacts) => (artifacts, Vec::new()),
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
) -> anyhow::Result<Vec<ArtifactOutput>> {
    let eligible = modules
        .iter()
        .enumerate()
        .filter(|(_, module)| module.status == ModuleStatus::Decompiled)
        .collect::<Vec<_>>();
    if eligible.is_empty() {
        return Ok(Vec::new());
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
    let recovered = wakaru_core::recover_angular_components_from_module_views(
        &views,
        wakaru_core::AngularRecoveryOptions::default(),
    )?;

    let mut seen = modules
        .iter()
        .filter_map(|module| wakaru_core::safe_relative_module_path(&module.filename).ok())
        .map(|path| path.to_string_lossy().to_lowercase())
        .collect::<HashSet<_>>();
    recovered
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
        .collect()
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
