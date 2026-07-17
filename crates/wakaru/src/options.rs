#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum RewriteLevel {
    Minimal,
    #[default]
    Standard,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum DceMode {
    #[default]
    Off,
    TransformOnly,
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

#[derive(Debug, Clone, Default)]
pub struct DecompileOptions {
    rewrite: RewriteOptions,
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

    pub fn output_source_map(&self) -> bool {
        self.output_source_map
    }

    pub fn with_rewrite(mut self, rewrite: RewriteOptions) -> Self {
        self.rewrite = rewrite;
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
#[non_exhaustive]
pub enum ScopeHoistMode {
    Disabled,
    #[default]
    Fallback,
    Recursive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModuleMode {
    Raw,
    Decompile(RewriteOptions),
}

impl Default for ModuleMode {
    fn default() -> Self {
        Self::Decompile(RewriteOptions::default())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnmatchedInput {
    Skip,
    #[default]
    Process,
    Preserve,
    Error,
}

#[derive(Debug, Clone, Default)]
pub struct UnpackOptions {
    modules: ModuleMode,
    scope_hoist: ScopeHoistMode,
    unmatched: UnmatchedInput,
    diagnostics: bool,
    output_source_maps: bool,
}

impl UnpackOptions {
    pub fn modules(&self) -> &ModuleMode {
        &self.modules
    }

    pub fn scope_hoist(&self) -> ScopeHoistMode {
        self.scope_hoist
    }

    pub fn unmatched(&self) -> UnmatchedInput {
        self.unmatched
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

    pub fn with_scope_hoist(mut self, mode: ScopeHoistMode) -> Self {
        self.scope_hoist = mode;
        self
    }

    pub fn with_unmatched(mut self, unmatched: UnmatchedInput) -> Self {
        self.unmatched = unmatched;
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
        assert_eq!(unpack.scope_hoist(), ScopeHoistMode::Fallback);
        assert_eq!(unpack.unmatched(), UnmatchedInput::Process);
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
            .with_scope_hoist(ScopeHoistMode::Recursive)
            .with_unmatched(UnmatchedInput::Skip)
            .with_diagnostics(true)
            .with_output_source_maps(true);

        assert_eq!(options.scope_hoist(), ScopeHoistMode::Recursive);
        assert_eq!(options.unmatched(), UnmatchedInput::Skip);
        assert!(options.diagnostics());
        assert!(options.output_source_maps());
    }
}
