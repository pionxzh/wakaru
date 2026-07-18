pub mod amd;
pub mod browserify;
pub mod closure_module_manager;
pub mod esbuild;
pub mod metro;
pub mod scope_hoist;
pub mod systemjs;
pub mod webpack4;
pub mod webpack5;
mod wrappers;

use std::panic::{self, AssertUnwindSafe};

use swc_core::atoms::Atom;
use swc_core::common::{
    sync::Lrc, BytePos, FileName, Globals, LineCol, Mark, SourceMap, Span, Spanned, SyntaxContext,
    GLOBALS,
};
use swc_core::ecma::ast::{Decl, Module, ModuleDecl, ModuleItem, Stmt, VarDecl};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
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
    /// Mapping points from this module's emitted code back to the original
    /// input source. Used internally to compose provenance when this emitted
    /// module is split again.
    pub generated_source_map: Vec<GeneratedSourceMapPoint>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GeneratedSourceMapPoint {
    pub generated_offset: u32,
    pub source_offset: u32,
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

pub(crate) fn source_fallback_for_stmts(cm: &SourceMap, statements: &[Stmt]) -> String {
    let (Some(first), Some(last)) = (statements.first(), statements.last()) else {
        return String::new();
    };
    let first_span = first.span();
    let last_span = last.span();
    if first_span.lo.0 == 0 || last_span.hi.0 == 0 || first_span.lo > last_span.hi {
        return String::new();
    }
    let file = cm.lookup_byte_offset(first_span.lo).sf;
    let start = first_span.lo.0.saturating_sub(file.start_pos.0) as usize;
    let end = last_span.hi.0.saturating_sub(file.start_pos.0) as usize;
    file.src.get(start..end).unwrap_or_default().to_string()
}

pub(crate) fn generated_source_map_points(
    generated_code: &str,
    cm: &SourceMap,
    mappings: &[(BytePos, LineCol)],
) -> Vec<GeneratedSourceMapPoint> {
    let line_starts = line_start_offsets(generated_code);
    let mut points = mappings
        .iter()
        .filter_map(|(source_pos, generated_pos)| {
            if source_pos.0 == 0 {
                return None;
            }
            let generated_offset =
                line_col_to_byte_offset(&line_starts, generated_code, generated_pos)?;
            let file = cm.lookup_byte_offset(*source_pos).sf;
            let source_offset = source_pos.0.checked_sub(file.start_pos.0)?;
            (source_offset as usize <= file.src.len()).then_some(GeneratedSourceMapPoint {
                generated_offset,
                source_offset,
            })
        })
        .collect::<Vec<_>>();

    points.sort_unstable_by_key(|point| (point.generated_offset, point.source_offset));
    points.dedup();
    points
}

fn line_start_offsets(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (idx, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

fn line_col_to_byte_offset(line_starts: &[usize], source: &str, loc: &LineCol) -> Option<u32> {
    let line_start = *line_starts.get(loc.line as usize)?;
    let line_end = source[line_start..]
        .find('\n')
        .map(|offset| line_start + offset)
        .unwrap_or(source.len());
    let line = &source[line_start..line_end];
    let mut utf16_col = 0u32;
    let mut byte_col = 0usize;
    for ch in line.chars() {
        if utf16_col == loc.col {
            break;
        }
        utf16_col = utf16_col.checked_add(ch.len_utf16() as u32)?;
        byte_col = byte_col.checked_add(ch.len_utf8())?;
        if utf16_col > loc.col {
            return None;
        }
    }
    if utf16_col != loc.col {
        return None;
    }
    let offset = line_start.checked_add(byte_col)?;
    (offset <= source.len()).then_some(offset as u32)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BundleFormat {
    Webpack5,
    Webpack4,
    Browserify,
    ClosureModuleManager,
    SystemJs,
    Esbuild,
    Metro,
    Amd,
    ScopeHoisted,
}

impl BundleFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webpack5 => "webpack5",
            Self::Webpack4 => "webpack4",
            Self::Browserify => "browserify",
            Self::ClosureModuleManager => "closure-module-manager",
            Self::SystemJs => "systemjs",
            Self::Esbuild => "esbuild",
            Self::Metro => "metro",
            Self::Amd => "amd",
            Self::ScopeHoisted => "scope-hoisted",
        }
    }
}

pub struct UnpackResult {
    pub modules: Vec<UnpackedModule>,
    pub allow_cycle_premerge: bool,
    pub format: BundleFormat,
}

/// Detector-owned AST that has completed bundler-specific normalization.
///
/// This is private to the core pipeline. Public/raw unpack APIs materialize it
/// into `UnpackedModule::code`; the normal driver can instead consume it at the
/// Phase 1 boundary and avoid the intermediate emit/parse round trip.
pub(crate) struct PreparedModuleAst {
    pub(crate) globals: Globals,
    pub(crate) module: Module,
    pub(crate) unresolved_mark: Mark,
}

/// Internal detector result. `prepared` is always aligned one-for-one with
/// `result.modules`; a `None` entry means that module is source-only.
pub(crate) struct DetectedBundle {
    pub(crate) result: UnpackResult,
    pub(crate) prepared: Vec<Option<PreparedModuleAst>>,
    materialize_cm: Option<Lrc<SourceMap>>,
}

impl DetectedBundle {
    pub(crate) fn from_result(result: UnpackResult) -> Self {
        let prepared = std::iter::repeat_with(|| None)
            .take(result.modules.len())
            .collect();
        Self {
            result,
            prepared,
            materialize_cm: None,
        }
    }

    pub(crate) fn new(
        result: UnpackResult,
        prepared: Vec<Option<PreparedModuleAst>>,
        materialize_cm: Lrc<SourceMap>,
    ) -> Self {
        assert_eq!(
            result.modules.len(),
            prepared.len(),
            "prepared AST sidecar must align with unpacked modules"
        );
        Self {
            result,
            prepared,
            materialize_cm: Some(materialize_cm),
        }
    }

    pub(crate) fn into_parts(self) -> (UnpackResult, Vec<Option<PreparedModuleAst>>) {
        (self.result, self.prepared)
    }

    pub(crate) fn materialize(mut self) -> anyhow::Result<UnpackResult> {
        let cm = self.materialize_cm.take();
        for (module, prepared) in self.result.modules.iter_mut().zip(self.prepared) {
            let Some(prepared) = prepared else {
                continue;
            };
            let cm = cm
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("prepared module is missing its source map"))?;
            let (code, generated_source_map) = prepared.materialize(cm.clone())?;
            module.code = code;
            module.generated_source_map = generated_source_map;
        }
        Ok(self.result)
    }
}

impl From<UnpackResult> for DetectedBundle {
    fn from(result: UnpackResult) -> Self {
        Self::from_result(result)
    }
}

impl PreparedModuleAst {
    pub(crate) fn materialize(
        self,
        cm: Lrc<SourceMap>,
    ) -> anyhow::Result<(String, Vec<GeneratedSourceMapPoint>)> {
        let Self {
            globals, module, ..
        } = self;
        GLOBALS.set(&globals, || {
            let span = tracing::info_span!("unpacker: prepared emit");
            let _enter = span.enter();
            emit_module_with_source_map(&module, cm)
        })
    }
}

pub(crate) fn emit_module_with_source_map(
    module: &Module,
    cm: Lrc<SourceMap>,
) -> anyhow::Result<(String, Vec<GeneratedSourceMapPoint>)> {
    let mut output = Vec::new();
    let mut srcmap_buf = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: Config::default().with_minify(false),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm.clone(), "\n", &mut output, Some(&mut srcmap_buf)),
        };
        emitter
            .emit_module(module)
            .map_err(|error| anyhow::anyhow!("emit error: {error:?}"))?;
    }
    let code = String::from_utf8(output).map_err(|error| anyhow::anyhow!("utf8 error: {error}"))?;
    let mappings = generated_source_map_points(&code, &cm, &srcmap_buf);
    Ok((code, mappings))
}

impl UnpackResult {
    pub(crate) fn new(modules: Vec<UnpackedModule>, format: BundleFormat) -> Self {
        Self {
            modules,
            allow_cycle_premerge: true,
            format,
        }
    }

    pub(crate) fn without_cycle_premerge(
        modules: Vec<UnpackedModule>,
        format: BundleFormat,
    ) -> Self {
        Self {
            modules,
            allow_cycle_premerge: false,
            format,
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
    try_prepare_bundle(source)?
        .map(DetectedBundle::materialize)
        .transpose()
}

pub(crate) fn try_prepare_bundle(source: &str) -> anyhow::Result<Option<DetectedBundle>> {
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

        let result = {
            let span = tracing::info_span!("detect_amd");
            let _enter = span.enter();
            amd::detect_from_module(&module, cm)
        };
        Ok(result.map(DetectedBundle::from_result))
    })
}

fn detect_bundle_candidate(
    module: &Module,
    cm: Lrc<SourceMap>,
    source: &str,
    allow_runtime_entry: bool,
) -> Option<DetectedBundle> {
    let result = {
        let span = tracing::info_span!("detect_webpack5");
        let _enter = span.enter();
        webpack5::detect_from_module_prepared(module, cm.clone())
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
            return result.map(DetectedBundle::from_result);
        }
    }

    let result = {
        let span = tracing::info_span!("detect_webpack4");
        let _enter = span.enter();
        webpack4::detect_from_module(module, cm.clone())
    };
    if result.is_some() {
        return result.map(DetectedBundle::from_result);
    }

    let result = {
        let span = tracing::info_span!("detect_webpack5_chunk");
        let _enter = span.enter();
        webpack5::detect_chunk_from_module_prepared(module, cm.clone())
    };
    if result.is_some() {
        return result;
    }

    let result = {
        let span = tracing::info_span!("detect_browserify");
        let _enter = span.enter();
        browserify::detect_from_module_prepared(module, cm.clone())
    };
    if result.is_some() {
        return result;
    }

    let result = {
        let span = tracing::info_span!("detect_closure_module_manager");
        let _enter = span.enter();
        closure_module_manager::detect_from_module(module, cm.clone(), source)
    };
    if result.is_some() {
        return result.map(DetectedBundle::from_result);
    }

    let result = {
        let span = tracing::info_span!("detect_systemjs");
        let _enter = span.enter();
        systemjs::detect_from_module(module, cm.clone())
    };
    if result.is_some() {
        return result.map(DetectedBundle::from_result);
    }

    let result = {
        let span = tracing::info_span!("detect_esbuild");
        let _enter = span.enter();
        esbuild::detect_from_module_with_source(module, Some(source), cm.clone())
    };
    if result.is_some() {
        return result.map(DetectedBundle::from_result);
    }

    let span = tracing::info_span!("detect_metro");
    let _enter = span.enter();
    metro::detect_from_module_prepared(module, cm)
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
    let parsed = match panic::catch_unwind(AssertUnwindSafe(|| parser.parse_module())) {
        Ok(result) => result,
        Err(_) => return Err(anyhow::anyhow!("SWC parser panicked on {filename}")),
    };
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
    fn source_fallback_rejects_empty_and_dummy_statement_ranges() {
        assert!(source_fallback_for_stmts(&SourceMap::default(), &[]).is_empty());
        let statements = [Stmt::Empty(swc_core::ecma::ast::EmptyStmt {
            span: Default::default(),
        })];
        assert!(source_fallback_for_stmts(&SourceMap::default(), &statements).is_empty());
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
