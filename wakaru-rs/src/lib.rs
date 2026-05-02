pub mod driver;
pub mod facts;
pub mod namespace_decomposition;
pub mod reexport_consolidation;
pub mod rules;
pub mod sourcemap_rename;
pub mod unpacker;
pub mod utils;

pub use driver::{
    decompile, format_trace_events, trace_rules, unpack, unpack_raw, DecompileOptions,
    RuleTraceEvent, RuleTraceOptions,
};
pub use facts::{
    collect_module_facts, ExportFact, ExportKind, ImportFact, ImportKind, ModuleFacts,
    ModuleFactsMap,
};
pub use rules::{
    apply_default_rules_with_options, apply_rules_between, apply_rules_between_with_options,
    apply_rules_until, apply_rules_until_with_level, apply_rules_until_with_options, rule_names,
    RewriteLevel,
};
pub use sourcemap_rename::{extract_sources, parse_sourcemap};
pub use unpacker::{unpack_webpack4, UnpackResult, UnpackedModule};

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
