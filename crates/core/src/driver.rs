mod diagnostics;
mod discovery;
mod error;
mod io;
mod normalize;
mod output;
mod single_file;
mod trace;
mod types;
mod unpack;
mod unpack_cleanup;
mod unpack_cycles;

pub use crate::unpacker::BundleFormat;
pub use discovery::is_detected_unpack_input;
pub use error::{DriverError, DriverErrorKind, DriverResult};
pub use normalize::{normalize, NormalizeOptions};
pub use output::{deduplicate_path, safe_relative_module_path};
pub use single_file::{decompile, decompile_owned, OwnedDecompileFailure};
pub use trace::{format_trace_events, trace_rules, RuleTraceEvent, RuleTraceOptions};
pub use types::{
    CapturedUnpackOutput, DceMode, DecompileOptions, DecompileOutput, PreparedInputId,
    PreparedModuleOutput, PreparedModuleProvenance, PreparedUnpackOutput, UnpackWarning,
    UnpackWarningKind,
};
pub use unpack::{
    prepare_unpack_input, prepare_unpack_input_with_policy, unpack_prepared_inputs,
    unpack_prepared_inputs_with_policy_and_capture, PreparedInputDetection, PreparedUnpackInput,
    ScopeHoistPolicy,
};

/// Legacy adapters retained for `wakaru-core`'s integration tests.
///
/// Production callers should use the published `wakaru` facade. This module
/// remains public only because Cargo integration tests are separate crates.
#[doc(hidden)]
pub mod test_support {
    pub use super::types::{ModuleProvenance, UnpackInput, UnpackOutput};
    pub use super::unpack::{unpack, unpack_files, unpack_files_raw, unpack_raw};
}
