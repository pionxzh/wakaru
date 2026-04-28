//! Post-Stage-2 fact extraction.
//!
//! After Stage 2 (helper unwrapping + module-system reconstruction), the AST has
//! clean `import`/`export` declarations. This module extracts a per-module fact
//! summary from that reconstructed form.
//!
//! These facts are the foundation for cross-module analysis in the multi-module
//! `unpack()` path. Single-file `decompile()` does not use them.

use std::fmt;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    Callee, Decl, DefaultDecl, Expr, ExportSpecifier, ImportSpecifier, Lit, Module, ModuleDecl,
    ModuleItem,
};

/// How a binding was imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportKind {
    /// `import x from "..."`
    Default,
    /// `import * as x from "..."`
    Namespace,
    /// `import { foo } from "..."` or `import { foo as bar } from "..."`
    Named(Atom),
}

/// One import binding extracted from the post-Stage-2 AST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFact {
    /// The local binding name (what the module uses internally).
    pub local: Atom,
    /// The module specifier.
    pub source: Atom,
    /// How it was imported.
    pub kind: ImportKind,
}

/// How a binding was exported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportKind {
    /// `export default ...`
    Default,
    /// `export function foo() {}`, `export const foo = ...`, or `export { foo }`
    Named,
}

/// One export extracted from the post-Stage-2 AST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportFact {
    /// The exported name (what consumers import by).
    /// `"default"` for default exports.
    pub exported: Atom,
    /// The local binding name, if any.
    /// For `export { foo as bar }`, local is `foo` and exported is `bar`.
    /// For `export default expr`, local is `None`.
    pub local: Option<Atom>,
    /// How it was exported.
    pub kind: ExportKind,
}

/// Facts extracted from one module after Stage 2.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleFacts {
    pub imports: Vec<ImportFact>,
    pub exports: Vec<ExportFact>,
    /// If this module is a passthrough re-export (`export default require("./X.js")`),
    /// this is the target module specifier. Importers can be redirected to the target.
    pub passthrough_target: Option<Atom>,
}

/// Cross-module fact storage with normalized module key lookup.
///
/// Module specifiers come in several forms — `"./lib/foo.js"`, `"lib/foo.js"`,
/// `"lib/foo"` — but must all resolve to the same module. This map stores facts
/// under a canonical key and provides lookup that handles common variants.
///
/// Canonical form: no leading `./`, preserves the rest (e.g. `"lib/foo.js"`).
#[derive(Debug, Clone, Default)]
pub struct ModuleFactsMap {
    inner: std::collections::HashMap<String, ModuleFacts>,
}

impl ModuleFactsMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert facts for a module. The key is normalized to canonical form.
    pub fn insert(&mut self, key: &str, facts: ModuleFacts) {
        self.inner.insert(Self::canonicalize(key), facts);
    }

    /// Look up facts by module specifier. Tries the specifier as-is, then
    /// common variants (with/without `./`, with/without `.js`).
    pub fn get(&self, specifier: &str) -> Option<&ModuleFacts> {
        let canon = Self::canonicalize(specifier);

        // Try canonical form first
        if let Some(f) = self.inner.get(&canon) {
            return Some(f);
        }

        // Try common extensions added
        let has_ext = [".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx"]
            .iter()
            .any(|ext| canon.ends_with(ext));
        if !has_ext {
            for ext in [".js", ".jsx"] {
                let with_ext = format!("{canon}{ext}");
                if let Some(f) = self.inner.get(&with_ext) {
                    return Some(f);
                }
            }
        }

        // Try with extension stripped
        for ext in [".js", ".jsx"] {
            if let Some(stripped) = canon.strip_suffix(ext) {
                if let Some(f) = self.inner.get(stripped) {
                    return Some(f);
                }
            }
        }

        None
    }

    /// Number of modules in the map.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Normalize a module specifier to canonical form.
    /// Strips leading `./` — the canonical key is always a relative path
    /// without the dot-slash prefix.
    fn canonicalize(specifier: &str) -> String {
        specifier.strip_prefix("./").unwrap_or(specifier).to_string()
    }
}

impl fmt::Display for ModuleFactsMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut keys: Vec<&String> = self.inner.keys().collect();
        keys.sort();
        for (i, key) in keys.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            let facts = &self.inner[*key];
            write!(f, "── {key} ──\n{facts}")?;
        }
        Ok(())
    }
}

// ── Display implementations for debugging/inspection ───────────────

impl fmt::Display for ImportKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportKind::Default => write!(f, "default"),
            ImportKind::Namespace => write!(f, "namespace"),
            ImportKind::Named(name) => write!(f, "named({name})"),
        }
    }
}

impl fmt::Display for ExportKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportKind::Default => write!(f, "default"),
            ExportKind::Named => write!(f, "named"),
        }
    }
}

impl fmt::Display for ImportFact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "import {} from \"{}\" [{}]", self.local, self.source, self.kind)
    }
}

impl fmt::Display for ExportFact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.local {
            Some(local) if local.as_ref() != self.exported.as_ref() => {
                write!(f, "export {} as {} [{}]", local, self.exported, self.kind)
            }
            _ => write!(f, "export {} [{}]", self.exported, self.kind),
        }
    }
}

impl fmt::Display for ModuleFacts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.imports.is_empty() && self.exports.is_empty() {
            return write!(f, "(no imports or exports)");
        }
        for (i, import) in self.imports.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{import}")?;
        }
        if !self.imports.is_empty() && !self.exports.is_empty() {
            writeln!(f)?;
        }
        for (i, export) in self.exports.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{export}")?;
        }
        Ok(())
    }
}

/// Extract import/export facts from a module AST that has completed Stage 2.
///
/// This collector reads the reconstructed ESM declarations produced by `UnEsm`
/// and the helper unwrapping rules. It should be called immediately after Stage 2,
/// before later rules modify the import/export structure.
pub fn collect_module_facts(module: &Module) -> ModuleFacts {
    let mut facts = ModuleFacts::default();

    for item in &module.body {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::Import(import) => {
                if import.type_only {
                    continue;
                }
                let source = str_to_atom(&import.src.value);
                for spec in &import.specifiers {
                    let (local, kind) = match spec {
                        ImportSpecifier::Default(s) => {
                            (s.local.sym.clone(), ImportKind::Default)
                        }
                        ImportSpecifier::Namespace(s) => {
                            (s.local.sym.clone(), ImportKind::Namespace)
                        }
                        ImportSpecifier::Named(s) => {
                            let imported_name = s
                                .imported
                                .as_ref()
                                .map(|i| export_name_to_atom(i))
                                .unwrap_or_else(|| s.local.sym.clone());
                            (s.local.sym.clone(), ImportKind::Named(imported_name))
                        }
                    };
                    facts.imports.push(ImportFact {
                        local,
                        source: source.clone(),
                        kind,
                    });
                }
            }
            ModuleDecl::ExportDefaultDecl(export) => {
                let local = match &export.decl {
                    DefaultDecl::Fn(f) => f.ident.as_ref().map(|id| id.sym.clone()),
                    DefaultDecl::Class(c) => c.ident.as_ref().map(|id| id.sym.clone()),
                    DefaultDecl::TsInterfaceDecl(_) => None,
                };
                facts.exports.push(ExportFact {
                    exported: "default".into(),
                    local,
                    kind: ExportKind::Default,
                });
            }
            ModuleDecl::ExportDefaultExpr(_) => {
                facts.exports.push(ExportFact {
                    exported: "default".into(),
                    local: None,
                    kind: ExportKind::Default,
                });
            }
            ModuleDecl::ExportDecl(export) => match &export.decl {
                Decl::Var(var) => {
                    for decl in &var.decls {
                        if let swc_core::ecma::ast::Pat::Ident(binding) = &decl.name {
                            facts.exports.push(ExportFact {
                                exported: binding.id.sym.clone(),
                                local: Some(binding.id.sym.clone()),
                                kind: ExportKind::Named,
                            });
                        }
                    }
                }
                Decl::Fn(f) => {
                    facts.exports.push(ExportFact {
                        exported: f.ident.sym.clone(),
                        local: Some(f.ident.sym.clone()),
                        kind: ExportKind::Named,
                    });
                }
                Decl::Class(c) => {
                    facts.exports.push(ExportFact {
                        exported: c.ident.sym.clone(),
                        local: Some(c.ident.sym.clone()),
                        kind: ExportKind::Named,
                    });
                }
                _ => {}
            },
            ModuleDecl::ExportNamed(named) => {
                for spec in &named.specifiers {
                    match spec {
                        ExportSpecifier::Named(s) => {
                            let local_name = export_name_to_atom(&s.orig);
                            let exported_name = s
                                .exported
                                .as_ref()
                                .map(|e| export_name_to_atom(e))
                                .unwrap_or_else(|| local_name.clone());
                            let kind = if exported_name.as_ref() == "default" {
                                ExportKind::Default
                            } else {
                                ExportKind::Named
                            };
                            facts.exports.push(ExportFact {
                                exported: exported_name,
                                local: Some(local_name),
                                kind,
                            });
                        }
                        ExportSpecifier::Default(s) => {
                            facts.exports.push(ExportFact {
                                exported: "default".into(),
                                local: Some(s.exported.sym.clone()),
                                kind: ExportKind::Default,
                            });
                        }
                        ExportSpecifier::Namespace(s) => {
                            facts.exports.push(ExportFact {
                                exported: export_name_to_atom(&s.name),
                                local: None,
                                kind: ExportKind::Named,
                            });
                        }
                    }
                }
            }
            ModuleDecl::ExportAll(_) => {
                // `export * from "..."` — not enumerable locally, skip for v1
            }
            _ => {}
        }
    }

    facts.passthrough_target = detect_passthrough(module);
    facts
}

/// Detect `export default require("./X.js")` — a pure passthrough module that
/// re-exports another module's namespace as its default export. Returns the
/// target specifier.
///
/// Only matches modules whose body contains nothing except the single
/// `export default require("./X.js")`. Any other statement (side effects,
/// imports, additional exports) disqualifies the module.
fn detect_passthrough(module: &Module) -> Option<Atom> {
    if module.body.len() != 1 {
        return None;
    }

    let ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export)) = &module.body[0] else {
        return None;
    };
    let Expr::Call(call) = export.expr.as_ref() else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(ident) = callee.as_ref() else {
        return None;
    };
    if ident.sym != "require" || call.args.len() != 1 || call.args[0].spread.is_some() {
        return None;
    }
    let Expr::Lit(Lit::Str(s)) = call.args[0].expr.as_ref() else {
        return None;
    };
    Some(str_to_atom(&s.value))
}

fn export_name_to_atom(name: &swc_core::ecma::ast::ModuleExportName) -> Atom {
    match name {
        swc_core::ecma::ast::ModuleExportName::Ident(id) => id.sym.clone(),
        swc_core::ecma::ast::ModuleExportName::Str(s) => str_to_atom(&s.value),
    }
}

fn str_to_atom(value: &swc_core::atoms::Wtf8Atom) -> Atom {
    Atom::from(value.as_str().unwrap_or(""))
}
