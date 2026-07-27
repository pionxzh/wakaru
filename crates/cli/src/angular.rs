use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::json_output::{JsonModule, JsonModuleKind, JsonModuleStatus};
use crate::CliOutputArtifact;

pub(crate) struct SingleFileArtifactSidecar {
    pub(crate) path: PathBuf,
    pub(crate) artifact: CliOutputArtifact,
}

pub(crate) struct SingleFileAngularMetadata {
    pub(crate) kind: JsonModuleKind,
    pub(crate) status: JsonModuleStatus,
    pub(crate) source_filename: Option<String>,
}

pub(crate) fn json_module_for_single_file_sidecar(
    sidecar: &SingleFileArtifactSidecar,
) -> JsonModule {
    JsonModule {
        filename: sidecar.path.to_string_lossy().into_owned(),
        kind: sidecar.artifact.kind,
        status: sidecar.artifact.status,
        source_filename: sidecar.artifact.source_filename.clone(),
    }
}

pub(crate) fn single_file_angular_metadata(
    angular: bool,
    selected: Option<&CliOutputArtifact>,
    sidecars: &[SingleFileArtifactSidecar],
    output_filename: &str,
) -> Option<SingleFileAngularMetadata> {
    if !angular {
        return None;
    }

    if let Some(artifact) = selected {
        return Some(SingleFileAngularMetadata {
            kind: artifact.kind,
            status: artifact.status,
            source_filename: Some(output_filename.to_string()),
        });
    }

    Some(SingleFileAngularMetadata {
        kind: JsonModuleKind::JavaScript,
        status: if sidecars.is_empty() {
            JsonModuleStatus::Decompiled
        } else {
            JsonModuleStatus::AngularModuleSourceJs
        },
        source_filename: (!sidecars.is_empty()).then(|| output_filename.to_string()),
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AngularArtifactSummary {
    pub(crate) complete: usize,
    pub(crate) partial: usize,
}

pub(crate) fn angular_artifact_summary(
    artifacts: &[CliOutputArtifact],
) -> Option<AngularArtifactSummary> {
    let summary = artifacts.iter().fold(
        AngularArtifactSummary::default(),
        |mut summary, artifact| {
            match artifact.status {
                JsonModuleStatus::RecoveredAngularModule => summary.complete += 1,
                JsonModuleStatus::PartialAngularModule => summary.partial += 1,
                _ => {}
            }
            summary
        },
    );

    (summary.complete > 0 || summary.partial > 0).then_some(summary)
}

pub(crate) fn format_angular_artifact_summary(summary: AngularArtifactSummary) -> String {
    format!(
        "angular modules: {} complete, {} partial",
        summary.complete, summary.partial
    )
}

pub(crate) fn single_file_angular_sidecar_path(
    artifact_filename: &str,
    output_path: &Path,
) -> PathBuf {
    let sidecar_name = Path::new(artifact_filename)
        .file_name()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| OsStr::new("module.angular.ts"));
    output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.join(sidecar_name))
        .unwrap_or_else(|| PathBuf::from(sidecar_name))
}

pub(crate) fn is_angular_output_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ts"))
}

pub(crate) fn ensure_angular_sidecar_does_not_overwrite_input(
    sidecar_path: &Path,
    input_path: Option<&Path>,
) -> Result<()> {
    let Some(input_path) = input_path else {
        return Ok(());
    };
    if !sidecar_path.exists() {
        return Ok(());
    }
    let Ok(sidecar_path) = fs::canonicalize(sidecar_path) else {
        return Ok(());
    };
    let Ok(input_path) = fs::canonicalize(input_path) else {
        return Ok(());
    };
    if sidecar_path == input_path {
        bail!(
            "refusing to write Angular sidecar over input file {}; choose a different output path",
            input_path.display()
        );
    }
    Ok(())
}

pub(crate) fn write_angular_sidecars(sidecars: &[SingleFileArtifactSidecar]) -> Result<()> {
    for sidecar in sidecars {
        fs::write(&sidecar.path, &sidecar.artifact.code)
            .with_context(|| format!("failed to write {}", sidecar.path.display()))?;
    }
    Ok(())
}
