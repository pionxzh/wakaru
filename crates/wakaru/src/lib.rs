//! Wakaru: a JavaScript decompiler exposed as a compiler-like service.
//!
//! The stable crate root offers two end-to-end operations:
//!
//! - [`decompile`] — rewrite one minified/transpiled source into readable,
//!   modern JavaScript.
//! - [`unpack`] — split one or more bundle/chunk inputs into modules and
//!   decompile each; [`UnpackJob`] is the incremental intake form of the same
//!   operation for directory walkers.
//!
//! Optional workflows live in cohesive namespaces: [`bun`] (single-file
//! executable extraction), [`debug`] (normalization, rule tracing, rule
//! metadata), [`sourcemap`] (embedded-source extraction), and [`vue`]
//! (experimental standalone SFC recovery).
//!
//! The API is deliberately not an SWC transformation toolkit: it never exposes
//! SWC AST types, individual rewrite visitors, detector objects, prepared
//! ASTs, or cross-module fact structures. Partial recovery is represented
//! structurally ([`ModuleStatus`], [`Diagnostic`]) instead of being hidden in
//! warning strings.
//!
//! Design decisions, compatibility boundaries, and the internal processing
//! invariants behind this surface are recorded in `docs/public-api.md`.
//!
//! # Publishing model
//!
//! The façade is published as `wakaru`. Cargo cannot package a façade with an
//! unpublished path dependency, so `wakaru-core` is also published as a
//! lockstep, exact-version implementation dependency. It remains explicitly
//! unsupported as an integration surface and may change whenever the façade
//! version changes.
//!
//! # Examples
//!
//! Single-file decompile:
//!
//! ```no_run
//! use wakaru::{decompile, DecompileOptions, Source};
//!
//! # fn main() -> wakaru::Result<()> {
//! let minified = String::from("var a=!0;");
//! let output = decompile(Source::new("input.js", minified), DecompileOptions::default())?;
//!
//! println!("{}", output.module.code);
//! for diagnostic in output.diagnostics {
//!     eprintln!("{}", diagnostic.message);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Bundle/chunk set:
//!
//! ```no_run
//! use wakaru::{
//!     unpack, DceMode, ModuleMode, RewriteLevel, RewriteOptions, Source, UnmatchedInput,
//!     UnpackMode, UnpackOptions,
//! };
//!
//! # fn main() -> wakaru::Result<()> {
//! # let (entry, chunk) = (String::new(), String::new());
//! let output = unpack(
//!     vec![Source::new("entry.js", entry), Source::new("chunk.js", chunk)],
//!     UnpackOptions::default()
//!         .with_modules(ModuleMode::Decompile(
//!             RewriteOptions::default()
//!                 .with_level(RewriteLevel::Standard)
//!                 .with_dce(DceMode::TransformOnly),
//!         ))
//!         .with_mode(UnpackMode::Auto)
//!         .with_unmatched(UnmatchedInput::Process)
//!         .with_diagnostics(true),
//! )?;
//!
//! for module in output.modules {
//!     println!("{}: {} bytes", module.filename, module.code.len());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Directory-style detected-only processing with bounded intake memory:
//!
//! ```no_run
//! use wakaru::{InputAction, Source, UnmatchedInput, UnpackJob, UnpackOptions};
//!
//! # fn main() -> anyhow::Result<()> {
//! # let javascript_candidate_paths: Vec<std::path::PathBuf> = Vec::new();
//! let mut job = UnpackJob::new(UnpackOptions::default().with_unmatched(UnmatchedInput::Skip))?;
//!
//! for path in javascript_candidate_paths {
//!     let code = std::fs::read_to_string(&path)?;
//!     job.push(Source::new(path.to_string_lossy(), code))?;
//! }
//!
//! let output = job.finish()?;
//! let skipped = output
//!     .inputs
//!     .iter()
//!     .filter(|input| input.action == InputAction::Skipped)
//!     .count();
//! # let _ = skipped;
//! # Ok(())
//! # }
//! ```

pub mod bun;
mod artifacts;
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
    DceMode, DecompileOptions, ModuleMode, RecoveryOptions, RewriteLevel, RewriteOptions,
    UnmatchedInput, UnpackMode, UnpackOptions,
};
pub use output::{
    ArtifactKind, ArtifactOutput, ArtifactStatus, BundleFormat, DecompileOutput, Diagnostic,
    DiagnosticCode, DiagnosticSeverity, EntryStatus, InputAction, InputDetection, InputId,
    InputReceipt, InputReport, ModuleOutput, ModuleStatus, OutputSafety, SourceSpan, UnpackOutput,
};
pub use source::{Source, SourceParts};
pub use unpack::{unpack, UnpackJob};
