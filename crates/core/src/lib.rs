#![allow(
    clippy::borrowed_box,
    clippy::boxed_local,
    clippy::ptr_arg,
    clippy::type_complexity,
    clippy::vec_box
)]

pub(crate) mod analysis;
pub mod driver;
pub mod facts;
pub(crate) mod js_names;
pub(crate) mod module_path;
pub mod namespace_decomposition;
pub mod reexport_consolidation;
pub mod rules;
pub mod sourcemap_rename;
pub mod tdz_check;
pub mod unpacker;
pub mod utils;
pub mod vue_recovery;
pub mod vue_template;

pub use driver::{
    decompile, deduplicate_path, format_trace_events, is_detected_unpack_input, normalize,
    safe_relative_module_path, trace_rules, unpack, unpack_files, unpack_files_raw, unpack_raw,
    BundleFormat, DceMode, DecompileOptions, DecompileOutput, NormalizeOptions, RuleTraceEvent,
    RuleTraceOptions, UnpackInput, UnpackOutput, UnpackWarning, UnpackWarningKind,
};
pub use facts::{
    collect_module_facts, ExportFact, ExportKind, HelperExportFact, HelperKind, ImportFact,
    ImportKind, ModuleFacts, ModuleFactsMap, TypeScriptHelperExportFact, TypeScriptHelperKind,
};
pub use rules::{
    apply_rules, rule_descriptors, rule_names, RewriteAssumptions, RewriteLevel, RewritePolicy,
    RuleDescriptor, RulePipelineOptions, RuleStage,
};
pub use sourcemap_rename::{extract_source_entries, parse_sourcemap, resolve_source_path};
pub use tdz_check::{check_tdz, TdzViolation};
pub use unpacker::{scope_hoist, unpack_webpack4, UnpackResult, UnpackedModule};
pub use vue_recovery::{
    decompile_vue_sfc, recover_vue_sfc_from_js, recover_vue_sfc_source_from_js,
};

/// Unpack a webpack4 bundle and return the raw (pre-decompile-rules) module code.
/// Each element is `(filename, code)`. Returns `None` if the source is not recognized
/// as a webpack4 bundle.
pub fn unpack_webpack4_raw(source: &str) -> Option<Vec<(String, String)>> {
    let result = unpacker::unpack_webpack4_raw(source)?;
    Some(
        result
            .modules
            .into_iter()
            .map(|m| (m.filename, m.code))
            .collect(),
    )
}
