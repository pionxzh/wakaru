use std::path::Path;

use crate::error::{Error, ErrorKind, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EmbeddedSource {
    pub path: String,
    pub content: String,
}

pub fn embedded_sources(data: &[u8]) -> Result<Vec<EmbeddedSource>> {
    let map = wakaru_core::parse_sourcemap(data)
        .map_err(|error| Error::new(ErrorKind::SourceMap, None, error))?;
    Ok(wakaru_core::extract_source_entries(&map, Path::new(""))
        .into_iter()
        .map(|(path, content)| EmbeddedSource {
            path: path.to_string_lossy().replace('\\', "/"),
            content,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_normalized_embedded_paths_without_filesystem_types() {
        let data = br#"{"version":3,"sources":["webpack:///../src/main.js"],"sourcesContent":["export const value = 1;"],"names":[],"mappings":""}"#;
        let sources = embedded_sources(data).expect("valid source map should parse");
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].path, "src/main.js");
        assert_eq!(sources[0].content, "export const value = 1;");
    }
}
