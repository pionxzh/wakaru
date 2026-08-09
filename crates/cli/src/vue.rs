use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use wakaru_core::is_likely_vue_sfc_source;

use crate::json_output::{JsonModuleKind, JsonModuleStatus};
use crate::CliOutputArtifact;

fn read_relative_import_source(base_filename: &str, specifier: &str) -> Option<String> {
    if base_filename == "<stdin>" {
        return None;
    }
    if !(specifier.starts_with("./") || specifier.starts_with("../")) {
        return None;
    }
    let specifier = strip_import_query_and_hash(specifier);
    let base = Path::new(base_filename);
    let parent = base.parent()?;
    relative_import_path_candidates(parent.join(specifier))
        .into_iter()
        .find_map(|path| fs::read_to_string(path).ok())
}

pub(crate) fn recover_single_file_vue_sidecar(code: &str, output_filename: &str) -> Option<String> {
    let resolver_filename = output_filename.to_string();
    wakaru::vue::recover(
        wakaru::Source::new(output_filename, code),
        wakaru::vue::RecoveryOptions::default().with_import_resolver(move |specifier: &str| {
            read_relative_import_source(&resolver_filename, specifier)
        }),
    )
    .ok()?
    .into_iter()
    .next()
    .map(|recovered| recovered.source)
}

pub(crate) fn recover_single_file_vue_after_unpack(
    code: &str,
    filename: &str,
    rewrite: wakaru::RewriteOptions,
    diagnostics: bool,
) -> Option<String> {
    let output = wakaru::unpack(
        vec![wakaru::Source::new(filename, code)],
        wakaru::UnpackOptions::default()
            .with_modules(wakaru::ModuleMode::Decompile(rewrite))
            .with_unmatched(wakaru::UnmatchedInput::Process)
            .with_diagnostics(diagnostics),
    )
    .ok()?;
    output
        .modules
        .iter()
        .find_map(|module| recover_single_file_vue_sidecar(&module.code, &module.filename))
}

pub(crate) struct SingleFileVueMetadata {
    pub(crate) kind: JsonModuleKind,
    pub(crate) status: JsonModuleStatus,
    pub(crate) source_filename: Option<String>,
    pub(crate) vue_sidecar_filename: Option<String>,
}

pub(crate) fn vue_sfc_js_artifact_status(
    recovered_vue_sfc: bool,
    likely_vue_sfc: bool,
) -> JsonModuleStatus {
    if recovered_vue_sfc {
        JsonModuleStatus::VueSfcSourceJs
    } else if likely_vue_sfc {
        JsonModuleStatus::VueSfcFallbackJs
    } else {
        JsonModuleStatus::Decompiled
    }
}

pub(crate) fn single_file_vue_metadata(
    vue_sfc: bool,
    recovered_vue_sfc: bool,
    js_primary_vue_output: bool,
    output_code: &str,
    output_filename: &str,
    vue_sidecar_path: Option<&Path>,
) -> Option<SingleFileVueMetadata> {
    if !vue_sfc {
        return None;
    }

    let likely_vue_sfc =
        recovered_vue_sfc || is_likely_vue_sfc_source(output_code).unwrap_or(false);
    Some(SingleFileVueMetadata {
        kind: if recovered_vue_sfc && !js_primary_vue_output {
            JsonModuleKind::VueSfc
        } else {
            JsonModuleKind::JavaScript
        },
        status: if recovered_vue_sfc {
            if js_primary_vue_output {
                JsonModuleStatus::VueSfcSourceJs
            } else {
                JsonModuleStatus::RecoveredVueSfc
            }
        } else if likely_vue_sfc {
            JsonModuleStatus::VueSfcFallbackJs
        } else {
            JsonModuleStatus::Decompiled
        },
        source_filename: likely_vue_sfc.then(|| output_filename.to_string()),
        vue_sidecar_filename: vue_sidecar_path.map(|path| path.to_string_lossy().into_owned()),
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct VueSfcArtifactSummary {
    recovered: usize,
    fallback: usize,
}

pub(crate) fn vue_sfc_artifact_summary(
    artifacts: &[CliOutputArtifact],
) -> Option<VueSfcArtifactSummary> {
    let summary =
        artifacts
            .iter()
            .fold(VueSfcArtifactSummary::default(), |mut summary, artifact| {
                match artifact.status {
                    JsonModuleStatus::RecoveredVueSfc => summary.recovered += 1,
                    JsonModuleStatus::VueSfcFallbackJs => summary.fallback += 1,
                    _ => {}
                }
                summary
            });

    (summary.recovered > 0 || summary.fallback > 0).then_some(summary)
}

pub(crate) fn format_vue_sfc_artifact_summary(summary: VueSfcArtifactSummary) -> String {
    format!(
        "vue-sfc: {} recovered, {} fallback",
        summary.recovered, summary.fallback
    )
}

pub(crate) fn resolve_unpack_import_source(
    module_sources: &HashMap<String, String>,
    base_filename: &str,
    specifier: &str,
) -> Option<String> {
    if !(specifier.starts_with("./") || specifier.starts_with("../")) {
        return None;
    }

    let specifiers = import_lookup_specifiers(specifier);

    for specifier in &specifiers {
        if let Some(base_relative) =
            normalize_relative_module_specifier_from_base(base_filename, specifier)
        {
            if let Some(source) = find_resolved_module_source(module_sources, &base_relative) {
                return Some(source);
            }
        }
    }

    for specifier in &specifiers {
        if let Some(root_relative) = normalize_relative_module_specifier(specifier) {
            if let Some(source) = find_resolved_module_source(module_sources, &root_relative) {
                return Some(source);
            }
        }
    }

    None
}

const VUE_IMPORT_RESOLVE_EXTENSIONS: &[&str] = &["vue", "js", "mjs", "cjs"];

fn strip_import_query_and_hash(specifier: &str) -> &str {
    specifier
        .find(['?', '#'])
        .map_or(specifier, |idx| &specifier[..idx])
}

fn import_lookup_specifiers(specifier: &str) -> Vec<&str> {
    let stripped = strip_import_query_and_hash(specifier);
    if stripped == specifier {
        vec![specifier]
    } else {
        vec![specifier, stripped]
    }
}

fn relative_import_path_candidates(path: PathBuf) -> Vec<PathBuf> {
    let mut candidates = vec![path.clone()];
    if path.extension().is_none() {
        candidates.extend(
            VUE_IMPORT_RESOLVE_EXTENSIONS
                .iter()
                .map(|ext| path.with_extension(ext)),
        );
        candidates.extend(
            VUE_IMPORT_RESOLVE_EXTENSIONS
                .iter()
                .map(|ext| path.join(format!("index.{ext}"))),
        );
    }
    candidates
}

fn find_resolved_module_source(
    module_sources: &HashMap<String, String>,
    normalized: &str,
) -> Option<String> {
    module_lookup_candidates(normalized)
        .into_iter()
        .find_map(|candidate| module_sources.get(&candidate).cloned())
}

fn module_lookup_candidates(normalized: &str) -> Vec<String> {
    let mut candidates = vec![normalized.to_string()];
    if Path::new(normalized).extension().is_none() {
        candidates.extend(
            VUE_IMPORT_RESOLVE_EXTENSIONS
                .iter()
                .map(|ext| format!("{normalized}.{ext}")),
        );
        candidates.extend(
            VUE_IMPORT_RESOLVE_EXTENSIONS
                .iter()
                .map(|ext| format!("{normalized}/index.{ext}")),
        );
    }
    candidates
}

fn normalize_relative_module_specifier(specifier: &str) -> Option<String> {
    normalize_relative_module_path(Vec::new(), specifier)
}

fn normalize_relative_module_specifier_from_base(
    base_filename: &str,
    specifier: &str,
) -> Option<String> {
    let mut parts = normalized_path_parts(base_filename);
    parts.pop()?;
    normalize_relative_module_path(parts, specifier)
}

fn normalize_relative_module_path(mut parts: Vec<String>, path: &str) -> Option<String> {
    for part in path.replace('\\', "/").split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part.to_string()),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn normalized_path_parts(path: &str) -> Vec<String> {
    path.replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(ToString::to_string)
        .collect()
}

fn vue_output_filename(filename: &str) -> String {
    let path = Path::new(filename);
    let stem_preserves_vue_extension = path
        .file_stem()
        .and_then(|stem| Path::new(stem).extension())
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("vue"));
    if stem_preserves_vue_extension {
        return path.with_extension("").to_string_lossy().to_string();
    }
    if path.extension().is_some() {
        return path.with_extension("vue").to_string_lossy().to_string();
    }
    format!("{filename}.vue")
}

pub(crate) fn vue_output_filename_for_component(
    filename: &str,
    component_name: Option<&str>,
    disambiguate: bool,
) -> String {
    let vue_filename = vue_output_filename(filename);
    if !disambiguate {
        return vue_filename;
    }
    let Some(component_name) = component_name.and_then(safe_vue_component_filename_part) else {
        return vue_filename;
    };
    let (dir, file) = vue_filename
        .rfind(['/', '\\'])
        .map(|index| vue_filename.split_at(index + 1))
        .unwrap_or(("", vue_filename.as_str()));
    let stem = file.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(file);
    let stem = if stem.is_empty() { "component" } else { stem };
    format!("{dir}{stem}.{component_name}.vue")
}

fn safe_vue_component_filename_part(name: &str) -> Option<String> {
    let safe = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    (!safe.is_empty() && safe.chars().any(|ch| ch != '_')).then_some(safe)
}

pub(crate) fn vue_js_output_filename(filename: &str) -> String {
    let path = Path::new(filename);
    if path.extension().is_some_and(|ext| ext == "vue") {
        return format!("{filename}.js");
    }
    filename.to_string()
}

pub(crate) fn single_file_vue_sidecar_path(input_filename: &str, output_path: &Path) -> PathBuf {
    let input_file_name = if input_filename == "<stdin>" {
        output_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("output")
    } else {
        Path::new(input_filename)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("output")
    };
    let sidecar_name = vue_output_filename(input_file_name);
    output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.join(&sidecar_name))
        .unwrap_or_else(|| PathBuf::from(sidecar_name))
}

pub(crate) fn is_vue_output_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("vue"))
}

pub(crate) fn ensure_vue_sidecar_does_not_overwrite_input(
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
            "refusing to write Vue sidecar over input file {}; choose a different output path",
            input_path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{temp_test_dir, vue_render_module_source};
    use wakaru_core::{
        decompile, decompile_vue_sfc, DecompileOptions, VueSfcDecompileOptions,
        VueSfcRecoveryOptions,
    };

    #[test]
    fn vue_output_filename_replaces_known_extension() {
        assert_eq!(vue_output_filename("module-1.js"), "module-1.vue");
        assert_eq!(
            vue_output_filename("src/App.render.mjs"),
            "src/App.render.vue"
        );
        assert_eq!(vue_output_filename("src/App.vue.js"), "src/App.vue");
        assert_eq!(vue_output_filename("src/App.VUE.ts"), "src/App.VUE");
        assert_eq!(vue_output_filename("module-plain"), "module-plain.vue");
    }

    #[test]
    fn vue_js_output_filename_avoids_vue_artifact_collision() {
        assert_eq!(vue_js_output_filename("src/App.vue"), "src/App.vue.js");
        assert_eq!(vue_js_output_filename("src/App.js"), "src/App.js");
        assert_eq!(vue_js_output_filename("module-plain"), "module-plain");
    }

    #[test]
    fn vue_output_filename_for_component_disambiguates_multi_sfc_modules() {
        assert_eq!(
            vue_output_filename_for_component("entry.js", Some("HelloWorld"), true),
            "entry.HelloWorld.vue"
        );
        assert_eq!(
            vue_output_filename_for_component("src/entry.js", Some("App"), true),
            "src/entry.App.vue"
        );
        assert_eq!(
            vue_output_filename_for_component("entry.js", Some("../App"), true),
            "entry.___App.vue"
        );
        assert_eq!(
            vue_output_filename_for_component("entry.js", Some("App"), false),
            "entry.vue"
        );
    }

    #[test]
    fn vue_sfc_relative_import_resolver_ignores_stdin_base() {
        assert_eq!(read_relative_import_source("<stdin>", "./main.js"), None);
    }

    #[test]
    fn vue_sfc_relative_import_resolver_reads_extensionless_and_query_paths() {
        let dir = temp_test_dir("vue-sfc-relative-import-resolver");
        let components_dir = dir.join("components");
        fs::create_dir_all(&components_dir).expect("create temp dir");
        let input_path = dir.join("App.js");
        fs::write(&input_path, "export default {};").expect("write input");
        fs::write(components_dir.join("Child.vue"), "export default {};").expect("write component");
        fs::write(components_dir.join("Panel.js"), "export default {};").expect("write js module");
        fs::create_dir_all(components_dir.join("Dialog")).expect("create index dir");
        fs::write(
            components_dir.join("Dialog").join("index.vue"),
            "export default {};",
        )
        .expect("write index component");

        assert_eq!(
            read_relative_import_source(
                input_path.to_str().expect("input path should be utf8"),
                "./components/Child.vue?vue&type=script"
            ),
            Some("export default {};".to_string())
        );
        assert_eq!(
            read_relative_import_source(
                input_path.to_str().expect("input path should be utf8"),
                "./components/Child?vue&type=script"
            ),
            Some("export default {};".to_string())
        );
        assert_eq!(
            read_relative_import_source(
                input_path.to_str().expect("input path should be utf8"),
                "./components/Panel"
            ),
            Some("export default {};".to_string())
        );
        assert_eq!(
            read_relative_import_source(
                input_path.to_str().expect("input path should be utf8"),
                "./components/Dialog"
            ),
            Some("export default {};".to_string())
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn vue_sfc_unpack_import_resolver_reads_root_relative_module_source() {
        let module_sources = HashMap::from([(
            "src/components/ChildPanel.vue".to_string(),
            "export default {};".to_string(),
        )]);

        assert_eq!(
            resolve_unpack_import_source(
                &module_sources,
                "src/App.vue",
                "./src/components/ChildPanel.vue"
            ),
            Some("export default {};".to_string())
        );
    }

    #[test]
    fn vue_sfc_unpack_import_resolver_reads_module_relative_source() {
        let module_sources = HashMap::from([(
            "src/components/ChildPanel.vue".to_string(),
            "export default {};".to_string(),
        )]);

        assert_eq!(
            resolve_unpack_import_source(
                &module_sources,
                "src/App.vue",
                "./components/ChildPanel.vue"
            ),
            Some("export default {};".to_string())
        );
    }

    #[test]
    fn vue_sfc_unpack_import_resolver_prefers_module_relative_over_root_collision() {
        let module_sources = HashMap::from([
            (
                "components/ChildPanel.vue".to_string(),
                "export default { name: 'RootChild' };".to_string(),
            ),
            (
                "src/components/ChildPanel.vue".to_string(),
                "export default { name: 'ScopedChild' };".to_string(),
            ),
        ]);

        assert_eq!(
            resolve_unpack_import_source(
                &module_sources,
                "src/App.vue",
                "./components/ChildPanel.vue"
            ),
            Some("export default { name: 'ScopedChild' };".to_string())
        );
    }

    #[test]
    fn vue_sfc_unpack_import_resolver_reads_extensionless_query_and_index_sources() {
        let module_sources = HashMap::from([
            (
                "src/components/ChildPanel.vue".to_string(),
                "export default {};".to_string(),
            ),
            (
                "src/components/Panel.js".to_string(),
                "export const panel = true;".to_string(),
            ),
            (
                "src/components/Dialog/index.vue".to_string(),
                "export const dialog = true;".to_string(),
            ),
        ]);

        assert_eq!(
            resolve_unpack_import_source(
                &module_sources,
                "src/App.vue",
                "./components/ChildPanel.vue?vue&type=script"
            ),
            Some("export default {};".to_string())
        );
        assert_eq!(
            resolve_unpack_import_source(
                &module_sources,
                "src/App.vue",
                "./components/ChildPanel?vue&type=script"
            ),
            Some("export default {};".to_string())
        );
        assert_eq!(
            resolve_unpack_import_source(&module_sources, "src/App.vue", "./components/Panel"),
            Some("export const panel = true;".to_string())
        );
        assert_eq!(
            resolve_unpack_import_source(&module_sources, "src/App.vue", "./components/Dialog"),
            Some("export const dialog = true;".to_string())
        );
    }

    #[test]
    fn vue_sfc_unpack_import_resolver_reads_parent_relative_sources() {
        let module_sources = HashMap::from([(
            "src/components/ChildPanel.vue".to_string(),
            "export default {};".to_string(),
        )]);

        assert_eq!(
            resolve_unpack_import_source(
                &module_sources,
                "src/views/App.vue",
                "../components/ChildPanel"
            ),
            Some("export default {};".to_string())
        );
    }

    #[test]
    fn single_file_vue_metadata_describes_recovered_sfc_output() {
        let output = decompile_vue_sfc(
            vue_render_module_source(),
            VueSfcDecompileOptions {
                decompile: DecompileOptions {
                    filename: "src/App.vue".to_string(),
                    ..Default::default()
                },
                recovery: VueSfcRecoveryOptions::default(),
            },
        )
        .expect("vue sfc decompile should succeed");

        let metadata = single_file_vue_metadata(
            true,
            output.recovered_sfc,
            false,
            &output.output.code,
            "src/App.vue",
            None,
        )
        .expect("vue metadata should be emitted");

        assert!(output.recovered_sfc);
        assert_eq!(metadata.kind, JsonModuleKind::VueSfc);
        assert_eq!(metadata.status, JsonModuleStatus::RecoveredVueSfc);
        assert_eq!(metadata.source_filename.as_deref(), Some("src/App.vue"));
        assert_eq!(metadata.vue_sidecar_filename, None);
    }

    #[test]
    fn single_file_vue_metadata_describes_js_primary_sidecar_output() {
        let output = decompile(
            vue_render_module_source(),
            DecompileOptions {
                filename: "src/App.vue".to_string(),
                ..Default::default()
            },
        )
        .expect("decompile should succeed");
        let sidecar = recover_single_file_vue_sidecar(&output.code, "src/App.vue");
        let sidecar_path = PathBuf::from("dist/App.vue");

        let metadata = single_file_vue_metadata(
            true,
            sidecar.is_some(),
            true,
            &output.code,
            "src/App.vue",
            Some(&sidecar_path),
        )
        .expect("vue metadata should be emitted");

        assert!(sidecar.is_some());
        assert_eq!(metadata.kind, JsonModuleKind::JavaScript);
        assert_eq!(metadata.status, JsonModuleStatus::VueSfcSourceJs);
        assert_eq!(metadata.source_filename.as_deref(), Some("src/App.vue"));
        assert_eq!(
            metadata.vue_sidecar_filename.as_deref(),
            Some("dist/App.vue")
        );
    }

    #[test]
    fn single_file_vue_metadata_describes_plain_js_output() {
        let output = decompile(
            "export const value = 1;",
            DecompileOptions {
                filename: "src/plain.js".to_string(),
                ..Default::default()
            },
        )
        .expect("decompile should succeed");

        let metadata =
            single_file_vue_metadata(true, false, false, &output.code, "src/plain.js", None)
                .expect("vue metadata should be emitted under --vue-sfc");

        assert_eq!(metadata.kind, JsonModuleKind::JavaScript);
        assert_eq!(metadata.status, JsonModuleStatus::Decompiled);
        assert_eq!(metadata.source_filename, None);
        assert_eq!(metadata.vue_sidecar_filename, None);
    }

    #[test]
    fn single_file_vue_metadata_describes_likely_vue_fallback_output() {
        let output = decompile(
            r#"
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("div", null, "Hi");
}
"#,
            DecompileOptions {
                filename: "src/Broken.vue".to_string(),
                ..Default::default()
            },
        )
        .expect("decompile should succeed");

        let metadata =
            single_file_vue_metadata(true, false, false, &output.code, "src/Broken.vue", None)
                .expect("vue metadata should be emitted under --vue-sfc");

        assert_eq!(metadata.kind, JsonModuleKind::JavaScript);
        assert_eq!(metadata.status, JsonModuleStatus::VueSfcFallbackJs);
        assert_eq!(metadata.source_filename.as_deref(), Some("src/Broken.vue"));
        assert_eq!(metadata.vue_sidecar_filename, None);
    }

    #[test]
    fn vue_sidecar_recovery_errors_fall_back_to_js_primary_output() {
        assert!(recover_single_file_vue_sidecar("function {", "src/App.js").is_none());
    }

    #[test]
    fn vue_sfc_js_artifact_status_marks_only_likely_vue_fallbacks() {
        assert_eq!(
            vue_sfc_js_artifact_status(false, false),
            JsonModuleStatus::Decompiled
        );
        assert_eq!(
            vue_sfc_js_artifact_status(false, true),
            JsonModuleStatus::VueSfcFallbackJs
        );
        assert_eq!(
            vue_sfc_js_artifact_status(true, true),
            JsonModuleStatus::VueSfcSourceJs
        );
    }

    fn test_cli_artifact(status: JsonModuleStatus) -> CliOutputArtifact {
        CliOutputArtifact {
            filename: "module.js".to_string(),
            code: "export {};".to_string(),
            kind: JsonModuleKind::JavaScript,
            status,
            source_filename: None,
            source_map_filename: None,
        }
    }

    #[test]
    fn vue_sfc_artifact_summary_counts_recovered_and_fallback_modules() {
        let artifacts = vec![
            test_cli_artifact(JsonModuleStatus::VueSfcSourceJs),
            test_cli_artifact(JsonModuleStatus::RecoveredVueSfc),
            test_cli_artifact(JsonModuleStatus::VueSfcFallbackJs),
            test_cli_artifact(JsonModuleStatus::Decompiled),
        ];

        let summary = vue_sfc_artifact_summary(&artifacts);
        assert_eq!(
            summary,
            Some(VueSfcArtifactSummary {
                recovered: 1,
                fallback: 1
            })
        );
        assert_eq!(
            format_vue_sfc_artifact_summary(summary.expect("summary")),
            "vue-sfc: 1 recovered, 1 fallback"
        );
    }

    #[test]
    fn vue_sfc_artifact_summary_ignores_plain_js_modules() {
        let artifacts = vec![test_cli_artifact(JsonModuleStatus::Decompiled)];

        assert_eq!(vue_sfc_artifact_summary(&artifacts), None);
    }
}
