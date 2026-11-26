//! Lazy-factory helpers that one input exports and another imports.
//!
//! With code splitting, esbuild, Bun, and rolldown declare `__commonJS` and
//! `__esm` once, in a shared chunk, and every other chunk imports them:
//!
//! ```js
//! // runtime chunk
//! var __commonJS = (cb, mod) => () => (mod || cb((mod = { exports: {} }).exports, mod), mod.exports);
//! export { __commonJS as b };
//! // consumer chunk
//! import { b as e } from "./runtime.js";
//! var require_x = e((exports, module) => { … });
//! ```
//!
//! The detector finds helpers by their declaration, which the consumer does
//! not hold. The proof that an imported name is a helper therefore comes from
//! the exporting input: [`exported_lazy_helpers`] lists the exports whose
//! binding is declared by one top-level `var` with a lazy-helper initializer
//! and never written in that module (an export is a live binding, so a write
//! anywhere in the exporter could change what the importer calls). How the
//! consumer calls the name proves nothing; [`factory_position_imports`] only
//! marks a consumer worth detecting again once its siblings are known.
//!
//! The driver resolves each consumer's relative specifiers against the other
//! inputs and runs detection again inside [`with_imported_helpers`]. The
//! detector then reads, through [`imported_helper_syms`], the local names
//! that the module being detected imports from a proven export. The names
//! are taken from the module's own top-level import declarations, so a
//! module detected inside another (an unwrapped wrapper body) sees none.

use std::cell::RefCell;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    CallExpr, Callee, Decl, ExportSpecifier, Expr, ImportPhase, ImportSpecifier, Module,
    ModuleDecl, ModuleExportName, ModuleItem, Pat, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::collections::{HashMap, HashSet};

use super::factories::{
    collect_commonjs_helper_syms, collect_helper_syms, factory_shape_helper_sym,
    locally_exported_atoms,
};
use super::inline_factories::{inlined_factory_helpers, unwritten_helper_positions};
use crate::utils::paren::strip_parens;

/// What a proven lazy helper builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LazyHelperKind {
    /// A CommonJS module wrapper (`__commonJS`, `__commonJSMin`).
    CommonJs,
    /// A lazy ES module initializer (`__esm`, `__esmMin`).
    EsmInit,
}

/// One name a module imports from a relative specifier and calls in factory
/// position: `var x = name(factory)`, or `name(factory)()` where a minifier
/// inlined the factory into its only call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactoryPositionImport {
    /// The specifier as written, such as `./chunk-a.js`.
    pub specifier: String,
    /// The name the specifier exports.
    pub imported: String,
    /// The local binding the module calls.
    pub local: String,
    /// The module also uses what the call built the way a lazy factory is
    /// used: it calls a binding the call initialized with no arguments
    /// (`x()`), or the call is the inlined `name(factory)()`. Without this,
    /// the import only qualifies because such a binding is exported, which
    /// says nothing about this module alone.
    pub lazy_call_evidence: bool,
}

/// Proven helper exports of other inputs, keyed by the specifier the module
/// being detected uses for them, then by exported name.
pub(crate) type ImportedLazyHelpers = HashMap<String, HashMap<Atom, LazyHelperKind>>;

thread_local! {
    static IMPORTED: RefCell<Option<ImportedLazyHelpers>> = const { RefCell::new(None) };
}

/// Runs `detect` with `helpers` visible to the esbuild detector on this
/// thread. Detection of one input runs on one thread.
pub(crate) fn with_imported_helpers<R>(
    helpers: ImportedLazyHelpers,
    detect: impl FnOnce() -> R,
) -> R {
    let outer = IMPORTED.with(|imported| imported.replace(Some(helpers)));
    let result = detect();
    IMPORTED.with(|imported| imported.replace(outer));
    result
}

/// Local names `module` imports from a proven helper export of another input.
#[derive(Default)]
pub(super) struct ImportedHelperSyms {
    pub(super) all: HashSet<Atom>,
    pub(super) commonjs: HashSet<Atom>,
}

pub(super) fn imported_helper_syms(module: &Module) -> ImportedHelperSyms {
    IMPORTED.with(|imported| {
        let imported = imported.borrow();
        let mut syms = ImportedHelperSyms::default();
        let Some(helpers) = imported.as_ref() else {
            return syms;
        };
        for (specifier, local, imported_name) in named_imports(module) {
            let Some(kind) = helpers
                .get(specifier.as_str())
                .and_then(|exports| exports.get(&imported_name))
            else {
                continue;
            };
            if *kind == LazyHelperKind::CommonJs {
                syms.commonjs.insert(local.clone());
            }
            syms.all.insert(local);
        }
        syms
    })
}

/// What one input contributes to linking helpers across inputs.
#[derive(Debug, Clone, Default)]
pub(crate) struct LazyHelperLinkFacts {
    /// See [`exported_lazy_helpers`].
    pub(crate) exports: Vec<(String, LazyHelperKind)>,
    /// See [`factory_position_imports`].
    pub(crate) factory_imports: Vec<FactoryPositionImport>,
}

pub(crate) fn lazy_helper_link_facts(module: &Module) -> LazyHelperLinkFacts {
    LazyHelperLinkFacts {
        exports: exported_lazy_helpers(module),
        factory_imports: factory_position_imports(module),
    }
}

/// Exports of `module` proven to be lazy helpers, by exported name.
///
/// The exported binding must be declared by one top-level `var` declarator
/// whose initializer has the lazy-helper shape, and never written in the
/// module. Only `export { local as name }` lists are read; bundlers export
/// their runtime helpers that way.
pub(crate) fn exported_lazy_helpers(module: &Module) -> Vec<(String, LazyHelperKind)> {
    let helper_syms = collect_helper_syms(module);
    if helper_syms.is_empty() {
        return Vec::new();
    }
    let mut exported = Vec::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export)) = item else {
            continue;
        };
        if export.src.is_some() || export.type_only {
            continue;
        }
        for specifier in &export.specifiers {
            let ExportSpecifier::Named(named) = specifier else {
                continue;
            };
            if named.is_type_only {
                continue;
            }
            let ModuleExportName::Ident(local) = &named.orig else {
                continue;
            };
            if !helper_syms.contains(&local.sym) {
                continue;
            }
            let name = named.exported.as_ref().unwrap_or(&named.orig);
            exported.push((local.sym.clone(), export_name_atom(name)));
        }
    }
    if exported.is_empty() {
        return Vec::new();
    }
    let candidates: HashSet<Atom> = exported.iter().map(|(local, _)| local.clone()).collect();
    let unwritten = unwritten_helper_positions(module, &candidates);
    let commonjs = collect_commonjs_helper_syms(module);
    exported
        .into_iter()
        .filter(|(local, _)| unwritten.contains_key(local))
        .map(|(local, name)| {
            let kind = if commonjs.contains(&local) {
                LazyHelperKind::CommonJs
            } else {
                LazyHelperKind::EsmInit
            };
            (name.to_string(), kind)
        })
        .collect()
}

/// Names `module` imports from a relative specifier and calls in factory
/// position (`var x = name(factory)` or the inlined `name(factory)()`), when
/// the module also treats a result as a lazy factory: it calls such an `x`
/// with no arguments (or the call is inlined), or it exports `x`, since
/// other chunks may call a factory that this one only defines. Component
/// wrappers (`var C = wrap({ setup() {} })`) whose result is never called
/// bare are not listed.
///
/// A non-empty result proves nothing about the names; it marks the module for
/// detection again once the inputs that export them are known.
pub(crate) fn factory_position_imports(module: &Module) -> Vec<FactoryPositionImport> {
    let imports: Vec<(String, Atom, Atom)> = named_imports(module)
        .filter(|(specifier, _, _)| is_relative_specifier(specifier))
        .collect();
    if imports.is_empty() {
        return Vec::new();
    }
    let locals: HashSet<Atom> = imports.iter().map(|(_, local, _)| local.clone()).collect();
    let inlined = inlined_factory_helpers(module, &locals, false);
    // Helper local → bindings it initializes in factory position.
    let mut bound: HashMap<Atom, Vec<Atom>> = HashMap::default();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let (Some(helper), Pat::Ident(binding)) =
                (factory_shape_helper_sym(decl, &locals), &decl.name)
            else {
                continue;
            };
            bound
                .entry(helper)
                .or_default()
                .push(binding.id.sym.clone());
        }
    }
    if inlined.is_empty() && bound.is_empty() {
        return Vec::new();
    }
    let called_bare = zero_argument_callees(module);
    let exported = locally_exported_atoms(&module.body);
    imports
        .into_iter()
        .filter_map(|(specifier, local, imported)| {
            let bindings = bound.get(&local).map(Vec::as_slice).unwrap_or_default();
            let lazy_call_evidence =
                inlined.contains(&local) || bindings.iter().any(|b| called_bare.contains(b));
            let exported_factory = bindings.iter().any(|b| exported.contains(b));
            (lazy_call_evidence || exported_factory).then(|| FactoryPositionImport {
                specifier,
                imported: imported.to_string(),
                local: local.to_string(),
                lazy_call_evidence,
            })
        })
        .collect()
}

/// Names called with no arguments anywhere in `module` (`x()`). Matched by
/// name: this is evidence for a warning and a second detection, not proof.
fn zero_argument_callees(module: &Module) -> HashSet<Atom> {
    struct Callees(HashSet<Atom>);
    impl Visit for Callees {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if call.args.is_empty() {
                if let Callee::Expr(callee) = &call.callee {
                    if let Expr::Ident(ident) = strip_parens(callee) {
                        self.0.insert(ident.sym.clone());
                    }
                }
            }
            call.visit_children_with(self);
        }
    }
    let mut callees = Callees(HashSet::default());
    module.visit_with(&mut callees);
    callees.0
}

/// `(specifier, local, imported)` for each named import specifier of a
/// top-level value import.
fn named_imports(module: &Module) -> impl Iterator<Item = (String, Atom, Atom)> + '_ {
    module.body.iter().flat_map(|item| {
        let specifiers = match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import))
                if !import.type_only && import.phase == ImportPhase::Evaluation =>
            {
                let specifier = import.src.value.to_string_lossy().into_owned();
                import
                    .specifiers
                    .iter()
                    .filter_map(move |import_specifier| {
                        let ImportSpecifier::Named(named) = import_specifier else {
                            return None;
                        };
                        if named.is_type_only {
                            return None;
                        }
                        let imported = named
                            .imported
                            .as_ref()
                            .map(export_name_atom)
                            .unwrap_or_else(|| named.local.sym.clone());
                        Some((specifier.clone(), named.local.sym.clone(), imported))
                    })
                    .collect::<Vec<_>>()
            }
            _ => Vec::new(),
        };
        specifiers.into_iter()
    })
}

fn export_name_atom(name: &ModuleExportName) -> Atom {
    match name {
        ModuleExportName::Ident(ident) => ident.sym.clone(),
        ModuleExportName::Str(str) => str.value.to_string_lossy().into_owned().into(),
    }
}

pub(crate) fn is_relative_specifier(specifier: &str) -> bool {
    specifier.starts_with("./") || specifier.starts_with("../")
}
