use std::fmt;

use crate::rules::RewriteLevel;

#[derive(Debug, Clone)]
pub struct DecompileOptions {
    pub filename: String,
    /// Raw bytes of a v3 source map. When provided, enables:
    /// - Import deduplication (merges repeated imports of the same specifier)
    /// - Source-map-driven identifier rename (recovers original variable names)
    pub sourcemap: Option<Vec<u8>>,
    /// Run late dead-code-elimination cleanup (`DeadImports`, `DeadDecls`).
    /// Disable this in tests that want to snapshot structural restoration
    /// separately from cleanup.
    pub dead_code_elimination: bool,
    /// Controls how aggressively wakaru recovers likely original source patterns.
    pub level: RewriteLevel,
    /// When true and no bundle format is detected, attempt heuristic splitting
    /// of scope-hoisted bundles (Rollup/Vite/flat esbuild).
    pub heuristic_split: bool,
    /// Run post-transform diagnostic checks (lexical use-before-declaration,
    /// output parse verification). Results are returned as warnings.
    pub diagnostics: bool,
}

impl Default for DecompileOptions {
    fn default() -> Self {
        Self {
            filename: String::new(),
            sourcemap: None,
            dead_code_elimination: false,
            level: RewriteLevel::Standard,
            heuristic_split: false,
            diagnostics: false,
        }
    }
}

/// One physical JavaScript input to a multi-file unpack operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpackInput {
    pub filename: String,
    pub source: String,
}

/// Result of an unpack operation: the extracted modules plus any non-fatal
/// warnings (e.g. per-module parse failures that fell back to raw code).
#[derive(Debug, Clone, Default)]
pub struct UnpackOutput {
    pub modules: Vec<(String, String)>,
    pub warnings: Vec<UnpackWarning>,
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
    DecompileFailed,
    InputParseRecovered,
    TdzViolation,
    DuplicateDeclaration,
    OutputParseRecovered,
    OutputParseFailed,
}

impl UnpackWarningKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RawNormalizationFailed => "raw_normalization_failed",
            Self::FactCollectionParseFailed => "fact_collection_parse_failed",
            Self::DecompileFailed => "decompile_failed",
            Self::InputParseRecovered => "input_parse_recovered",
            Self::TdzViolation => "tdz_violation",
            Self::DuplicateDeclaration => "duplicate_declaration",
            Self::OutputParseRecovered => "output_parse_recovered",
            Self::OutputParseFailed => "output_parse_failed",
        }
    }

    /// Diagnostic warnings signal potential issues in transform output
    /// but do not indicate data loss or parse failure during unpack.
    pub fn is_diagnostic(self) -> bool {
        matches!(self, Self::InputParseRecovered | Self::TdzViolation)
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
}

impl DecompileOutput {
    pub fn has_errors(&self) -> bool {
        self.warnings.iter().any(|w| w.kind.is_error())
    }
}
