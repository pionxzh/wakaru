/// Identifies one input of one operation call.
///
/// IDs are assigned by input order for each call, starting at zero, so they
/// are unambiguous even when multiple inputs share a filename. An `InputId`
/// is meaningful only within the returned operation result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InputId(u32);

impl InputId {
    pub(crate) fn from_index(index: usize) -> Self {
        Self(index as u32)
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

/// A byte range in one input source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSpan {
    pub input: InputId,
    /// Inclusive UTF-8 byte offset.
    pub start: u32,
    /// Exclusive UTF-8 byte offset.
    pub end: u32,
}

/// Whether a module is a bundle entry.
///
/// [`Unknown`](EntryStatus::Unknown) is distinct from
/// [`NonEntry`](EntryStatus::NonEntry): several bundle/chunk shapes can
/// identify an entry positively without proving that every other module is
/// not an entry. `NonEntry` is emitted only for formats whose runtime
/// metadata defines a complete entry set (webpack, Browserify, Closure
/// ModuleManager, and Metro); AMD, SystemJS, esbuild, and heuristic
/// scope-hoist entry flags remain `Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntryStatus {
    Entry,
    NonEntry,
    /// The detector did not establish whether this module is an entry.
    Unknown,
}

/// How this module artifact was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModuleStatus {
    /// The normal rule pipeline completed.
    Decompiled,
    /// Raw mode was selected; detector-specific normalization may still have
    /// run.
    Raw,
    /// The original unmatched input was returned unchanged.
    Preserved,
    /// Decompilation failed and Wakaru returned the best available raw
    /// module. Always accompanied by at least one operational
    /// [`Diagnostic`], so a successful operation can contain partial
    /// failures without hiding them in unstructured messages.
    DecompileFailed,
}

/// One output module artifact.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ModuleOutput {
    /// Logical output filename. Unpack output guarantees a unique,
    /// normalized, slash-separated relative filename; single-file decompile
    /// preserves the input filename.
    pub filename: String,
    pub code: String,
    pub source_map: Option<String>,
    pub provenance: Vec<SourceSpan>,
    /// Inspect-only source context for static analysis. When
    /// [`UnpackMode::Inspect`](crate::UnpackMode::Inspect) splits one large
    /// scope-hoist write component into finer modules, siblings carry the
    /// same coarse source spans here. This is evidence context, not package
    /// identity, and is empty for normal unpack output.
    pub inspection_context: Vec<SourceSpan>,
    pub entry: EntryStatus,
    pub status: ModuleStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArtifactKind {
    /// Legacy per-component artifact kind retained for API compatibility.
    AngularComponent,
    AngularModule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArtifactStatus {
    Complete,
    Partial,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ArtifactOutput {
    /// Unique, normalized, slash-separated relative output filename.
    pub filename: String,
    pub code: String,
    pub kind: ArtifactKind,
    pub status: ArtifactStatus,
    /// Indices into the root operation's module output.
    pub module_indices: Vec<usize>,
}

/// Result of [`decompile`](crate::decompile).
///
/// One module artifact type is shared with unpack rather than defining a
/// single-file-only module type: for single-file decompile, `entry` is always
/// [`EntryStatus::Unknown`] and `provenance` is empty.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DecompileOutput {
    pub module: ModuleOutput,
    pub artifacts: Vec<ArtifactOutput>,
    pub diagnostics: Vec<Diagnostic>,
}

/// A structurally detected bundler format.
///
/// This enum contains structural detector results only. Heuristic scope-hoist
/// recovery is represented by [`InputDetection::HeuristicScopeHoisted`]
/// rather than pretending it identified a bundler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BundleFormat {
    Webpack5,
    Webpack4,
    Browserify,
    ClosureModuleManager,
    Metro,
    SystemJs,
    Esbuild,
    Amd,
}

impl BundleFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webpack5 => "webpack5",
            Self::Webpack4 => "webpack4",
            Self::Browserify => "browserify",
            Self::ClosureModuleManager => "closure-module-manager",
            Self::Metro => "metro",
            Self::SystemJs => "systemjs",
            Self::Esbuild => "esbuild",
            Self::Amd => "amd",
        }
    }
}

/// What detection concluded about one input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputDetection {
    Structural(BundleFormat),
    /// Scope-hoisted modules were recovered heuristically. This is not a
    /// detected bundler format.
    HeuristicScopeHoisted,
    Plain,
}

/// What the operation did with one input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputAction {
    Unpacked,
    Processed,
    Preserved,
    Skipped,
}

/// Immediate result of [`UnpackJob::push`](crate::UnpackJob::push), letting a
/// walker report detection progress without waiting for `finish`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct InputReceipt {
    pub id: InputId,
    pub detection: InputDetection,
}

/// Per-input summary in [`UnpackOutput::inputs`], in input order.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct InputReport {
    pub id: InputId,
    pub filename: String,
    pub detection: InputDetection,
    pub action: InputAction,
    /// Indices into [`UnpackOutput::modules`]. A module with provenance from
    /// multiple inputs appears in every applicable report. A synthesized
    /// module with no narrower input identity appears in every processed
    /// input report without fabricating source spans.
    pub module_indices: Vec<usize>,
}

/// Whether the output module graph is safe to execute.
///
/// [`InspectionOnly`](OutputSafety::InspectionOnly) is returned whenever
/// [`UnpackMode::Inspect`](crate::UnpackMode::Inspect) was requested. It
/// marks the operation's output contract even if a particular input does not
/// happen to contain a retained cycle; callers must not infer execution
/// safety by inspecting filenames or diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputSafety {
    /// Normal unpacking policy was used.
    Normal,
    /// Fine-grained inspection boundaries were retained. The emitted module
    /// graph may not preserve the input bundle's initialization order.
    InspectionOnly,
}

/// Result of [`unpack`](crate::unpack) /
/// [`UnpackJob::finish`](crate::UnpackJob::finish).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct UnpackOutput {
    pub modules: Vec<ModuleOutput>,
    pub artifacts: Vec<ArtifactOutput>,
    /// One report per input, in input order.
    pub inputs: Vec<InputReport>,
    pub diagnostics: Vec<Diagnostic>,
    pub safety: OutputSafety,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DiagnosticCode {
    InputParseRecovered,
    RawNormalizationFailed,
    FactCollectionFailed,
    WebpackFactoryRecoveryFailed,
    DecompileFailed,
    TdzViolation,
    DuplicateDeclaration,
    ImportCycle,
    OutputParseRecovered,
    OutputParseFailed,
    ArtifactRecoveryReport,
    ArtifactRecoveryFailed,
}

impl DiagnosticCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InputParseRecovered => "input_parse_recovered",
            Self::RawNormalizationFailed => "raw_normalization_failed",
            Self::FactCollectionFailed => "fact_collection_failed",
            Self::WebpackFactoryRecoveryFailed => "webpack_factory_recovery_failed",
            Self::DecompileFailed => "decompile_failed",
            Self::TdzViolation => "tdz_violation",
            Self::DuplicateDeclaration => "duplicate_declaration",
            Self::ImportCycle => "import_cycle",
            Self::OutputParseRecovered => "output_parse_recovered",
            Self::OutputParseFailed => "output_parse_failed",
            Self::ArtifactRecoveryReport => "artifact_recovery_report",
            Self::ArtifactRecoveryFailed => "artifact_recovery_failed",
        }
    }
}

/// A recoverable problem associated with an operation, input, or module.
///
/// Diagnostics carry everything that does not abort the operation; fatal
/// failures are [`Error`](crate::Error).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub code: DiagnosticCode,
    pub message: String,
    pub input: Option<InputId>,
    /// Index into the operation's module output. For
    /// [`DecompileOutput`], the only module has index zero.
    pub module: Option<usize>,
    pub span: Option<SourceSpan>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_ids_are_call_local_ordered_values() {
        assert_eq!(InputId::from_index(0).get(), 0);
        assert_eq!(InputId::from_index(7).get(), 7);
    }
}
