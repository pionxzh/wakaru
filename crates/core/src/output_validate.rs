//! Emitted-module graph validator for normal unpack output.
//!
//! A development/benchmark tool: parses a set of emitted modules and reports
//! structural defects that would make the output fail to load as ESM —
//! dangling relative references, imports of names the provider does not
//! export, local exports of missing bindings, duplicate exports or
//! declarations, unresolved CommonJS runtime bindings in ESM, and writes to
//! imported or `const` bindings. Unresolved identifiers are reported only
//! when the graph itself proves them wrong: the name is declared at module
//! scope by exactly one other emitted module, or (with original inputs
//! supplied) the name is not free anywhere in the input.
//!
//! Raw output is deliberately out of scope: `--raw` promises only "no
//! readability transforms" and carries no module-graph contract. Validate
//! normal output only.
//!
//! The checks are conservative: a provider whose export set is unknowable
//! (it re-exports an external package or a missing module) suppresses
//! missing-name findings for its consumers rather than guessing.

use crate::collections::{HashMap, HashSet};
use std::path::Path;

use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, Span, Spanned, GLOBALS};
use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, AssignExpr, AssignTarget, AssignTargetPat, BindingIdent,
    BlockStmt, CallExpr, Callee, CatchClause, Constructor, Decl, DefaultDecl, Expr, ForHead,
    ForInStmt, ForOfStmt, ForStmt, Function, FunctionBody, Id, Ident, ImportSpecifier, Lit, Module,
    ModuleDecl, ModuleExportName, ModuleItem, ObjectPatProp, ParamOrTsParamProp, Pat, Program,
    PropName, SimpleAssignTarget, Stmt, Str, SwitchStmt, UnaryExpr, UnaryOp, UpdateExpr, VarDecl,
    VarDeclKind,
};
use swc_core::ecma::atoms::Atom;
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};

use crate::utils::paren::strip_parens;

/// A structural defect found in emitted output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFinding {
    /// The module the defect was found in.
    pub filename: String,
    /// One-based source line within `filename`.
    pub line: usize,
    /// One-based source column within `filename`.
    pub column: usize,
    pub kind: OutputFindingKind,
    /// Human-readable detail (specifier, binding name, parse message).
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputFindingKind {
    /// The file does not parse for its inferred source goal. Ambiguous files
    /// may fall back from module to classic-script parsing.
    ParseError,
    /// A `./`-relative import/export/require target is not in the module set.
    DanglingRelativeRef,
    /// A named or default import of a name the provider does not
    /// unambiguously export.
    MissingImportedName,
    /// A local export clause references no local declaration or import.
    MissingLocalExport,
    /// The same name is exported more than once by one module.
    DuplicateExport,
    /// Lexical declarations conflict with one another or with a `var`
    /// declaration in the same module or nested lexical scope.
    DuplicateDeclaration,
    /// Assignment or update targeting an imported binding.
    AssignToImport,
    /// Assignment or update targeting a `const` binding.
    AssignToConst,
    /// An ESM output module still reads or writes the free CommonJS runtime
    /// binding `module` or `exports`.
    EsmCommonJsResidual,
    /// A free identifier that the emitted graph proves undeclared: exactly one
    /// other module declares the name at module scope, or the name is not
    /// free in the supplied original input.
    UnresolvedReference,
}

impl OutputFindingKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutputFindingKind::ParseError => "parse_error",
            OutputFindingKind::DanglingRelativeRef => "dangling_relative_ref",
            OutputFindingKind::MissingImportedName => "missing_imported_name",
            OutputFindingKind::MissingLocalExport => "missing_local_export",
            OutputFindingKind::DuplicateExport => "duplicate_export",
            OutputFindingKind::DuplicateDeclaration => "duplicate_declaration",
            OutputFindingKind::AssignToImport => "assign_to_import",
            OutputFindingKind::AssignToConst => "assign_to_const",
            OutputFindingKind::EsmCommonJsResidual => "esm_commonjs_residual",
            OutputFindingKind::UnresolvedReference => "unresolved_reference",
        }
    }
}

/// Validate a set of emitted modules as one graph.
///
/// `modules` maps output-relative filenames (`/`-separated, e.g.
/// `"src/entry.js"`) to their source text. Findings are returned in input
/// module order, then AST order within a module.
pub fn validate_output_modules(modules: &[(String, String)]) -> Vec<OutputFinding> {
    validate_output_modules_with_inputs(modules, &[])
}

/// Validate a set of emitted modules as one graph and, additionally, compare
/// their free identifiers against the original bundle inputs.
///
/// `inputs` maps input filenames to their source text. A free identifier in
/// an output module is reported as [`OutputFindingKind::UnresolvedReference`]
/// when no input file uses the same name freely: host globals, `typeof`
/// probes, and dependency bugs that the input already contains stay silent,
/// while a name the rewrite pipeline or the splitter left undeclared does
/// not. An input that fails to parse is reported as a
/// [`OutputFindingKind::ParseError`] on the input filename. With no inputs
/// this is [`validate_output_modules`].
pub fn validate_output_modules_with_inputs(
    modules: &[(String, String)],
    inputs: &[(String, String)],
) -> Vec<OutputFinding> {
    GLOBALS.set(&Default::default(), || validate_inner(modules, inputs))
}

fn validate_inner(modules: &[(String, String)], inputs: &[(String, String)]) -> Vec<OutputFinding> {
    let filenames: HashSet<&str> = modules.iter().map(|(name, _)| name.as_str()).collect();
    let source_goals = classify_source_goals(modules, &filenames);
    let mut findings = Vec::new();
    let mut infos = Vec::new();

    for (filename, source) in modules {
        match analyze_module(
            filename,
            source,
            &filenames,
            source_goals
                .get(filename)
                .copied()
                .unwrap_or(SourceGoal::Ambiguous),
        ) {
            Ok((info, mut local_findings)) => {
                findings.append(&mut local_findings);
                infos.push(info);
            }
            Err(error) => findings.push(OutputFinding {
                filename: filename.clone(),
                line: error.line,
                column: error.column,
                kind: OutputFindingKind::ParseError,
                message: error.message,
            }),
        }
    }

    let info_by_filename: HashMap<&str, &ModuleInfo> = infos
        .iter()
        .map(|info| (info.filename.as_str(), info))
        .collect();
    for info in &infos {
        for import in &info.named_imports {
            let resolution = resolve_export(
                &info_by_filename,
                import.target.as_str(),
                &import.name,
                &mut HashSet::default(),
            );
            let detail = match resolution {
                ResolvedExport::Ambiguous => Some(format!(
                    "\"{}\" is ambiguous through star exports of {}",
                    import.name, import.target
                )),
                ResolvedExport::NotFound => Some(format!(
                    "\"{}\" is not exported by {}",
                    import.name, import.target
                )),
                ResolvedExport::Found(_) | ResolvedExport::Unknown => None,
            };
            if let Some(detail) = detail {
                findings.push(OutputFinding {
                    filename: info.filename.clone(),
                    line: import.line,
                    column: import.column,
                    kind: OutputFindingKind::MissingImportedName,
                    message: detail,
                });
            }
        }
    }

    findings.extend(unresolved_reference_findings(&infos, inputs));

    findings
}

/// Report free identifiers that the emitted graph, or the original input,
/// proves undeclared. Two independent proofs:
///
/// 1. Exactly one *other* emitted module declares the name at module scope.
///    A split that separated a declaration from its users without adding the
///    import/export edge produces exactly this shape. Names declared by
///    several modules are ambiguous (reused minified locals) and skipped.
///    This is a spelling heuristic, not binding identity, so input evidence
///    overrides it when available.
/// 2. With inputs supplied: the name is free in no input file. Whatever was
///    free in the input (host globals, define constants, dependency bugs) is
///    a faithful passthrough and suppresses both proofs; a name that is free
///    only in the output was introduced by the rewrite. An input that fails
///    to parse disables this proof instead of leaving a partial baseline.
///
/// ECMAScript built-ins and module-runtime names (`require`, `define`, ...)
/// are excluded from both proofs: rewrite rules legitimately introduce
/// `undefined` or `Object` where the input spelled them differently, and the
/// runtime names are wakaru's deliberate representation of unconverted
/// module edges (or are reported by the CommonJS residual check). Well-known
/// host globals are excluded from the sibling proof only: a polyfill module
/// that declares `console` at module scope does not make every other
/// module's `console` a defect.
fn unresolved_reference_findings(
    infos: &[ModuleInfo],
    inputs: &[(String, String)],
) -> Vec<OutputFinding> {
    let mut declaring_modules: HashMap<&Atom, Vec<&str>> = HashMap::default();
    for info in infos {
        for name in &info.module_scope_names {
            declaring_modules
                .entry(name)
                .or_default()
                .push(info.filename.as_str());
        }
    }

    let mut findings = Vec::new();
    // A partial baseline proves nothing: if any input fails to parse, report
    // that and skip the input comparison rather than call names absent from
    // a file that was never read.
    let mut input_free_names = (!inputs.is_empty()).then(HashSet::default);
    for (filename, source) in inputs {
        match collect_input_free_names(filename, source) {
            Ok(names) => {
                if let Some(free_names) = input_free_names.as_mut() {
                    free_names.extend(names);
                }
            }
            Err(error) => {
                input_free_names = None;
                findings.push(OutputFinding {
                    filename: filename.clone(),
                    line: error.line,
                    column: error.column,
                    kind: OutputFindingKind::ParseError,
                    message: format!("input {}", error.message),
                });
            }
        }
    }

    for info in infos {
        for reference in &info.unresolved_refs {
            if is_ecmascript_builtin_global(&reference.name)
                || is_module_runtime_name(&reference.name)
            {
                continue;
            }
            // Input evidence is authoritative: a name the input already used
            // freely is a faithful passthrough even when some unrelated
            // output module happens to declare a local with the same spelling.
            if input_free_names
                .as_ref()
                .is_some_and(|free_names| free_names.contains(&reference.name))
            {
                continue;
            }
            let siblings: Vec<&str> = declaring_modules
                .get(&reference.name)
                .map(|modules| {
                    modules
                        .iter()
                        .copied()
                        .filter(|module| *module != info.filename.as_str())
                        .collect()
                })
                .unwrap_or_default();
            let sibling = match siblings.as_slice() {
                [sibling] if !is_well_known_host_global(&reference.name) => Some(*sibling),
                _ => None,
            };
            let message = if let Some(sibling) = sibling {
                Some(format!(
                    "unresolved identifier \"{}\" is declared at module scope only by {sibling}, which this module does not import",
                    reference.name
                ))
            } else if input_free_names.is_some() {
                Some(format!(
                    "unresolved identifier \"{}\" is not a free identifier anywhere in the input",
                    reference.name
                ))
            } else {
                None
            };
            if let Some(message) = message {
                findings.push(OutputFinding {
                    filename: info.filename.clone(),
                    line: reference.line,
                    column: reference.column,
                    kind: OutputFindingKind::UnresolvedReference,
                    message,
                });
            }
        }
    }
    findings
}

/// Every identifier the resolver leaves unresolved in an input file,
/// including `typeof` operands and writes: anything free in the input excuses
/// the same free name in the output.
fn collect_input_free_names(
    filename: &str,
    source: &str,
) -> Result<HashSet<Atom>, ValidationParseError> {
    let ParsedModule { mut module, .. } =
        parse_for_validation(filename, source, SourceGoal::Ambiguous)?;
    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
    let mut collector = UnresolvedRefCollector {
        unresolved_mark,
        skip_typeof_operands: false,
        references: Vec::new(),
    };
    module.visit_with(&mut collector);
    Ok(collector
        .references
        .into_iter()
        .map(|(name, _)| name)
        .collect())
}

/// Free names that emitted modules carry on purpose: CommonJS/AMD runtime
/// bindings the pipeline keeps for edges it cannot convert (`require`,
/// `define`), the CommonJS names reported by the residual check, and the
/// bundler `global` helper's rewrite target.
fn is_module_runtime_name(name: &Atom) -> bool {
    matches!(
        name.as_ref(),
        "require" | "module" | "exports" | "define" | "global" | "__dirname" | "__filename"
    )
}

/// Host globals that polyfill or shim modules commonly declare at module
/// scope. This is a noise filter for the sibling proof, not an environment
/// model: an unlisted name yields a false positive that the input comparison
/// still removes.
fn is_well_known_host_global(name: &Atom) -> bool {
    matches!(
        name.as_ref(),
        "window"
            | "self"
            | "document"
            | "navigator"
            | "location"
            | "history"
            | "screen"
            | "console"
            | "process"
            | "Buffer"
            | "setTimeout"
            | "clearTimeout"
            | "setInterval"
            | "clearInterval"
            | "setImmediate"
            | "clearImmediate"
            | "queueMicrotask"
            | "requestAnimationFrame"
            | "cancelAnimationFrame"
            | "requestIdleCallback"
            | "structuredClone"
            | "fetch"
            | "Headers"
            | "Request"
            | "Response"
            | "AbortController"
            | "AbortSignal"
            | "URL"
            | "URLSearchParams"
            | "TextEncoder"
            | "TextDecoder"
            | "Blob"
            | "File"
            | "FileReader"
            | "FormData"
            | "XMLHttpRequest"
            | "WebSocket"
            | "EventSource"
            | "Worker"
            | "MessageChannel"
            | "MessagePort"
            | "MessageEvent"
            | "Event"
            | "EventTarget"
            | "CustomEvent"
            | "ErrorEvent"
            | "PromiseRejectionEvent"
            | "Node"
            | "Element"
            | "HTMLElement"
            | "SVGElement"
            | "DocumentFragment"
            | "Text"
            | "Comment"
            | "Range"
            | "MutationObserver"
            | "ResizeObserver"
            | "IntersectionObserver"
            | "PerformanceObserver"
            | "performance"
            | "crypto"
            | "localStorage"
            | "sessionStorage"
            | "indexedDB"
            | "matchMedia"
            | "getComputedStyle"
            | "atob"
            | "btoa"
            | "alert"
            | "confirm"
            | "prompt"
            | "Image"
            | "Audio"
            | "Option"
            | "DOMParser"
            | "XMLSerializer"
            | "DOMException"
            | "ReadableStream"
            | "WritableStream"
            | "TransformStream"
            | "CompressionStream"
            | "DecompressionStream"
            | "BroadcastChannel"
            | "SharedWorker"
            | "ServiceWorker"
            | "Notification"
            | "postMessage"
            | "importScripts"
            | "WebAssembly"
            | "Deno"
            | "Bun"
    )
}

/// Value, function, constructor, and namespace properties of the ECMAScript
/// global object (ES2025), plus the implicit `arguments` binding.
fn is_ecmascript_builtin_global(name: &Atom) -> bool {
    matches!(
        name.as_ref(),
        "globalThis"
            | "Infinity"
            | "NaN"
            | "undefined"
            | "arguments"
            | "eval"
            | "isFinite"
            | "isNaN"
            | "parseFloat"
            | "parseInt"
            | "decodeURI"
            | "decodeURIComponent"
            | "encodeURI"
            | "encodeURIComponent"
            | "escape"
            | "unescape"
            | "AggregateError"
            | "Array"
            | "ArrayBuffer"
            | "BigInt"
            | "BigInt64Array"
            | "BigUint64Array"
            | "Boolean"
            | "DataView"
            | "Date"
            | "Error"
            | "EvalError"
            | "FinalizationRegistry"
            | "Float16Array"
            | "Float32Array"
            | "Float64Array"
            | "Function"
            | "Int8Array"
            | "Int16Array"
            | "Int32Array"
            | "Iterator"
            | "Map"
            | "Number"
            | "Object"
            | "Promise"
            | "Proxy"
            | "RangeError"
            | "ReferenceError"
            | "RegExp"
            | "Set"
            | "SharedArrayBuffer"
            | "String"
            | "Symbol"
            | "SyntaxError"
            | "TypeError"
            | "Uint8Array"
            | "Uint8ClampedArray"
            | "Uint16Array"
            | "Uint32Array"
            | "URIError"
            | "WeakMap"
            | "WeakRef"
            | "WeakSet"
            | "Atomics"
            | "JSON"
            | "Math"
            | "Reflect"
            | "Intl"
    )
}

struct ModuleInfo {
    filename: String,
    /// Explicitly exported names, including "default". Star re-exports are
    /// tracked separately and excluded from duplicate detection.
    explicit_exports: Vec<ExplicitExport>,
    /// How an explicit export resolves. The first entry wins here; duplicate
    /// explicit names are reported separately and do not need a second graph
    /// interpretation.
    explicit_resolutions: HashMap<Atom, ExplicitExportResolution>,
    /// Resolved in-set targets of `export * from`.
    star_targets: Vec<String>,
    /// The export set is unknowable: `export * from` an external package or
    /// a module outside the validated set.
    open_exports: bool,
    named_imports: Vec<NamedImport>,
    /// Names declared at module scope by `var`/`let`/`const`/`using`,
    /// function, and class declarations (import bindings excluded).
    module_scope_names: HashSet<Atom>,
    /// Free identifier uses, excluding direct `typeof` operands, the
    /// CommonJS runtime names reported separately, import/export specifiers,
    /// and intrinsic JSX element names.
    unresolved_refs: Vec<UnresolvedRef>,
}

struct UnresolvedRef {
    name: Atom,
    line: usize,
    column: usize,
}

struct NamedImport {
    /// Resolved in-set target filename.
    target: String,
    /// The external name requested from the provider ("default" for default
    /// imports).
    name: Atom,
    line: usize,
    column: usize,
}

struct ExplicitExport {
    name: Atom,
    line: usize,
    column: usize,
}

#[derive(Clone)]
enum ExplicitExportResolution {
    /// A local binding declared or referenced by this module.
    Local(Atom),
    /// An indirect named re-export from another emitted module.
    Reexport { target: String, imported: Atom },
    /// A namespace object for another emitted module.
    Namespace(String),
    /// A definite explicit export whose precise binding is outside the
    /// validated graph or otherwise unnecessary for ambiguity checks.
    Synthetic,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ExportOrigin {
    module: String,
    binding: Atom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolvedExport {
    Found(ExportOrigin),
    NotFound,
    Ambiguous,
    /// An external, missing, or unparsable provider prevents a safe claim.
    Unknown,
}

/// Resolve one requested export using ESM's star-export ambiguity rule. Two
/// star paths may forward the same origin (a diamond) without conflict, while
/// distinct origins make the name ambiguous and therefore not exported.
fn resolve_export(
    infos: &HashMap<&str, &ModuleInfo>,
    filename: &str,
    name: &Atom,
    resolving: &mut HashSet<(String, Atom)>,
) -> ResolvedExport {
    let key = (filename.to_string(), name.clone());
    if !resolving.insert(key.clone()) {
        return ResolvedExport::NotFound;
    }

    let result = resolve_export_inner(infos, filename, name, resolving);
    resolving.remove(&key);
    result
}

fn resolve_export_inner(
    infos: &HashMap<&str, &ModuleInfo>,
    filename: &str,
    name: &Atom,
    resolving: &mut HashSet<(String, Atom)>,
) -> ResolvedExport {
    let Some(info) = infos.get(filename).copied() else {
        return ResolvedExport::Unknown;
    };

    if let Some(explicit) = info.explicit_resolutions.get(name) {
        return match explicit {
            ExplicitExportResolution::Local(binding) => ResolvedExport::Found(ExportOrigin {
                module: filename.to_string(),
                binding: binding.clone(),
            }),
            ExplicitExportResolution::Reexport { target, imported } => {
                resolve_export(infos, target, imported, resolving)
            }
            ExplicitExportResolution::Namespace(target) => ResolvedExport::Found(ExportOrigin {
                module: target.clone(),
                binding: Atom::from("*namespace*"),
            }),
            ExplicitExportResolution::Synthetic => ResolvedExport::Found(ExportOrigin {
                module: filename.to_string(),
                binding: name.clone(),
            }),
        };
    }

    // `export *` never forwards default, including from an external module.
    if name.as_ref() == "default" {
        return ResolvedExport::NotFound;
    }

    let mut found = None;
    let mut ambiguous = false;
    let mut unknown = info.open_exports;
    for target in &info.star_targets {
        match resolve_export(infos, target, name, resolving) {
            ResolvedExport::Found(origin) => {
                if found.as_ref().is_some_and(|existing| existing != &origin) {
                    ambiguous = true;
                } else {
                    found = Some(origin);
                }
            }
            ResolvedExport::NotFound => {}
            ResolvedExport::Ambiguous => ambiguous = true,
            ResolvedExport::Unknown => unknown = true,
        }
    }

    if unknown {
        ResolvedExport::Unknown
    } else if ambiguous {
        ResolvedExport::Ambiguous
    } else if let Some(origin) = found {
        ResolvedExport::Found(origin)
    } else {
        ResolvedExport::NotFound
    }
}

fn analyze_module(
    filename: &str,
    source: &str,
    filenames: &HashSet<&str>,
    source_goal: SourceGoal,
) -> Result<(ModuleInfo, Vec<OutputFinding>), ValidationParseError> {
    let ParsedModule {
        mut module,
        source_map,
    } = parse_for_validation(filename, source, source_goal)?;
    let is_esm = source_goal == SourceGoal::Module;

    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

    let mut findings = Vec::new();
    let mut info = ModuleInfo {
        filename: filename.to_string(),
        explicit_exports: Vec::new(),
        explicit_resolutions: HashMap::default(),
        star_targets: Vec::new(),
        open_exports: false,
        named_imports: Vec::new(),
        module_scope_names: HashSet::default(),
        unresolved_refs: Vec::new(),
    };
    let mut import_bindings: HashMap<Id, Atom> = HashMap::default();

    // ESM resolves a local export of an imported binding to the *source*
    // module's binding, so `import { x } from "./a.js"; export { x };` and
    // `export { x } from "./a.js";` are the same edge. Map each import local
    // to that indirect resolution up front (exports may precede imports in
    // the body). External and dangling specifiers keep their raw text, which
    // `resolve_export` treats as an unknown provider.
    let mut import_reexports: HashMap<Id, ExplicitExportResolution> = HashMap::default();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        let Some(spec_value) = import.src.value.as_str() else {
            continue;
        };
        let target = resolve_in_set(filename, spec_value, filenames)
            .unwrap_or_else(|| spec_value.to_string());
        for spec in &import.specifiers {
            let (local, resolution) = match spec {
                ImportSpecifier::Named(named) => {
                    let imported = named
                        .imported
                        .as_ref()
                        .map(module_export_name_atom)
                        .unwrap_or_else(|| named.local.sym.clone());
                    (
                        &named.local,
                        ExplicitExportResolution::Reexport {
                            target: target.clone(),
                            imported,
                        },
                    )
                }
                ImportSpecifier::Default(default) => (
                    &default.local,
                    ExplicitExportResolution::Reexport {
                        target: target.clone(),
                        imported: Atom::from("default"),
                    },
                ),
                ImportSpecifier::Namespace(ns) => (
                    &ns.local,
                    ExplicitExportResolution::Namespace(target.clone()),
                ),
            };
            import_reexports.insert(local.to_id(), resolution);
        }
    }

    for item in &module.body {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::Import(import) => {
                let target = check_relative_ref(
                    filename,
                    &import.src,
                    "import",
                    filenames,
                    &source_map,
                    &mut findings,
                );
                for spec in &import.specifiers {
                    let (local, requested, span) = match spec {
                        ImportSpecifier::Named(named) => {
                            let requested = named
                                .imported
                                .as_ref()
                                .map(module_export_name_atom)
                                .unwrap_or_else(|| named.local.sym.clone());
                            (&named.local, Some(requested), named.span)
                        }
                        ImportSpecifier::Default(default) => {
                            (&default.local, Some(Atom::from("default")), default.span)
                        }
                        ImportSpecifier::Namespace(ns) => (&ns.local, None, ns.span),
                    };
                    import_bindings.insert(local.to_id(), local.sym.clone());
                    if let (Some(target), Some(name)) = (&target, requested) {
                        let (line, column) = source_location(&source_map, span);
                        info.named_imports.push(NamedImport {
                            target: target.clone(),
                            name,
                            line,
                            column,
                        });
                    }
                }
            }
            ModuleDecl::ExportDecl(export) => {
                for (name, span) in export_decl_bindings(&export.decl) {
                    record_explicit_export(
                        &mut info,
                        name.clone(),
                        ExplicitExportResolution::Local(name),
                        span,
                        &source_map,
                    );
                }
            }
            ModuleDecl::ExportNamed(named) => {
                let target = named.src.as_ref().and_then(|src| {
                    check_relative_ref(
                        filename,
                        src,
                        "export",
                        filenames,
                        &source_map,
                        &mut findings,
                    )
                });
                for spec in &named.specifiers {
                    match spec {
                        swc_core::ecma::ast::ExportSpecifier::Named(spec) => {
                            let orig = module_export_name_atom(&spec.orig);
                            let exported = spec
                                .exported
                                .as_ref()
                                .map(module_export_name_atom)
                                .unwrap_or_else(|| orig.clone());
                            let resolution = if let Some(target) = &target {
                                ExplicitExportResolution::Reexport {
                                    target: target.clone(),
                                    imported: orig.clone(),
                                }
                            } else if named.src.is_none() {
                                match local_export_resolution(
                                    &spec.orig,
                                    &import_reexports,
                                    unresolved_mark,
                                ) {
                                    Some(resolution) => resolution,
                                    None => {
                                        findings.push(finding_at_span(
                                            filename,
                                            &source_map,
                                            spec.span,
                                            OutputFindingKind::MissingLocalExport,
                                            format!(
                                                "local export binding \"{orig}\" is not declared"
                                            ),
                                        ));
                                        ExplicitExportResolution::Synthetic
                                    }
                                }
                            } else {
                                ExplicitExportResolution::Synthetic
                            };
                            record_explicit_export(
                                &mut info,
                                exported,
                                resolution,
                                spec.span,
                                &source_map,
                            );
                            if let Some(target) = &target {
                                let (line, column) = source_location(&source_map, spec.span);
                                info.named_imports.push(NamedImport {
                                    target: target.clone(),
                                    name: orig,
                                    line,
                                    column,
                                });
                            }
                        }
                        swc_core::ecma::ast::ExportSpecifier::Namespace(spec) => {
                            let resolution = target
                                .clone()
                                .map(ExplicitExportResolution::Namespace)
                                .unwrap_or(ExplicitExportResolution::Synthetic);
                            record_explicit_export(
                                &mut info,
                                module_export_name_atom(&spec.name),
                                resolution,
                                spec.span,
                                &source_map,
                            );
                        }
                        swc_core::ecma::ast::ExportSpecifier::Default(spec) => {
                            record_explicit_export(
                                &mut info,
                                spec.exported.sym.clone(),
                                ExplicitExportResolution::Synthetic,
                                spec.span(),
                                &source_map,
                            );
                        }
                    }
                }
            }
            ModuleDecl::ExportDefaultDecl(_) | ModuleDecl::ExportDefaultExpr(_) => {
                record_explicit_export(
                    &mut info,
                    Atom::from("default"),
                    ExplicitExportResolution::Synthetic,
                    decl.span(),
                    &source_map,
                );
            }
            ModuleDecl::ExportAll(export_all) => {
                match check_relative_ref(
                    filename,
                    &export_all.src,
                    "export",
                    filenames,
                    &source_map,
                    &mut findings,
                ) {
                    Some(target) => info.star_targets.push(target),
                    None => {
                        // Bare specifier (external package) or dangling: the
                        // export set is unknowable either way.
                        info.open_exports = true;
                    }
                }
            }
            _ => {}
        }
    }

    let mut duplicates_seen: HashSet<Atom> = HashSet::default();
    let mut counted: HashSet<Atom> = HashSet::default();
    for export in &info.explicit_exports {
        if !counted.insert(export.name.clone()) && duplicates_seen.insert(export.name.clone()) {
            findings.push(OutputFinding {
                filename: filename.to_string(),
                line: export.line,
                column: export.column,
                kind: OutputFindingKind::DuplicateExport,
                message: format!("duplicate export \"{}\"", export.name),
            });
        }
    }

    if is_esm {
        findings.extend(duplicate_declaration_findings(
            filename,
            &module,
            &source_map,
        ));
    }

    let mut const_collector = ConstBindingCollector {
        bindings: HashMap::default(),
    };
    module.visit_with(&mut const_collector);

    if is_esm {
        let mut residual_collector = EsmCommonJsResidualCollector {
            unresolved_mark,
            residuals: Vec::new(),
        };
        module.visit_with(&mut residual_collector);
        findings.extend(
            residual_collector
                .residuals
                .into_iter()
                .map(|(name, span)| {
                    finding_at_span(
                        filename,
                        &source_map,
                        span,
                        OutputFindingKind::EsmCommonJsResidual,
                        format!(
                            "unresolved CommonJS runtime binding \"{name}\" remains in ESM output"
                        ),
                    )
                }),
        );
    }

    let mut ref_visitor = BodyVisitor {
        filename,
        filenames,
        source_map: &source_map,
        unresolved_mark,
        writes: Vec::new(),
        dangling: Vec::new(),
    };
    module.visit_with(&mut ref_visitor);
    findings.extend(ref_visitor.dangling);

    // Do not classify unresolved identifier writes here. A resolver mark only
    // proves that the binding is outside this file; browsers, Node, workers,
    // and embedder hosts expose different outer bindings. Without an explicit
    // environment model, calling any such write definitely invalid would turn
    // host-global accesses into fatal false positives.
    for (id, name, span) in &ref_visitor.writes {
        if import_bindings.contains_key(id) {
            findings.push(finding_at_span(
                filename,
                &source_map,
                *span,
                OutputFindingKind::AssignToImport,
                format!("assignment to imported binding \"{name}\""),
            ));
        } else if const_collector.bindings.contains_key(id) {
            findings.push(finding_at_span(
                filename,
                &source_map,
                *span,
                OutputFindingKind::AssignToConst,
                format!("assignment to const binding \"{name}\""),
            ));
        }
    }

    info.module_scope_names = module_scope_declared_names(&module);
    let mut unresolved_collector = UnresolvedRefCollector {
        unresolved_mark,
        skip_typeof_operands: true,
        references: Vec::new(),
    };
    module.visit_with(&mut unresolved_collector);
    info.unresolved_refs = unresolved_collector
        .references
        .into_iter()
        .filter(|(name, _)| !matches!(name.as_ref(), "module" | "exports"))
        .map(|(name, span)| {
            let (line, column) = source_location(&source_map, span);
            UnresolvedRef { name, line, column }
        })
        .collect();

    Ok((info, findings))
}

/// Names bound at module scope by declarations, including hoisted `var`s and
/// exported declarations. Import bindings are not declarations: a module that
/// merely imports a name is not where the name lives.
fn module_scope_declared_names(module: &Module) -> HashSet<Atom> {
    let mut bindings = Vec::new();
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                record_direct_lexical_decl(&export.decl, &mut bindings);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(export)) => {
                let ident = match &export.decl {
                    DefaultDecl::Fn(function) => function.ident.as_ref(),
                    DefaultDecl::Class(class) => class.ident.as_ref(),
                    DefaultDecl::TsInterfaceDecl(_) => None,
                };
                if let Some(ident) = ident {
                    bindings.push(ScopeBinding {
                        name: ident.sym.clone(),
                        span: ident.span,
                        kind: DeclarationKind::Lexical,
                    });
                }
            }
            ModuleItem::Stmt(Stmt::Decl(decl)) => record_direct_lexical_decl(decl, &mut bindings),
            _ => {}
        }
    }
    // `var` declarations hoist to module scope from any nested statement,
    // including exported ones.
    let mut var_collector = VarBindingCollector::default();
    module.visit_with(&mut var_collector);
    bindings.extend(var_collector.bindings);
    bindings.into_iter().map(|binding| binding.name).collect()
}

/// Collect free identifier uses by resolver identity. Import declarations and
/// export specifiers are skipped (they are validated separately), and
/// lowercase JSX element names are intrinsic tags rather than bindings.
struct UnresolvedRefCollector {
    unresolved_mark: Mark,
    skip_typeof_operands: bool,
    references: Vec<(Atom, Span)>,
}

impl Visit for UnresolvedRefCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.ctxt.outer() == self.unresolved_mark {
            self.references.push((ident.sym.clone(), ident.span));
        }
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        if self.skip_typeof_operands
            && unary.op == UnaryOp::TypeOf
            && matches!(strip_parens(&unary.arg), Expr::Ident(_))
        {
            return;
        }
        unary.visit_children_with(self);
    }

    fn visit_import_decl(&mut self, _: &swc_core::ecma::ast::ImportDecl) {}

    fn visit_export_specifier(&mut self, _: &swc_core::ecma::ast::ExportSpecifier) {}

    fn visit_jsx_element_name(&mut self, name: &swc_core::ecma::ast::JSXElementName) {
        if let swc_core::ecma::ast::JSXElementName::Ident(ident) = name {
            if ident
                .sym
                .chars()
                .next()
                .is_some_and(|first| first.is_ascii_lowercase())
            {
                return;
            }
        }
        name.visit_children_with(self);
    }
}

/// The source goal a filename states on its own, for the validator, the
/// rewrite pipeline, and output finalization alike: `.mjs`/`.mts` is a module,
/// `.cjs`/`.cts` a script (either case), and every other name says nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceGoal {
    Module,
    Script,
    Ambiguous,
}

/// Classify each emitted file's source goal. Explicit `.mjs` / `.mts` names,
/// module syntax, module-only parses, and in-set import targets select ESM.
/// Explicit `.cjs` / `.cts` names remain script/CommonJS even when imported by
/// an ESM file. All other syntaxless files may fall back to classic script.
fn classify_source_goals(
    modules: &[(String, String)],
    filenames: &HashSet<&str>,
) -> HashMap<String, SourceGoal> {
    let mut source_goals: HashMap<String, SourceGoal> = modules
        .iter()
        .map(|(filename, _)| (filename.clone(), filename_source_goal(filename)))
        .collect();
    let mut referenced_targets = Vec::new();

    for (filename, source) in modules {
        match source_goals
            .get(filename)
            .copied()
            .unwrap_or(SourceGoal::Ambiguous)
        {
            SourceGoal::Module => {
                if let Ok(parsed) = parse_program(filename, source, true) {
                    referenced_targets.extend(esm_target_specifiers(filename, &parsed.module));
                }
            }
            SourceGoal::Script => {
                if let Ok(parsed) = parse_program(filename, source, false) {
                    referenced_targets.extend(esm_target_specifiers(filename, &parsed.module));
                }
            }
            SourceGoal::Ambiguous => match parse_program(filename, source, true) {
                Ok(parsed) => {
                    if has_module_syntax(&parsed.module)
                        || parse_program(filename, source, false).is_err()
                    {
                        source_goals.insert(filename.clone(), SourceGoal::Module);
                    }
                    referenced_targets.extend(esm_target_specifiers(filename, &parsed.module));
                }
                Err(_) => {
                    if let Ok(parsed) = parse_program(filename, source, false) {
                        referenced_targets.extend(esm_target_specifiers(filename, &parsed.module));
                    }
                }
            },
        }
    }

    for (from_filename, specifier) in referenced_targets {
        if let Some(target) = resolve_in_set(&from_filename, &specifier, filenames) {
            if source_goals.get(&target) != Some(&SourceGoal::Script) {
                source_goals.insert(target, SourceGoal::Module);
            }
        }
    }
    source_goals
}

pub(crate) fn filename_source_goal(filename: &str) -> SourceGoal {
    let Some(extension) = Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
    else {
        return SourceGoal::Ambiguous;
    };
    if extension.eq_ignore_ascii_case("mjs") || extension.eq_ignore_ascii_case("mts") {
        SourceGoal::Module
    } else if extension.eq_ignore_ascii_case("cjs") || extension.eq_ignore_ascii_case("cts") {
        SourceGoal::Script
    } else {
        SourceGoal::Ambiguous
    }
}

fn esm_target_specifiers(filename: &str, module: &Module) -> Vec<(String, String)> {
    let mut collector = EsmTargetSpecifierCollector {
        filename: filename.to_string(),
        specifiers: Vec::new(),
    };
    module.visit_with(&mut collector);
    collector.specifiers
}

struct EsmTargetSpecifierCollector {
    specifiers: Vec<(String, String)>,
    filename: String,
}

impl Visit for EsmTargetSpecifierCollector {
    fn visit_module(&mut self, module: &Module) {
        module.visit_children_with(self);
    }

    fn visit_import_decl(&mut self, import: &swc_core::ecma::ast::ImportDecl) {
        if let Some(specifier) = import.src.value.as_str() {
            self.specifiers
                .push((self.filename.clone(), specifier.to_string()));
        }
    }

    fn visit_named_export(&mut self, export: &swc_core::ecma::ast::NamedExport) {
        if let Some(source) = export.src.as_ref() {
            if let Some(specifier) = source.value.as_str() {
                self.specifiers
                    .push((self.filename.clone(), specifier.to_string()));
            }
        }
        export.visit_children_with(self);
    }

    fn visit_export_all(&mut self, export: &swc_core::ecma::ast::ExportAll) {
        if let Some(specifier) = export.src.value.as_str() {
            self.specifiers
                .push((self.filename.clone(), specifier.to_string()));
        }
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if matches!(call.callee, Callee::Import(_)) {
            if let Some(argument) = call
                .args
                .first()
                .filter(|argument| argument.spread.is_none())
            {
                if let Expr::Lit(Lit::Str(specifier)) = argument.expr.as_ref() {
                    if let Some(specifier) = specifier.value.as_str() {
                        self.specifiers
                            .push((self.filename.clone(), specifier.to_string()));
                    }
                }
            }
        }
        call.visit_children_with(self);
    }
}

/// Parse with an explicit module or script goal when the filename/graph proves
/// one. Otherwise try module first and fall back to classic script; single-file
/// decompile output can legitimately be sloppy-mode code.
fn parse_for_validation(
    filename: &str,
    source: &str,
    source_goal: SourceGoal,
) -> Result<ParsedModule, ValidationParseError> {
    match source_goal {
        SourceGoal::Module => parse_program(filename, source, true),
        SourceGoal::Script => parse_program(filename, source, false),
        SourceGoal::Ambiguous => match parse_program(filename, source, true) {
            Ok(module) => Ok(module),
            Err(module_error) => match parse_program(filename, source, false) {
                Ok(parsed) if !has_module_syntax(&parsed.module) => Ok(parsed),
                _ => Err(module_error),
            },
        },
    }
}

struct ParsedModule {
    module: Module,
    source_map: Lrc<SourceMap>,
}

struct ValidationParseError {
    message: String,
    line: usize,
    column: usize,
}

fn parse_program(
    filename: &str,
    source: &str,
    as_module: bool,
) -> Result<ParsedModule, ValidationParseError> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        FileName::Custom(filename.to_string()).into(),
        source.to_string(),
    );
    // JSX is always enabled: standard-level UnJsx emits JSX syntax into .js
    // files. This is safe for plain JS — an expression cannot begin with `<`,
    // so no comparison is reinterpreted.
    let syntax = Syntax::Es(EsSyntax {
        jsx: true,
        ..Default::default()
    });
    let lexer = Lexer::new(syntax, Default::default(), StringInput::from(&*fm), None);
    let mut parser = Parser::new_from(lexer);
    let parsed: Result<Program, _> = if as_module {
        parser.parse_module().map(Program::Module)
    } else {
        parser.parse_script().map(Program::Script)
    };
    let module = parsed.map_err(|error| {
        parse_error_at_span(
            &cm,
            error.span(),
            format!("parse failed: {:?}", error.kind()),
        )
    })?;
    if let Some(error) = parser.take_errors().into_iter().next() {
        return Err(parse_error_at_span(
            &cm,
            error.span(),
            format!("parse failed: {:?}", error.kind()),
        ));
    }
    let module = match module {
        Program::Module(module) => module,
        Program::Script(script) => Module {
            span: script.span,
            body: script.body.into_iter().map(ModuleItem::Stmt).collect(),
            shebang: script.shebang,
        },
    };
    Ok(ParsedModule {
        module,
        source_map: cm,
    })
}

fn source_location(source_map: &SourceMap, span: Span) -> (usize, usize) {
    if span.lo.0 == 0 {
        return (1, 1);
    }
    let location = source_map.lookup_char_pos(span.lo);
    (location.line, location.col_display + 1)
}

/// `export { x }` where `x` is an import binding is an indirect export of the
/// source module's binding, not a local one.
fn local_export_resolution(
    orig: &ModuleExportName,
    import_reexports: &HashMap<Id, ExplicitExportResolution>,
    unresolved_mark: Mark,
) -> Option<ExplicitExportResolution> {
    let ModuleExportName::Ident(ident) = orig else {
        return None;
    };
    if let Some(resolution) = import_reexports.get(&ident.to_id()) {
        Some(resolution.clone())
    } else if ident.ctxt.outer() == unresolved_mark {
        None
    } else {
        Some(ExplicitExportResolution::Local(ident.sym.clone()))
    }
}

fn record_explicit_export(
    info: &mut ModuleInfo,
    name: Atom,
    resolution: ExplicitExportResolution,
    span: Span,
    source_map: &SourceMap,
) {
    let (line, column) = source_location(source_map, span);
    info.explicit_resolutions
        .entry(name.clone())
        .or_insert(resolution);
    info.explicit_exports
        .push(ExplicitExport { name, line, column });
}

fn finding_at_span(
    filename: &str,
    source_map: &SourceMap,
    span: Span,
    kind: OutputFindingKind,
    message: String,
) -> OutputFinding {
    let (line, column) = source_location(source_map, span);
    OutputFinding {
        filename: filename.to_string(),
        line,
        column,
        kind,
        message,
    }
}

fn parse_error_at_span(
    source_map: &SourceMap,
    span: Span,
    message: String,
) -> ValidationParseError {
    let (line, column) = source_location(source_map, span);
    ValidationParseError {
        message,
        line,
        column,
    }
}

fn has_module_syntax(module: &Module) -> bool {
    module
        .body
        .iter()
        .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
}

fn export_decl_bindings(decl: &Decl) -> Vec<(Atom, Span)> {
    match decl {
        Decl::Var(var) => {
            let mut collector = ExportBindingCollector::default();
            for declarator in &var.decls {
                declarator.name.visit_with(&mut collector);
            }
            collector.bindings
        }
        Decl::Fn(f) => vec![(f.ident.sym.clone(), f.ident.span)],
        Decl::Class(c) => vec![(c.ident.sym.clone(), c.ident.span)],
        _ => Vec::new(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeclarationKind {
    Lexical,
    Var,
    Function,
    Parameter,
}

struct ScopeBinding {
    name: Atom,
    span: Span,
    kind: DeclarationKind,
}

/// Collect declaration conflicts that are ESM early errors. Each lexical
/// scope compares its direct lexical declarations with `var` declarations
/// that hoist through nested statements to that scope. Blocks, switch case
/// lists, loop-head scopes, and function bodies therefore need independent
/// checks rather than one resolver-identity sweep over the module.
fn duplicate_declaration_findings(
    filename: &str,
    module: &Module,
    source_map: &SourceMap,
) -> Vec<OutputFinding> {
    declaration_conflicts(module, true)
        .into_iter()
        .map(|conflict| {
            finding_at_span(
                filename,
                source_map,
                conflict.span,
                OutputFindingKind::DuplicateDeclaration,
                format!(
                    "duplicate {} declaration \"{}\"",
                    conflict.scope, conflict.name
                ),
            )
        })
        .collect()
}

/// Producer diagnostics also accept classic scripts. Check lexical conflicts
/// there without imposing ESM restrictions on repeated functions or parameters.
/// Use printed names and syntactic scopes: resolver identities on invalid
/// declarations need not agree, and `var` can cross nested block boundaries.
pub(crate) fn conflicting_lexical_declaration_names(module: &Module) -> Vec<Atom> {
    let mut seen = HashSet::default();
    declaration_conflicts(module, false)
        .into_iter()
        .filter_map(|conflict| seen.insert(conflict.name.clone()).then_some(conflict.name))
        .collect()
}

struct DeclarationConflict {
    name: Atom,
    span: Span,
    scope: &'static str,
}

fn declaration_conflicts(module: &Module, esm_early_errors: bool) -> Vec<DeclarationConflict> {
    let mut bindings = direct_module_lexical_bindings(module);
    let mut var_collector = VarBindingCollector::default();
    module.visit_with(&mut var_collector);
    bindings.extend(var_collector.bindings);

    let mut visitor = DuplicateDeclarationVisitor {
        esm_early_errors,
        conflicts: Vec::new(),
        reported_locations: HashSet::default(),
    };
    visitor.check_scope(bindings, "module-scope");
    module.visit_children_with(&mut visitor);
    visitor.conflicts
}

fn direct_module_lexical_bindings(module: &Module) -> Vec<ScopeBinding> {
    let mut bindings = Vec::new();
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                for specifier in &import.specifiers {
                    let local = match specifier {
                        ImportSpecifier::Named(named) => &named.local,
                        ImportSpecifier::Default(default) => &default.local,
                        ImportSpecifier::Namespace(namespace) => &namespace.local,
                    };
                    bindings.push(ScopeBinding {
                        name: local.sym.clone(),
                        span: local.span,
                        kind: DeclarationKind::Lexical,
                    });
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                record_direct_lexical_decl(&export.decl, &mut bindings);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(export)) => {
                let ident = match &export.decl {
                    DefaultDecl::Fn(function) => function.ident.as_ref(),
                    DefaultDecl::Class(class) => class.ident.as_ref(),
                    DefaultDecl::TsInterfaceDecl(_) => None,
                };
                if let Some(ident) = ident {
                    bindings.push(ScopeBinding {
                        name: ident.sym.clone(),
                        span: ident.span,
                        kind: DeclarationKind::Lexical,
                    });
                }
            }
            ModuleItem::Stmt(swc_core::ecma::ast::Stmt::Decl(decl)) => {
                record_direct_lexical_decl(decl, &mut bindings);
            }
            _ => {}
        }
    }
    bindings
}

fn record_direct_lexical_decl(decl: &Decl, bindings: &mut Vec<ScopeBinding>) {
    match decl {
        Decl::Var(var) if var.kind != VarDeclKind::Var => {
            for declarator in &var.decls {
                record_scope_binding_pat(&declarator.name, DeclarationKind::Lexical, bindings);
            }
        }
        Decl::Using(using_decl) => {
            for declarator in &using_decl.decls {
                record_scope_binding_pat(&declarator.name, DeclarationKind::Lexical, bindings);
            }
        }
        Decl::Fn(function) => bindings.push(ScopeBinding {
            name: function.ident.sym.clone(),
            span: function.ident.span,
            kind: DeclarationKind::Function,
        }),
        Decl::Class(class) => bindings.push(ScopeBinding {
            name: class.ident.sym.clone(),
            span: class.ident.span,
            kind: DeclarationKind::Lexical,
        }),
        _ => {}
    }
}

fn record_scope_binding_pat(pat: &Pat, kind: DeclarationKind, bindings: &mut Vec<ScopeBinding>) {
    let mut collector = ScopeBindingPatternCollector::default();
    pat.visit_with(&mut collector);
    bindings.extend(
        collector
            .bindings
            .into_iter()
            .map(|(name, span)| ScopeBinding { name, span, kind }),
    );
}

#[derive(Default)]
struct ScopeBindingPatternCollector {
    bindings: Vec<(Atom, Span)>,
}

impl Visit for ScopeBindingPatternCollector {
    fn visit_binding_ident(&mut self, binding: &BindingIdent) {
        self.bindings
            .push((binding.id.sym.clone(), binding.id.span));
    }

    // Defaults and computed keys are expressions, not bindings in the pattern.
    fn visit_expr(&mut self, _: &Expr) {}
}

#[derive(Default)]
struct VarBindingCollector {
    bindings: Vec<ScopeBinding>,
}

impl Visit for VarBindingCollector {
    fn visit_var_decl(&mut self, decl: &VarDecl) {
        if decl.kind == VarDeclKind::Var {
            for declarator in &decl.decls {
                record_scope_binding_pat(
                    &declarator.name,
                    DeclarationKind::Var,
                    &mut self.bindings,
                );
            }
        }
        decl.visit_children_with(self);
    }

    // `var` only hoists to the nearest function or static-block boundary.
    fn visit_function(&mut self, _: &swc_core::ecma::ast::Function) {}

    fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}

    fn visit_class(&mut self, _: &swc_core::ecma::ast::Class) {}
}

fn direct_statement_lexical_bindings<'a>(
    statements: impl IntoIterator<Item = &'a Stmt>,
) -> Vec<ScopeBinding> {
    let mut bindings = Vec::new();
    for statement in statements {
        if let Stmt::Decl(decl) = statement {
            record_direct_lexical_decl(decl, &mut bindings);
        }
    }
    bindings
}

fn function_body_bindings(body: &FunctionBody) -> Vec<ScopeBinding> {
    let mut bindings = direct_statement_lexical_bindings(&body.stmts);
    // Function declarations at the top of a function body contribute var
    // names, unlike block-level function declarations in strict ESM code.
    for statement in &body.stmts {
        if let Stmt::Decl(Decl::Fn(function)) = statement {
            if let Some(binding) = bindings.iter_mut().find(|binding| {
                binding.name == function.ident.sym && binding.span == function.ident.span
            }) {
                binding.kind = DeclarationKind::Var;
            }
        }
    }
    let mut var_collector = VarBindingCollector::default();
    body.visit_with(&mut var_collector);
    bindings.extend(var_collector.bindings);
    bindings
}

fn parameter_bindings<'a>(parameters: impl IntoIterator<Item = &'a Pat>) -> Vec<ScopeBinding> {
    let mut bindings = Vec::new();
    for parameter in parameters {
        record_scope_binding_pat(parameter, DeclarationKind::Parameter, &mut bindings);
    }
    bindings
}

/// Function declarations in a function body are var-scoped, so only direct
/// lexical variables, classes, and `using` declarations conflict with formal
/// parameters. Block-level function declarations elsewhere remain lexical.
fn direct_function_body_lexical_bindings(body: &FunctionBody) -> Vec<ScopeBinding> {
    let mut bindings = Vec::new();
    for statement in &body.stmts {
        if let Stmt::Decl(decl) = statement {
            if !matches!(decl, Decl::Fn(_)) {
                record_direct_lexical_decl(decl, &mut bindings);
            }
        }
    }
    bindings
}

struct DuplicateDeclarationVisitor {
    esm_early_errors: bool,
    conflicts: Vec<DeclarationConflict>,
    reported_locations: HashSet<(Atom, u32)>,
}

impl DuplicateDeclarationVisitor {
    fn check_scope(&mut self, mut bindings: Vec<ScopeBinding>, scope: &'static str) {
        bindings.sort_by_key(|binding| binding.span.lo);
        let mut first_kinds: HashMap<Atom, DeclarationKind> = HashMap::default();
        let mut reported_names = HashSet::default();
        for mut binding in bindings {
            if !self.esm_early_errors && binding.kind == DeclarationKind::Function {
                binding.kind = DeclarationKind::Var;
            }
            let Some(first_kind) = first_kinds.get(&binding.name).copied() else {
                first_kinds.insert(binding.name.clone(), binding.kind);
                continue;
            };
            // Repeated `var` declarations denote the same binding and are
            // legal. Every other repeat in this lexical scope is an early
            // error.
            if (first_kind == DeclarationKind::Var && binding.kind == DeclarationKind::Var)
                || (!self.esm_early_errors
                    && first_kind == DeclarationKind::Parameter
                    && binding.kind == DeclarationKind::Parameter)
            {
                continue;
            }
            if reported_names.insert(binding.name.clone())
                && self
                    .reported_locations
                    .insert((binding.name.clone(), binding.span.lo.0))
            {
                self.conflicts.push(DeclarationConflict {
                    name: binding.name,
                    span: binding.span,
                    scope,
                });
            }
        }
    }

    fn check_block(&mut self, block: &BlockStmt) {
        let mut bindings = direct_statement_lexical_bindings(&block.stmts);
        let mut var_collector = VarBindingCollector::default();
        block.visit_with(&mut var_collector);
        bindings.extend(var_collector.bindings);
        self.check_scope(bindings, "block-scope");
    }

    fn check_loop_head(&mut self, head: &ForHead, body: &Stmt) {
        let mut bindings = Vec::new();
        match head {
            ForHead::VarDecl(decl) if decl.kind != VarDeclKind::Var => {
                for declarator in &decl.decls {
                    record_scope_binding_pat(
                        &declarator.name,
                        DeclarationKind::Lexical,
                        &mut bindings,
                    );
                }
            }
            ForHead::UsingDecl(decl) => {
                for declarator in &decl.decls {
                    record_scope_binding_pat(
                        &declarator.name,
                        DeclarationKind::Lexical,
                        &mut bindings,
                    );
                }
            }
            ForHead::VarDecl(_) | ForHead::Pat(_) => {}
        }
        if bindings.is_empty() {
            return;
        }
        let mut var_collector = VarBindingCollector::default();
        body.visit_with(&mut var_collector);
        bindings.extend(var_collector.bindings);
        self.check_scope(bindings, "for-scope");
    }
}

impl Visit for DuplicateDeclarationVisitor {
    fn visit_function(&mut self, function: &Function) {
        let mut bindings = parameter_bindings(function.params.iter().map(|param| &param.pat));
        if let Some(body) = &function.body {
            bindings.extend(direct_function_body_lexical_bindings(body));
        }
        self.check_scope(bindings, "function-parameter");
        function.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        let mut bindings = parameter_bindings(arrow.params.iter());
        if let ArrowFunctionBody::FunctionBody(body) = arrow.body.as_ref() {
            bindings.extend(direct_function_body_lexical_bindings(body));
        }
        self.check_scope(bindings, "function-parameter");
        arrow.visit_children_with(self);
    }

    fn visit_constructor(&mut self, constructor: &Constructor) {
        let mut bindings = parameter_bindings(constructor.params.iter().filter_map(|param| {
            let ParamOrTsParamProp::Param(param) = param else {
                return None;
            };
            Some(&param.pat)
        }));
        if let Some(body) = &constructor.body {
            bindings.extend(direct_function_body_lexical_bindings(body));
        }
        self.check_scope(bindings, "function-parameter");
        constructor.visit_children_with(self);
    }

    fn visit_catch_clause(&mut self, catch: &CatchClause) {
        if let Some(parameter) = &catch.param {
            let mut bindings = parameter_bindings(std::iter::once(parameter));
            bindings.extend(direct_statement_lexical_bindings(&catch.body.stmts));
            self.check_scope(bindings, "catch-parameter");
        }
        catch.visit_children_with(self);
    }

    fn visit_block_stmt(&mut self, block: &BlockStmt) {
        self.check_block(block);
        block.visit_children_with(self);
    }

    fn visit_function_body(&mut self, body: &FunctionBody) {
        self.check_scope(function_body_bindings(body), "function-body");
        body.visit_children_with(self);
    }

    fn visit_switch_stmt(&mut self, switch: &SwitchStmt) {
        let mut bindings = direct_statement_lexical_bindings(
            switch.cases.iter().flat_map(|case| case.cons.iter()),
        );
        let mut var_collector = VarBindingCollector::default();
        switch.visit_with(&mut var_collector);
        bindings.extend(var_collector.bindings);
        self.check_scope(bindings, "switch-scope");
        switch.visit_children_with(self);
    }

    fn visit_for_stmt(&mut self, statement: &ForStmt) {
        if let Some(swc_core::ecma::ast::VarDeclOrExpr::VarDecl(decl)) = &statement.init {
            if decl.kind != VarDeclKind::Var {
                let head = ForHead::VarDecl(decl.clone());
                self.check_loop_head(&head, &statement.body);
            }
        }
        statement.visit_children_with(self);
    }

    fn visit_for_in_stmt(&mut self, statement: &ForInStmt) {
        self.check_loop_head(&statement.left, &statement.body);
        statement.visit_children_with(self);
    }

    fn visit_for_of_stmt(&mut self, statement: &ForOfStmt) {
        self.check_loop_head(&statement.left, &statement.body);
        statement.visit_children_with(self);
    }
}

#[derive(Default)]
struct ExportBindingCollector {
    bindings: Vec<(Atom, Span)>,
}

impl Visit for ExportBindingCollector {
    fn visit_binding_ident(&mut self, binding: &BindingIdent) {
        self.bindings
            .push((binding.id.sym.clone(), binding.id.span));
    }

    // Default values and computed property keys are expressions, not bindings
    // declared by the exported pattern. Do not collect nested function params.
    fn visit_expr(&mut self, _: &Expr) {}
}

fn module_export_name_atom(name: &ModuleExportName) -> Atom {
    match name {
        ModuleExportName::Ident(ident) => ident.sym.clone(),
        ModuleExportName::Str(s) => Atom::from(s.value.as_str().unwrap_or_default()),
    }
}

/// Resolve a specifier against the module set. Returns the resolved in-set
/// filename, `None` for bare/external specifiers, and records a dangling
/// finding for relative specifiers that don't resolve to a set member.
fn check_relative_ref(
    from_filename: &str,
    spec: &Str,
    context: &str,
    filenames: &HashSet<&str>,
    source_map: &SourceMap,
    findings: &mut Vec<OutputFinding>,
) -> Option<String> {
    let spec_value = spec.value.as_str()?;
    if !(spec_value.starts_with("./") || spec_value.starts_with("../")) {
        return None;
    }
    match resolve_in_set(from_filename, spec_value, filenames) {
        Some(target) => Some(target),
        None => {
            findings.push(finding_at_span(
                from_filename,
                source_map,
                spec.span,
                OutputFindingKind::DanglingRelativeRef,
                format!("unresolved relative {context} \"{spec_value}\""),
            ));
            None
        }
    }
}

fn resolve_in_set(from_filename: &str, spec: &str, filenames: &HashSet<&str>) -> Option<String> {
    let target = crate::module_path::resolve_relative_specifier(from_filename, spec)?;
    if filenames.contains(target.as_str()) {
        return Some(target);
    }
    let with_js = format!("{target}.js");
    if filenames.contains(with_js.as_str()) {
        return Some(with_js);
    }
    None
}

/// Collect every `const`-declared binding id (any scope; the resolver makes
/// ids unique, so no scope tracking is needed).
struct ConstBindingCollector {
    bindings: HashMap<Id, Atom>,
}

/// Collect runtime uses of free `module` / `exports` from files that the
/// emitted syntax identifies as ESM. Resolver contexts distinguish those
/// globals from same-spelled locals and parameters. A direct `typeof module`
/// or `typeof exports` probe is safe even when the binding is absent, so it is
/// not a defect by itself; dereferencing a member beneath `typeof` is not safe
/// and remains visible.
struct EsmCommonJsResidualCollector {
    unresolved_mark: Mark,
    residuals: Vec<(Atom, Span)>,
}

impl EsmCommonJsResidualCollector {
    fn unresolved_commonjs_name(&self, ident: &Ident) -> Option<Atom> {
        (ident.ctxt.outer() == self.unresolved_mark
            && matches!(ident.sym.as_ref(), "module" | "exports"))
        .then(|| ident.sym.clone())
    }
}

impl Visit for EsmCommonJsResidualCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        if let Some(name) = self.unresolved_commonjs_name(ident) {
            self.residuals.push((name, ident.span));
        }
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        if unary.op == UnaryOp::TypeOf
            && matches!(strip_parens(&unary.arg), Expr::Ident(ident)
                if self.unresolved_commonjs_name(ident).is_some())
        {
            return;
        }
        unary.visit_children_with(self);
    }
}

impl Visit for ConstBindingCollector {
    fn visit_var_decl(&mut self, decl: &VarDecl) {
        if decl.kind == VarDeclKind::Const {
            for declarator in &decl.decls {
                let pat_ids: Vec<Id> = find_pat_ids(&declarator.name);
                for id in pat_ids {
                    let name = id.0.clone();
                    self.bindings.insert(id, name);
                }
            }
        }
        decl.visit_children_with(self);
    }
}

/// Walk the whole module recording binding writes (by resolved id) and
/// dangling `require("./…")` / `import("./…")` references.
struct BodyVisitor<'a> {
    filename: &'a str,
    filenames: &'a HashSet<&'a str>,
    source_map: &'a SourceMap,
    unresolved_mark: Mark,
    writes: Vec<(Id, Atom, Span)>,
    dangling: Vec<OutputFinding>,
}

impl BodyVisitor<'_> {
    fn record_write(&mut self, ident: &Ident) {
        self.writes
            .push((ident.to_id(), ident.sym.clone(), ident.span));
    }

    fn write_target_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Ident(ident) => self.record_write(ident),
            Expr::Paren(paren) => self.write_target_expr(&paren.expr),
            _ => expr.visit_with(self),
        }
    }

    fn write_pat(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(binding) => self.record_write(&binding.id),
            Pat::Array(array) => {
                for element in array.elems.iter().flatten() {
                    self.write_pat(element);
                }
            }
            Pat::Object(object) => {
                for prop in &object.props {
                    match prop {
                        ObjectPatProp::KeyValue(key_value) => {
                            if let PropName::Computed(computed) = &key_value.key {
                                computed.visit_with(self);
                            }
                            self.write_pat(&key_value.value);
                        }
                        ObjectPatProp::Assign(assign) => {
                            self.record_write(&assign.key);
                            assign.value.visit_with(self);
                        }
                        ObjectPatProp::Rest(rest) => self.write_pat(&rest.arg),
                    }
                }
            }
            Pat::Assign(assign) => {
                self.write_pat(&assign.left);
                assign.right.visit_with(self);
            }
            Pat::Rest(rest) => self.write_pat(&rest.arg),
            Pat::Expr(expr) => self.write_target_expr(expr),
            Pat::Invalid(_) => {}
        }
    }
}

impl Visit for BodyVisitor<'_> {
    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
                self.record_write(&binding.id)
            }
            AssignTarget::Simple(SimpleAssignTarget::Paren(paren)) => {
                self.write_target_expr(&paren.expr)
            }
            AssignTarget::Simple(simple) => simple.visit_children_with(self),
            AssignTarget::Pat(AssignTargetPat::Array(array)) => {
                for element in array.elems.iter().flatten() {
                    self.write_pat(element);
                }
            }
            AssignTarget::Pat(AssignTargetPat::Object(object)) => {
                for prop in &object.props {
                    match prop {
                        ObjectPatProp::KeyValue(key_value) => {
                            if let PropName::Computed(computed) = &key_value.key {
                                computed.visit_with(self);
                            }
                            self.write_pat(&key_value.value);
                        }
                        ObjectPatProp::Assign(assign) => {
                            self.record_write(&assign.key);
                            assign.value.visit_with(self);
                        }
                        ObjectPatProp::Rest(rest) => self.write_pat(&rest.arg),
                    }
                }
            }
            AssignTarget::Pat(AssignTargetPat::Invalid(_)) => {}
        }
        assign.right.visit_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        self.write_target_expr(&update.arg);
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        if let ForHead::Pat(pat) = &stmt.left {
            self.write_pat(pat);
        } else {
            stmt.left.visit_with(self);
        }
        stmt.right.visit_with(self);
        stmt.body.visit_with(self);
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        if let ForHead::Pat(pat) = &stmt.left {
            self.write_pat(pat);
        } else {
            stmt.left.visit_with(self);
        }
        stmt.right.visit_with(self);
        stmt.body.visit_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        let context = match &call.callee {
            Callee::Import(_) => Some("dynamic import"),
            Callee::Expr(callee) => match callee.as_ref() {
                Expr::Ident(ident)
                    if ident.sym.as_ref() == "require"
                        && ident.ctxt.outer() == self.unresolved_mark =>
                {
                    Some("require")
                }
                _ => None,
            },
            _ => None,
        };
        if let Some(context) = context {
            if let Some(arg) = call.args.first() {
                if arg.spread.is_none() {
                    if let Expr::Lit(Lit::Str(s)) = arg.expr.as_ref() {
                        check_relative_ref(
                            self.filename,
                            s,
                            context,
                            self.filenames,
                            self.source_map,
                            &mut self.dangling,
                        );
                    }
                }
            }
        }
        call.visit_children_with(self);
    }
}
