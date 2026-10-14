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
    /// `vtp_component`, when the variable has one: `URL`, `PATH` or `HOST` for
    /// a built-in `__u` variable. It tells apart two macros that have no name.
    pub component: Option<String>,
}

/// One tag the container lists.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Tag {
    pub tag_id: u64,
    /// GTM's tag type, such as `__html` (Custom HTML) or `__gaawe` (a GA4
    /// event tag).
    pub function: String,
}

/// An operand of a [`Predicate`]: a reference to a macro, a literal value, or a
/// list of them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Operand {
    /// The macro at this index of [`Container::macros`].
    Macro { index: u64 },
    /// A literal string, number or boolean, as text.
    Value(String),
    /// An array argument, such as the alternatives of an `any_of` comparison.
    List(Vec<Operand>),
}

/// One entry of the container's `predicates` array: a comparison inside a
/// trigger's condition.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Predicate {
    /// The row in [`Container::predicates`], which a [`Trigger`] references.
    pub index: usize,
    /// The comparison, such as `_eq` (equals), `_cn` (contains) or `_re`
    /// (matches).
    pub function: String,
    pub arg0: Operand,
    pub arg1: Operand,
    pub ignore_case: bool,
    /// The comparison holds when `arg0` matches any element of a list `arg1`.
    pub any_of: bool,
}

/// One trigger, compiled: the condition under which it applies, and the tags it
/// fires or blocks.
///
/// It is an entry of the container's `rules` array; the runtime stores tags by
/// their position in that array, and the extraction turns those into the
/// `tag_id`s in `fires` and `blocks`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Trigger {
    /// The row in [`Container::triggers`].
    pub index: usize,
    /// `when`: predicate indices that must all hold.
    pub when: Vec<usize>,
    /// `unless`: predicate indices that must all not hold.
    pub unless: Vec<usize>,
    /// The `tag_id`s this trigger fires.
    pub fires: Vec<u64>,
    /// The `tag_id`s this trigger blocks.
    pub blocks: Vec<u64>,
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
    /// The container's tags.
    pub tags: Vec<Tag>,
    /// The predicates a [`Trigger`]'s condition references.
    pub predicates: Vec<Predicate>,
    /// The triggers: what fires or blocks which tag, and under what condition.
    pub triggers: Vec<Trigger>,
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
                component: m.component,
            })
            .collect(),
        tags: container
            .tags
            .into_iter()
            .map(|t| Tag {
                tag_id: t.tag_id,
                function: t.function,
            })
            .collect(),
        predicates: container
            .predicates
            .into_iter()
            .map(|p| Predicate {
                index: p.index,
                function: p.function,
                arg0: operand(p.arg0),
                arg1: operand(p.arg1),
                ignore_case: p.ignore_case,
                any_of: p.any_of,
            })
            .collect(),
        triggers: container
            .triggers
            .into_iter()
            .map(|t| Trigger {
                index: t.index,
                when: t.when,
                unless: t.unless,
                fires: t.fires,
                blocks: t.blocks,
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

fn operand(value: core::GtmOperand) -> Operand {
    match value {
        core::GtmOperand::Macro { index } => Operand::Macro { index },
        core::GtmOperand::Value(text) => Operand::Value(text),
        core::GtmOperand::List(items) => Operand::List(items.into_iter().map(operand).collect()),
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
