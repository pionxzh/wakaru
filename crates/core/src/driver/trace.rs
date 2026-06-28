use anyhow::{anyhow, Result};
use swc_core::common::{sync::Lrc, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::Module;
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;

use super::io::{parse_js, print_trace_module};
use super::types::DecompileOptions;
use super::unpack::detect_bundle;
use crate::rules::{apply_rules_with_observer, rule_names, RulePipelineOptions};

#[derive(Debug, Clone)]
pub struct RuleTraceOptions {
    /// First rule to run and trace. When omitted, tracing starts at the
    /// beginning of the normal single-file rule pipeline.
    pub start_from: Option<String>,
    /// Last rule to run and trace. When omitted, tracing stops at the end of
    /// the normal single-file rule pipeline.
    pub stop_after: Option<String>,
    /// When true, only include rules whose rendered output changed.
    pub only_changed: bool,
}

impl Default for RuleTraceOptions {
    fn default() -> Self {
        Self {
            start_from: None,
            stop_after: None,
            only_changed: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleTraceEvent {
    pub rule: &'static str,
    pub changed: bool,
    pub before: String,
    pub after: String,
}

pub fn trace_rules(
    source: &str,
    options: DecompileOptions,
    trace_options: RuleTraceOptions,
) -> Result<Vec<RuleTraceEvent>> {
    validate_trace_rule_name("trace start rule", trace_options.start_from.as_deref())?;
    validate_trace_rule_name("trace stop rule", trace_options.stop_after.as_deref())?;

    if detect_bundle(source, &options.filename)?.is_some() {
        return Err(anyhow!(
            "rule tracing currently supports single-file inputs only; use normal decompile or unpack for bundles"
        ));
    }

    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_js(source, &options.filename, cm.clone())?;

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        let mut previous = print_trace_module(&module, cm.clone())?;
        let mut events = Vec::new();
        let mut render_error: Option<anyhow::Error> = None;

        {
            let mut observer = |rule: &'static str, module: &Module| {
                if render_error.is_some() {
                    return;
                }
                match print_trace_module(module, cm.clone()) {
                    Ok(after) => {
                        let changed = after != previous;
                        if changed || !trace_options.only_changed {
                            events.push(RuleTraceEvent {
                                rule,
                                changed,
                                before: previous.clone(),
                                after: after.clone(),
                            });
                        }
                        previous = after;
                    }
                    Err(error) => {
                        render_error = Some(error);
                    }
                }
            };

            apply_rules_with_observer(
                &mut module,
                unresolved_mark,
                RulePipelineOptions {
                    start_from: trace_options.start_from.as_deref(),
                    stop_after: trace_options.stop_after.as_deref(),
                    dce_mode: options.dce_mode,
                    rewrite_level: options.level,
                    module_facts: None,
                    current_filename: None,
                },
                &mut observer,
            );
        }

        if let Some(error) = render_error {
            return Err(error);
        }

        Ok(events)
    })
}

fn validate_trace_rule_name(label: &str, rule_name: Option<&str>) -> Result<()> {
    let Some(rule_name) = rule_name else {
        return Ok(());
    };
    if rule_names().contains(&rule_name) {
        Ok(())
    } else {
        Err(anyhow!("unknown {label}: {rule_name}"))
    }
}

/// Render a trace event list as a git-style unified diff log.
///
/// Prints the initial source once, then for each event:
/// - changed: a unified diff against the previous rendering
/// - unchanged: a single header line
///
/// The per-rule "before" string is implied by the previous event's output, so
/// it's never repeated — only the delta is shown.
pub fn format_trace_events(events: &[RuleTraceEvent]) -> String {
    use similar::TextDiff;

    let mut out = String::new();

    let Some(first) = events.first() else {
        return out;
    };

    out.push_str("=== initial ===\n");
    out.push_str(&first.before);
    if !first.before.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');

    for event in events {
        if !event.changed {
            out.push_str("=== ");
            out.push_str(event.rule);
            out.push_str(" (unchanged) ===\n\n");
            continue;
        }

        out.push_str("=== ");
        out.push_str(event.rule);
        out.push_str(" ===\n");

        let diff = TextDiff::from_lines(&event.before, &event.after);
        let mut unified = diff.unified_diff();
        unified.missing_newline_hint(false);
        for hunk in unified.iter_hunks() {
            out.push_str(&hunk.to_string());
        }
        out.push('\n');
    }

    out
}
