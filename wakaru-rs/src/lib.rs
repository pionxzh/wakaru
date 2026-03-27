pub mod driver;
pub mod rules;
pub mod unpacker;
pub mod utils;

pub use driver::{decompile, unpack, DecompileOptions};
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
