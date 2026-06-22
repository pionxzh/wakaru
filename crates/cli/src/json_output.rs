use serde::Serialize;

#[derive(Serialize)]
pub struct JsonUnpackOutput {
    pub detected_formats: Vec<String>,
    pub modules: Vec<JsonModule>,
    pub warnings: Vec<JsonWarning>,
    pub total: usize,
    pub failed: usize,
    pub elapsed_ms: u64,
}

#[derive(Serialize)]
pub struct JsonDecompileOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_map: Option<String>,
    pub warnings: Vec<JsonWarning>,
    pub elapsed_ms: u64,
}

#[derive(Serialize)]
pub struct JsonModule {
    pub filename: String,
    pub kind: JsonModuleKind,
    pub status: JsonModuleStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_filename: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum JsonModuleKind {
    #[serde(rename = "javascript")]
    JavaScript,
    #[serde(rename = "vue_sfc")]
    VueSfc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum JsonModuleStatus {
    #[serde(rename = "decompiled")]
    Decompiled,
    #[serde(rename = "vue_sfc_source_js")]
    VueSfcSourceJs,
    #[serde(rename = "recovered_vue_sfc")]
    RecoveredVueSfc,
    #[serde(rename = "vue_sfc_fallback_js")]
    VueSfcFallbackJs,
}

#[derive(Serialize)]
pub struct JsonWarning {
    pub filename: String,
    pub kind: String,
    pub is_error: bool,
    pub message: String,
}

impl JsonWarning {
    pub fn from_core(w: &wakaru_core::UnpackWarning) -> Self {
        Self {
            filename: w.filename.clone(),
            kind: w.kind.as_str().to_string(),
            is_error: w.kind.is_error(),
            message: w.message.clone(),
        }
    }
}
