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

/// The arrays `data.resource` carries in every container; together they are the detection shape.
const RESOURCE_KEYS: [&str; 4] = ["macros", "tags", "predicates", "rules"];

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

/// One entry of the container's macro array, by index, as the generated `index.md` lists it.
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
    /// Every macro that names its `function`, so a reader of `__gtm_macro_30` can answer "what is
    /// 30?".
    pub macros: Vec<GtmMacroRef>,
    /// Escape modes this module does not understand, and anything else skipped.
    pub warnings: Vec<String>,
}

/// Reads a container out of source text, for callers that have not parsed the input yet.
///
/// Every `--unpack` input passes through here, so text that cannot hold the container's keys is
/// rejected before it is parsed; the structural check in [`extract`] still decides.
pub fn extract_from_source(source: &str, filename: &str) -> Option<GtmContainer> {
    if !RESOURCE_KEYS
        .iter()
        .chain(std::iter::once(&"resource"))
        .all(|key| source.contains(key))
    {
        return None;
    }
    let source_map: Lrc<SourceMap> = Default::default();
    let module = crate::driver::io::parse_js(source, filename, source_map).ok()?;
    extract(&module)
}

/// Look for a GTM container in a parsed module and split it into readable files.
///
/// Returns `None` when the module has no `var data = {…}` whose `resource` object has `macros`,
/// `tags`, `predicates` and `rules` arrays — the object shape the format is recognizable by. Detection
/// does not look at the file name or the URL, so a container saved by hand is recognized too.
pub fn extract(module: &Module) -> Option<GtmContainer> {
    let mut finder = DataObjectFinder::default();
    module.visit_with(&mut finder);
    let data = GtmValue::from_object(&finder.data?);
    let resource = data.get("resource")?;
    if !RESOURCE_KEYS
        .iter()
        .all(|key| resource.get(key).and_then(GtmValue::as_array).is_some())
    {
        return None;
    }
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

/// Joins a template array (`["template", …]`) back into a string. A field with no macro
/// reference is stored as a plain string instead, and is returned as it is.
///
/// The escape modes say where a reference sits:
///
/// - `8, 16`: code position, so the identifier drops in as a reference.
/// - `7`: inside a string literal, where a bare identifier would become plain text and lose the
///   data flow, so the literal is closed and the reference concatenated instead.
/// - `9` (inside a regular expression literal), `3` (an HTML attribute value) and `12` (a URL
///   component): the reference is text there, and the identifier is written as that text.
///
/// Any other combination is reported as a warning rather than guessed at, and keeps the reference
/// as the identifier `__gtm_macro_<n>_escape_<modes>`: a comment would not survive the pipeline,
/// and dropping it would delete an argument or operand without a trace in the code.
fn join_template(value: &GtmValue, warnings: &mut Vec<String>) -> String {
    if let GtmValue::Str(text) = value {
        return text.clone();
    }
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
                match escape_modes(items).as_slice() {
                    [] | [8] | [16] | [8, 16] | [9] | [3] | [12] => {
                        out.push_str(&format!("__gtm_macro_{macro_index}"))
                    }
                    [7] => {
                        let quote = enclosing_quote(&out);
                        out.push_str(&format!("{quote} + __gtm_macro_{macro_index} + {quote}"));
                    }
                    modes => {
                        let modes = modes
                            .iter()
                            .map(u64::to_string)
                            .collect::<Vec<_>>()
                            .join("_");
                        warnings.push(format!(
                            "macro {macro_index} uses escape mode {modes}, which is not understood"
                        ));
                        out.push_str(&format!("__gtm_macro_{macro_index}_escape_{modes}"));
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

fn escape_modes(items: &[GtmValue]) -> Vec<u64> {
    match items.first() {
        Some(GtmValue::Str(kind)) if kind == "escape" => {
            items[2..].iter().filter_map(GtmValue::as_u64).collect()
        }
        _ => Vec::new(),
    }
}

/// The quote character of the string literal a mode-7 macro sits inside, so the substitution can
/// close and reopen it with the same one.
///
/// The scan covers only the current script — from the end of the last `<script…>` open tag, or the
/// whole text for a Custom JS body — and tracks string, comment and regular expression state, so an
/// apostrophe in the markup around the script, a quote of the other kind inside the literal, or a
/// quote or `//` inside a regular expression does not decide it. A `/` starts a regular expression
/// after punctuation that cannot end an operand; after a keyword such as `return` it is read as
/// division.
fn enclosing_quote(out: &str) -> char {
    let start = out
        .to_ascii_lowercase()
        .rfind("<script")
        .and_then(|open| out[open..].find('>').map(|end| open + end + 1))
        .unwrap_or(0);
    let mut chars = out[start..].chars().peekable();
    let mut quote = None;
    // the last character of code outside strings, comments and whitespace
    let mut previous = '(';
    while let Some(c) = chars.next() {
        if let Some(open) = quote {
            if c == '\\' {
                chars.next();
            } else if c == open {
                quote = None;
                previous = c;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => quote = Some(c),
            '/' if chars.peek() == Some(&'/') => {
                while chars.next_if(|next| *next != '\n').is_some() {}
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut last = '\0';
                for next in chars.by_ref() {
                    if last == '*' && next == '/' {
                        break;
                    }
                    last = next;
                }
            }
            '/' if "(,=:[!&|?{};+-*%<>~^".contains(previous) => {
                let mut in_class = false;
                while let Some(next) = chars.next() {
                    match next {
                        '\\' => {
                            chars.next();
                        }
                        '[' => in_class = true,
                        ']' => in_class = false,
                        '/' if !in_class => break,
                        '\n' => break,
                        _ => {}
                    }
                }
                previous = ')';
            }
            c if c.is_whitespace() => {}
            c => previous = c,
        }
    }
    quote.unwrap_or('"')
}

struct GtmScript {
    body: String,
    url: Option<String>,
}

/// `type` values a browser runs as classic or module JavaScript, besides GTM's own
/// `text/gtmscript` and a missing `type`.
const JAVASCRIPT_TYPES: [&str; 6] = [
    "text/javascript",
    "application/javascript",
    "application/x-javascript",
    "text/ecmascript",
    "application/ecmascript",
    "module",
];

/// Picks the JavaScript `<script>` elements out of a Custom HTML tag. GTM rewrites most of them to
/// `type="text/gtmscript"` and moves an external one's `src` into `data-gtmsrc`, but tags also keep
/// plain `<script>` and `<script src>` elements, so both forms are taken. An external script has no
/// body and its URL goes to the index, which is why the file numbers — the scripts' positions in the
/// tag — can have gaps. Scripts of another `type` (JSON, templates) and other elements, such as the
/// `<noscript>` fallbacks, are left alone.
fn split_gtm_scripts(html: &str) -> Vec<GtmScript> {
    // ASCII lowercasing keeps byte offsets, so positions found in `lower` index `html` too
    let lower = html.to_ascii_lowercase();
    let mut scripts = Vec::new();
    let mut cursor = 0;
    while let Some(start) = lower[cursor..].find("<script") {
        let open_start = cursor + start;
        let name_end = open_start + "<script".len();
        // `<scripts>` or `<script-loader>` is another element
        if !lower[name_end..].starts_with(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
        {
            cursor = name_end;
            continue;
        }
        let Some(open_end) = html[name_end..].find('>') else {
            break;
        };
        let attributes = parse_attributes(&html[name_end..name_end + open_end]);
        let body_start = name_end + open_end + 1;
        let body_end = lower[body_start..]
            .find("</script")
            .map_or(html.len(), |end| body_start + end);
        cursor = lower[body_end..]
            .find('>')
            .map_or(html.len(), |end| body_end + end + 1);

        let attribute = |name: &str| {
            attributes
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.trim())
        };
        let is_javascript = match attribute("type") {
            None | Some("") => true,
            Some(kind) => {
                let kind = kind.to_ascii_lowercase();
                kind == "text/gtmscript" || JAVASCRIPT_TYPES.contains(&kind.as_str())
            }
        };
        if !is_javascript {
            continue;
        }
        match attribute("data-gtmsrc").or_else(|| attribute("src")) {
            Some(url) => scripts.push(GtmScript {
                body: String::new(),
                url: Some(url.to_string()),
            }),
            None => scripts.push(GtmScript {
                body: html[body_start..body_end].to_string(),
                url: None,
            }),
        }
    }
    scripts
}

/// The attributes of an open tag, names lowercased, with `""` for one that has no value.
fn parse_attributes(source: &str) -> Vec<(String, String)> {
    let mut attributes = Vec::new();
    let mut rest = source;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '/');
        if rest.is_empty() {
            break;
        }
        let name_end = rest
            .find(|c: char| c.is_ascii_whitespace() || c == '=' || c == '/')
            .unwrap_or(rest.len());
        let name = rest[..name_end].to_ascii_lowercase();
        rest = rest[name_end..].trim_start();
        let mut value = String::new();
        if let Some(after) = rest.strip_prefix('=') {
            let after = after.trim_start();
            match after.chars().next() {
                Some(quote @ ('"' | '\'')) => {
                    let inner = &after[1..];
                    let end = inner.find(quote).unwrap_or(inner.len());
                    value = inner[..end].to_string();
                    rest = inner.get(end + 1..).unwrap_or("");
                }
                _ => {
                    let end = after
                        .find(|c: char| c.is_ascii_whitespace())
                        .unwrap_or(after.len());
                    value = after[..end].to_string();
                    rest = &after[end..];
                }
            }
        }
        if !name.is_empty() {
            attributes.push((name, value));
        }
    }
    attributes
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
  {"function":"__jsm","vtp_javascript":["template","(function(){var a=localStorage.getItem(\"x\");return a})();"]},
  {"function":"__v","vtp_name":"order_amount","vtp_dataLayerVersion":2},
  {"function":"__v","vtp_dataLayerVersion":1}
],"predicates":[{"function":"_eq"}],"rules":[{"function":"_r"}],"tags":[
  {"function":"__html","tag_id":7,"vtp_html":["template","<script type=\"text/gtmscript\">var t=\"",["escape",["macro",1],7],"\";code(",["escape",["macro",0],8],",",["escape",["macro",9],2],");</script>"]},
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
            "(function(){var a=localStorage.getItem(\"x\");return a})();"
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
            "var t=\"\" + __gtm_macro_1 + \"\";code(__gtm_macro_0,__gtm_macro_9_escape_2);"
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
        assert!(container.warnings[0].contains("escape mode 2"));
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
    fn a_module_without_a_resource_object_is_not_a_container() {
        assert!(extract(&parse("var data = {\"other\": 1};")).is_none());
        assert!(extract(&parse(
            "var config = {\"resource\": {\"macros\": [], \"tags\": [], \"predicates\": [], \"rules\": []}};"
        ))
        .is_none());
        // `macros` and `tags` alone are ordinary words for application data
        assert!(extract(&parse(
            "var data = {\"resource\": {\"macros\": [], \"tags\": []}};"
        ))
        .is_none());
    }

    #[test]
    fn text_without_the_resource_keys_is_rejected_before_parsing() {
        // not valid JavaScript: a parse would fail too, but the key check answers first
        assert!(
            extract_from_source("var data = {resource: {macros: [], tags: [}", "x.js").is_none()
        );
        assert!(extract_from_source(CONTAINER, "gtm.js").is_some());
    }

    fn html_tag_source(container: &str) -> String {
        extract(&parse(container))
            .expect("container detected")
            .files
            .into_iter()
            .find(|file| file.path.starts_with("tags/"))
            .expect("tag file")
            .source
    }

    #[test]
    fn mode_7_uses_the_quote_that_opened_the_literal() {
        // the single-quoted literal holds a double quote before the reference, and the markup
        // around the script holds an apostrophe; neither decides the enclosing quote
        let source = html_tag_source(
            r#"var data={"resource":{"macros":[{"function":"__v"}],"predicates":[],"rules":[],"tags":[
                {"function":"__html","tag_id":4,"vtp_html":["template","<p>don't</p><script type=\"text/gtmscript\">/* it's */ var t='say \"hi\" ",["escape",["macro",0],7],"';</script>"]}
            ]}};"#,
        );
        assert_eq!(
            source,
            "/* it's */ var t='say \"hi\" ' + __gtm_macro_0 + '';"
        );
    }

    #[test]
    fn mode_7_after_an_earlier_substitution_stays_in_the_same_literal() {
        let source = html_tag_source(
            r#"var data={"resource":{"macros":[{"function":"__v"}],"predicates":[],"rules":[],"tags":[
                {"function":"__html","tag_id":4,"vtp_html":["template","<script type=\"text/gtmscript\">var u='a=",["escape",["macro",0],7],"&b=",["escape",["macro",0],7],"';</script>"]}
            ]}};"#,
        );
        assert_eq!(
            source,
            "var u='a=' + __gtm_macro_0 + '&b=' + __gtm_macro_0 + '';"
        );
    }

    fn gtm_tag(vtp_html: &str) -> String {
        format!(
            r#"var data={{"resource":{{"macros":[{{"function":"__v"}}],"predicates":[],"rules":[],"tags":[
                {{"function":"__html","tag_id":4,"vtp_html":{vtp_html}}}
            ]}}}};"#
        )
    }

    fn tag_files(container: &GtmContainer) -> Vec<(&str, &str)> {
        container
            .files
            .iter()
            .filter(|file| file.path.starts_with("tags/"))
            .map(|file| (file.path.as_str(), file.source.as_str()))
            .collect()
    }

    #[test]
    fn a_field_without_macro_references_is_a_plain_string() {
        // wild-observed: a Custom HTML tag that references no macro stores `vtp_html` as a string,
        // not a template array
        let container = extract(&parse(&gtm_tag(
            r#""<script type=\"text/gtmscript\">track();</script>""#,
        )))
        .expect("container detected");
        assert_eq!(
            tag_files(&container),
            vec![("tags/html-4-0.js", "track();")]
        );
    }

    #[test]
    fn plain_script_elements_are_taken_too() {
        // wild-observed: GTM leaves some `<script>` and `<script src>` elements unrewritten
        let container = extract(&parse(&gtm_tag(
            r#""<SCRIPT>a();</SCRIPT><script type=\"text/javascript\">b();</script><script async src=\"https://cdn.example/x.js\"></script><script type=\"application/ld+json\">{}</script><scripts>c()</scripts><noscript><img src=\"//p\"></noscript>""#,
        )))
        .expect("container detected");
        assert_eq!(
            tag_files(&container),
            vec![("tags/html-4-0.js", "a();"), ("tags/html-4-1.js", "b();")]
        );
        assert_eq!(
            container.external_scripts,
            vec![GtmExternalScript {
                tag_id: 4,
                url: "https://cdn.example/x.js".to_string()
            }]
        );
    }

    #[test]
    fn regex_attribute_and_url_modes_write_the_reference_as_text() {
        // wild-observed: mode 9 sits inside a regular expression literal, 3 inside an HTML
        // attribute value and 12 inside a URL; none is code position, and none is a warning
        let container = extract(&parse(&gtm_tag(
            r#"["template","<img src=\"//p.example/?a=",["escape",["macro",0],12],"\" alt=\"",["escape",["macro",0],3],"\"><script>s=s.replace(/",["escape",["macro",0],9],"/i,\"x\");</script>"]"#,
        )))
        .expect("container detected");
        assert_eq!(
            tag_files(&container),
            vec![("tags/html-4-0.js", "s=s.replace(/__gtm_macro_0/i,\"x\");")]
        );
        assert!(container.warnings.is_empty(), "{:?}", container.warnings);
    }

    #[test]
    fn an_unknown_mode_combination_names_every_mode() {
        let container = extract(&parse(&gtm_tag(
            r#"["template","<script>f(",["escape",["macro",0],7,12],");</script>"]"#,
        )))
        .expect("container detected");
        assert_eq!(
            tag_files(&container),
            vec![("tags/html-4-0.js", "f(__gtm_macro_0_escape_7_12);")]
        );
        assert!(container.warnings[0].contains("escape mode 7_12"));
    }

    #[test]
    fn mode_7_skips_quotes_and_slashes_inside_a_regular_expression() {
        // a `//` and a quote inside the regex must not read as a comment or a string
        let source = html_tag_source(
            r#"var data={"resource":{"macros":[{"function":"__v"}],"predicates":[],"rules":[],"tags":[
                {"function":"__html","tag_id":4,"vtp_html":["template","<script type=\"text/gtmscript\">if(/^https?:\\/\\/[\"']/.test(u))x='a",["escape",["macro",0],7],"';</script>"]}
            ]}};"#,
        );
        assert_eq!(
            source,
            r#"if(/^https?:\/\/["']/.test(u))x='a' + __gtm_macro_0 + '';"#
        );
    }
}
