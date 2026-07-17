# Public API v2 proposal

Status: approved design proposal; implementation pending. This document
specifies a breaking Rust API. It does not describe an implemented
compatibility layer.

The public façade is published as `wakaru`. The existing `wakaru-core` crate
becomes an unpublished, explicitly internal engine crate so its implementation
surface can evolve without becoming a semver contract.

## Goals

- Expose Wakaru as a compiler-like service, not as an SWC transformation
  toolkit.
- Keep parsed and prepared modules internal until final output requires text.
- Detect and parse each input once in the normal path.
- Represent partial recovery explicitly instead of requiring callers to infer
  it from warning strings.
- Keep inputs, module artifacts, provenance, diagnostics, and detection reports
  structurally associated.
- Leave room to add formats and diagnostics without another breaking release.

## Supported surface

The stable crate root contains two end-to-end operations and their domain
types:

```rust
pub fn decompile(
    input: Source,
    options: DecompileOptions,
) -> Result<DecompileOutput>;

pub fn unpack(
    inputs: Vec<Source>,
    options: UnpackOptions,
) -> Result<UnpackOutput>;
```

For directory walkers and other producers that should not retain every
candidate source simultaneously, the same unpack operation also has an opaque
incremental intake form:

```rust
pub struct UnpackJob {
    // Private prepared inputs and reports.
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct InputReceipt {
    pub id: InputId,
    pub detection: InputDetection,
}

impl UnpackJob {
    pub fn new(options: UnpackOptions) -> Result<Self>;

    /// Detect and prepare one input immediately. A skipped plain input is
    /// released before this method returns.
    pub fn push(&mut self, input: Source) -> Result<InputReceipt>;

    /// Run the shared cross-module phases and materialize final output.
    pub fn finish(self) -> Result<UnpackOutput>;
}
```

The free `unpack(Vec<Source>, ...)` function is semantically equivalent to
pushing those sources into one `UnpackJob` in order and finishing it. Both
forms use the same detection and processing semantics; this contract does not
require the Vec form to use a literally serial push loop.

`UnpackJob::new` validates option combinations. A failed `push` does not add an
input or consume an `InputId`; the job remains usable. A successful push
returns the assigned ID and detection result, allowing a walker to report
detection progress without waiting for `finish`.

`UnmatchedInput::Error` is an operation-level policy evaluated by `finish`,
not a `push` error. A plain input is successfully assigned an ID and reported
by `push`; the job records the policy violation and remains usable. `finish`
then returns `ErrorKind::InvalidInput` if any pushed input was plain. This keeps
the Vec and job forms equivalent even when a caller continues pushing after a
plain input.

Finishing a job with no successfully pushed inputs returns
`ErrorKind::InvalidInput`. Finishing a non-empty job whose inputs were all
skipped returns `Ok(UnpackOutput)` with zero modules and one skipped
`InputReport` per input.

`decompile` always processes exactly one source module. `unpack` treats its
inputs as one logical bundle/chunk set. Under `ModuleMode::Decompile`, modules
selected for processing participate in the same cross-module fact graph.

The public API does not expose detector objects, prepared ASTs, SWC AST types,
individual rewrite visitors, or cross-module fact structures.

## Input

```rust
#[derive(Debug, Clone)]
pub struct Source {
    // Private fields.
}

impl Source {
    pub fn new(
        filename: impl Into<String>,
        code: impl Into<String>,
    ) -> Self;

    pub fn with_source_map(self, source_map: impl Into<Vec<u8>>) -> Self;

    pub fn filename(&self) -> &str;
    pub fn code(&self) -> &str;
    pub fn source_map(&self) -> Option<&[u8]>;

    pub fn into_parts(self) -> SourceParts;
}

#[derive(Debug, Clone)]
pub struct SourceParts {
    pub filename: String,
    pub code: String,
    pub source_map: Option<Vec<u8>>,
}
```

The operation consumes `Source`. Passing an owned `String` therefore permits
Wakaru to move it into SWC's source storage instead of requiring the
`source.to_string()` copy imposed by the current borrowed API. Callers that
need to retain the source can clone it explicitly.

Input source maps are valid for `decompile`. They are always invalid for
`unpack`, because extracted modules no longer use the bundle's generated
coordinates. Supplying one to `unpack` fails with
`ErrorKind::InvalidOptions` before the shared module phases run;
`UnpackJob::push` rejects that input. Output source-map generation remains
independently configurable.

## Rewrite and operation options

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RewriteLevel {
    Minimal,
    Standard,
    Aggressive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DceMode {
    Off,
    TransformOnly,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RewriteOptions {
    // Private fields.
}

impl RewriteOptions {
    pub fn level(&self) -> RewriteLevel;
    pub fn dce(&self) -> DceMode;
    pub fn with_level(self, level: RewriteLevel) -> Self;
    pub fn with_dce(self, dce: DceMode) -> Self;
}

#[derive(Debug, Clone)]
pub struct DecompileOptions {
    // Private fields.
}

impl DecompileOptions {
    pub fn rewrite(&self) -> RewriteOptions;
    pub fn diagnostics(&self) -> bool;
    pub fn output_source_map(&self) -> bool;
    pub fn with_rewrite(self, rewrite: RewriteOptions) -> Self;
    pub fn with_diagnostics(self, enabled: bool) -> Self;
    pub fn with_output_source_map(self, enabled: bool) -> Self;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScopeHoistMode {
    /// Run structural bundle detectors only.
    Disabled,
    /// Try heuristic scope-hoist splitting only when structural detection
    /// does not match.
    Fallback,
    /// Also try heuristic splitting inside structurally extracted modules.
    Recursive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModuleMode {
    /// Perform detector-specific extraction and normalization, but do not run
    /// the normal rewrite pipeline.
    Raw,
    /// Run the normal rewrite pipeline with these options.
    Decompile(RewriteOptions),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnmatchedInput {
    /// Do not produce a module for a plain input.
    Skip,
    /// Apply the selected `ModuleMode` to a plain input.
    Process,
    /// Return the original plain input without rewriting it.
    Preserve,
    /// Fail the operation when an input is not recognized as a bundle.
    Error,
}

#[derive(Debug, Clone)]
pub struct UnpackOptions {
    // Private fields.
}

impl UnpackOptions {
    pub fn modules(&self) -> &ModuleMode;
    pub fn scope_hoist(&self) -> ScopeHoistMode;
    pub fn unmatched(&self) -> UnmatchedInput;
    pub fn diagnostics(&self) -> bool;
    pub fn output_source_maps(&self) -> bool;
    pub fn with_modules(self, modules: ModuleMode) -> Self;
    pub fn with_scope_hoist(self, mode: ScopeHoistMode) -> Self;
    pub fn with_unmatched(self, unmatched: UnmatchedInput) -> Self;
    pub fn with_diagnostics(self, enabled: bool) -> Self;
    pub fn with_output_source_maps(self, enabled: bool) -> Self;
}
```

`RewriteOptions`, `DecompileOptions`, and `UnpackOptions` implement `Default`.
Their fields are private so new options can be added without breaking callers;
the `with_*` methods provide builder-style mutation.

Recommended defaults:

`RewriteOptions::default()` selects `RewriteLevel::Standard` and
`DceMode::Off`. `DecompileOptions::default()` disables optional diagnostics and
source-map output. `UnpackOptions::default()` decompiles modules using default
rewrite options, uses `ScopeHoistMode::Fallback`, processes unmatched inputs,
and disables optional diagnostics and source-map output.

CLI defaults can continue to select `DceMode::TransformOnly` without making
that behavior the library default.

The optional diagnostics setting enables additional validation such as TDZ
checks, duplicate declaration checks, import-cycle reporting, and output parse
verification. It does not suppress operational diagnostics describing a parse
recovery, per-module failure, or raw fallback.

Under `ModuleMode::Raw`, optional post-transform diagnostics are not run;
the diagnostics setting is ignored. Operational extraction and normalization
diagnostics are still returned. Combining `ModuleMode::Raw` with requested
output source maps is `ErrorKind::InvalidOptions`; raw mode never silently
ignores a requested output source map.

## Output artifacts

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InputId(u32);

impl InputId {
    pub fn get(self) -> u32;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSpan {
    pub input: InputId,
    /// Inclusive UTF-8 byte offset.
    pub start: u32,
    /// Exclusive UTF-8 byte offset.
    pub end: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntryStatus {
    Entry,
    NonEntry,
    /// The detector did not establish whether this module is an entry.
    Unknown,
}

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
    /// Decompilation failed and Wakaru returned the best available raw module.
    DecompileFailed,
}

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
    pub entry: EntryStatus,
    pub status: ModuleStatus,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DecompileOutput {
    pub module: ModuleOutput,
    pub diagnostics: Vec<Diagnostic>,
}
```

`EntryStatus::Unknown` is distinct from `NonEntry`: several bundle/chunk
shapes can identify an entry positively without proving that every other
module is not an entry.

`ModuleStatus::DecompileFailed` always has at least one associated operational
diagnostic. A successfully returned operation can therefore contain partial
failures without hiding them in unstructured messages.

For single-file `decompile`, `entry` is always `EntryStatus::Unknown` and
`provenance` is empty. Keeping one artifact type is preferred over a second
single-file-only module type.

Single-file failure behavior is explicit:

- invalid options, an invalid input source map, or an unrecoverable input parse
  returns `Err`;
- after the input parses successfully, a transformation, fixer, or output
  emission failure normally returns `Ok` with the original input as the
  best-effort artifact, `ModuleStatus::DecompileFailed`, no output source map,
  and an operational diagnostic;
- `ErrorKind::Emit` and `ErrorKind::Internal` are reserved for failures where
  Wakaru cannot return any coherent artifact or uphold its result invariants.

## Input reports and detection

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BundleFormat {
    Webpack5,
    Webpack4,
    Browserify,
    SystemJs,
    Esbuild,
    Amd,
}

impl BundleFormat {
    pub fn as_str(self) -> &'static str;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputDetection {
    Structural(BundleFormat),
    /// Scope-hoisted modules were recovered heuristically. This is not a
    /// detected bundler format.
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

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct InputReport {
    pub id: InputId,
    pub filename: String,
    pub detection: InputDetection,
    pub action: InputAction,
    /// Indices into `UnpackOutput::modules`. A module with provenance from
    /// multiple inputs appears in every applicable report.
    pub module_indices: Vec<usize>,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct UnpackOutput {
    pub modules: Vec<ModuleOutput>,
    /// One report per input, in input order.
    pub inputs: Vec<InputReport>,
    pub diagnostics: Vec<Diagnostic>,
}
```

`InputId` is assigned by input order for each call, starting at zero. It is
unambiguous even when multiple inputs have the same filename. It is meaningful
only within the returned operation result.

`BundleFormat` contains structural detector results only. Heuristic
scope-hoist recovery is represented by `InputDetection::HeuristicScopeHoisted`
rather than pretending it identified a bundler.

## Diagnostics and fatal errors

```rust
pub type Result<T> = std::result::Result<T, Error>;

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
    pub fn as_str(self) -> &'static str;
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub code: DiagnosticCode,
    pub message: String,
    pub input: Option<InputId>,
    /// Index into the operation's module output. For `DecompileOutput`, the
    /// only module has index zero.
    pub module: Option<usize>,
    pub span: Option<SourceSpan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    InvalidOptions,
    InvalidInput,
    Parse,
    SourceMap,
    Emit,
    Internal,
}

#[derive(Debug)]
pub struct Error {
    // Private fields and private implementation details.
}

impl Error {
    pub fn kind(&self) -> ErrorKind;
    pub fn input_filename(&self) -> Option<&str>;
}

impl std::fmt::Display for Error;
impl std::error::Error for Error;
```

`Error` represents a fatal operation failure: invalid options, an unusable
top-level input, or a failure that prevents Wakaru from returning a coherent
result. Recoverable per-module problems belong in `Diagnostic` and
`ModuleStatus`, not in `Error`.

SWC error types and `anyhow::Error` are never part of the public contract.

## Auxiliary namespaces

The stable root stays small. Optional workflows live in cohesive namespaces
and use Wakaru-owned types.

```rust
pub mod debug {
    pub fn normalize(input: Source, options: NormalizeOptions) -> Result<String>;

    pub fn trace_rules(
        input: Source,
        rewrite: RewriteOptions,
        options: TraceOptions,
    ) -> Result<Vec<TraceEvent>>;

    pub fn rules() -> &'static [RuleInfo];
}

pub mod sourcemap {
    pub fn embedded_sources(data: &[u8]) -> Result<Vec<EmbeddedSource>>;

    pub struct EmbeddedSource {
        /// Normalized, slash-separated relative path with bundler prefixes and
        /// escaping parent components removed.
        pub path: String,
        pub content: String,
    }
}

pub mod vue {
    pub trait ImportResolver: Send + Sync {
        fn resolve(&self, specifier: &str) -> Option<String>;
    }

    impl<F> ImportResolver for F
    where
        F: Fn(&str) -> Option<String> + Send + Sync,
    {
        // Delegates to the closure.
    }

    pub struct RecoveryOptions {
        // Private owned fields.
    }

    impl RecoveryOptions {
        pub fn with_preferred_component_name(
            self,
            name: impl Into<String>,
        ) -> Self;

        pub fn with_import_resolver(
            self,
            resolver: impl ImportResolver + 'static,
        ) -> Self;
    }

    pub fn recover(
        input: Source,
        options: RecoveryOptions,
    ) -> Result<Vec<RecoveredSfc>>;

    pub struct RecoveredSfc {
        pub name: Option<String>,
        pub source: String,
    }
}
```

`RecoveryOptions` implements `Default`. Its preferred name is owned, and its
resolver is stored internally in an `Arc`; the public type carries no lifetime.
`vue::recover` is plural by contract because one compiled module can contain
multiple recoverable components. An empty vector means no SFC was recovered.

The exact option and result fields for `debug` and `vue` should be specified in
their own focused review. Their boundary is fixed here: they do not expose SWC
types, individual visitors, or the internal Vue template IR. In particular,
there is no framework-specific `vue::decompile` composition operation;
callers with standalone text can use `vue::recover`, while end-to-end
component recovery belongs in the root operations described below.

Standalone `vue::recover` operates on its owned `Source` and parses it as an
independent operation. It is not covered by the root operation's prepared-AST
no-reparse invariant.

### Future framework recovery

Vue should not establish a per-framework end-to-end composition pattern. If
component recovery becomes a supported root workflow, or Wakaru adds Svelte,
Angular, or another framework, recovery is added as an option on `decompile`
and `unpack`. The integrated phase consumes Wakaru's module graph and prepared
state before final materialization instead of reparsing every emitted module
once per framework.

The common artifact must support multiple files from the start: a component
may produce one `.vue`/`.svelte` file or an implementation, template, and
styles as separate files. It should also associate itself with module indices,
following `InputReport`, and guarantee unique normalized artifact filenames.
The integrated path resolves sibling imports from Wakaru's own module graph;
caller-supplied import resolution remains useful only for standalone namespace
operations such as `vue::recover`.

No speculative `Framework` or `ComponentOutput` types are included in v2.
Private option fields and non-exhaustive result types allow that integrated
surface to be added without a breaking change. Future framework namespaces may
provide framework-specific standalone recovery and option types, but should
not add `svelte::decompile`, `angular::decompile`, or equivalent composition
entry points.

Filesystem path validation, output-directory writes, filename collision
handling, and source-map extraction to disk remain CLI responsibilities.

Tracing remains the observability mechanism for phase and per-rule timings,
but span names, fields, and nesting are instrumentation details rather than a
stable public API contract.

## Not public

The following current surfaces become private implementation details:

- `rules` and every individual rule visitor
- `facts` and cross-module fact maps
- `unpacker` and detector-specific entry points
- `UnpackedModule`, `UnpackResult`, and prepared AST types
- namespace decomposition and re-export consolidation passes
- TDZ visitors and SWC-facing source-map rename helpers
- SWC AST, `Mark`, `SyntaxContext`, `SourceMap`, and visitor types
- output path and filesystem-oriented helpers

Rule names and stable rule metadata remain available through `debug`; rule
execution and AST mutation do not.

Crate integration tests that currently require public visitors should become
crate-local tests. Production visibility should not be determined by the test
layout.

## Internal processing boundary

This is not public API, but it is a required architectural invariant for the
public operations above:

```rust
struct PreparedInput {
    id: InputId,
    detection: InputDetection,
    modules: Vec<PreparedModule>,
    allow_cycle_premerge: bool,
}

struct PreparedModule {
    metadata: ModuleMetadata,
    payload: ModulePayload,
}

enum ModulePayload {
    Ast(PreparedAst),
    Source(String),
}
```

A plain input is a `PreparedInput` containing one module. A structural or
heuristic bundle contains zero or more extracted modules. There is no separate
public or phase-level webpack route.

All selected modules cross the same fact-collection and phase-2 boundaries.
Detector-specific work ends before that boundary. `ModulePayload` only
controls how the common pipeline obtains its initial AST.

### Performance invariants

1. Each physical input is detected at most once per `unpack` call.
2. A compatible plain JavaScript input is parsed at most once before rules.
3. A prepared module is not emitted and reparsed before the rule pipeline.
4. `UnmatchedInput::Skip` does not require a separate detection preflight.
5. `UnpackJob::push` releases a skipped candidate's source before accepting the
   next candidate. Peak intake memory is therefore bounded by retained
   detected/processed inputs rather than every file visited by a directory
   walk.
6. Text is materialized only for final output, explicit raw output, or a
   recovery fallback.
7. Source-map modes may take an explicitly slower path when source-coordinate
   state cannot safely cross the parallel boundary.
8. All output ordering is deterministic regardless of Rayon scheduling.

These invariants prevent the public API from reintroducing the emit/parse
round trips removed by the prepared-AST work.

## Examples

Single-file decompile:

```rust
use wakaru::{decompile, DecompileOptions, Source};

let output = decompile(
    Source::new("input.js", minified_code),
    DecompileOptions::default(),
)?;

println!("{}", output.module.code);
for diagnostic in output.diagnostics {
    eprintln!("{}", diagnostic.message);
}
```

Bundle/chunk set:

```rust
use wakaru::{
    unpack, DceMode, ModuleMode, RewriteLevel, RewriteOptions, ScopeHoistMode,
    Source, UnmatchedInput, UnpackOptions,
};

let output = unpack(
    vec![
        Source::new("entry.js", entry),
        Source::new("chunk.js", chunk),
    ],
    UnpackOptions::default()
        .with_modules(ModuleMode::Decompile(
            RewriteOptions::default()
                .with_level(RewriteLevel::Standard)
                .with_dce(DceMode::TransformOnly),
        ))
        .with_scope_hoist(ScopeHoistMode::Fallback)
        .with_unmatched(UnmatchedInput::Process)
        .with_diagnostics(true),
)?;

for module in output.modules {
    write_module(module.filename, module.code)?;
}
```

Directory-style detected-only processing:

```rust
use wakaru::{InputAction, Source, UnmatchedInput, UnpackJob, UnpackOptions};

let mut job = UnpackJob::new(
    UnpackOptions::default().with_unmatched(UnmatchedInput::Skip),
)?;

for path in javascript_candidate_paths {
    let code = std::fs::read_to_string(&path)?;
    job.push(Source::new(path.to_string_lossy(), code))?;
}

let output = job.finish()?;

let skipped = output
    .inputs
    .iter()
    .filter(|input| input.action == InputAction::Skipped)
    .count();
```

The CLI performs the filesystem walk and pushes one candidate at a time. It
does not call a boolean detection API first, and skipped source strings are not
retained for the rest of the walk.

## Decisions in this revision

- Publish the stable façade as `wakaru`; keep `wakaru-core` unpublished and
  internal.
- Use private option fields with builder methods.
- Keep `InputReport::module_indices`, because synthesized modules can have no
  provenance.
- Keep the failure-oriented name `ModuleStatus::DecompileFailed`.
- Keep `Vec<Source>` on the convenience function for a simple bindable
  signature.
- Add `UnpackJob` for incremental intake. A `Vec`-only API cannot bound
  directory-walk memory: every candidate string is already resident before
  Wakaru gets a chance to drop skipped inputs. Both forms delegate to the same
  detection and processing implementation.
- Return `InputReceipt` from `UnpackJob::push` so detection progress is
  available during intake.
- Evaluate `UnmatchedInput::Error` at `finish`, preserving operation-level
  failure semantics while keeping the job usable after every successful
  push.
- Keep framework namespaces limited to standalone recovery; future
  end-to-end component recovery integrates with the root operations and uses a
  framework-neutral, multi-file artifact model.
