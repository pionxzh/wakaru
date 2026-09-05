use std::fmt;

use crate::facts::ModuleFactsMap;
use crate::rules::RewriteLevel;
use crate::unpacker::BundleFormat;

/// Controls how dead-code elimination behaves in the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DceMode {
    /// No dead-code cleanup.
    Off,
    /// Remove only transform-induced dead code: items that became dead because
    /// of pipeline rewrites. Pre-existing dead code in the input is preserved.
    TransformOnly,
    /// Remove all dead code (full reachability sweep).
    Full,
}

impl DceMode {
    pub fn is_enabled(self) -> bool {
        self != DceMode::Off
    }
}

#[derive(Debug, Clone)]
pub struct DecompileOptions {
    pub filename: String,
    /// Raw bytes of a v3 source map for single-file decompilation. Input maps
    /// are rejected in unpack mode because extracted modules no longer use the
    /// bundle's generated coordinates. When provided, enables:
    /// - Import deduplication (merges repeated imports of the same specifier)
    /// - Source-map-driven identifier rename (recovers original variable names)
    pub sourcemap: Option<Vec<u8>>,
    /// Controls late dead-code-elimination cleanup (`DeadImports`, `DeadDecls`).
    pub dce_mode: DceMode,
    /// Controls how aggressively wakaru recovers likely original source patterns.
    pub level: RewriteLevel,
    /// When true, attempt heuristic splitting of top-level scope-hoisted
    /// bundles (Rollup/Vite/flat esbuild) when no structural bundle is
    /// detected. At `aggressive` level, also retry scope-hoist splitting inside
    /// modules extracted by a structural bundle detector.
    pub heuristic_split: bool,
    /// Run post-transform diagnostic checks (lexical use-before-declaration,
    /// output parse verification). Results are returned as warnings.
    pub diagnostics: bool,
    /// Generate a v3 source map mapping decompiled output back to the input.
    pub emit_source_map: bool,
}

impl Default for DecompileOptions {
    fn default() -> Self {
        Self {
            filename: String::new(),
            sourcemap: None,
            dce_mode: DceMode::Off,
            level: RewriteLevel::Standard,
            heuristic_split: false,
            diagnostics: false,
            emit_source_map: false,
        }
    }
}

/// One physical JavaScript input to a multi-file unpack operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpackInput {
    pub filename: String,
    pub source: String,
}

/// Structured result returned by the prepared-input unpack path used by the
/// lockstep `wakaru` facade.
///
/// Unlike the legacy test adapter below, every module owns its source map and
/// provenance, so consumers never need to join parallel vectors by filename.
#[derive(Debug, Clone, Default)]
pub struct PreparedUnpackOutput {
    pub modules: Vec<PreparedModuleOutput>,
    pub warnings: Vec<UnpackWarning>,
    pub detected_formats: Vec<BundleFormat>,
}

/// One processed module from a prepared-input unpack operation.
#[derive(Debug, Clone, Default)]
pub struct PreparedModuleOutput {
    pub filename: String,
    pub code: String,
    pub source_map: Option<String>,
    pub provenance: PreparedModuleProvenance,
}

/// Typed identity of one physical input in a prepared unpack operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PreparedInputId(usize);

impl PreparedInputId {
    pub(crate) fn from_index(index: usize) -> Self {
        Self(index)
    }

    pub fn index(self) -> usize {
        self.0
    }
}

/// Provenance attached directly to one [`PreparedModuleOutput`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreparedModuleProvenance {
    /// Position of the physical input in the prepared-input vector. `None` is
    /// reserved for synthesized modules that cannot be attributed to one
    /// input.
    pub input: Option<PreparedInputId>,
    /// 0-based byte ranges `(start, end)` into the physical input.
    pub ranges: Vec<(u32, u32)>,
    /// Inspect-only coarse write-component ranges used as analysis context.
    /// Empty for normal output and when a nested mapping is unavailable.
    pub inspection_context_ranges: Vec<(u32, u32)>,
    /// Whether the detector identified this output as an entry module.
    pub is_entry: bool,
}

impl PreparedUnpackOutput {
    /// True when there are non-diagnostic warnings (parse failures, decompile
    /// errors). Diagnostic warnings like TDZ violations are excluded.
    pub fn has_errors(&self) -> bool {
        self.warnings.iter().any(|warning| warning.kind.is_error())
    }
}

/// Result of an unpack operation: the extracted modules plus any non-fatal
/// warnings (e.g. per-module parse failures that fell back to raw code).
///
/// This parallel-vector shape is retained only for the legacy core test
/// adapter. Production callers use [`PreparedUnpackOutput`] through the
/// published `wakaru` facade.
#[derive(Debug, Clone, Default)]
pub struct UnpackOutput {
    pub modules: Vec<(String, String)>,
    /// Per-module provenance: where in the original input each module came
    /// from. Parallel to `modules` by filename; modules without recoverable
    /// spans have empty `ranges`.
    pub provenance: Vec<ModuleProvenance>,
    pub warnings: Vec<UnpackWarning>,
    pub detected_formats: Vec<BundleFormat>,
    /// Per-module source maps (filename, source map JSON). Only populated when
    /// `DecompileOptions::emit_source_map` is set.
    pub source_maps: Vec<(String, String)>,
}

/// Lockstep façade result for an explicitly requested pre-rewrite source view.
///
/// This keeps the ordinary [`PreparedUnpackOutput`] contract unchanged. The
/// sidecar is generic module source; framework meaning is assigned only by the
/// caller.
#[doc(hidden)]
#[derive(Debug, Clone, Default)]
pub struct CapturedUnpackOutput {
    pub output: PreparedUnpackOutput,
    pub pre_rewrite_modules: Vec<(String, String)>,
    /// Post-Stage-2 transport facts for the surviving captured modules.
    ///
    /// Keys and relative sources use the same final filenames as `output` and
    /// `pre_rewrite_modules`. Framework artifact analyzers may use these facts
    /// to project their own semantic evidence across proven module edges.
    pub module_facts: ModuleFactsMap,
}

/// Byte-range provenance for one unpacked module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleProvenance {
    /// Output module filename (same value as in `UnpackOutput::modules`).
    pub filename: String,
    /// Input filename the ranges refer to. Empty for single-source unpacks
    /// (the caller knows the input) and for synthesized modules.
    pub input: String,
    /// 0-based byte ranges `(start, end)` into the input source.
    pub ranges: Vec<(u32, u32)>,
    /// Inspect-only coarse write-component ranges. Empty in normal output.
    pub inspection_context_ranges: Vec<(u32, u32)>,
    /// Whether the detector identified this output as an entry module.
    pub is_entry: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpackWarning {
    pub filename: String,
    pub kind: UnpackWarningKind,
    pub message: String,
}

impl UnpackWarning {
    pub(super) fn new(
        filename: impl Into<String>,
        kind: UnpackWarningKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            filename: filename.into(),
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for UnpackWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.filename, self.message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnpackWarningKind {
    RawNormalizationFailed,
    FactCollectionParseFailed,
    WebpackFactoryRecoveryFailed,
    DecompileFailed,
    InputParseRecovered,
    TdzViolation,
    DuplicateDeclaration,
    ImportCycle,
    OutputParseRecovered,
    OutputParseFailed,
}

impl UnpackWarningKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RawNormalizationFailed => "raw_normalization_failed",
            Self::FactCollectionParseFailed => "fact_collection_parse_failed",
            Self::WebpackFactoryRecoveryFailed => "webpack_factory_recovery_failed",
            Self::DecompileFailed => "decompile_failed",
            Self::InputParseRecovered => "input_parse_recovered",
            Self::TdzViolation => "tdz_violation",
            Self::DuplicateDeclaration => "duplicate_declaration",
            Self::ImportCycle => "import_cycle",
            Self::OutputParseRecovered => "output_parse_recovered",
            Self::OutputParseFailed => "output_parse_failed",
        }
    }

    /// Diagnostic warnings signal potential issues in transform output
    /// but do not indicate data loss or parse failure during unpack.
    pub fn is_diagnostic(self) -> bool {
        matches!(
            self,
            Self::InputParseRecovered | Self::TdzViolation | Self::ImportCycle
        )
    }

    pub fn is_error(self) -> bool {
        !self.is_diagnostic()
    }
}

impl UnpackOutput {
    /// True when there are non-diagnostic warnings (parse failures, decompile
    /// errors). Diagnostic warnings like TDZ violations are excluded.
    pub fn has_errors(&self) -> bool {
        self.warnings.iter().any(|w| w.kind.is_error())
    }
}

/// Result of a single-file decompile: the output code plus any non-fatal
/// warnings (e.g. TDZ violations detected after transformation).
#[derive(Debug, Clone, Default)]
pub struct DecompileOutput {
    pub code: String,
    pub warnings: Vec<UnpackWarning>,
    /// v3 source map JSON mapping the decompiled output back to the input.
    /// Only populated when `DecompileOptions::emit_source_map` is set.
    pub source_map: Option<String>,
}

impl DecompileOutput {
    pub fn has_errors(&self) -> bool {
        self.warnings.iter().any(|w| w.kind.is_error())
    }
}
