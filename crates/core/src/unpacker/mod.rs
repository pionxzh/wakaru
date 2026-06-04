pub mod browserify;
pub mod esbuild;
pub mod scope_hoist;
pub mod systemjs;
pub mod webpack4;
pub mod webpack5;

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, SourceMap, SyntaxContext, GLOBALS};
use swc_core::ecma::ast::{Decl, Module, ModuleDecl, ModuleItem, Stmt, VarDecl};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};

pub struct UnpackedModule {
    pub id: String,
    pub is_entry: bool,
    pub code: String,
    pub filename: String,
}

pub struct UnpackResult {
    pub modules: Vec<UnpackedModule>,
}

pub(crate) type BindingId = (Atom, SyntaxContext);

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

        let result = {
            let span = tracing::info_span!("detect_webpack5");
            let _enter = span.enter();
            webpack5::detect_from_module(&module, cm.clone())
        };
        if result.is_some() {
            return Ok(result);
        }

        let result = {
            let span = tracing::info_span!("detect_webpack5_runtime_entry");
            let _enter = span.enter();
            webpack5::detect_runtime_entry_from_module(&module, source)
        };
        if result.is_some() {
            return Ok(result);
        }

        let result = {
            let span = tracing::info_span!("detect_webpack4");
            let _enter = span.enter();
            webpack4::detect_from_module(&module, cm.clone())
        };
        if result.is_some() {
            return Ok(result);
        }

        let result = {
            let span = tracing::info_span!("detect_webpack5_chunk");
            let _enter = span.enter();
            webpack5::detect_chunk_from_module(&module, cm.clone())
        };
        if result.is_some() {
            return Ok(result);
        }

        let result = {
            let span = tracing::info_span!("detect_browserify");
            let _enter = span.enter();
            browserify::detect_from_module(&module, cm.clone())
        };
        if result.is_some() {
            return Ok(result);
        }

        let result = {
            let span = tracing::info_span!("detect_systemjs");
            let _enter = span.enter();
            systemjs::detect_from_module(&module, cm.clone())
        };
        if result.is_some() {
            return Ok(result);
        }

        let result = {
            let span = tracing::info_span!("detect_esbuild");
            let _enter = span.enter();
            esbuild::detect_from_module(&module, cm)
        };
        Ok(result)
    })
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
