mod diagnostics;
mod discovery;
mod io;
mod normalize;
mod output;
mod single_file;
mod trace;
mod types;
mod unpack;
mod unpack_cleanup;
mod unpack_cycles;

pub use discovery::is_detected_unpack_input;
pub use normalize::{normalize, NormalizeOptions};
pub use output::{deduplicate_path, safe_relative_module_path};
pub use single_file::decompile;
pub use trace::{format_trace_events, trace_rules, RuleTraceEvent, RuleTraceOptions};
pub use types::{
    DceMode, DecompileOptions, DecompileOutput, UnpackInput, UnpackOutput, UnpackWarning,
    UnpackWarningKind,
};
pub use unpack::{unpack, unpack_files, unpack_files_raw, unpack_raw};
