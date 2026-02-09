use wakaru_rs::{decompile, DecompileOptions};

#[allow(dead_code)]
pub fn render(source: &str) -> String {
    decompile(
        source,
        DecompileOptions {
            filename: "fixture.js".to_string(),
        },
    )
    .expect("decompile should succeed")
}

#[allow(dead_code)]
pub fn normalize(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[allow(dead_code)]
pub fn assert_normalized_eq(output: &str, expected: &str) {
    assert_eq!(normalize(output), normalize(expected));
}

#[allow(dead_code)]
pub fn compact(input: &str) -> String {
    input.chars().filter(|c| !c.is_whitespace()).collect::<String>()
}

#[allow(dead_code)]
pub fn assert_compact_eq(output: &str, expected: &str) {
    assert_eq!(compact(output), compact(expected));
}
