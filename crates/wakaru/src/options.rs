/// Rewrite aggressiveness. Rules gate risky subpatterns internally rather
/// than moving entire rules in or out of the pipeline; see
/// `docs/rewrite-assumptions.md` for the named assumptions each level may
/// rely on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum RewriteLevel {
    /// High-confidence, semantics-preserving rewrites only.
    Minimal,
    /// Readability-oriented default.
    #[default]
    Standard,
    /// Speculative recovery.
    Aggressive,
}

impl RewriteLevel {
    pub(crate) fn into_core(self) -> wakaru_core::RewriteLevel {
        match self {
            RewriteLevel::Minimal => wakaru_core::RewriteLevel::Minimal,
            RewriteLevel::Standard => wakaru_core::RewriteLevel::Standard,
            RewriteLevel::Aggressive => wakaru_core::RewriteLevel::Aggressive,
        }
    }
}

/// Dead-code cleanup policy for the late pipeline phase.
///
/// The library default is [`Off`](DceMode::Off) so callers can observe
/// structural restoration separately from cleanup. The Wakaru CLI defaults to
/// [`TransformOnly`](DceMode::TransformOnly) without that being the library
/// default.
///
/// When heuristic scope-hoist detection itself produces multiple top-level
/// modules, decompilation disables DCE for that output set: the heuristic
/// split does not establish a complete enough reachability graph to apply the
/// requested mode safely. Structural bundle output, including recursively
/// split structural modules, honors the selected mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum DceMode {
    /// No dead-code cleanup.
    #[default]
    Off,
    /// Remove only leftovers the rewrite pipeline itself introduced,
    /// preserving code that was already dead in the input. Original ESM
    /// import specifiers are retained (an unused import still performs an
    /// observable link-time export check); only synthesized dead specifiers
    /// are removed.
    TransformOnly,
    /// Full reachability sweep.
    Full,
}

impl DceMode {
    pub(crate) fn into_core(self) -> wakaru_core::DceMode {
        match self {
            DceMode::Off => wakaru_core::DceMode::Off,
            DceMode::TransformOnly => wakaru_core::DceMode::TransformOnly,
            DceMode::Full => wakaru_core::DceMode::Full,
        }
    }
}

/// Rewrite pipeline options: [`RewriteLevel`] and [`DceMode`].
///
/// Fields are private so options can be added without breaking callers; the
/// `with_*` methods provide builder-style mutation. `Default` selects
/// [`RewriteLevel::Standard`] and [`DceMode::Off`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RewriteOptions {
    level: RewriteLevel,
    dce: DceMode,
}

impl RewriteOptions {
    pub fn level(&self) -> RewriteLevel {
        self.level
    }

    pub fn dce(&self) -> DceMode {
        self.dce
    }

    pub fn with_level(mut self, level: RewriteLevel) -> Self {
        self.level = level;
        self
    }

    pub fn with_dce(mut self, dce: DceMode) -> Self {
        self.dce = dce;
        self
    }
}

/// Options for [`decompile`](crate::decompile).
///
/// `Default` disables optional diagnostics and source-map output.
///
/// The diagnostics setting enables additional validation such as TDZ checks,
/// duplicate-declaration checks, import-cycle reporting, and output parse
/// verification. It does not suppress operational diagnostics describing a
/// parse recovery, per-module failure, or raw fallback — those are always
/// returned.
#[derive(Debug, Clone, Default)]
pub struct DecompileOptions {
    rewrite: RewriteOptions,
    recovery: RecoveryOptions,
    diagnostics: bool,
    output_source_map: bool,
}

impl DecompileOptions {
    pub fn rewrite(&self) -> RewriteOptions {
        self.rewrite
    }

    pub fn diagnostics(&self) -> bool {
        self.diagnostics
    }

    pub fn recovery(&self) -> RecoveryOptions {
        self.recovery
    }

    pub fn output_source_map(&self) -> bool {
        self.output_source_map
    }

    pub fn with_rewrite(mut self, rewrite: RewriteOptions) -> Self {
        self.rewrite = rewrite;
        self
    }

    pub fn with_recovery(mut self, recovery: RecoveryOptions) -> Self {
        self.recovery = recovery;
        self
    }

    pub fn with_diagnostics(mut self, enabled: bool) -> Self {
        self.diagnostics = enabled;
        self
    }

    pub fn with_output_source_map(mut self, enabled: bool) -> Self {
        self.output_source_map = enabled;
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoveryOptions {
    angular_components: bool,
}

impl RecoveryOptions {
    pub fn angular_components(&self) -> bool {
        self.angular_components
    }

    pub fn with_angular_components(mut self, enabled: bool) -> Self {
        self.angular_components = enabled;
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnpackMode {
    /// Detect structural bundles and fall back to safe heuristic scope-hoist
    /// splitting. Aggressive rewrites also enable safe recursive splitting.
    #[default]
    Auto,
    /// Run structural bundle detectors without heuristic scope-hoist
    /// splitting.
    Strict,
    /// Recursively retain fine-grained scope-hoist clusters for static
    /// inspection. The resulting module graph may not be safe to execute:
    /// finer synthetic clusters are kept even when their emitted imports form
    /// a cycle that changes eager ESM initialization order. This is an
    /// inspection policy, not an execution-safe reconstruction policy;
    /// [`ModuleMode`] remains independent.
    Inspect,
}

/// How each selected module is processed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModuleMode {
    /// Perform detector-specific extraction and normalization, but do not run
    /// the normal rewrite pipeline.
    ///
    /// Optional post-transform diagnostics are not run in raw mode; the
    /// diagnostics setting is ignored, while operational extraction and
    /// normalization diagnostics are still returned. Combining raw mode with
    /// requested output source maps is
    /// [`ErrorKind::InvalidOptions`](crate::ErrorKind::InvalidOptions) — raw
    /// mode never silently ignores a requested output source map.
    Raw,
    /// Run the normal rewrite pipeline with these options.
    Decompile(RewriteOptions),
}

impl Default for ModuleMode {
    fn default() -> Self {
        Self::Decompile(RewriteOptions::default())
    }
}

/// Policy for inputs that are not recognized as a bundle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnmatchedInput {
    /// Do not produce a module for a plain input.
    Skip,
    /// Apply the selected [`ModuleMode`] to a plain input.
    #[default]
    Process,
    /// Return the original plain input without rewriting it.
    Preserve,
    /// Fail the operation when an input is not recognized as a bundle.
    ///
    /// This is an operation-level policy evaluated by
    /// [`UnpackJob::finish`](crate::UnpackJob::finish), not a `push` error: a
    /// plain input is still assigned an ID and reported by `push`, the job
    /// records the policy violation and remains usable, and `finish` returns
    /// [`ErrorKind::InvalidInput`](crate::ErrorKind::InvalidInput) if any
    /// pushed input was plain. This keeps the `Vec` and job forms equivalent
    /// even when a caller continues pushing after a plain input.
    Error,
}

/// Options for [`unpack`](crate::unpack) and [`UnpackJob`](crate::UnpackJob).
///
/// `Default` decompiles modules using default rewrite options, uses
/// [`UnpackMode::Auto`], processes unmatched inputs, and disables optional
/// diagnostics and source-map output. The diagnostics setting has the same
/// semantics as on [`DecompileOptions`].
#[derive(Debug, Clone, Default)]
pub struct UnpackOptions {
    modules: ModuleMode,
    mode: UnpackMode,
    unmatched: UnmatchedInput,
    recovery: RecoveryOptions,
    diagnostics: bool,
    output_source_maps: bool,
}

impl UnpackOptions {
    pub fn modules(&self) -> &ModuleMode {
        &self.modules
    }

    pub fn mode(&self) -> UnpackMode {
        self.mode
    }

    pub fn unmatched(&self) -> UnmatchedInput {
        self.unmatched
    }

    pub fn recovery(&self) -> RecoveryOptions {
        self.recovery
    }

    pub fn diagnostics(&self) -> bool {
        self.diagnostics
    }

    pub fn output_source_maps(&self) -> bool {
        self.output_source_maps
    }

    pub fn with_modules(mut self, modules: ModuleMode) -> Self {
        self.modules = modules;
        self
    }

    pub fn with_mode(mut self, mode: UnpackMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_unmatched(mut self, unmatched: UnmatchedInput) -> Self {
        self.unmatched = unmatched;
        self
    }

    pub fn with_recovery(mut self, recovery: RecoveryOptions) -> Self {
        self.recovery = recovery;
        self
    }

    pub fn with_diagnostics(mut self, enabled: bool) -> Self {
        self.diagnostics = enabled;
        self
    }

    pub fn with_output_source_maps(mut self, enabled: bool) -> Self {
        self.output_source_maps = enabled;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_public_contract() {
        let rewrite = RewriteOptions::default();
        assert_eq!(rewrite.level(), RewriteLevel::Standard);
        assert_eq!(rewrite.dce(), DceMode::Off);

        let unpack = UnpackOptions::default();
        assert!(matches!(unpack.modules(), ModuleMode::Decompile(_)));
        assert_eq!(unpack.mode(), UnpackMode::Auto);
        assert_eq!(unpack.unmatched(), UnmatchedInput::Process);
        assert!(!unpack.recovery().angular_components());
        assert!(!unpack.diagnostics());
        assert!(!unpack.output_source_maps());
    }

    #[test]
    fn builders_update_private_options() {
        let rewrite = RewriteOptions::default()
            .with_level(RewriteLevel::Aggressive)
            .with_dce(DceMode::TransformOnly);
        let options = UnpackOptions::default()
            .with_modules(ModuleMode::Decompile(rewrite))
            .with_mode(UnpackMode::Inspect)
            .with_unmatched(UnmatchedInput::Skip)
            .with_recovery(RecoveryOptions::default().with_angular_components(true))
            .with_diagnostics(true)
            .with_output_source_maps(true);

        assert_eq!(options.mode(), UnpackMode::Inspect);
        assert_eq!(options.unmatched(), UnmatchedInput::Skip);
        assert!(options.recovery().angular_components());
        assert!(options.diagnostics());
        assert!(options.output_source_maps());
    }
}
