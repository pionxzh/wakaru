use serde::Serialize;
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

#[wasm_bindgen(js_name = "decompile")]
pub fn decompile(
    source: &str,
    level: Option<String>,
    sourcemap: Option<Vec<u8>>,
) -> Result<String, JsValue> {
    let level = parse_level(level.as_deref());
    let options = wakaru_core::DecompileOptions {
        filename: "input.js".to_string(),
        sourcemap,
        level,
        ..Default::default()
    };
    wakaru_core::decompile(source, options).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen(js_name = "unpack")]
pub fn unpack(
    source: &str,
    level: Option<String>,
    heuristic_split: Option<bool>,
) -> Result<JsValue, JsValue> {
    let level = parse_level(level.as_deref());
    let options = wakaru_core::DecompileOptions {
        filename: "input.js".to_string(),
        level,
        heuristic_split: heuristic_split.unwrap_or(true),
        ..Default::default()
    };
    let pairs = wakaru_core::unpack(source, options).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let modules: Vec<WakaruModule> = pairs
        .into_iter()
        .map(|(filename, code)| WakaruModule { filename, code })
        .collect();
    serde_wasm_bindgen::to_value(&modules).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen(js_name = "ruleNames")]
pub fn rule_names() -> JsValue {
    let names = wakaru_core::rule_names();
    serde_wasm_bindgen::to_value(&names).unwrap_or(JsValue::NULL)
}

fn parse_level(level: Option<&str>) -> wakaru_core::RewriteLevel {
    match level {
        Some("minimal") => wakaru_core::RewriteLevel::Minimal,
        Some("aggressive") => wakaru_core::RewriteLevel::Aggressive,
        _ => wakaru_core::RewriteLevel::Standard,
    }
}

#[wasm_bindgen(typescript_custom_section)]
const TS_DEFS: &str = r#"
export interface WakaruModule {
    filename: string;
    code: string;
}

export function decompile(
    source: string,
    level?: "minimal" | "standard" | "aggressive",
    sourcemap?: Uint8Array,
): string;

export function unpack(
    source: string,
    level?: "minimal" | "standard" | "aggressive",
    heuristicSplit?: boolean,
): WakaruModule[];

export function ruleNames(): string[];
"#;
