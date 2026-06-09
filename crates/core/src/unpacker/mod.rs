pub mod browserify;
pub mod esbuild;
pub mod scope_hoist;
pub mod systemjs;
pub mod webpack4;
pub mod webpack5;
mod wrappers;

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, SourceMap, Span, SyntaxContext, GLOBALS};
use swc_core::ecma::ast::{Decl, Module, ModuleDecl, ModuleItem, Stmt, VarDecl};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};

#[derive(Default)]
pub struct UnpackedModule {
    pub id: String,
    pub is_entry: bool,
    pub code: String,
    pub filename: String,
    /// Byte ranges in the original input source this module was extracted
    /// from (provenance). Empty when the extraction site has no real spans
    /// (fully synthesized modules).
    pub source_ranges: Vec<(u32, u32)>,
    /// Input filename the ranges refer to. Unpackers leave this empty; the
    /// driver fills it in for multi-source unpacks.
    pub source_input: String,
}

/// Convert an AST span to a 0-based byte range into the parsed source.
///
/// Returns `None` for dummy/synthesized spans and anything that does not
/// fall inside the span's source file.
pub(crate) fn span_byte_range(cm: &SourceMap, span: Span) -> Option<(u32, u32)> {
    if span.lo.0 == 0 || span.hi.0 == 0 || span.lo > span.hi {
        return None;
    }
    let file = cm.lookup_byte_offset(span.lo).sf;
    let start = span.lo.0.checked_sub(file.start_pos.0)?;
    let end = span.hi.0.checked_sub(file.start_pos.0)?;
    (end as usize <= file.src.len()).then_some((start, end))
}

/// Collect byte ranges for a sequence of spans, sorted and coalesced
/// (overlapping or touching ranges are merged).
pub(crate) fn spans_byte_ranges(
    cm: &SourceMap,
    spans: impl Iterator<Item = Span>,
) -> Vec<(u32, u32)> {
    let mut ranges: Vec<(u32, u32)> = spans.filter_map(|s| span_byte_range(cm, s)).collect();
    ranges.sort_unstable();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for (lo, hi) in ranges {
        match out.last_mut() {
            Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
            _ => out.push((lo, hi)),
        }
    }
    out
}

pub struct UnpackResult {
    pub modules: Vec<UnpackedModule>,
    pub allow_cycle_premerge: bool,
}

impl UnpackResult {
    pub(crate) fn new(modules: Vec<UnpackedModule>) -> Self {
        Self {
            modules,
            allow_cycle_premerge: true,
        }
    }

    pub(crate) fn without_cycle_premerge(modules: Vec<UnpackedModule>) -> Self {
        Self {
            modules,
            allow_cycle_premerge: false,
        }
    }
}

pub(crate) type BindingId = (Atom, SyntaxContext);

/// Convert a bundler-provided module path into a relative output path.
///
/// This is component-based instead of replacement-based so overlapping strings
/// like `....//foo` cannot turn into `../foo` after a single sanitation pass.
pub(crate) fn sanitize_relative_path(raw: &str, fallback: &str) -> String {
    let normalized = raw.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty() && *part != "." && *part != "..")
        .collect();

    if parts.is_empty() {
        fallback.to_string()
    } else {
        parts.join("/")
    }
}

pub fn unpack_bundle(source: &str) -> Option<UnpackResult> {
    try_unpack_bundle(source).ok().flatten()
}

pub fn try_unpack_bundle(source: &str) -> anyhow::Result<Option<UnpackResult>> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = {
            let span = tracing::info_span!("parse_bundle");
            let _enter = span.enter();
            parse_es_module(source, "bundle.js", cm.clone())?
        };

        if let Some(result) = detect_bundle_candidate(&module, cm.clone(), source, true) {
            return Ok(Some(result));
        }

        let unwrapped_candidates = wrappers::collect_unwrap_candidates(&module);
        for candidate in &unwrapped_candidates {
            if let Some(result) = detect_bundle_candidate(candidate, cm.clone(), source, false) {
                return Ok(Some(result));
            }
        }

        Ok(None)
    })
}

fn detect_bundle_candidate(
    module: &Module,
    cm: Lrc<SourceMap>,
    source: &str,
    allow_runtime_entry: bool,
) -> Option<UnpackResult> {
    let result = {
        let span = tracing::info_span!("detect_webpack5");
        let _enter = span.enter();
        webpack5::detect_from_module(module, cm.clone())
    };
    if result.is_some() {
        return result;
    }

    if allow_runtime_entry {
        let result = {
            let span = tracing::info_span!("detect_webpack5_runtime_entry");
            let _enter = span.enter();
            webpack5::detect_runtime_entry_from_module(module, source)
        };
        if result.is_some() {
            return result;
        }
    }

    let result = {
        let span = tracing::info_span!("detect_webpack4");
        let _enter = span.enter();
        webpack4::detect_from_module(module, cm.clone())
    };
    if result.is_some() {
        return result;
    }

    let result = {
        let span = tracing::info_span!("detect_webpack5_chunk");
        let _enter = span.enter();
        webpack5::detect_chunk_from_module(module, cm.clone())
    };
    if result.is_some() {
        return result;
    }

    let result = {
        let span = tracing::info_span!("detect_browserify");
        let _enter = span.enter();
        browserify::detect_from_module(module, cm.clone())
    };
    if result.is_some() {
        return result;
    }

    let result = {
        let span = tracing::info_span!("detect_systemjs");
        let _enter = span.enter();
        systemjs::detect_from_module(module, cm.clone())
    };
    if result.is_some() {
        return result;
    }

    let span = tracing::info_span!("detect_esbuild");
    let _enter = span.enter();
    esbuild::detect_from_module(module, cm)
}

pub fn try_unpack_bundle_raw(source: &str) -> anyhow::Result<Option<UnpackResult>> {
    try_unpack_bundle(source)
}

pub(crate) fn parse_es_module(
    source: &str,
    filename: &str,
    cm: Lrc<SourceMap>,
) -> anyhow::Result<Module> {
    let fm = cm.new_source_file(
        FileName::Custom(filename.to_string()).into(),
        source.to_string(),
    );
    let lexer = Lexer::new(
        Syntax::Es(EsSyntax {
            jsx: true,
            ..Default::default()
        }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let parsed = parser.parse_module();
    let parser_errors: Vec<String> = parser
        .take_errors()
        .into_iter()
        .map(|error| format!("{error:?}"))
        .collect();

    match (parsed, parser_errors.is_empty()) {
        (Ok(module), _) => Ok(module),
        (Err(error), true) => Err(anyhow::anyhow!("failed to parse {filename}: {error:?}")),
        (Err(error), false) => Err(anyhow::anyhow!(
            "failed to parse {filename}: {error:?}; {}",
            parser_errors.join("; ")
        )),
    }
}

pub(crate) fn module_item_declared_names(item: &ModuleItem) -> Vec<Atom> {
    module_item_declared_binding_ids(item)
        .into_iter()
        .map(|(sym, _)| sym)
        .collect()
}

pub(crate) fn module_item_declared_binding_ids(item: &ModuleItem) -> Vec<BindingId> {
    match item {
        ModuleItem::Stmt(Stmt::Decl(decl)) => decl_declared_names(decl),
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => decl_declared_names(&export.decl),
        _ => vec![],
    }
}

fn decl_declared_names(decl: &Decl) -> Vec<BindingId> {
    match decl {
        Decl::Fn(f) => vec![(f.ident.sym.clone(), f.ident.ctxt)],
        Decl::Class(c) => vec![(c.ident.sym.clone(), c.ident.ctxt)],
        Decl::Var(var) => var_declared_names(var),
        _ => vec![],
    }
}

fn var_declared_names(var: &VarDecl) -> Vec<BindingId> {
    use swc_core::ecma::ast::Id;
    use swc_core::ecma::utils::find_pat_ids;

    let mut ids = Vec::new();
    for decl in &var.decls {
        let pat_ids: Vec<Id> = find_pat_ids(&decl.name);
        ids.extend(pat_ids);
    }
    ids
}

pub fn unpack_webpack4(source: &str) -> Option<UnpackResult> {
    webpack4::detect_and_extract(source)
}

pub fn unpack_webpack4_raw(source: &str) -> Option<UnpackResult> {
    webpack4::detect_and_extract_raw(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_relative_path_drops_only_path_components() {
        assert_eq!(
            sanitize_relative_path("....//node_modules/@wakaru/cli/bin/wakaru", "module.js"),
            "..../node_modules/@wakaru/cli/bin/wakaru"
        );
        assert_eq!(
            sanitize_relative_path(".\\..\\node_modules\\debug\\src\\index", "module.js"),
            "node_modules/debug/src/index"
        );
        assert_eq!(
            sanitize_relative_path("./src/../utils/./helper.js", "module.js"),
            "src/utils/helper.js"
        );
    }

    #[test]
    fn sanitize_relative_path_uses_fallback_for_empty_or_traversal_only_paths() {
        assert_eq!(sanitize_relative_path("", "module.js"), "module.js");
        assert_eq!(sanitize_relative_path("./", "module.js"), "module.js");
        assert_eq!(sanitize_relative_path("../../..", "module.js"), "module.js");
        assert_eq!(sanitize_relative_path("..\\..\\", "module.js"), "module.js");
    }

    #[test]
    fn try_unpack_bundle_distinguishes_parse_errors_from_non_bundles() {
        let err = match try_unpack_bundle("const = ;") {
            Ok(_) => panic!("invalid source should fail to parse"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("bundle.js"),
            "error should include parser filename: {err}"
        );

        let result = try_unpack_bundle("const value = 1;").expect("valid source should parse");
        assert!(
            result.is_none(),
            "valid non-bundle source should return None"
        );
    }
}
