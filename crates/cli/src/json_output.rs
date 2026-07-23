use serde::Serialize;

#[derive(Serialize)]
pub struct JsonUnpackOutput {
    pub detected_formats: Vec<String>,
    pub safety: String,
    pub modules: Vec<JsonModule>,
    pub warnings: Vec<JsonWarning>,
    pub total: usize,
    pub failed: usize,
    pub elapsed_ms: u64,
}

#[derive(Serialize)]
pub struct JsonChunkEnumerationOutput {
    pub input: String,
    pub detected_format: Option<String>,
    pub enumeration: Option<JsonChunkEnumeration>,
}

#[derive(Serialize)]
pub struct JsonChunkEnumeration {
    pub public_path: JsonPublicPath,
    /// webpack runtime chunk-filename table enumeration.
    pub assets: Vec<JsonChunkAsset>,
    /// Literal relative ESM specifiers (native code-splitting). Omitted when
    /// empty. Each is a relative sibling-chunk URL; resolve against the entry.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relative_imports: Vec<JsonRelativeImport>,
}

#[derive(Serialize)]
pub struct JsonRelativeImport {
    pub specifier: String,
    /// `import` | `export_from` | `dynamic_import`.
    pub kind: String,
}

#[derive(Serialize)]
pub struct JsonPublicPath {
    /// `static` | `script_relative` | `runtime_computed` | `not_found`.
    pub status: String,
    /// The literal value for `static`, or the script-relative suffix for
    /// `script_relative`. Absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Serialize)]
pub struct JsonChunkAsset {
    /// `js` | `css`.
    pub kind: String,
    /// `enumerated` | `no_static_chunk_ids` | `dynamic_template`.
    pub status: String,
    /// Debug-oriented placeholder rendering of the filename template.
    /// Absent when the template is dynamic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Relative URLs exactly as the runtime template renders them;
    /// `public_path` is not prepended.
    pub urls: Vec<JsonChunkUrl>,
}

#[derive(Serialize)]
pub struct JsonChunkUrl {
    pub chunk_id: String,
    pub url: String,
    /// `filename_map` | `ensure_call`.
    pub source: String,
}

#[derive(Serialize)]
pub struct JsonDecompileOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_map: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<JsonModuleKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<JsonModuleStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vue_sidecar_filename: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<JsonModule>,
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
    #[serde(rename = "angular_component")]
    AngularComponent,
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
    #[serde(rename = "angular_component_source_js")]
    AngularComponentSourceJs,
    #[serde(rename = "recovered_angular_component")]
    RecoveredAngularComponent,
    #[serde(rename = "partial_angular_component")]
    PartialAngularComponent,
}

#[derive(Serialize)]
pub struct JsonWarning {
    pub filename: String,
    pub kind: String,
    pub is_error: bool,
    pub message: String,
}

impl JsonWarning {
    pub fn new(
        filename: impl Into<String>,
        kind: impl Into<String>,
        is_error: bool,
        message: impl Into<String>,
    ) -> Self {
        Self {
            filename: filename.into(),
            kind: kind.into(),
            is_error,
            message: message.into(),
        }
    }
}
