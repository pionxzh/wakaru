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
    let options = wakaru_core::DecompileOptions {
        filename: "input.js".to_string(),
        sourcemap,
        dce_mode: wakaru_core::DceMode::TransformOnly,
        level,
        diagnostics: diagnostics.unwrap_or(false),
        emit_source_map: emit_source_map.unwrap_or(false),
        ..Default::default()
    };
    let output =
        wakaru_core::decompile(source, options).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let vue_sfc = recover_vue_sfc_preview(&output.code, vue_sfc.unwrap_or(false))
        .map_err(|e| JsValue::from_str(&e))?;
    let formatted = format_code(output.code, "input.js", formatter);
    let result = WakaruDecompileResult {
        code: formatted.code,
        source_map: output.source_map,
        vue_sfc,
        warnings: collect_warnings(output.warnings, formatted.warning),
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
    let options = wakaru_core::DecompileOptions {
        filename: "input.js".to_string(),
        level,
        heuristic_split: heuristic_split.unwrap_or(true),
        diagnostics: diagnostics.unwrap_or(false),
        emit_source_map: emit_source_map.unwrap_or(false),
        ..Default::default()
    };
    let output =
        wakaru_core::unpack(source, options).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let mut format_warnings = Vec::new();
    let result = WakaruUnpackResult {
        modules: output
            .modules
            .into_iter()
            .map(|(filename, code)| {
                let formatted = format_code(code, &filename, formatter);
                if let Some(warning) = formatted.warning {
                    format_warnings.push(warning);
                }
                WakaruModule {
                    filename,
                    code: formatted.code,
                }
            })
            .collect(),
        source_maps: output
            .source_maps
            .into_iter()
            .map(|(filename, map)| WakaruSourceMap { filename, map })
            .collect(),
        warnings: collect_warnings(output.warnings, format_warnings),
    };
    serde_wasm_bindgen::to_value(&result).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen(js_name = "ruleNames")]
pub fn rule_names() -> JsValue {
    let names = wakaru_core::rule_names();
    serde_wasm_bindgen::to_value(&names).unwrap_or(JsValue::NULL)
}

fn parse_level(level: Option<&str>) -> wakaru_core::RewriteLevel {
    wakaru_core::RewriteLevel::from_str_or_default(level)
}

fn parse_formatter(formatter: Option<bool>) -> CodeFormatter {
    match formatter {
        Some(true) => CodeFormatter::Oxc,
        None | Some(false) => CodeFormatter::None,
    }
}

fn recover_vue_sfc_preview(source: &str, enabled: bool) -> Result<Option<String>, String> {
    if !enabled {
        return Ok(None);
    }

    wakaru_core::recover_vue_sfc_source_from_js(source, Default::default())
        .map_err(|error| error.to_string())
}

fn collect_warnings(
    warnings: Vec<wakaru_core::UnpackWarning>,
    format_warnings: impl IntoIterator<Item = wakaru_formatter::FormatWarning>,
) -> Vec<WakaruWarning> {
    warnings
        .into_iter()
        .map(|warning| WakaruWarning {
            filename: warning.filename,
            kind: warning.kind.as_str(),
            message: warning.message,
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
    | "raw_normalization_failed"
    | "fact_collection_parse_failed"
    | "decompile_failed"
    | "tdz_violation"
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
    use super::recover_vue_sfc_preview;

    const VUE_RENDER_MODULE: &str = r#"
        import { createElementVNode as _createElementVNode } from "vue";
        export function render(_ctx, _cache) {
            return _createElementVNode("div", { class: "card" }, "Hello");
        }
    "#;

    #[test]
    fn vue_sfc_preview_is_opt_in() {
        assert_eq!(
            recover_vue_sfc_preview(VUE_RENDER_MODULE, false).unwrap(),
            None
        );
    }

    #[test]
    fn vue_sfc_preview_recovers_generated_render_module() {
        let recovered = recover_vue_sfc_preview(VUE_RENDER_MODULE, true)
            .unwrap()
            .expect("Vue render module should recover");

        assert!(recovered.contains("<template>"));
        assert!(recovered.contains("<div class=\"card\">Hello</div>"));
    }
}
