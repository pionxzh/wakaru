pub mod bun;
pub mod debug;
mod decompile;
mod error;
mod options;
mod output;
mod source;
pub mod sourcemap;
mod unpack;
pub mod vue;

pub use decompile::decompile;
pub use error::{Error, ErrorKind, Result};
pub use options::{
    DceMode, DecompileOptions, ModuleMode, RewriteLevel, RewriteOptions, UnmatchedInput,
    UnpackMode, UnpackOptions,
};
pub use output::{
    BundleFormat, DecompileOutput, Diagnostic, DiagnosticCode, DiagnosticSeverity, EntryStatus,
    InputAction, InputDetection, InputId, InputReceipt, InputReport, ModuleOutput, ModuleStatus,
    OutputSafety, SourceSpan, UnpackOutput,
};
pub use source::{Source, SourceParts};
pub use unpack::{unpack, UnpackJob};
