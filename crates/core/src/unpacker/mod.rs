pub mod browserify;
pub mod esbuild;
pub mod scope_hoist;
pub mod webpack4;
pub mod webpack5;

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, SourceMap, SyntaxContext, GLOBALS};
use swc_core::ecma::ast::{Decl, Module, ModuleDecl, ModuleItem, Pat, Stmt, VarDecl};
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
        let module = parse_es_module(source, "bundle.js", cm.clone())?;

        Ok(webpack5::detect_from_module(&module, cm.clone())
            .or_else(|| webpack4::detect_from_module(&module, cm.clone()))
            .or_else(|| webpack5::detect_chunk_from_module(&module, cm.clone()))
            .or_else(|| browserify::detect_from_module(&module, cm.clone()))
            .or_else(|| esbuild::detect_from_module(&module, cm)))
    })
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
        (Ok(module), true) => Ok(module),
        (Ok(_), false) => Err(anyhow::anyhow!(
            "failed to parse {filename}: {}",
            parser_errors.join("; ")
        )),
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
    let mut ids = Vec::new();
    for decl in &var.decls {
        collect_pat_binding_ids(&decl.name, &mut ids);
    }
    ids
}

fn collect_pat_binding_ids(pat: &Pat, out: &mut Vec<BindingId>) {
    match pat {
        Pat::Ident(bi) => out.push((bi.id.sym.clone(), bi.id.ctxt)),
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_pat_binding_ids(elem, out);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                        collect_pat_binding_ids(&kv.value, out);
                    }
                    swc_core::ecma::ast::ObjectPatProp::Assign(assign) => {
                        out.push((assign.key.sym.clone(), assign.key.ctxt));
                    }
                    swc_core::ecma::ast::ObjectPatProp::Rest(rest) => {
                        collect_pat_binding_ids(&rest.arg, out);
                    }
                }
            }
        }
        Pat::Rest(rest) => collect_pat_binding_ids(&rest.arg, out),
        Pat::Assign(assign) => collect_pat_binding_ids(&assign.left, out),
        _ => {}
    }
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
