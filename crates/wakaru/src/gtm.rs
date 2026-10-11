//! Google Tag Manager container extraction.
//!
//! A container (`gtm.js?id=GTM-…`) is a `var data = {…}` object holding the
//! site's tags and variables, followed by Google's runtime, which is the same
//! in every container. [`extract_container`] returns the code the site added —
//! Custom JavaScript variable bodies and Custom HTML scripts — as separate
//! pieces, and skips the runtime. The pieces are JavaScript text: pass them to
//! [`decompile`](crate::decompile) or [`unpack`](crate::unpack) like any other
//! source.
//!
//! A macro reference stays the identifier `__gtm_macro_<n>`, where `n` is the
//! index in [`Container::macros`]. A reference inside a string literal closes
//! the literal and is concatenated, so it stays a reference in the code.
//!
//! ```
//! let source = r#"var data = {"resource": {"version": "3",
//!   "macros": [{"function": "__v", "vtp_name": "user"}],
//!   "tags": [{"function": "__html", "tag_id": 7,
//!     "vtp_html": ["template", "<script>track(", ["escape", ["macro", 0], 8, 16], ");</script>"]}],
//!   "predicates": [], "rules": []}};"#;
//! let container = wakaru::gtm::extract_container(source).expect("a container");
//! assert_eq!(container.scripts[0].code, "track(__gtm_macro_0);");
//! assert_eq!(container.macros[0].name.as_deref(), Some("user"));
//! ```

use wakaru_core::gtm as core;

/// The container field a script or template part came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Field {
    /// `vtp_javascript` of the Custom JavaScript variable at this index of the
    /// macro array.
    CustomJavaScriptVariable { macro_index: usize },
    /// `vtp_html` of the Custom HTML tag with this `tag_id`.
    CustomHtmlTag { tag_id: u64 },
}

/// One piece of JavaScript the container carries.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Script {
    pub field: Field,
    /// Position among the tag's JavaScript `<script>` elements, external ones
    /// included, so a tag's positions can have gaps. Always `0` for a Custom
    /// JavaScript variable.
    pub position: usize,
    /// The script text with macro references substituted. It is not
    /// guaranteed to parse: a Custom HTML script can hold a fragment.
    pub code: String,
}

/// A Custom HTML script loaded from a URL (`data-gtmsrc` or `src`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExternalScript {
    pub tag_id: u64,
    pub position: usize,
    pub url: String,
}

/// An entry of the container's macro array that names its `function`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Macro {
    /// The `n` in `__gtm_macro_<n>`.
    pub index: usize,
    /// GTM's variable type, such as `__v` (data layer) or `__jsm` (Custom
    /// JavaScript).
    pub function: String,
    /// `vtp_name`, when the variable has one.
    pub name: Option<String>,
}

/// A template part that was not understood. Its macro reference is kept as
/// the identifier `__gtm_macro_<n>_escape_<modes>` in the text, so nothing is
/// dropped; `macro_index` is `None` for a part that is not a macro reference.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnsupportedPart {
    pub field: Field,
    pub macro_index: Option<u64>,
    /// The escape modes, in order. Modes `3`, `7`, `8`, `9`, `12` and `16`
    /// are understood.
    pub modes: Vec<u64>,
}

/// The parts of a container worth reading.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Container {
    /// `data.resource.version`.
    pub version: Option<String>,
    pub macros: Vec<Macro>,
    /// Custom JavaScript variables in macro order, then Custom HTML scripts in
    /// tag order.
    pub scripts: Vec<Script>,
    pub external_scripts: Vec<ExternalScript>,
    pub unsupported_parts: Vec<UnsupportedPart>,
}

/// Read a Google Tag Manager container out of JavaScript source.
///
/// Returns `None` unless the source holds a `var data = {…}` whose `resource`
/// carries `macros`, `tags`, `predicates` and `rules` arrays. Detection reads
/// that object shape, never a file name or URL. Text without those keys is
/// rejected without being parsed, so calling this on every input is cheap.
pub fn extract_container(source: &str) -> Option<Container> {
    let container = core::extract_from_source(source)?;
    Some(Container {
        version: container.version,
        macros: container
            .macros
            .into_iter()
            .map(|m| Macro {
                index: m.index,
                function: m.function,
                name: m.name,
            })
            .collect(),
        scripts: container
            .scripts
            .into_iter()
            .map(|script| Script {
                field: field(script.field),
                position: script.position,
                code: script.source,
            })
            .collect(),
        external_scripts: container
            .external_scripts
            .into_iter()
            .map(|script| ExternalScript {
                tag_id: script.tag_id,
                position: script.position,
                url: script.url,
            })
            .collect(),
        unsupported_parts: container
            .unsupported_parts
            .into_iter()
            .map(|part| UnsupportedPart {
                field: field(part.field),
                macro_index: part.macro_index,
                modes: part.modes,
            })
            .collect(),
    })
}

fn field(field: core::GtmField) -> Field {
    match field {
        core::GtmField::CustomJavaScriptMacro { macro_index } => {
            Field::CustomJavaScriptVariable { macro_index }
        }
        core::GtmField::CustomHtmlTag { tag_id } => Field::CustomHtmlTag { tag_id },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_javascript_is_not_a_container() {
        assert!(extract_container("var data = {resource: {}}; console.log(data);").is_none());
    }

    #[test]
    fn maps_every_part_of_the_container() {
        let container = extract_container(
            r#"var data = {"resource": {"macros": [{"function": "__jsm", "vtp_javascript": ["template", "(function(){return 1})();"]}],
              "tags": [{"function": "__html", "tag_id": 4, "vtp_html": ["template", "<script src=\"https://cdn.example/a.js\"></script><script>f(", ["escape", ["macro", 0], 2], ")</script>"]}],
              "predicates": [], "rules": []}};"#,
        )
        .expect("a container");
        assert_eq!(
            container.scripts,
            vec![
                Script {
                    field: Field::CustomJavaScriptVariable { macro_index: 0 },
                    position: 0,
                    code: "(function(){return 1})();".to_string(),
                },
                Script {
                    field: Field::CustomHtmlTag { tag_id: 4 },
                    position: 1,
                    code: "f(__gtm_macro_0_escape_2)".to_string(),
                },
            ]
        );
        assert_eq!(
            container.external_scripts,
            vec![ExternalScript {
                tag_id: 4,
                position: 0,
                url: "https://cdn.example/a.js".to_string(),
            }]
        );
        assert_eq!(
            container.unsupported_parts,
            vec![UnsupportedPart {
                field: Field::CustomHtmlTag { tag_id: 4 },
                macro_index: Some(0),
                modes: vec![2],
            }]
        );
        assert_eq!(container.macros[0].function, "__jsm");
    }
}
