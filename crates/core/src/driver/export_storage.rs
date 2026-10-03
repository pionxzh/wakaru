use anyhow::anyhow;
use swc_core::common::{sync::Lrc, Mark, SourceMap, Span, GLOBALS};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;

use super::io::parse_js;
use super::types::DecompileOptions;
use super::unpack::detect_bundle;
use super::{DriverError, DriverErrorKind, DriverResult};
use crate::rules::{
    analyze_export_storage, apply_rules, rule_names, ExportStorageReport, RulePipelineOptions,
};

/// The per-name CommonJS export storage decisions for one single-file input,
/// taken on the module as it reaches `UnEsm`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonJsExportReport {
    /// Set when a module-level condition failed, so no name was classified.
    pub gate: Option<String>,
    /// False when the module never refers to `exports`.
    pub uses_exports: bool,
    pub exports: Vec<CommonJsExportDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonJsExportDecision {
    pub name: String,
    /// `getter`, `mirror`, `property`, or `unrecovered`.
    pub storage: &'static str,
    /// The binding a getter returns or a mirror copies.
    pub binding: Option<String>,
    /// Rejected models in decision order, as `model: reason (line:column)`.
    pub rejected: Vec<String>,
    /// Plain writes, leading `void 0` sentinels, other writes, reads, calls,
    /// getters, and how many of all accesses are deferred.
    pub writes: usize,
    pub sentinels: usize,
    pub other_writes: usize,
    pub reads: usize,
    pub calls: usize,
    pub getters: usize,
    pub deferred: usize,
}

/// Run the single-file pipeline up to `UnEsm` and report how each CommonJS
/// export name would be stored.
pub fn explain_commonjs_exports(
    source: &str,
    options: DecompileOptions,
) -> DriverResult<CommonJsExportReport> {
    if detect_bundle(source, &options.filename)
        .map_err(|error| DriverError::new(DriverErrorKind::Parse, error))?
        .is_some()
    {
        return Err(DriverError::new(
            DriverErrorKind::InvalidInput,
            anyhow!(
                "export storage analysis supports single-file inputs only; unpack the bundle and analyze one module"
            ),
        ));
    }
    let names = rule_names();
    let un_esm = names
        .iter()
        .position(|name| *name == "UnEsm")
        .expect("the pipeline registers UnEsm");
    let stop_after = un_esm.checked_sub(1).map(|index| names[index]);

    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_js(source, &options.filename, cm.clone())
            .map_err(|error| DriverError::new(DriverErrorKind::Parse, error))?;
        let unresolved_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
        if let Some(stop_after) = stop_after {
            apply_rules(
                &mut module,
                unresolved_mark,
                RulePipelineOptions {
                    stop_after: Some(stop_after),
                    ..RulePipelineOptions::default()
                        .with_dce_mode(options.dce_mode)
                        .with_rewrite_level(options.level)
                        .with_current_filename(&options.filename)
                },
            );
        }

        let location = |span: Option<Span>| {
            span.filter(|span| !span.is_dummy()).map(|span| {
                let loc = cm.lookup_char_pos(span.lo);
                format!(" ({}:{})", loc.line, loc.col_display + 1)
            })
        };
        Ok(match analyze_export_storage(&module, unresolved_mark) {
            ExportStorageReport::NoCommonJsExports => CommonJsExportReport {
                gate: None,
                uses_exports: false,
                exports: Vec::new(),
            },
            ExportStorageReport::ModuleGate { message, span, .. } => CommonJsExportReport {
                gate: Some(format!("{message}{}", location(span).unwrap_or_default())),
                uses_exports: true,
                exports: Vec::new(),
            },
            ExportStorageReport::Names(decisions) => CommonJsExportReport {
                gate: None,
                uses_exports: true,
                exports: decisions
                    .into_iter()
                    .map(|decision| CommonJsExportDecision {
                        name: decision.name.to_string(),
                        storage: decision.storage.as_str(),
                        binding: decision.binding,
                        rejected: decision
                            .rejected
                            .iter()
                            .map(|rejection| {
                                format!(
                                    "{}: {}{}",
                                    rejection.storage.as_str(),
                                    rejection.message,
                                    location(rejection.span).unwrap_or_default()
                                )
                            })
                            .collect(),
                        writes: decision.accesses.writes,
                        sentinels: decision.accesses.sentinels,
                        other_writes: decision.accesses.other_writes,
                        reads: decision.accesses.reads,
                        calls: decision.accesses.calls,
                        getters: decision.accesses.getters,
                        deferred: decision.accesses.deferred,
                    })
                    .collect(),
            },
        })
    })
}
