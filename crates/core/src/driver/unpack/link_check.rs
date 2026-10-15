//! Link check for unpack output: an ES module output that imports a name its
//! sibling ES module output does not export fails to link in Node
//! (`does not provide an export named …`), so none of the module graph runs.
//!
//! Each module contributes a [`LinkReport`] from its final Phase 2 AST, so
//! nothing is parsed again. A sibling with no `import`/`export` is CommonJS to
//! Node's syntax detection, and any name may come from its `module.exports`;
//! it is not checked. A name that may come through `export *` is not checked
//! either, but `default` never does, so it still is.
//! [`crate::output_validate`] performs the full check for `debug validate`.

use crate::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    Decl, ExportSpecifier, ImportSpecifier, Module, ModuleDecl, ModuleExportName, ModuleItem,
};
use swc_core::ecma::utils::find_pat_ids;

use super::super::types::{UnpackWarning, UnpackWarningKind};
use crate::module_path::resolve_relative_specifier;

/// A module's own export names and the names it imports from relative
/// sources, from its final AST. The default, for a module kept as raw code,
/// is neither checked nor checked against.
#[derive(Default)]
pub(super) struct LinkReport {
    /// Any top-level `import` or `export`: Node loads the file as ESM.
    is_esm: bool,
    exports: HashSet<Atom>,
    has_export_all: bool,
    /// `(source, names)` of each `import { … } from` / `import x from` and
    /// `export { … } from`; `default` for a default import.
    imports: Vec<(String, Vec<Atom>)>,
}

pub(super) fn collect_link_report(module: &Module) -> LinkReport {
    let mut report = LinkReport {
        is_esm: false,
        exports: HashSet::default(),
        has_export_all: false,
        imports: Vec::new(),
    };
    for item in &module.body {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        report.is_esm = true;
        match decl {
            ModuleDecl::Import(import) if !import.type_only => {
                let names: Vec<Atom> = import
                    .specifiers
                    .iter()
                    .filter_map(|specifier| match specifier {
                        ImportSpecifier::Default(_) => Some("default".into()),
                        ImportSpecifier::Named(named) if !named.is_type_only => Some(
                            named
                                .imported
                                .as_ref()
                                .map_or_else(|| named.local.sym.clone(), export_name),
                        ),
                        _ => None,
                    })
                    .collect();
                if let (false, Some(src)) = (names.is_empty(), import.src.value.as_str()) {
                    report.imports.push((src.to_string(), names));
                }
            }
            ModuleDecl::ExportDecl(export) => match &export.decl {
                Decl::Var(var) => {
                    for declarator in &var.decls {
                        let ids: Vec<swc_core::ecma::ast::Id> = find_pat_ids(&declarator.name);
                        report.exports.extend(ids.into_iter().map(|(sym, _)| sym));
                    }
                }
                Decl::Fn(function) => {
                    report.exports.insert(function.ident.sym.clone());
                }
                Decl::Class(class) => {
                    report.exports.insert(class.ident.sym.clone());
                }
                _ => {}
            },
            ModuleDecl::ExportNamed(named) if !named.type_only => {
                let mut reexported = Vec::new();
                for specifier in &named.specifiers {
                    match specifier {
                        ExportSpecifier::Named(specifier) => {
                            let exported = specifier.exported.as_ref().unwrap_or(&specifier.orig);
                            report.exports.insert(export_name(exported));
                            reexported.push(export_name(&specifier.orig));
                        }
                        ExportSpecifier::Namespace(specifier) => {
                            report.exports.insert(export_name(&specifier.name));
                        }
                        ExportSpecifier::Default(specifier) => {
                            report.exports.insert(specifier.exported.sym.clone());
                            reexported.push("default".into());
                        }
                    }
                }
                if let (false, Some(src)) = (
                    reexported.is_empty(),
                    named.src.as_ref().and_then(|src| src.value.as_str()),
                ) {
                    report.imports.push((src.to_string(), reexported));
                }
            }
            ModuleDecl::ExportDefaultDecl(_) | ModuleDecl::ExportDefaultExpr(_) => {
                report.exports.insert("default".into());
            }
            ModuleDecl::ExportAll(_) => report.has_export_all = true,
            _ => {}
        }
    }
    report
}

fn export_name(name: &ModuleExportName) -> Atom {
    match name {
        ModuleExportName::Ident(ident) => ident.sym.clone(),
        ModuleExportName::Str(str) => Atom::from(str.value.as_str().unwrap_or_default()),
    }
}

/// One warning per importing module and source, naming every import that
/// the sibling ES module output does not export.
pub(super) fn collect_missing_imported_name_warnings(
    reports: &[(&str, &LinkReport)],
) -> Vec<UnpackWarning> {
    let by_filename: HashMap<&str, &LinkReport> = reports.iter().copied().collect();
    let mut warnings = Vec::new();
    for (filename, report) in reports {
        if !report.is_esm {
            continue;
        }
        for (source, names) in &report.imports {
            let Some(target) = resolve_relative_specifier(filename, source) else {
                continue;
            };
            let Some(provider) = by_filename.get(target.as_str()) else {
                continue;
            };
            if !provider.is_esm {
                continue;
            }
            let missing: Vec<String> = names
                .iter()
                .filter(|name| {
                    !provider.exports.contains(*name)
                        && (!provider.has_export_all || name.as_ref() == "default")
                })
                .map(|name| format!("`{name}`"))
                .collect();
            if missing.is_empty() {
                continue;
            }
            warnings.push(UnpackWarning::new(
                *filename,
                UnpackWarningKind::MissingImportedName,
                format!(
                    "imports {} from \"{source}\", which does not export {}; the module fails to link when it loads",
                    missing.join(", "),
                    if missing.len() == 1 { "it" } else { "them" },
                ),
            ));
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::io::parse_js;
    use swc_core::common::GLOBALS;

    fn warnings(modules: &[(&str, &str)]) -> Vec<UnpackWarning> {
        GLOBALS.set(&Default::default(), || {
            let reports: Vec<(&str, LinkReport)> = modules
                .iter()
                .map(|(filename, code)| {
                    let module = parse_js(code, filename, Default::default())
                        .unwrap_or_else(|_| panic!("{filename} parses"));
                    (*filename, collect_link_report(&module))
                })
                .collect();
            let refs: Vec<(&str, &LinkReport)> = reports
                .iter()
                .map(|(name, report)| (*name, report))
                .collect();
            collect_missing_imported_name_warnings(&refs)
        })
    }

    #[test]
    fn reports_a_default_import_of_an_es_module_without_one() {
        let found = warnings(&[
            ("a.js", "import p from \"./p.js\";\np.f = 1;\n"),
            ("p.js", "export const f = 0;\n"),
        ]);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].filename, "a.js");
        assert_eq!(found[0].kind, UnpackWarningKind::MissingImportedName);
        assert!(found[0].message.contains("`default` from \"./p.js\""));
    }

    #[test]
    fn reports_missing_named_imports_and_reexports() {
        let found = warnings(&[
            (
                "a.js",
                "import { f, g } from \"./p.js\";\nexport { h } from \"./p.js\";\n",
            ),
            ("p.js", "export function f() {}\n"),
        ]);
        let messages: Vec<&str> = found.iter().map(|w| w.message.as_str()).collect();
        assert_eq!(messages.len(), 2, "{messages:#?}");
        assert!(messages[0].contains("`g` from"), "{messages:#?}");
        assert!(messages[1].contains("`h` from"), "{messages:#?}");
    }

    #[test]
    fn a_commonjs_sibling_or_an_export_star_name_is_not_checked() {
        // Node's syntax detection loads a file without import/export as
        // CommonJS, whose names come from `module.exports`.
        assert!(warnings(&[
            ("a.js", "import p, { f } from \"./p.js\";\n"),
            ("p.js", "module.exports = { f: 1 };\n"),
        ])
        .is_empty());
        // A name may come through `export *`; `default` never does.
        let found = warnings(&[
            ("a.js", "import p, { f } from \"./p.js\";\n"),
            ("p.js", "export * from \"./q.js\";\n"),
            ("q.js", "export const f = 1;\n"),
        ]);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert!(found[0].message.contains("imports `default` from"));
    }

    #[test]
    fn found_exports_and_external_sources_are_not_reported() {
        assert!(warnings(&[
            (
                "a.js",
                "import p, { f as g, \"x y\" as z } from \"./p.js\";\nimport e from \"ext\";\nimport * as ns from \"./p.js\";\n",
            ),
            (
                "p.js",
                "const f = 1, z = 2;\nexport { f, z as \"x y\" };\nexport default f;\n",
            ),
        ])
        .is_empty());
    }
}
