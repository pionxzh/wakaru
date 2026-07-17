use crate::error::{from_core_driver_error, Error, ErrorKind, Result};
use crate::{RewriteOptions, Source};

#[derive(Debug, Clone, Default)]
pub struct NormalizeOptions {
    rename_bindings: bool,
}

impl NormalizeOptions {
    pub fn rename_bindings(&self) -> bool {
        self.rename_bindings
    }

    pub fn with_rename_bindings(mut self, enabled: bool) -> Self {
        self.rename_bindings = enabled;
        self
    }
}

#[derive(Debug, Clone)]
pub struct TraceOptions {
    start_from: Option<String>,
    stop_after: Option<String>,
    only_changed: bool,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self {
            start_from: None,
            stop_after: None,
            only_changed: true,
        }
    }
}

impl TraceOptions {
    pub fn start_from(&self) -> Option<&str> {
        self.start_from.as_deref()
    }

    pub fn stop_after(&self) -> Option<&str> {
        self.stop_after.as_deref()
    }

    pub fn only_changed(&self) -> bool {
        self.only_changed
    }

    pub fn with_start_from(mut self, rule: impl Into<String>) -> Self {
        self.start_from = Some(rule.into());
        self
    }

    pub fn with_stop_after(mut self, rule: impl Into<String>) -> Self {
        self.stop_after = Some(rule.into());
        self
    }

    pub fn with_only_changed(mut self, enabled: bool) -> Self {
        self.only_changed = enabled;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TraceEvent {
    pub rule: &'static str,
    pub changed: bool,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuleStage {
    Syntax,
    Helpers,
    Structural,
    Complex,
    Modernization,
    Cleanup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RuleInfo {
    pub id: &'static str,
    pub stage: RuleStage,
    pub requires: &'static [&'static str],
}

pub fn normalize(input: Source, options: NormalizeOptions) -> Result<String> {
    let input = input.into_parts();
    let core_options = wakaru_core::NormalizeOptions {
        rename_bindings: options.rename_bindings,
        filename: input.filename.clone(),
    };
    wakaru_core::normalize(&input.code, &core_options)
        .map_err(|error| Error::new(ErrorKind::Parse, Some(input.filename), error))
}

pub fn trace_rules(
    input: Source,
    rewrite: RewriteOptions,
    options: TraceOptions,
) -> Result<Vec<TraceEvent>> {
    let input = input.into_parts();
    if input.source_map.is_some() {
        return Err(Error::new(
            ErrorKind::InvalidOptions,
            Some(input.filename),
            anyhow::anyhow!("rule tracing does not accept an input source map"),
        ));
    }
    let core_options = wakaru_core::DecompileOptions {
        filename: input.filename.clone(),
        dce_mode: rewrite.dce().into_core(),
        level: rewrite.level().into_core(),
        ..Default::default()
    };
    let core_trace = wakaru_core::RuleTraceOptions {
        start_from: options.start_from,
        stop_after: options.stop_after,
        only_changed: options.only_changed,
    };
    wakaru_core::trace_rules(&input.code, core_options, core_trace)
        .map(|events| {
            events
                .into_iter()
                .map(|event| TraceEvent {
                    rule: event.rule,
                    changed: event.changed,
                    before: event.before,
                    after: event.after,
                })
                .collect()
        })
        .map_err(|error| {
            let kind = from_core_driver_error(error.kind());
            Error::new(kind, Some(input.filename), error.into_inner())
        })
}

pub fn rules() -> &'static [RuleInfo] {
    static RULES: std::sync::OnceLock<Box<[RuleInfo]>> = std::sync::OnceLock::new();
    RULES.get_or_init(|| {
        wakaru_core::rule_descriptors()
            .iter()
            .map(|descriptor| RuleInfo {
                id: descriptor.id,
                stage: map_rule_stage(descriptor.stage),
                requires: descriptor.requires,
            })
            .collect()
    })
}

fn map_rule_stage(stage: wakaru_core::RuleStage) -> RuleStage {
    match stage {
        wakaru_core::RuleStage::Syntax => RuleStage::Syntax,
        wakaru_core::RuleStage::Helpers => RuleStage::Helpers,
        wakaru_core::RuleStage::Structural => RuleStage::Structural,
        wakaru_core::RuleStage::Complex => RuleStage::Complex,
        wakaru_core::RuleStage::Modernization => RuleStage::Modernization,
        wakaru_core::RuleStage::Cleanup => RuleStage::Cleanup,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_rule_metadata_has_no_execution_surface() {
        let rules = rules();
        assert!(!rules.is_empty());
        assert_eq!(rules.len(), wakaru_core::rule_names().len());
        assert_eq!(rules[0].id, wakaru_core::rule_names()[0]);
    }

    #[test]
    fn normalize_uses_the_source_filename_for_typescript() {
        let output = normalize(
            Source::new("input.ts", "const value: number = 1;"),
            NormalizeOptions::default(),
        )
        .expect("TypeScript source should normalize");
        assert!(output.contains("value"));
    }

    #[test]
    fn trace_rejects_unknown_rule_with_typed_options_error() {
        let error = trace_rules(
            Source::new("input.js", "const value = 1;"),
            RewriteOptions::default(),
            TraceOptions::default().with_stop_after("NoSuchRule"),
        )
        .expect_err("unknown rule should fail");

        assert_eq!(error.kind(), ErrorKind::InvalidOptions);
    }

    #[test]
    fn trace_rejects_bundle_with_typed_input_error() {
        let error = trace_rules(
            Source::new(
                "bundle.js",
                "(self.webpackChunkapp = self.webpackChunkapp || []).push([[1], { 1: module => { module.exports = 1; } }]);",
            ),
            RewriteOptions::default(),
            TraceOptions::default(),
        )
        .expect_err("bundle tracing should fail");

        assert_eq!(error.kind(), ErrorKind::InvalidInput);
    }
}
