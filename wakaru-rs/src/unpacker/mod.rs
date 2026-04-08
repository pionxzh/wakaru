pub mod browserify;
pub mod esbuild;
pub mod webpack4;
pub mod webpack5;

pub struct UnpackedModule {
    pub id: String,
    pub is_entry: bool,
    pub code: String,
    pub filename: String,
}

pub struct UnpackResult {
    pub modules: Vec<UnpackedModule>,
}

pub fn unpack_bundle(source: &str) -> Option<UnpackResult> {
    webpack5::detect_and_extract(source)
        .or_else(|| webpack4::detect_and_extract(source))
        .or_else(|| webpack5::detect_and_extract_chunk(source))
        .or_else(|| browserify::detect_and_extract(source))
        .or_else(|| esbuild::detect_and_extract(source))
}

pub fn unpack_webpack4(source: &str) -> Option<UnpackResult> {
    webpack4::detect_and_extract(source)
}

pub fn unpack_webpack4_raw(source: &str) -> Option<UnpackResult> {
    webpack4::detect_and_extract_raw(source)
}
