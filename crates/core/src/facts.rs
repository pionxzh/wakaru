//! Post-Stage-2 fact extraction.
//!
//! After Stage 2 (helper unwrapping + module-system reconstruction), the AST has
//! clean `import`/`export` declarations. This module extracts a per-module fact
//! summary from that reconstructed form.
//!
//! These facts are the foundation for cross-module analysis in the multi-module
//! `unpack()` path. Single-file `decompile()` does not use them.

use std::{
    collections::{HashMap, HashSet},
    fmt,
};

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, AssignTarget, BlockStmtOrExpr, Callee, Decl, DefaultDecl,
    ExportSpecifier, Expr, Function, ImportSpecifier, Lit, MemberExpr, MemberProp, Module,
    ModuleDecl, ModuleItem, Prop, PropName, ReturnStmt, SimpleAssignTarget, Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::rules::helper_matcher::{
    binding_key, binding_key_from_ident_pat, expr_matches_binding, BindingKey,
};
use crate::rules::transpiler_helper_utils::{
    collect_inline_ts_helpers_deep, collect_transpiler_helpers, LocalHelperContext,
    TranspilerHelperKind, TsHelperKind,
};
use crate::utils::paren::strip_parens;

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
    let deep_inline_helpers = collect_inline_ts_helpers_deep(module);
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
        push_ts_helper_export(
            &mut helper_exports,
            export.exported.clone(),
            Some(local.clone()),
            kind,
        );
    }

    collect_registered_ts_helper_exports(module, &deep_inline_helpers, &mut helper_exports);

    helper_exports
}

fn helper_kind_from_transpiler_ts(kind: TranspilerHelperKind) -> Option<TypeScriptHelperKind> {
    match kind {
        TranspilerHelperKind::ObjectWithoutProperties => Some(TypeScriptHelperKind::Rest),
        _ => None,
    }
}

fn collect_registered_ts_helper_exports(
    module: &Module,
    inline_helpers: &HashMap<BindingKey, TsHelperKind>,
    helper_exports: &mut Vec<TypeScriptHelperExportFact>,
) {
    let registrars = collect_ts_helper_export_registrars(module);

    struct RegistrarExportCollector<'a, 'b> {
        inline_helpers: &'a HashMap<BindingKey, TsHelperKind>,
        registrars: &'a HashSet<BindingKey>,
        helper_exports: &'b mut Vec<TypeScriptHelperExportFact>,
    }

    impl Visit for RegistrarExportCollector<'_, '_> {
        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if !is_ts_helper_export_registration_call(call, self.registrars) {
                call.visit_children_with(self);
                return;
            }
            let [export_arg, local_arg] = call.args.as_slice() else {
                call.visit_children_with(self);
                return;
            };
            if export_arg.spread.is_some() || local_arg.spread.is_some() {
                call.visit_children_with(self);
                return;
            }
            let Expr::Lit(Lit::Str(exported)) = export_arg.expr.as_ref() else {
                call.visit_children_with(self);
                return;
            };
            let Expr::Ident(local) = local_arg.expr.as_ref() else {
                call.visit_children_with(self);
                return;
            };
            let Some(exported_name) = exported.value.as_str() else {
                call.visit_children_with(self);
                return;
            };
            let Some(exported_kind) = ts_helper_kind_from_name(exported_name) else {
                call.visit_children_with(self);
                return;
            };
            let Some(local_kind) = self
                .inline_helpers
                .get(&binding_key(local))
                .copied()
                .map(helper_kind_from_ts)
            else {
                call.visit_children_with(self);
                return;
            };
            if local_kind == exported_kind {
                push_ts_helper_export(
                    self.helper_exports,
                    str_to_atom(&exported.value),
                    Some(local.sym.clone()),
                    local_kind,
                );
            }
            call.visit_children_with(self);
        }
    }

    let mut collector = RegistrarExportCollector {
        inline_helpers,
        registrars: &registrars,
        helper_exports,
    };
    module.visit_with(&mut collector);
}

fn collect_ts_helper_export_registrars(module: &Module) -> HashSet<BindingKey> {
    struct RegistrarCollector {
        registrars: HashSet<BindingKey>,
    }

    impl Visit for RegistrarCollector {
        fn visit_fn_decl(&mut self, fn_decl: &swc_core::ecma::ast::FnDecl) {
            if function_is_ts_helper_export_registrar(&fn_decl.function) {
                self.registrars.insert(binding_key(&fn_decl.ident));
            }
            fn_decl.visit_children_with(self);
        }

        fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
            if declarator
                .init
                .as_deref()
                .is_some_and(expr_is_ts_helper_export_registrar)
            {
                if let Some(key) = binding_key_from_ident_pat(&declarator.name) {
                    self.registrars.insert(key);
                }
            }
            declarator.visit_children_with(self);
        }

        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if assign.op == AssignOp::Assign
                && expr_is_ts_helper_export_registrar(strip_parens(assign.right.as_ref()))
            {
                if let Some(key) = assign_target_binding_key(&assign.left) {
                    self.registrars.insert(key);
                }
            }
            assign.visit_children_with(self);
        }

        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            self.registrars
                .extend(callback_registrars_from_callable_call(call));
            call.visit_children_with(self);
        }
    }

    let mut collector = RegistrarCollector {
        registrars: HashSet::new(),
    };
    module.visit_with(&mut collector);
    collector.registrars
}

fn is_ts_helper_export_registration_call(
    call: &swc_core::ecma::ast::CallExpr,
    registrars: &HashSet<BindingKey>,
) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Ident(callee) = strip_parens(callee.as_ref()) else {
        return false;
    };
    registrars.contains(&binding_key(callee))
}

fn expr_is_ts_helper_export_registrar(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Fn(fn_expr) => function_is_ts_helper_export_registrar(&fn_expr.function),
        Expr::Arrow(arrow) => arrow_is_ts_helper_export_registrar(arrow),
        _ => false,
    }
}

fn function_is_ts_helper_export_registrar(function: &Function) -> bool {
    if function.params.len() < 2 {
        return false;
    }
    let Some(name_key) = binding_key_from_ident_pat(&function.params[0].pat) else {
        return false;
    };
    let Some(value_key) = binding_key_from_ident_pat(&function.params[1].pat) else {
        return false;
    };
    let Some(body) = &function.body else {
        return false;
    };
    body_writes_helper_export(&body.stmts, &name_key, &value_key)
}

fn arrow_is_ts_helper_export_registrar(arrow: &ArrowExpr) -> bool {
    if arrow.params.len() < 2 {
        return false;
    }
    let Some(name_key) = binding_key_from_ident_pat(&arrow.params[0]) else {
        return false;
    };
    let Some(value_key) = binding_key_from_ident_pat(&arrow.params[1]) else {
        return false;
    };
    match arrow.body.as_ref() {
        BlockStmtOrExpr::BlockStmt(body) => {
            body_writes_helper_export(&body.stmts, &name_key, &value_key)
        }
        BlockStmtOrExpr::Expr(expr) => expr_writes_helper_export(expr, &name_key, &value_key),
    }
}

fn callback_registrars_from_callable_call(
    call: &swc_core::ecma::ast::CallExpr,
) -> HashSet<BindingKey> {
    let Callee::Expr(callee) = &call.callee else {
        return HashSet::new();
    };
    let Some(params) = callable_param_keys(strip_parens(callee.as_ref())) else {
        return HashSet::new();
    };

    let mut registrars = HashSet::new();
    for (index, arg) in call.args.iter().enumerate() {
        if arg.spread.is_some() {
            continue;
        }
        let Some(callback_param) = callback_registrar_param(arg.expr.as_ref()) else {
            continue;
        };
        let Some(Some(param)) = params.get(index) else {
            continue;
        };
        if callable_passes_registrar_to_param(strip_parens(callee.as_ref()), param) {
            registrars.insert(callback_param);
        }
    }
    registrars
}

fn callable_param_keys(expr: &Expr) -> Option<Vec<Option<BindingKey>>> {
    match expr {
        Expr::Fn(fn_expr) => Some(
            fn_expr
                .function
                .params
                .iter()
                .map(|param| binding_key_from_ident_pat(&param.pat))
                .collect(),
        ),
        Expr::Arrow(arrow) => Some(
            arrow
                .params
                .iter()
                .map(binding_key_from_ident_pat)
                .collect(),
        ),
        _ => None,
    }
}

fn callback_registrar_param(expr: &Expr) -> Option<BindingKey> {
    match strip_parens(expr) {
        Expr::Fn(fn_expr) => fn_expr
            .function
            .params
            .first()
            .and_then(|param| binding_key_from_ident_pat(&param.pat)),
        Expr::Arrow(arrow) => arrow.params.first().and_then(binding_key_from_ident_pat),
        _ => None,
    }
}

fn callable_passes_registrar_to_param(expr: &Expr, param: &BindingKey) -> bool {
    match expr {
        Expr::Fn(fn_expr) => fn_expr
            .function
            .body
            .as_ref()
            .is_some_and(|body| body_passes_registrar_to_param(&body.stmts, param)),
        Expr::Arrow(arrow) => match arrow.body.as_ref() {
            BlockStmtOrExpr::BlockStmt(body) => body_passes_registrar_to_param(&body.stmts, param),
            BlockStmtOrExpr::Expr(expr) => expr_passes_registrar_to_param(expr, param),
        },
        _ => false,
    }
}

fn body_passes_registrar_to_param(stmts: &[Stmt], param: &BindingKey) -> bool {
    let factories = collect_ts_helper_export_registrar_factories(stmts);

    struct RegistrarArgumentFinder<'a> {
        param: &'a BindingKey,
        factories: &'a HashSet<BindingKey>,
        found: bool,
    }

    impl Visit for RegistrarArgumentFinder<'_> {
        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if self.found {
                return;
            }
            if call_passes_registrar_to_param(call, self.param, self.factories) {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = RegistrarArgumentFinder {
        param,
        factories: &factories,
        found: false,
    };
    stmts.visit_with(&mut finder);
    finder.found
}

fn expr_passes_registrar_to_param(expr: &Expr, param: &BindingKey) -> bool {
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    call_passes_registrar_to_param(call, param, &HashSet::new())
}

fn call_passes_registrar_to_param(
    call: &swc_core::ecma::ast::CallExpr,
    param: &BindingKey,
    factories: &HashSet<BindingKey>,
) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !expr_matches_binding(strip_parens(callee.as_ref()), param) {
        return false;
    }
    let Some(first_arg) = call.args.first() else {
        return false;
    };
    first_arg.spread.is_none()
        && expr_is_ts_helper_export_registrar_value(first_arg.expr.as_ref(), factories)
}

fn expr_is_ts_helper_export_registrar_value(expr: &Expr, factories: &HashSet<BindingKey>) -> bool {
    if expr_is_ts_helper_export_registrar(expr) {
        return true;
    }
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    match strip_parens(callee.as_ref()) {
        Expr::Ident(id) => factories.contains(&binding_key(id)),
        Expr::Fn(fn_expr) => function_returns_ts_helper_export_registrar(&fn_expr.function),
        Expr::Arrow(arrow) => arrow_returns_ts_helper_export_registrar(arrow),
        _ => false,
    }
}

fn collect_ts_helper_export_registrar_factories(stmts: &[Stmt]) -> HashSet<BindingKey> {
    struct FactoryCollector {
        factories: HashSet<BindingKey>,
    }

    impl Visit for FactoryCollector {
        fn visit_fn_decl(&mut self, fn_decl: &swc_core::ecma::ast::FnDecl) {
            if function_returns_ts_helper_export_registrar(&fn_decl.function) {
                self.factories.insert(binding_key(&fn_decl.ident));
            }
        }

        fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
            if declarator
                .init
                .as_deref()
                .is_some_and(expr_returns_ts_helper_export_registrar)
            {
                if let Some(key) = binding_key_from_ident_pat(&declarator.name) {
                    self.factories.insert(key);
                }
            }
        }

        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if assign.op == AssignOp::Assign
                && expr_returns_ts_helper_export_registrar(strip_parens(assign.right.as_ref()))
            {
                if let Some(key) = assign_target_binding_key(&assign.left) {
                    self.factories.insert(key);
                }
            }
            assign.visit_children_with(self);
        }
    }

    let mut collector = FactoryCollector {
        factories: HashSet::new(),
    };
    stmts.visit_with(&mut collector);
    collector.factories
}

fn expr_returns_ts_helper_export_registrar(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Fn(fn_expr) => function_returns_ts_helper_export_registrar(&fn_expr.function),
        Expr::Arrow(arrow) => arrow_returns_ts_helper_export_registrar(arrow),
        _ => false,
    }
}

fn function_returns_ts_helper_export_registrar(function: &Function) -> bool {
    let Some(target_key) = function
        .params
        .first()
        .and_then(|param| binding_key_from_ident_pat(&param.pat))
    else {
        return false;
    };
    let Some(body) = &function.body else {
        return false;
    };
    body.stmts
        .iter()
        .any(|stmt| return_stmt_returns_target_registrar(stmt, &target_key))
}

fn arrow_returns_ts_helper_export_registrar(arrow: &ArrowExpr) -> bool {
    let Some(target_key) = arrow.params.first().and_then(binding_key_from_ident_pat) else {
        return false;
    };
    match arrow.body.as_ref() {
        BlockStmtOrExpr::BlockStmt(body) => body
            .stmts
            .iter()
            .any(|stmt| return_stmt_returns_target_registrar(stmt, &target_key)),
        BlockStmtOrExpr::Expr(expr) => expr_is_target_ts_helper_export_registrar(expr, &target_key),
    }
}

fn return_stmt_returns_target_registrar(stmt: &Stmt, target_key: &BindingKey) -> bool {
    let Stmt::Return(ReturnStmt {
        arg: Some(expr), ..
    }) = stmt
    else {
        return false;
    };
    expr_is_target_ts_helper_export_registrar(expr, target_key)
}

fn expr_is_target_ts_helper_export_registrar(expr: &Expr, target_key: &BindingKey) -> bool {
    match strip_parens(expr) {
        Expr::Fn(fn_expr) => {
            function_is_target_ts_helper_export_registrar(&fn_expr.function, target_key)
        }
        Expr::Arrow(arrow) => arrow_is_target_ts_helper_export_registrar(arrow, target_key),
        _ => false,
    }
}

fn function_is_target_ts_helper_export_registrar(
    function: &Function,
    target_key: &BindingKey,
) -> bool {
    if function.params.len() < 2 {
        return false;
    }
    let Some(name_key) = binding_key_from_ident_pat(&function.params[0].pat) else {
        return false;
    };
    let Some(value_key) = binding_key_from_ident_pat(&function.params[1].pat) else {
        return false;
    };
    let Some(body) = &function.body else {
        return false;
    };
    body_writes_target_helper_export(&body.stmts, target_key, &name_key, &value_key)
}

fn arrow_is_target_ts_helper_export_registrar(arrow: &ArrowExpr, target_key: &BindingKey) -> bool {
    if arrow.params.len() < 2 {
        return false;
    }
    let Some(name_key) = binding_key_from_ident_pat(&arrow.params[0]) else {
        return false;
    };
    let Some(value_key) = binding_key_from_ident_pat(&arrow.params[1]) else {
        return false;
    };
    match arrow.body.as_ref() {
        BlockStmtOrExpr::BlockStmt(body) => {
            body_writes_target_helper_export(&body.stmts, target_key, &name_key, &value_key)
        }
        BlockStmtOrExpr::Expr(expr) => {
            expr_writes_target_helper_export(expr, target_key, &name_key, &value_key)
        }
    }
}

fn body_writes_target_helper_export(
    stmts: &[Stmt],
    target_key: &BindingKey,
    name_key: &BindingKey,
    value_key: &BindingKey,
) -> bool {
    struct TargetExportWriteFinder<'a> {
        target_key: &'a BindingKey,
        name_key: &'a BindingKey,
        value_key: &'a BindingKey,
        found: bool,
    }

    impl Visit for TargetExportWriteFinder<'_> {
        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if is_target_helper_export_assignment(
                assign,
                self.target_key,
                self.name_key,
                self.value_key,
            ) {
                self.found = true;
                return;
            }
            assign.visit_children_with(self);
        }

        fn visit_function(&mut self, _: &Function) {}

        fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    }

    let mut finder = TargetExportWriteFinder {
        target_key,
        name_key,
        value_key,
        found: false,
    };
    stmts.visit_with(&mut finder);
    finder.found
}

fn expr_writes_target_helper_export(
    expr: &Expr,
    target_key: &BindingKey,
    name_key: &BindingKey,
    value_key: &BindingKey,
) -> bool {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return false;
    };
    is_target_helper_export_assignment(assign, target_key, name_key, value_key)
}

fn is_target_helper_export_assignment(
    assign: &AssignExpr,
    target_key: &BindingKey,
    name_key: &BindingKey,
    value_key: &BindingKey,
) -> bool {
    if assign.op != AssignOp::Assign || !expr_produces_registered_value(&assign.right, value_key) {
        return false;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return false;
    };
    member_of_target_indexed_by(member, target_key, name_key)
}

fn expr_produces_registered_value(expr: &Expr, value_key: &BindingKey) -> bool {
    match strip_parens(expr) {
        expr if expr_matches_binding(expr, value_key) => true,
        Expr::Cond(cond) => {
            expr_produces_registered_value(&cond.cons, value_key)
                || expr_produces_registered_value(&cond.alt, value_key)
        }
        Expr::Call(call) => call.args.iter().any(|arg| {
            arg.spread.is_none() && expr_matches_binding(strip_parens(&arg.expr), value_key)
        }),
        _ => false,
    }
}

fn body_writes_helper_export(
    stmts: &[swc_core::ecma::ast::Stmt],
    name_key: &BindingKey,
    value_key: &BindingKey,
) -> bool {
    struct ExportWriteFinder<'a> {
        name_key: &'a BindingKey,
        value_key: &'a BindingKey,
        found: bool,
    }

    impl Visit for ExportWriteFinder<'_> {
        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if is_helper_export_assignment(assign, self.name_key, self.value_key) {
                self.found = true;
                return;
            }
            assign.visit_children_with(self);
        }

        fn visit_function(&mut self, _: &Function) {}

        fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    }

    let mut finder = ExportWriteFinder {
        name_key,
        value_key,
        found: false,
    };
    stmts.visit_with(&mut finder);
    finder.found
}

fn expr_writes_helper_export(expr: &Expr, name_key: &BindingKey, value_key: &BindingKey) -> bool {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return false;
    };
    is_helper_export_assignment(assign, name_key, value_key)
}

fn is_helper_export_assignment(
    assign: &AssignExpr,
    name_key: &BindingKey,
    value_key: &BindingKey,
) -> bool {
    if assign.op != AssignOp::Assign
        || !expr_matches_binding(strip_parens(&assign.right), value_key)
    {
        return false;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return false;
    };
    exports_member_indexed_by(member, name_key)
}

fn exports_member_indexed_by(member: &MemberExpr, name_key: &BindingKey) -> bool {
    if !matches!(
        &member.prop,
        MemberProp::Computed(computed) if expr_matches_binding(strip_parens(&computed.expr), name_key)
    ) {
        return false;
    }
    is_exports_object(strip_parens(&member.obj))
}

fn member_of_target_indexed_by(
    member: &MemberExpr,
    target_key: &BindingKey,
    name_key: &BindingKey,
) -> bool {
    if !matches!(
        &member.prop,
        MemberProp::Computed(computed) if expr_matches_binding(strip_parens(&computed.expr), name_key)
    ) {
        return false;
    }
    expr_matches_binding(strip_parens(&member.obj), target_key)
}

fn is_exports_object(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Ident(id) => id.sym.as_ref() == "exports",
        Expr::Member(member) => {
            matches!(strip_parens(&member.obj), Expr::Ident(id) if id.sym.as_ref() == "module")
                && static_member_prop_name(&member.prop) == Some("exports")
        }
        _ => false,
    }
}

fn assign_target_binding_key(target: &AssignTarget) -> Option<BindingKey> {
    let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = target else {
        return None;
    };
    Some(binding_key(&binding.id))
}

fn static_member_prop_name(prop: &MemberProp) -> Option<&str> {
    match prop {
        MemberProp::Ident(id) => Some(id.sym.as_ref()),
        MemberProp::Computed(computed) => match strip_parens(&computed.expr) {
            Expr::Lit(Lit::Str(value)) => value.value.as_str(),
            _ => None,
        },
        MemberProp::PrivateName(_) => None,
    }
}

fn push_ts_helper_export(
    helper_exports: &mut Vec<TypeScriptHelperExportFact>,
    exported: Atom,
    local: Option<Atom>,
    kind: TypeScriptHelperKind,
) {
    if helper_exports
        .iter()
        .any(|helper| helper.exported == exported && helper.local == local && helper.kind == kind)
    {
        return;
    }
    helper_exports.push(TypeScriptHelperExportFact {
        exported,
        local,
        kind,
    });
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
        TranspilerHelperKind::CreateClass => None,
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

fn ts_helper_kind_from_name(name: &str) -> Option<TypeScriptHelperKind> {
    match name {
        "__awaiter" => Some(TypeScriptHelperKind::Awaiter),
        "__generator" => Some(TypeScriptHelperKind::Generator),
        "__values" | "_ts_values" => Some(TypeScriptHelperKind::Values),
        "__assign" => Some(TypeScriptHelperKind::Assign),
        "__rest" => Some(TypeScriptHelperKind::Rest),
        "__extends" => Some(TypeScriptHelperKind::Extends),
        "__importDefault" => Some(TypeScriptHelperKind::ImportDefault),
        "__importStar" => Some(TypeScriptHelperKind::ImportStar),
        "__createBinding" => Some(TypeScriptHelperKind::CreateBinding),
        "__setModuleDefault" => Some(TypeScriptHelperKind::SetModuleDefault),
        "__spreadArray" => Some(TypeScriptHelperKind::SpreadArray),
        "__classPrivateFieldGet" => Some(TypeScriptHelperKind::ClassPrivateFieldGet),
        "__classPrivateFieldSet" => Some(TypeScriptHelperKind::ClassPrivateFieldSet),
        _ => None,
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
