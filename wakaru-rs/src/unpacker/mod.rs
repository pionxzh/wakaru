pub mod webpack4;

pub struct UnpackedModule {
    pub id: usize,
    pub is_entry: bool,
    pub code: String,
    pub filename: String,
}

pub struct UnpackResult {
    pub modules: Vec<UnpackedModule>,
}

pub fn unpack_webpack4(source: &str) -> Option<UnpackResult> {
    webpack4::detect_and_extract(source)
}
