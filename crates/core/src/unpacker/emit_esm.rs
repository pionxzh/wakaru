//! Shared ESM emission helpers.
//!
//! The esbuild unpacker and the heuristic scope-hoist splitter both end in
//! the same place: per-module groups of `ModuleItem`s that need synthesized
//! `import`/`export` statements and plain (map-less) code generation. This
//! module holds the pieces that are genuinely identical between them, plus
//! the case-insensitive filename-dedup probing that the merge driver and
//! SystemJS unpacker also share. Emission that needs source maps goes
//! through `emit_module_with_source_map` in `unpacker/mod.rs` instead.

use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, SourceMap};
use swc_core::ecma::ast::{
    Decl, ExportDecl, ExportNamedSpecifier, ExportSpecifier, Ident, ImportDecl,
    ImportNamedSpecifier, ImportSpecifier, Module, ModuleDecl, ModuleExportName, ModuleItem,
    NamedExport, Stmt, Str,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};

/// How [`dedup_filename`] derives the `{stem}_{n}.{ext}` probe candidates.
/// The historical call sites used two subtly different schemes; both are
/// preserved here rather than papered over.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FilenameDedupStyle {
    /// Split at the last `.` anywhere in the string (a directory prefix
    /// stays part of the stem) and default the extension to `js`; collision
    /// keys are ASCII-lowercased. Matches the CLI-facing `deduplicate_path`
    /// logic (a fourth, `Path`-based copy of this probing lives in
    /// `crate::driver::output::deduplicate_path`).
    Flat,
    /// Split stem/extension with `std::path` semantics and keep the parent
    /// directory, falling back to `{fallback_stem}` / `js` when the path has
    /// no stem or extension; collision keys are Unicode-lowercased.
    PathAware { fallback_stem: &'static str },
}

/// Case-insensitive filename dedup. Probes `filename`, then `{stem}_2.{ext}`,
/// `{stem}_3.{ext}`, ... until a name whose lowercased key is not in `seen`
/// is found. Inserts the winner's key and returns the winner.
pub(crate) fn dedup_filename(
    filename: &str,
    seen: &mut HashSet<String>,
    style: FilenameDedupStyle,
) -> String {
    let key = match style {
        FilenameDedupStyle::Flat => filename.to_ascii_lowercase(),
        FilenameDedupStyle::PathAware { .. } => filename.to_lowercase(),
    };
    if seen.insert(key) {
        return filename.to_string();
    }
    match style {
        FilenameDedupStyle::Flat => {
            let (stem, ext) = match filename.rfind('.') {
                Some(i) => (&filename[..i], &filename[i + 1..]),
                None => (filename, "js"),
            };
            for n in 2u32.. {
                let candidate = format!("{stem}_{n}.{ext}");
                if seen.insert(candidate.to_ascii_lowercase()) {
                    return candidate;
                }
            }
        }
        FilenameDedupStyle::PathAware { fallback_stem } => {
            let path = std::path::Path::new(filename);
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(fallback_stem);
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("js");
            let parent = path.parent().unwrap_or_else(|| std::path::Path::new(""));
            for n in 2u32.. {
                let candidate = parent.join(format!("{stem}_{n}.{ext}"));
                let candidate = candidate.to_string_lossy().replace('\\', "/");
                if seen.insert(candidate.to_lowercase()) {
                    return candidate;
                }
            }
        }
    }
    unreachable!("open-ended suffix search must find an unused filename")
}

/// `import { a, b } from './from'` with every local named after its import.
pub(crate) fn make_named_import_stmt(names: &[Atom], from: &str) -> ModuleItem {
    let names: Vec<(Atom, Atom)> = names
        .iter()
        .map(|name| (name.clone(), name.clone()))
        .collect();
    make_named_import_stmt_with_aliases(&names, from)
}

/// `import { imported as local, ... } from './from'`. `from` is used as-is
/// when it already starts with `.` or `/`, and gets a `./` prefix otherwise.
pub(crate) fn make_named_import_stmt_with_aliases(
    names: &[(Atom, Atom)],
    from: &str,
) -> ModuleItem {
    let specifiers = names
        .iter()
        .map(|(imported, local)| {
            ImportSpecifier::Named(ImportNamedSpecifier {
                span: Default::default(),
                local: Ident::new(local.clone(), Default::default(), Default::default()),
                imported: if imported == local {
                    None
                } else {
                    Some(ModuleExportName::Ident(Ident::new(
                        imported.clone(),
                        Default::default(),
                        Default::default(),
                    )))
                },
                is_type_only: false,
            })
        })
        .collect();
    ModuleItem::ModuleDecl(ModuleDecl::Import(ImportDecl {
        span: Default::default(),
        specifiers,
        src: Box::new(Str {
            span: Default::default(),
            value: if from.starts_with('.') || from.starts_with('/') {
                from.into()
            } else {
                format!("./{from}").into()
            },
            raw: None,
        }),
        type_only: false,
        with: None,
        phase: Default::default(),
    }))
}

/// `export { a, b };`
pub(crate) fn make_named_export_stmt(names: &[Atom]) -> ModuleItem {
    let specifiers = names
        .iter()
        .map(|name| {
            ExportSpecifier::Named(ExportNamedSpecifier {
                span: Default::default(),
                orig: ModuleExportName::Ident(Ident::new(
                    name.clone(),
                    Default::default(),
                    Default::default(),
                )),
                exported: None,
                is_type_only: false,
            })
        })
        .collect();
    ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
        span: Default::default(),
        specifiers,
        src: None,
        type_only: false,
        with: None,
    }))
}

/// Promote a top-level `function`/`class` declaration whose name is in
/// `exported` to an inline `export` declaration, returning the promoted item
/// and the names it covers. Returns `None` for any other item shape.
///
/// Only the fn/class arms are shared: the var-declaration promotion rules
/// genuinely differ between callers (the esbuild unpacker rewrites exported
/// no-op arrows to `export function` and promotes declarations all-or-nothing;
/// the scope-hoist splitter can split partially-exported declarations), so
/// those stay caller-side.
pub(crate) fn try_promote_fn_class_export(
    item: &ModuleItem,
    exported: &HashSet<Atom>,
) -> Option<(ModuleItem, Vec<Atom>)> {
    match item {
        // `function foo() {}` → `export function foo() {}`
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl)))
            if exported.contains(&fn_decl.ident.sym) =>
        {
            let names = vec![fn_decl.ident.sym.clone()];
            let new_item = ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                span: Default::default(),
                decl: Decl::Fn(fn_decl.clone()),
            }));
            Some((new_item, names))
        }
        // `class Foo {}` → `export class Foo {}`
        ModuleItem::Stmt(Stmt::Decl(Decl::Class(class_decl)))
            if exported.contains(&class_decl.ident.sym) =>
        {
            let names = vec![class_decl.ident.sym.clone()];
            let new_item = ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                span: Default::default(),
                decl: Decl::Class(class_decl.clone()),
            }));
            Some((new_item, names))
        }
        _ => None,
    }
}

/// Wrap `items` in a fresh module and emit it under `filename`.
pub(crate) fn emit_items(items: Vec<ModuleItem>, filename: String, cm: Lrc<SourceMap>) -> String {
    let module = Module {
        span: Default::default(),
        body: items,
        shebang: None,
    };
    emit_module(module, filename, cm)
}

pub(crate) fn emit_module(module: Module, filename: String, cm: Lrc<SourceMap>) -> String {
    let _fm = cm.new_source_file(FileName::Custom(filename).into(), String::new());
    emit_module_raw(&module, cm).unwrap_or_default()
}

pub(crate) fn emit_module_raw(module: &Module, cm: Lrc<SourceMap>) -> anyhow::Result<String> {
    let mut output = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: Config::default().with_minify(false),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm.clone(), "\n", &mut output, None),
        };
        emitter.emit_module(module)?;
    }
    String::from_utf8(output).map_err(|e| anyhow::anyhow!("{e}"))
}
