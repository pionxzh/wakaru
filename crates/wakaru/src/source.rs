/// An owned input file: filename, code, and an optional input source map.
///
/// Operations consume `Source`. Passing an owned `String` lets Wakaru move it
/// into SWC's source storage instead of copying a borrowed `&str`; callers
/// that need to retain the source can clone it explicitly.
///
/// Input source maps are valid for [`decompile`](crate::decompile): rule
/// pipeline output is renamed using positions recovered from the map. They are
/// always invalid for [`unpack`](crate::unpack), because extracted modules no
/// longer use the bundle's generated coordinates —
/// [`UnpackJob::push`](crate::UnpackJob::push) rejects such an input with
/// [`ErrorKind::InvalidOptions`](crate::ErrorKind::InvalidOptions). Output
/// source-map generation is independently configurable on the options types.
#[derive(Debug, Clone)]
pub struct Source {
    filename: String,
    code: String,
    source_map: Option<Vec<u8>>,
}

impl Source {
    pub fn new(filename: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            filename: filename.into(),
            code: code.into(),
            source_map: None,
        }
    }

    pub fn with_source_map(mut self, source_map: impl Into<Vec<u8>>) -> Self {
        self.source_map = Some(source_map.into());
        self
    }

    pub fn filename(&self) -> &str {
        &self.filename
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn source_map(&self) -> Option<&[u8]> {
        self.source_map.as_deref()
    }

    pub fn into_parts(self) -> SourceParts {
        SourceParts {
            filename: self.filename,
            code: self.code,
            source_map: self.source_map,
        }
    }
}

/// The owned fields of a [`Source`], recovered via [`Source::into_parts`].
#[derive(Debug, Clone)]
pub struct SourceParts {
    pub filename: String,
    pub code: String,
    pub source_map: Option<Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_owns_and_returns_all_input_parts() {
        let source =
            Source::new("input.js", String::from("const x = 1;")).with_source_map(vec![1, 2, 3]);

        assert_eq!(source.filename(), "input.js");
        assert_eq!(source.code(), "const x = 1;");
        assert_eq!(source.source_map(), Some([1, 2, 3].as_slice()));

        let parts = source.into_parts();
        assert_eq!(parts.filename, "input.js");
        assert_eq!(parts.code, "const x = 1;");
        assert_eq!(parts.source_map, Some(vec![1, 2, 3]));
    }
}
