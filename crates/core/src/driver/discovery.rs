//! Bundle/chunk shape detection for `--unpack` directory scanning.
//!
//! When the unpacker is pointed at a directory, the CLI walks it recursively
//! and asks [`is_detected_unpack_input`] whether each candidate file looks like
//! a bundle/chunk the unpacker can split. The directory walking itself is
//! filesystem I/O that lives in the CLI; the detection here stays in core
//! because it is the decompiler's domain (it calls the unpacker).

/// True when `source` looks like a bundle/chunk that the unpacker can split.
///
/// Tries the structural unpacker first; when `heuristic_split` is enabled it
/// also accepts scope-hoisted bundles that split into more than one module.
pub fn is_detected_unpack_input(source: &str, heuristic_split: bool) -> bool {
    matches!(crate::unpacker::try_unpack_bundle(source), Ok(Some(_)))
        || (heuristic_split
            && crate::unpacker::scope_hoist::split_scope_hoisted(source)
                .is_some_and(|result| result.modules.len() > 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn webpack5_chunk_source() -> &'static str {
        r#"
(self.webpackChunk = self.webpackChunk || []).push([
  [1],
  {
    100: function(module, exports, require) {
      "use strict";
      require.r(exports);
      exports.default = 1;
    }
  }
]);
"#
    }

    #[test]
    fn detects_webpack_chunk_source() {
        assert!(is_detected_unpack_input(webpack5_chunk_source(), false));
    }

    #[test]
    fn rejects_plain_source() {
        assert!(!is_detected_unpack_input("const value = 1;", true));
    }
}
