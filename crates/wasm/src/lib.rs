use serde::Serialize;
use wakaru_formatter::{format_code, CodeFormatter};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

#[derive(Serialize)]
struct WakaruModule {
    filename: String,
    code: String,
}

#[derive(Serialize)]
struct WakaruUnpackResult {
    modules: Vec<WakaruModule>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    source_maps: Vec<WakaruSourceMap>,
    warnings: Vec<WakaruWarning>,
}

#[derive(Serialize)]
struct WakaruSourceMap {
    filename: String,
    map: String,
}

#[derive(Serialize)]
struct WakaruWarning {
    filename: String,
    kind: &'static str,
    message: String,
}

#[derive(Serialize)]
struct WakaruDecompileResult {
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_map: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    vue_sfc: Option<String>,
    warnings: Vec<WakaruWarning>,
}

#[wasm_bindgen(js_name = "decompile")]
pub fn decompile(
    source: &str,
    level: Option<String>,
    sourcemap: Option<Vec<u8>>,
    diagnostics: Option<bool>,
    formatter: Option<bool>,
    emit_source_map: Option<bool>,
    vue_sfc: Option<bool>,
) -> Result<JsValue, JsValue> {
    let level = parse_level(level.as_deref());
    let formatter = parse_formatter(formatter);
    let rewrite = wakaru::RewriteOptions::default()
        .with_level(level)
        .with_dce(wakaru::DceMode::TransformOnly);
    let options = wakaru::DecompileOptions::default()
        .with_rewrite(rewrite)
        .with_diagnostics(diagnostics.unwrap_or(false))
        .with_output_source_map(emit_source_map.unwrap_or(false));
    let mut input = wakaru::Source::new("input.js", source);
    if let Some(sourcemap) = sourcemap {
        input = input.with_source_map(sourcemap);
    }
    let output =
        wakaru::decompile(input, options).map_err(|error| JsValue::from_str(&error.to_string()))?;
    let vue_sfc = recover_vue_sfc_preview(&output.module.code, vue_sfc.unwrap_or(false));
    let formatted = format_code(output.module.code, "input.js", formatter);
    let result = WakaruDecompileResult {
        code: formatted.code,
        source_map: output.module.source_map,
        vue_sfc,
        warnings: collect_warnings(output.diagnostics, ["input.js"], formatted.warning),
    };
    serde_wasm_bindgen::to_value(&result).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen(js_name = "unpack")]
pub fn unpack(
    source: &str,
    level: Option<String>,
    heuristic_split: Option<bool>,
    diagnostics: Option<bool>,
    formatter: Option<bool>,
    emit_source_map: Option<bool>,
) -> Result<JsValue, JsValue> {
    let level = parse_level(level.as_deref());
    let formatter = parse_formatter(formatter);
    let rewrite = wakaru::RewriteOptions::default().with_level(level);
    let options = wakaru::UnpackOptions::default()
        .with_modules(wakaru::ModuleMode::Decompile(rewrite))
        .with_scope_hoist(if heuristic_split.unwrap_or(true) {
            wakaru::ScopeHoistMode::Fallback
        } else {
            wakaru::ScopeHoistMode::Disabled
        })
        .with_diagnostics(diagnostics.unwrap_or(false))
        .with_output_source_maps(emit_source_map.unwrap_or(false));
    let output = wakaru::unpack(vec![wakaru::Source::new("input.js", source)], options)
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    let mut format_warnings = Vec::new();
    let module_filenames = output
        .modules
        .iter()
        .map(|module| module.filename.clone())
        .collect::<Vec<_>>();
    let source_maps = output
        .modules
        .iter()
        .filter_map(|module| {
            module.source_map.as_ref().map(|map| WakaruSourceMap {
                filename: module.filename.clone(),
                map: map.clone(),
            })
        })
        .collect();
    let result = WakaruUnpackResult {
        modules: output
            .modules
            .into_iter()
            .map(|module| {
                let formatted = format_code(module.code, &module.filename, formatter);
                if let Some(warning) = formatted.warning {
                    format_warnings.push(warning);
                }
                WakaruModule {
                    filename: module.filename,
                    code: formatted.code,
                }
            })
            .collect(),
        source_maps,
        warnings: collect_warnings(output.diagnostics, module_filenames, format_warnings),
    };
    serde_wasm_bindgen::to_value(&result).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen(js_name = "ruleNames")]
pub fn rule_names() -> JsValue {
    let names = wakaru::debug::rules()
        .iter()
        .map(|rule| rule.id)
        .collect::<Vec<_>>();
    serde_wasm_bindgen::to_value(&names).unwrap_or(JsValue::NULL)
}

fn parse_level(level: Option<&str>) -> wakaru::RewriteLevel {
    match level {
        Some("minimal") => wakaru::RewriteLevel::Minimal,
        Some("aggressive") => wakaru::RewriteLevel::Aggressive,
        None | Some(_) => wakaru::RewriteLevel::Standard,
    }
}

fn parse_formatter(formatter: Option<bool>) -> CodeFormatter {
    match formatter {
        Some(true) => CodeFormatter::Oxc,
        None | Some(false) => CodeFormatter::None,
    }
}

fn recover_vue_sfc_preview(source: &str, enabled: bool) -> Option<String> {
    if !enabled {
        return None;
    }

    wakaru::vue::recover(
        wakaru::Source::new("input.js", source),
        wakaru::vue::RecoveryOptions::default(),
    )
    .ok()?
    .into_iter()
    .next()
    .map(|recovered| recovered.source)
}

fn collect_warnings(
    diagnostics: Vec<wakaru::Diagnostic>,
    module_filenames: impl IntoIterator<Item = impl AsRef<str>>,
    format_warnings: impl IntoIterator<Item = wakaru_formatter::FormatWarning>,
) -> Vec<WakaruWarning> {
    let module_filenames = module_filenames
        .into_iter()
        .map(|filename| filename.as_ref().to_string())
        .collect::<Vec<_>>();
    diagnostics
        .into_iter()
        .map(|diagnostic| WakaruWarning {
            filename: diagnostic
                .module
                .and_then(|index| module_filenames.get(index).cloned())
                .unwrap_or_else(|| "input.js".to_string()),
            kind: wasm_warning_kind(diagnostic.code),
            message: diagnostic.message,
        })
        .chain(format_warnings.into_iter().map(|warning| WakaruWarning {
            filename: warning.filename,
            kind: "formatter_failed",
            message: format!(
                "{} formatter failed, preserving output: {}",
                warning.formatter.as_str(),
                warning.message
            ),
        }))
        .collect()
}

fn wasm_warning_kind(code: wakaru::DiagnosticCode) -> &'static str {
    match code {
        // Preserve the existing WASM wire value even though the façade uses a
        // shorter public diagnostic name.
        wakaru::DiagnosticCode::FactCollectionFailed => "fact_collection_parse_failed",
        _ => code.as_str(),
    }
}

#[wasm_bindgen(typescript_custom_section)]
const TS_DEFS: &str = r#"
export interface WakaruModule {
    filename: string;
    code: string;
}

export interface WakaruDecompileResult {
    code: string;
    source_map?: string;
    vue_sfc?: string;
    warnings: WakaruWarning[];
}

export function decompile(
    source: string,
    level?: "minimal" | "standard" | "aggressive",
    sourcemap?: Uint8Array,
    diagnostics?: boolean,
    formatter?: boolean,
    emitSourceMap?: boolean,
    vueSfc?: boolean,
): WakaruDecompileResult;

export interface WakaruSourceMap {
    filename: string;
    map: string;
}

export interface WakaruUnpackResult {
    modules: WakaruModule[];
    source_maps?: WakaruSourceMap[];
    warnings: WakaruWarning[];
}

export type WakaruWarningKind =
    | "input_parse_recovered"
    | "raw_normalization_failed"
    | "fact_collection_parse_failed"
    | "decompile_failed"
    | "tdz_violation"
    | "duplicate_declaration"
    | "import_cycle"
    | "output_parse_recovered"
    | "output_parse_failed"
    | "formatter_failed";

export interface WakaruWarning {
    filename: string;
    kind: WakaruWarningKind;
    message: string;
}

export function unpack(
    source: string,
    level?: "minimal" | "standard" | "aggressive",
    heuristicSplit?: boolean,
    diagnostics?: boolean,
    formatter?: boolean,
    emitSourceMap?: boolean,
): WakaruUnpackResult;

export function ruleNames(): string[];
"#;

#[cfg(test)]
mod tests {
    use super::{recover_vue_sfc_preview, wasm_warning_kind};

    const WASM_BINDING_SOURCE: &str = include_str!("lib.rs");

    const VUE_RENDER_MODULE: &str = r#"
        import { createElementVNode as _createElementVNode } from "vue";
        export function render(_ctx, _cache) {
            return _createElementVNode("div", { class: "card" }, "Hello");
        }
    "#;

    #[test]
    fn vue_sfc_preview_is_opt_in() {
        assert_eq!(recover_vue_sfc_preview(VUE_RENDER_MODULE, false), None);
    }

    #[test]
    fn vue_sfc_preview_recovers_generated_render_module() {
        let recovered = recover_vue_sfc_preview(VUE_RENDER_MODULE, true)
            .expect("Vue render module should recover");

        assert!(recovered.contains("<template>"));
        assert!(recovered.contains("<div class=\"card\">Hello</div>"));
    }

    #[test]
    fn vue_sfc_preview_failure_is_non_fatal() {
        assert_eq!(recover_vue_sfc_preview("export {", true), None);
    }

    #[test]
    fn wasm_warning_kinds_match_the_typescript_union() {
        let warning_union = WASM_BINDING_SOURCE
            .split_once("export type WakaruWarningKind =")
            .expect("TypeScript warning union should exist")
            .1
            .split_once(';')
            .expect("TypeScript warning union should end with a semicolon")
            .0;
        let cases = [
            (
                wakaru::DiagnosticCode::InputParseRecovered,
                "input_parse_recovered",
            ),
            (
                wakaru::DiagnosticCode::RawNormalizationFailed,
                "raw_normalization_failed",
            ),
            (
                wakaru::DiagnosticCode::FactCollectionFailed,
                "fact_collection_parse_failed",
            ),
            (wakaru::DiagnosticCode::DecompileFailed, "decompile_failed"),
            (wakaru::DiagnosticCode::TdzViolation, "tdz_violation"),
            (
                wakaru::DiagnosticCode::DuplicateDeclaration,
                "duplicate_declaration",
            ),
            (wakaru::DiagnosticCode::ImportCycle, "import_cycle"),
            (
                wakaru::DiagnosticCode::OutputParseRecovered,
                "output_parse_recovered",
            ),
            (
                wakaru::DiagnosticCode::OutputParseFailed,
                "output_parse_failed",
            ),
        ];

        for (code, expected) in cases {
            assert_eq!(wasm_warning_kind(code), expected);
            assert!(
                warning_union.contains(&format!("\"{expected}\"")),
                "TypeScript warning union is missing {expected:?}"
            );
        }
        assert!(warning_union.contains("\"formatter_failed\""));
    }
}
