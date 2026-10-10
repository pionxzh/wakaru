//! Extraction of Google Tag Manager containers.
//!
//! A container (`https://www.googletagmanager.com/gtm.js?id=GTM-…`) is one script with two parts: a
//! `var data = {…}` object holding the site's tags, variables, triggers and custom templates, and
//! Google's Closure-compiled runtime, which is the same in every container. The container data is the
//! part worth reading: Custom HTML tags and Custom JS variables carry code that was added outside the
//! normal review process.
//!
//! The data object stores editable text as *template arrays* — `["template", "literal", <escape>,
//! …]` — where an escape is `["escape", ["macro", n], mode]`: a reference to the macro at index `n`.
//! This module joins those arrays back into strings, substituting `__gtm_macro_<n>` for the index the
//! generated `index.md` documents, and emits one file per Custom JS macro and per script tag found in
//! a Custom HTML tag.

use swc_core::common::{sync::Lrc, SourceMap};
use swc_core::ecma::ast::{
    Expr, Lit, Module, ObjectLit, Prop, PropName, PropOrSpread, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

/// A value read out of the container's `data.resource` object. Only what the extraction needs is
/// modelled; anything else is kept as [`GtmValue::Other`] so it can be ignored without panicking.
#[derive(Clone, Debug, PartialEq)]
pub enum GtmValue {
    Str(String),
    Num(f64),
    Bool(bool),
    Null,
    Array(Vec<GtmValue>),
    Object(Vec<(String, GtmValue)>),
    Other,
}

impl GtmValue {
    fn get(&self, key: &str) -> Option<&GtmValue> {
        match self {
            GtmValue::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            GtmValue::Str(value) => Some(value),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[GtmValue]> {
        match self {
            GtmValue::Array(values) => Some(values),
            _ => None,
        }
    }

    fn as_u64(&self) -> Option<u64> {
        match self {
            GtmValue::Num(value) if *value >= 0.0 => Some(*value as u64),
            _ => None,
        }
    }
}

/// One file the container was split into, with its path relative to the output directory.
#[derive(Clone, Debug, PartialEq)]
pub struct GtmFile {
    pub path: String,
    pub source: String,
}

/// A Custom HTML tag that loads an external script through `data-gtmsrc`. There is no body to write,
/// so the URL only shows up in the index.
#[derive(Clone, Debug, PartialEq)]
pub struct GtmExternalScript {
    pub tag_id: u64,
    pub url: String,
}

/// A macro that was used somewhere but is missing from the container.
#[derive(Clone, Debug, PartialEq)]
pub struct GtmMacroRef {
    pub index: u64,
    pub function: Option<String>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GtmContainer {
    pub version: Option<String>,
    pub files: Vec<GtmFile>,
    pub external_scripts: Vec<GtmExternalScript>,
    /// Macro references found in the emitted code, so a reader can answer "what is 30?".
    pub macros: Vec<GtmMacroRef>,
    /// Escape modes this module does not understand, and anything else skipped.
    pub warnings: Vec<String>,
}

/// Reads a container out of source text, for callers that have not parsed the input yet.
pub fn extract_from_source(source: &str, filename: &str) -> Option<GtmContainer> {
    let source_map: Lrc<SourceMap> = Default::default();
    let module = crate::driver::io::parse_js(source, filename, source_map).ok()?;
    extract(&module)
}

/// Look for a GTM container in a parsed module and split it into readable files.
///
/// Returns `None` when the module has no `var data = {…}` whose `resource` object has both a `macros`
/// and a `tags` array — the object shape the format is recognizable by. Detection does not look at the
/// file name or the URL, so a container saved by hand is recognized too.
pub fn extract(module: &Module) -> Option<GtmContainer> {
    let mut finder = DataObjectFinder::default();
    module.visit_with(&mut finder);
    let data = GtmValue::from_object(&finder.data?);
    let resource = data.get("resource")?;
    let macros = resource.get("macros")?.as_array()?;
    let tags = resource.get("tags")?.as_array()?;

    let mut container = GtmContainer {
        version: resource.get("version").and_then(|version| match version {
            GtmValue::Str(value) => Some(value.clone()),
            GtmValue::Num(value) => Some(format!("{}", *value as u64)),
            _ => None,
        }),
        files: Vec::new(),
        external_scripts: Vec::new(),
        macros: Vec::new(),
        warnings: Vec::new(),
    };

    for (index, macro_value) in macros.iter().enumerate() {
        let macro_index = index as u64;
        let function = macro_value.get("function").and_then(GtmValue::as_str);
        if let Some(function) = function {
            container.macros.push(GtmMacroRef {
                index: macro_index,
                function: Some(function.to_string()),
                name: macro_value
                    .get("vtp_name")
                    .and_then(GtmValue::as_str)
                    .map(ToOwned::to_owned),
            });
        }
        // Custom JS is the only macro whose body is code worth reading; everything else is a value
        // that the index already describes.
        if function == Some("__jsm") {
            if let Some(source) = macro_value.get("vtp_javascript") {
                let joined = join_template(source, &mut container.warnings);
                container.files.push(GtmFile {
                    path: format!("macros/jsm-{macro_index}.js"),
                    source: joined,
                });
            }
        }
    }

    for tag_value in tags.iter() {
        let function = tag_value.get("function").and_then(GtmValue::as_str);
        let tag_id = tag_value
            .get("tag_id")
            .and_then(GtmValue::as_u64)
            .unwrap_or(0);
        if function != Some("__html") {
            continue;
        }
        let Some(html) = tag_value.get("vtp_html") else {
            continue;
        };
        let html = join_template(html, &mut container.warnings);
        for (script_index, script) in split_gtm_scripts(&html).into_iter().enumerate() {
            match script.url {
                Some(url) => container
                    .external_scripts
                    .push(GtmExternalScript { tag_id, url }),
                None => container.files.push(GtmFile {
                    path: format!("tags/html-{tag_id}-{script_index}.js"),
                    source: script.body,
                }),
            }
        }
    }

    container.files.push(GtmFile {
        path: "index.md".to_string(),
        source: render_index(&container),
    });
    Some(container)
}

/// Joins a template array (`["template", …]`) back into a string.
///
/// `["escape", ["macro", n], 8 | 16]` sits in code position, so the identifier drops in as a
/// reference. `["escape", ["macro", n], 7]` sits inside a string literal, where a bare identifier
/// would become plain text and lose the data flow, so the literal is closed and the reference
/// concatenated instead. Any other mode keeps a comment placeholder and is reported as a warning
/// rather than guessed at.
fn join_template(value: &GtmValue, warnings: &mut Vec<String>) -> String {
    let Some(parts) = value.as_array() else {
        return String::new();
    };
    if parts.first().and_then(GtmValue::as_str) != Some("template") {
        return String::new();
    }
    let mut out = String::new();
    for part in &parts[1..] {
        match part {
            GtmValue::Str(text) => out.push_str(text),
            GtmValue::Array(items) => {
                let Some(macro_index) = macro_reference(items) else {
                    warnings
                        .push("template part that is neither text nor a macro reference".into());
                    continue;
                };
                match escape_mode(items) {
                    Some(8) | Some(16) | None => {
                        out.push_str(&format!("__gtm_macro_{macro_index}"))
                    }
                    Some(7) => {
                        let quote = enclosing_quote(&out);
                        out.push_str(&format!("{quote} + __gtm_macro_{macro_index} + {quote}"));
                    }
                    Some(mode) => {
                        warnings.push(format!(
                            "macro {macro_index} uses escape mode {mode}, which is not understood"
                        ));
                        out.push_str(&format!(
                            "/* gtm: __gtm_macro_{macro_index} (unsupported escape mode {mode}) */"
                        ));
                    }
                }
            }
            _ => {
                warnings.push("template part that is neither text nor a macro reference".into());
            }
        }
    }
    out
}

/// The macro a `["macro", n]` — or `["escape", ["macro", n], mode]` — reference points at.
fn macro_reference(items: &[GtmValue]) -> Option<u64> {
    match items.first()? {
        GtmValue::Str(kind) if kind == "macro" => items.get(1)?.as_u64(),
        GtmValue::Str(kind) if kind == "escape" => {
            let inner = items.get(1)?.as_array()?;
            if inner.first()?.as_str()? == "macro" {
                inner.get(1)?.as_u64()
            } else {
                None
            }
        }
        _ => None,
    }
}

fn escape_mode(items: &[GtmValue]) -> Option<u64> {
    match items.first()? {
        GtmValue::Str(kind) if kind == "escape" => items.get(2)?.as_u64(),
        _ => None,
    }
}

/// The quote character of the string literal a mode-7 macro sits inside, so the substitution can
/// close and reopen it with the same one.
fn enclosing_quote(out: &str) -> char {
    let bytes = out.as_bytes();
    let mut quote = '"';
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'"' && *byte != b'\'' {
            continue;
        }
        let mut backslashes = 0;
        let mut cursor = index;
        while cursor > 0 && bytes[cursor - 1] == b'\\' {
            backslashes += 1;
            cursor -= 1;
        }
        if backslashes % 2 == 0 {
            quote = *byte as char;
        }
    }
    quote
}

struct GtmScript {
    body: String,
    url: Option<String>,
}

/// Picks the `<script>` tags GTM rewrote out of a Custom HTML tag. Only `type="text/gtmscript"`
/// scripts carry the code the tag adds; an external one moved its `src` into `data-gtmsrc` and has no
/// body, so the file numbers are the scripts' positions in the tag and can have gaps. Other tags — including the ordinary `<noscript>` fallbacks GTM also writes — are left alone.
fn split_gtm_scripts(html: &str) -> Vec<GtmScript> {
    const CLOSE: &str = "</script>";
    let mut scripts = Vec::new();
    // offsets are kept against `html` throughout: a tag can be preceded by text (a newline, usually),
    // and measuring the body from the start of the tag instead skewed it by that much
    let mut cursor = 0;
    while let Some(start) = html[cursor..].find("<script") {
        let open_start = cursor + start;
        let Some(open_end) = html[open_start..].find('>') else {
            break;
        };
        let attributes = &html[open_start..open_start + open_end];
        let body_start = open_start + open_end + 1;
        let Some(body_end) = html[body_start..].find(CLOSE) else {
            break;
        };
        let body = &html[body_start..body_start + body_end];
        if attributes.contains("text/gtmscript") {
            match attribute_value(attributes, "data-gtmsrc") {
                Some(url) => scripts.push(GtmScript {
                    body: String::new(),
                    url: Some(url),
                }),
                None => scripts.push(GtmScript {
                    body: body.to_string(),
                    url: None,
                }),
            }
        }
        cursor = body_start + body_end + CLOSE.len();
    }
    scripts
}

fn attribute_value(attributes: &str, name: &str) -> Option<String> {
    let start = attributes.find(name)?;
    let rest = &attributes[start + name.len()..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value = &rest[1..];
    let end = value.find(quote)?;
    Some(value[..end].to_string())
}

fn render_index(container: &GtmContainer) -> String {
    let mut out = String::from("# Google Tag Manager container\n\n");
    if let Some(version) = &container.version {
        out.push_str(&format!("Container version: `{version}`\n"));
    }
    out.push_str(
        "\nCustom JS macros and Custom HTML scripts were written next to this file. Macro references\n\
         read `__gtm_macro_<n>`, where `<n>` is the index in the table below.\n\n## Macros\n\n\
         | # | function | name |\n|---|---|---|\n",
    );
    for macro_ref in &container.macros {
        out.push_str(&format!(
            "| {} | `{}` | {} |\n",
            macro_ref.index,
            macro_ref.function.as_deref().unwrap_or(""),
            macro_ref.name.as_deref().unwrap_or("")
        ));
    }
    out.push_str("\n## External scripts\n\n");
    if container.external_scripts.is_empty() {
        out.push_str("None.\n");
    } else {
        out.push_str("| tag | url |\n|---|---|\n");
        for script in &container.external_scripts {
            out.push_str(&format!("| {} | {} |\n", script.tag_id, script.url));
        }
    }
    if !container.warnings.is_empty() {
        out.push_str("\n## Warnings\n\n");
        for warning in &container.warnings {
            out.push_str(&format!("- {warning}\n"));
        }
    }
    out
}

#[derive(Default)]
struct DataObjectFinder {
    data: Option<ObjectLit>,
}

impl Visit for DataObjectFinder {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if self.data.is_none() {
            let name = match &declarator.name {
                swc_core::ecma::ast::Pat::Ident(binding) => binding.id.sym.as_str(),
                _ => "",
            };
            if name == "data" {
                if let Some(Expr::Object(object)) = declarator.init.as_deref() {
                    self.data = Some(object.clone());
                }
            }
        }
        declarator.visit_children_with(self);
    }
}

impl GtmValue {
    fn from_object(object: &ObjectLit) -> GtmValue {
        GtmValue::Object(
            object
                .props
                .iter()
                .filter_map(GtmValue::from_prop)
                .collect::<Vec<_>>(),
        )
    }

    fn from_prop(prop: &PropOrSpread) -> Option<(String, GtmValue)> {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        match prop.as_ref() {
            Prop::KeyValue(entry) => {
                let key = match &entry.key {
                    PropName::Ident(ident) => ident.sym.to_string(),
                    PropName::Str(value) => value.value.as_str()?.to_string(),
                    PropName::Num(value) => format!("{}", value.value),
                    _ => return None,
                };
                Some((key, GtmValue::from_expr(&entry.value)?))
            }
            Prop::Shorthand(ident) => Some((ident.sym.to_string(), GtmValue::Other)),
            _ => None,
        }
    }

    fn from_expr(expr: &Expr) -> Option<GtmValue> {
        match expr {
            Expr::Lit(Lit::Str(value)) => Some(GtmValue::Str(
                value
                    .value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| value.value.to_string_lossy().into_owned()),
            )),
            Expr::Lit(Lit::Num(value)) => Some(GtmValue::Num(value.value)),
            Expr::Lit(Lit::Bool(value)) => Some(GtmValue::Bool(value.value)),
            Expr::Lit(Lit::Null(_)) => Some(GtmValue::Null),
            Expr::Object(object) => Some(GtmValue::from_object(object)),
            Expr::Array(array) => Some(GtmValue::Array(
                array
                    .elems
                    .iter()
                    .map(|element| {
                        element
                            .as_ref()
                            .and_then(|element| GtmValue::from_expr(&element.expr))
                            .unwrap_or(GtmValue::Other)
                    })
                    .collect(),
            )),
            Expr::Unary(unary) if matches!(unary.op, swc_core::ecma::ast::UnaryOp::Minus) => {
                match GtmValue::from_expr(&unary.arg)? {
                    GtmValue::Num(value) => Some(GtmValue::Num(-value)),
                    _ => Some(GtmValue::Other),
                }
            }
            _ => Some(GtmValue::Other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::common::{sync::Lrc, FileName, SourceMap};
    use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};

    fn parse(source: &str) -> Module {
        let source_map: Lrc<SourceMap> = Default::default();
        let file = source_map.new_source_file(
            Lrc::new(FileName::Custom("test.js".into())),
            source.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax {
                explicit_resource_management: true,
                ..Default::default()
            }),
            Default::default(),
            StringInput::from(&*file),
            None,
        );
        Parser::new_from(lexer).parse_module().expect("parse")
    }

    /// A trimmed container: the shapes are real, the site data is not.
    const CONTAINER: &str = r#"
(function(w,d,s,l,i){w[l]=w[l]||[];
var data = {"resource":{"version":"40","macros":[
  {"function":"__jsm","vtp_javascript":["template","var a=localStorage.getItem(\"x\");return a;"]},
  {"function":"__v","vtp_name":"order_amount","vtp_dataLayerVersion":2},
  {"function":"__v","vtp_dataLayerVersion":1}
],"predicates":[{"function":"_eq"}],"rules":[{"function":"_r"}],"tags":[
  {"function":"__html","tag_id":7,"vtp_html":["template","<script type=\"text/gtmscript\">var t=\"",["escape",["macro",1],7],"\";code(",["escape",["macro",0],8],",",["escape",["macro",9],12],");</script>"]},
  {"function":"__html","tag_id":9,"vtp_html":["template","\n<script type=\"text/gtmscript\" data-gtmsrc=\"//example.com/loader.js\"></script>\n<script type=\"text/gtmscript\">beat();</script>"]},
  {"function":"__ua","tag_id":11,"vtp_trackingId":"UA-1"}
]},"runtime":"var runtime=1;"};
})(window,document);"#;

    #[test]
    fn splits_a_container_into_files() {
        let container = extract(&parse(CONTAINER)).expect("container detected");

        assert_eq!(container.version.as_deref(), Some("40"));
        let paths: Vec<&str> = container
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(
            paths,
            vec![
                "macros/jsm-0.js",
                "tags/html-7-0.js",
                // the number is the script's position inside its tag, so tag 9 starts at 1: its
                // first script is the external one, which has no body to write
                "tags/html-9-1.js",
                "index.md",
            ]
        );

        let macro_file = container
            .files
            .iter()
            .find(|file| file.path == "macros/jsm-0.js")
            .expect("macro file");
        assert_eq!(
            macro_file.source,
            "var a=localStorage.getItem(\"x\");return a;"
        );

        let html = container
            .files
            .iter()
            .find(|file| file.path == "tags/html-7-0.js")
            .expect("html file");
        // mode 7 closes the literal it sits in, mode 8 drops the identifier in as a reference, and
        // the mode the module does not understand keeps a placeholder instead of a guess
        assert_eq!(
            html.source,
            "var t=\"\" + __gtm_macro_1 + \"\";\
             code(__gtm_macro_0,/* gtm: __gtm_macro_9 (unsupported escape mode 12) */);"
        );

        let second = container
            .files
            .iter()
            .find(|file| file.path == "tags/html-9-1.js")
            .expect("second script of tag 9");
        assert_eq!(second.source, "beat();");
        assert_eq!(
            container.external_scripts,
            vec![GtmExternalScript {
                tag_id: 9,
                url: "//example.com/loader.js".to_string()
            }]
        );
        assert_eq!(container.warnings.len(), 1);
        assert!(container.warnings[0].contains("escape mode 12"));
    }

    #[test]
    fn index_names_the_macros_and_the_external_scripts() {
        let container = extract(&parse(CONTAINER)).expect("container detected");
        let index = container
            .files
            .iter()
            .find(|file| file.path == "index.md")
            .expect("index");
        assert!(index.source.contains("| 1 | `__v` | order_amount |"));
        assert!(index.source.contains("| 2 | `__v` |  |"));
        assert!(index.source.contains("| 9 | //example.com/loader.js |"));
    }

    #[test]
    fn ignores_scripts_that_are_not_gtm_rewritten() {
        let container = extract(&parse(
            r#"(function(){var data={"resource":{"version":"1","macros":[],"tags":[
                {"function":"__html","tag_id":3,"vtp_html":["template","<script>plain()</script><noscript><img src=\"//x\"></noscript>"]}
            ]}};})();"#,
        ))
        .expect("container detected");
        assert!(container
            .files
            .iter()
            .all(|file| file.path != "tags/html-3-0.js"));
    }

    #[test]
    fn a_module_without_a_resource_object_is_not_a_container() {
        assert!(extract(&parse("var data = {\"other\": 1};")).is_none());
        assert!(extract(&parse(
            "var config = {\"resource\": {\"macros\": [], \"tags\": []}};"
        ))
        .is_none());
    }
}
