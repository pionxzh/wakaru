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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSpan {
    pub input: InputId,
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntryStatus {
    Entry,
    NonEntry,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModuleStatus {
    Decompiled,
    Raw,
    Preserved,
    DecompileFailed,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ModuleOutput {
    pub filename: String,
    pub code: String,
    pub source_map: Option<String>,
    pub provenance: Vec<SourceSpan>,
    pub entry: EntryStatus,
    pub status: ModuleStatus,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DecompileOutput {
    pub module: ModuleOutput,
    pub diagnostics: Vec<Diagnostic>,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputDetection {
    Structural(BundleFormat),
    HeuristicScopeHoisted,
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputAction {
    Unpacked,
    Processed,
    Preserved,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct InputReceipt {
    pub id: InputId,
    pub detection: InputDetection,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct InputReport {
    pub id: InputId,
    pub filename: String,
    pub detection: InputDetection,
    pub action: InputAction,
    pub module_indices: Vec<usize>,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct UnpackOutput {
    pub modules: Vec<ModuleOutput>,
    pub inputs: Vec<InputReport>,
    pub diagnostics: Vec<Diagnostic>,
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
    DecompileFailed,
    TdzViolation,
    DuplicateDeclaration,
    ImportCycle,
    OutputParseRecovered,
    OutputParseFailed,
}

impl DiagnosticCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InputParseRecovered => "input_parse_recovered",
            Self::RawNormalizationFailed => "raw_normalization_failed",
            Self::FactCollectionFailed => "fact_collection_failed",
            Self::DecompileFailed => "decompile_failed",
            Self::TdzViolation => "tdz_violation",
            Self::DuplicateDeclaration => "duplicate_declaration",
            Self::ImportCycle => "import_cycle",
            Self::OutputParseRecovered => "output_parse_recovered",
            Self::OutputParseFailed => "output_parse_failed",
        }
    }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub code: DiagnosticCode,
    pub message: String,
    pub input: Option<InputId>,
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
