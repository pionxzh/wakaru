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
    AssignExpr, Callee, Decl, DefaultDecl, ExportSpecifier, Expr, ImportSpecifier, Lit, Module,
    ModuleDecl, ModuleItem, Prop, PropName,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::rules::transpiler_helper_utils::{
    collect_transpiler_helpers, LocalHelperContext, TranspilerHelperKind, TsHelperKind,
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

/// Transpiler/runtime helper identity proven from a module's exported AST shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperKind {
    InteropRequireDefault,
    InteropRequireWildcard,
    ToConsumableArray,
    Extends,
    ObjectSpread,
    SlicedToArray,
    ClassCallCheck,
    PossibleConstructorReturn,
    AssertThisInitialized,
    ObjectWithoutProperties,
    Inherits,
    CallSuper,
    AsyncToGenerator,
    TaggedTemplateLiteral,
    RegeneratorRuntime,
}

/// Raw TypeScript/tslib helper identity proven from a module's exported AST shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeScriptHelperKind {
    Awaiter,
    Generator,
    Values,
    Assign,
    Rest,
    Extends,
    ImportDefault,
    ImportStar,
    CreateBinding,
    SetModuleDefault,
    Read,
    Spread,
    SpreadArrays,
    SpreadArray,
    ClassPrivateFieldGet,
    ClassPrivateFieldSet,
}

/// One helper export extracted from the post-Stage-2 AST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperExportFact {
    /// The exported name that consumers import/access.
    /// `"default"` for default exports.
    pub exported: Atom,
    /// The local binding name, if any.
    pub local: Option<Atom>,
    /// The proven helper/runtime identity.
    pub kind: HelperKind,
}

/// One raw TypeScript/tslib helper export extracted from the post-Stage-2 AST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeScriptHelperExportFact {
    /// The exported name that consumers import/access.
    /// `"default"` for default exports.
    pub exported: Atom,
    /// The local binding name, if any.
    pub local: Option<Atom>,
    /// The proven TypeScript/tslib helper identity.
    pub kind: TypeScriptHelperKind,
}

/// Facts extracted from one module after Stage 2.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleFacts {
    pub imports: Vec<ImportFact>,
    pub exports: Vec<ExportFact>,
    pub helper_exports: Vec<HelperExportFact>,
    pub default_object_helper_exports: Vec<HelperExportFact>,
    pub ts_helper_exports: Vec<TypeScriptHelperExportFact>,
    /// If this module is a passthrough re-export (`export default require("./X.js")`),
    /// this is the target module specifier. Importers can be redirected to the target.
    pub passthrough_target: Option<Atom>,
    /// True when the module exports a recognized transpiler helper (Babel/tslib),
    /// including helper-dependency kinds with no rewrite mapping (e.g.
    /// `_defineProperty`). Used by dead-module elimination to treat the module as
    /// removable boilerplate.
    pub is_helper_module: bool,
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
        specifier
            .strip_prefix("./")
            .unwrap_or(specifier)
            .to_string()
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

impl fmt::Display for HelperKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HelperKind::InteropRequireDefault => write!(f, "interopRequireDefault"),
            HelperKind::InteropRequireWildcard => write!(f, "interopRequireWildcard"),
            HelperKind::ToConsumableArray => write!(f, "toConsumableArray"),
            HelperKind::Extends => write!(f, "extends"),
            HelperKind::ObjectSpread => write!(f, "objectSpread"),
            HelperKind::SlicedToArray => write!(f, "slicedToArray"),
            HelperKind::ClassCallCheck => write!(f, "classCallCheck"),
            HelperKind::PossibleConstructorReturn => write!(f, "possibleConstructorReturn"),
            HelperKind::AssertThisInitialized => write!(f, "assertThisInitialized"),
            HelperKind::ObjectWithoutProperties => write!(f, "objectWithoutProperties"),
            HelperKind::Inherits => write!(f, "inherits"),
            HelperKind::CallSuper => write!(f, "callSuper"),
            HelperKind::AsyncToGenerator => write!(f, "asyncToGenerator"),
            HelperKind::TaggedTemplateLiteral => write!(f, "taggedTemplateLiteral"),
            HelperKind::RegeneratorRuntime => write!(f, "regeneratorRuntime"),
        }
    }
}

impl fmt::Display for TypeScriptHelperKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeScriptHelperKind::Awaiter => write!(f, "ts:awaiter"),
            TypeScriptHelperKind::Generator => write!(f, "ts:generator"),
            TypeScriptHelperKind::Values => write!(f, "ts:values"),
            TypeScriptHelperKind::Assign => write!(f, "ts:assign"),
            TypeScriptHelperKind::Rest => write!(f, "ts:rest"),
            TypeScriptHelperKind::Extends => write!(f, "ts:extends"),
            TypeScriptHelperKind::ImportDefault => write!(f, "ts:importDefault"),
            TypeScriptHelperKind::ImportStar => write!(f, "ts:importStar"),
            TypeScriptHelperKind::CreateBinding => write!(f, "ts:createBinding"),
            TypeScriptHelperKind::SetModuleDefault => write!(f, "ts:setModuleDefault"),
            TypeScriptHelperKind::Read => write!(f, "ts:read"),
            TypeScriptHelperKind::Spread => write!(f, "ts:spread"),
            TypeScriptHelperKind::SpreadArrays => write!(f, "ts:spreadArrays"),
            TypeScriptHelperKind::SpreadArray => write!(f, "ts:spreadArray"),
            TypeScriptHelperKind::ClassPrivateFieldGet => write!(f, "ts:classPrivateFieldGet"),
            TypeScriptHelperKind::ClassPrivateFieldSet => write!(f, "ts:classPrivateFieldSet"),
        }
    }
}

impl fmt::Display for ImportFact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "import {} from \"{}\" [{}]",
            self.local, self.source, self.kind
        )
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

impl fmt::Display for HelperExportFact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.local {
            Some(local) if local.as_ref() != self.exported.as_ref() => {
                write!(
                    f,
                    "helper export {} as {} [{}]",
                    local, self.exported, self.kind
                )
            }
            _ => write!(f, "helper export {} [{}]", self.exported, self.kind),
        }
    }
}

impl fmt::Display for TypeScriptHelperExportFact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.local {
            Some(local) if local.as_ref() != self.exported.as_ref() => {
                write!(
                    f,
                    "ts helper export {} as {} [{}]",
                    local, self.exported, self.kind
                )
            }
            _ => write!(f, "ts helper export {} [{}]", self.exported, self.kind),
        }
    }
}

impl fmt::Display for ModuleFacts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.imports.is_empty()
            && self.exports.is_empty()
            && self.helper_exports.is_empty()
            && self.default_object_helper_exports.is_empty()
            && self.ts_helper_exports.is_empty()
        {
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
        if (!self.imports.is_empty() || !self.exports.is_empty()) && !self.helper_exports.is_empty()
        {
            writeln!(f)?;
        }
        for (i, helper) in self.helper_exports.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{helper}")?;
        }
        if (!self.imports.is_empty() || !self.exports.is_empty() || !self.helper_exports.is_empty())
            && !self.default_object_helper_exports.is_empty()
        {
            writeln!(f)?;
        }
        for (i, helper) in self.default_object_helper_exports.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "default object {helper}")?;
        }
        if (!self.imports.is_empty()
            || !self.exports.is_empty()
            || !self.helper_exports.is_empty()
            || !self.default_object_helper_exports.is_empty())
            && !self.ts_helper_exports.is_empty()
        {
            writeln!(f)?;
        }
        for (i, helper) in self.ts_helper_exports.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{helper}")?;
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
                        ImportSpecifier::Default(s) => (s.local.sym.clone(), ImportKind::Default),
                        ImportSpecifier::Namespace(s) => {
                            (s.local.sym.clone(), ImportKind::Namespace)
                        }
                        ImportSpecifier::Named(s) => {
                            let imported_name = s
                                .imported
                                .as_ref()
                                .map(export_name_to_atom)
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
            ModuleDecl::ExportDefaultExpr(export) => {
                let local = match export.expr.as_ref() {
                    Expr::Ident(ident) => Some(ident.sym.clone()),
                    _ => None,
                };
                facts.exports.push(ExportFact {
                    exported: "default".into(),
                    local,
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
                                .map(export_name_to_atom)
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
    let (helper_exports, exports_any_helper) = collect_helper_exports(module, &facts.exports);
    facts.helper_exports = helper_exports;
    facts.default_object_helper_exports = collect_default_object_helper_exports(module);
    facts.ts_helper_exports = collect_ts_helper_exports(module, &facts.exports);
    facts.is_helper_module = exports_any_helper
        || !facts.default_object_helper_exports.is_empty()
        || !facts.ts_helper_exports.is_empty();
    facts
}

/// Returns the rewrite-relevant helper exports plus whether the module exports
/// *any* recognized transpiler helper. The second value also covers helper kinds
/// that have no rewrite mapping (`DefineProperty`, `Typeof`, `HelperDependency`),
/// so a helper-dependency module like `_defineProperty` — which other helpers
/// import — is still identifiable as helper boilerplate.
fn collect_helper_exports(
    module: &Module,
    exports: &[ExportFact],
) -> (Vec<HelperExportFact>, bool) {
    let local_helpers = collect_transpiler_helpers(module);
    let mut helper_exports = Vec::new();
    let mut exports_any_helper = false;

    for export in exports {
        let Some(local) = &export.local else {
            continue;
        };

        let matches_transpiler = local_helpers.iter().any(|((sym, _), _)| sym == local);
        let kind = local_helpers
            .iter()
            .find_map(|((sym, _), kind)| {
                (sym == local)
                    .then(|| helper_kind_from_transpiler(*kind))
                    .flatten()
            })
            .or_else(|| {
                is_regenerator_runtime_binding(module, local)
                    .then_some(HelperKind::RegeneratorRuntime)
            });

        if matches_transpiler || kind.is_some() {
            exports_any_helper = true;
        }
        if let Some(kind) = kind {
            helper_exports.push(HelperExportFact {
                exported: export.exported.clone(),
                local: Some(local.clone()),
                kind,
            });
        }
    }

    (helper_exports, exports_any_helper)
}

fn collect_ts_helper_exports(
    module: &Module,
    exports: &[ExportFact],
) -> Vec<TypeScriptHelperExportFact> {
    let local_helpers = LocalHelperContext::collect(module);
    let mut helper_exports = Vec::new();

    for export in exports {
        let Some(local) = &export.local else {
            continue;
        };
        let kind = local_helpers
            .ts_helper_kind_by_symbol(local)
            .map(helper_kind_from_ts)
            .or_else(|| {
                local_helpers.helpers().iter().find_map(|((sym, _), kind)| {
                    (sym == local)
                        .then(|| helper_kind_from_transpiler_ts(*kind))
                        .flatten()
                })
            });
        let Some(kind) = kind else {
            continue;
        };
        helper_exports.push(TypeScriptHelperExportFact {
            exported: export.exported.clone(),
            local: Some(local.clone()),
            kind,
        });
    }

    helper_exports
}

fn helper_kind_from_transpiler_ts(kind: TranspilerHelperKind) -> Option<TypeScriptHelperKind> {
    match kind {
        TranspilerHelperKind::ObjectWithoutProperties => Some(TypeScriptHelperKind::Rest),
        _ => None,
    }
}

fn helper_kind_from_transpiler(kind: TranspilerHelperKind) -> Option<HelperKind> {
    match kind {
        TranspilerHelperKind::InteropRequireDefault => Some(HelperKind::InteropRequireDefault),
        TranspilerHelperKind::InteropRequireWildcard => Some(HelperKind::InteropRequireWildcard),
        TranspilerHelperKind::ToConsumableArray => Some(HelperKind::ToConsumableArray),
        TranspilerHelperKind::Extends => Some(HelperKind::Extends),
        TranspilerHelperKind::ObjectSpread => Some(HelperKind::ObjectSpread),
        TranspilerHelperKind::SlicedToArray => Some(HelperKind::SlicedToArray),
        TranspilerHelperKind::ClassCallCheck => Some(HelperKind::ClassCallCheck),
        TranspilerHelperKind::PossibleConstructorReturn => {
            Some(HelperKind::PossibleConstructorReturn)
        }
        TranspilerHelperKind::AssertThisInitialized => Some(HelperKind::AssertThisInitialized),
        TranspilerHelperKind::ObjectWithoutProperties => Some(HelperKind::ObjectWithoutProperties),
        TranspilerHelperKind::Inherits => Some(HelperKind::Inherits),
        TranspilerHelperKind::CallSuper => Some(HelperKind::CallSuper),
        TranspilerHelperKind::AsyncToGenerator => Some(HelperKind::AsyncToGenerator),
        TranspilerHelperKind::TaggedTemplateLiteral => Some(HelperKind::TaggedTemplateLiteral),
        TranspilerHelperKind::Typeof => None,
        TranspilerHelperKind::DefineProperty => None,
        TranspilerHelperKind::HelperDependency => None,
    }
}

fn collect_default_object_helper_exports(module: &Module) -> Vec<HelperExportFact> {
    let local_helpers = collect_transpiler_helpers(module);
    let local_kinds: std::collections::HashMap<Atom, HelperKind> = local_helpers
        .iter()
        .filter_map(|((local, _), kind)| {
            helper_kind_from_transpiler(*kind).map(|kind| (local.clone(), kind))
        })
        .collect();

    let mut helper_exports = Vec::new();
    for prop in default_export_object_props(module) {
        let Some((exported, local)) = object_prop_exported_local(prop) else {
            continue;
        };
        let Some(kind) = local_kinds.get(&local).copied() else {
            continue;
        };
        helper_exports.push(HelperExportFact {
            exported,
            local: Some(local),
            kind,
        });
    }

    helper_exports
}

fn default_export_object_props(module: &Module) -> impl Iterator<Item = &Prop> {
    module
        .body
        .iter()
        .filter_map(|item| {
            let ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export)) = item else {
                return None;
            };
            let Expr::Object(object) = export.expr.as_ref() else {
                return None;
            };
            Some(object.props.iter().filter_map(|prop| match prop {
                swc_core::ecma::ast::PropOrSpread::Prop(prop) => Some(prop.as_ref()),
                swc_core::ecma::ast::PropOrSpread::Spread(_) => None,
            }))
        })
        .flatten()
}

fn object_prop_exported_local(prop: &Prop) -> Option<(Atom, Atom)> {
    match prop {
        Prop::Shorthand(id) => Some((id.sym.clone(), id.sym.clone())),
        Prop::KeyValue(kv) => {
            let exported = prop_name_to_atom(&kv.key)?;
            let Expr::Ident(local) = kv.value.as_ref() else {
                return None;
            };
            Some((exported, local.sym.clone()))
        }
        _ => None,
    }
}

fn prop_name_to_atom(name: &PropName) -> Option<Atom> {
    match name {
        PropName::Ident(id) => Some(id.sym.clone()),
        PropName::Str(s) => Some(str_to_atom(&s.value)),
        PropName::Num(num) => Some(Atom::from(num.value.to_string())),
        _ => None,
    }
}

fn helper_kind_from_ts(kind: TsHelperKind) -> TypeScriptHelperKind {
    match kind {
        TsHelperKind::Awaiter => TypeScriptHelperKind::Awaiter,
        TsHelperKind::Generator => TypeScriptHelperKind::Generator,
        TsHelperKind::Values => TypeScriptHelperKind::Values,
        TsHelperKind::Assign => TypeScriptHelperKind::Assign,
        TsHelperKind::Rest => TypeScriptHelperKind::Rest,
        TsHelperKind::Extends => TypeScriptHelperKind::Extends,
        TsHelperKind::ImportDefault => TypeScriptHelperKind::ImportDefault,
        TsHelperKind::ImportStar => TypeScriptHelperKind::ImportStar,
        TsHelperKind::CreateBinding => TypeScriptHelperKind::CreateBinding,
        TsHelperKind::SetModuleDefault => TypeScriptHelperKind::SetModuleDefault,
        TsHelperKind::Read => TypeScriptHelperKind::Read,
        TsHelperKind::Spread => TypeScriptHelperKind::Spread,
        TsHelperKind::SpreadArrays => TypeScriptHelperKind::SpreadArrays,
        TsHelperKind::SpreadArray => TypeScriptHelperKind::SpreadArray,
        TsHelperKind::ClassPrivateFieldGet => TypeScriptHelperKind::ClassPrivateFieldGet,
        TsHelperKind::ClassPrivateFieldSet => TypeScriptHelperKind::ClassPrivateFieldSet,
    }
}

fn is_regenerator_runtime_binding(module: &Module, local: &Atom) -> bool {
    struct Finder<'a> {
        local: &'a Atom,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_function(&mut self, _: &swc_core::ecma::ast::Function) {}

        fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            let Some(right) = assign.right.as_ident() else {
                assign.visit_children_with(self);
                return;
            };
            if right.sym != *self.local {
                assign.visit_children_with(self);
                return;
            }

            let Some(left) = assign.left.as_simple() else {
                assign.visit_children_with(self);
                return;
            };

            if matches!(
                left.as_ident(),
                Some(ident) if ident.sym.as_ref() == "regeneratorRuntime"
            ) {
                self.found = true;
                return;
            }

            if let Some(member) = left.as_member() {
                if is_member_prop_name(&member.prop, "regeneratorRuntime") {
                    self.found = true;
                    return;
                }
            }

            assign.visit_children_with(self);
        }
    }

    let mut finder = Finder {
        local,
        found: false,
    };
    module.visit_with(&mut finder);
    finder.found
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

fn is_member_prop_name(prop: &swc_core::ecma::ast::MemberProp, name: &str) -> bool {
    match prop {
        swc_core::ecma::ast::MemberProp::Ident(id) => id.sym.as_ref() == name,
        swc_core::ecma::ast::MemberProp::Computed(computed) => {
            matches!(
                computed.expr.as_ref(),
                Expr::Lit(Lit::Str(s)) if s.value.as_str() == Some(name)
            )
        }
        swc_core::ecma::ast::MemberProp::PrivateName(_) => false,
    }
}
