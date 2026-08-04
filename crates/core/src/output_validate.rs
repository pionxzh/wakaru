//! Emitted-module graph validator for normal unpack output.
//!
//! A development/benchmark tool: parses a set of emitted modules and reports
//! structural defects that would make the output fail to load as ESM —
//! dangling relative references, imports of names the provider does not
//! export, duplicate exports, and writes to imported or `const` bindings.
//!
//! Raw output is deliberately out of scope: `--raw` promises only "no
//! readability transforms" and carries no module-graph contract. Validate
//! normal output only.
//!
//! The checks are conservative: a provider whose export set is unknowable
//! (it re-exports an external package or a missing module) suppresses
//! missing-name findings for its consumers rather than guessing.

use std::collections::{HashMap, HashSet};

use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, AssignTargetPat, CallExpr, Callee, Decl, Expr, ForHead, ForInStmt,
    ForOfStmt, Id, Ident, ImportSpecifier, Lit, Module, ModuleDecl, ModuleExportName, ModuleItem,
    ObjectPatProp, Pat, Program, PropName, SimpleAssignTarget, Str, UpdateExpr, VarDecl,
    VarDeclKind,
};
use swc_core::ecma::atoms::Atom;
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};

/// A structural defect found in emitted output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFinding {
    /// The module the defect was found in.
    pub filename: String,
    pub kind: OutputFindingKind,
    /// Human-readable detail (specifier, binding name, parse message).
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputFindingKind {
    /// The module does not parse (as a module, nor as a classic script when
    /// it contains no import/export syntax).
    ParseError,
    /// A `./`-relative import/export/require target is not in the module set.
    DanglingRelativeRef,
    /// A named or default import of a name the provider does not export.
    MissingImportedName,
    /// The same name is exported more than once by one module.
    DuplicateExport,
    /// Assignment or update targeting an imported binding.
    AssignToImport,
    /// Assignment or update targeting a `const` binding.
    AssignToConst,
}

impl OutputFindingKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutputFindingKind::ParseError => "parse_error",
            OutputFindingKind::DanglingRelativeRef => "dangling_relative_ref",
            OutputFindingKind::MissingImportedName => "missing_imported_name",
            OutputFindingKind::DuplicateExport => "duplicate_export",
            OutputFindingKind::AssignToImport => "assign_to_import",
            OutputFindingKind::AssignToConst => "assign_to_const",
        }
    }
}

/// Validate a set of emitted modules as one graph.
///
/// `modules` maps output-relative filenames (`/`-separated, e.g.
/// `"src/entry.js"`) to their source text. Findings are returned in input
/// module order, then AST order within a module.
pub fn validate_output_modules(modules: &[(String, String)]) -> Vec<OutputFinding> {
    GLOBALS.set(&Default::default(), || validate_inner(modules))
}

fn validate_inner(modules: &[(String, String)]) -> Vec<OutputFinding> {
    let filenames: HashSet<&str> = modules.iter().map(|(name, _)| name.as_str()).collect();
    let mut findings = Vec::new();
    let mut infos = Vec::new();

    for (filename, source) in modules {
        match analyze_module(filename, source, &filenames) {
            Ok((info, mut local_findings)) => {
                findings.append(&mut local_findings);
                infos.push(info);
            }
            Err(message) => findings.push(OutputFinding {
                filename: filename.clone(),
                kind: OutputFindingKind::ParseError,
                message,
            }),
        }
    }

    let exports = resolve_export_closures(&infos);
    for info in &infos {
        for import in &info.named_imports {
            let Some(target_exports) = exports.get(import.target.as_str()) else {
                continue; // dangling target: already reported, don't stack
            };
            if target_exports.open {
                continue;
            }
            if !target_exports.names.contains(&import.name) {
                findings.push(OutputFinding {
                    filename: info.filename.clone(),
                    kind: OutputFindingKind::MissingImportedName,
                    message: format!("\"{}\" is not exported by {}", import.name, import.target),
                });
            }
        }
    }

    findings
}

struct ModuleInfo {
    filename: String,
    /// Explicitly exported names, including "default". Star re-exports are
    /// tracked separately and excluded from duplicate detection.
    explicit_exports: Vec<Atom>,
    /// Resolved in-set targets of `export * from`.
    star_targets: Vec<String>,
    /// The export set is unknowable: `export * from` an external package or
    /// a module outside the validated set.
    open_exports: bool,
    named_imports: Vec<NamedImport>,
}

struct NamedImport {
    /// Resolved in-set target filename.
    target: String,
    /// The external name requested from the provider ("default" for default
    /// imports).
    name: Atom,
}

struct ExportClosure {
    names: HashSet<Atom>,
    open: bool,
}

/// Union star re-exports to a fixpoint so `export * from "./mid.js"` chains
/// resolve, and propagate openness through them.
fn resolve_export_closures(infos: &[ModuleInfo]) -> HashMap<String, ExportClosure> {
    let mut closures: HashMap<String, ExportClosure> = infos
        .iter()
        .map(|info| {
            (
                info.filename.clone(),
                ExportClosure {
                    names: info.explicit_exports.iter().cloned().collect(),
                    open: info.open_exports,
                },
            )
        })
        .collect();

    loop {
        let mut changed = false;
        for info in infos {
            for target in &info.star_targets {
                let Some(source) = closures.get(target.as_str()) else {
                    continue;
                };
                // `export *` never forwards default.
                let forwarded: Vec<Atom> = source
                    .names
                    .iter()
                    .filter(|name| name.as_ref() != "default")
                    .cloned()
                    .collect();
                let source_open = source.open;
                let own = closures
                    .get_mut(info.filename.as_str())
                    .expect("closure exists for every analyzed module");
                for name in forwarded {
                    changed |= own.names.insert(name);
                }
                if source_open && !own.open {
                    own.open = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return closures;
        }
    }
}

fn analyze_module(
    filename: &str,
    source: &str,
    filenames: &HashSet<&str>,
) -> Result<(ModuleInfo, Vec<OutputFinding>), String> {
    let mut module = parse_for_validation(filename, source)?;

    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

    let mut findings = Vec::new();
    let mut info = ModuleInfo {
        filename: filename.to_string(),
        explicit_exports: Vec::new(),
        star_targets: Vec::new(),
        open_exports: false,
        named_imports: Vec::new(),
    };
    let mut import_bindings: HashMap<Id, Atom> = HashMap::new();

    for item in &module.body {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::Import(import) => {
                let target =
                    check_relative_ref(filename, &import.src, "import", filenames, &mut findings);
                for spec in &import.specifiers {
                    let (local, requested) = match spec {
                        ImportSpecifier::Named(named) => {
                            let requested = named
                                .imported
                                .as_ref()
                                .map(module_export_name_atom)
                                .unwrap_or_else(|| named.local.sym.clone());
                            (&named.local, Some(requested))
                        }
                        ImportSpecifier::Default(default) => {
                            (&default.local, Some(Atom::from("default")))
                        }
                        ImportSpecifier::Namespace(ns) => (&ns.local, None),
                    };
                    import_bindings.insert(local.to_id(), local.sym.clone());
                    if let (Some(target), Some(name)) = (&target, requested) {
                        info.named_imports.push(NamedImport {
                            target: target.clone(),
                            name,
                        });
                    }
                }
            }
            ModuleDecl::ExportDecl(export) => {
                info.explicit_exports
                    .extend(export_decl_names(&export.decl));
            }
            ModuleDecl::ExportNamed(named) => {
                let target = named.src.as_ref().and_then(|src| {
                    check_relative_ref(filename, src, "export", filenames, &mut findings)
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
                            info.explicit_exports.push(exported);
                            if let Some(target) = &target {
                                info.named_imports.push(NamedImport {
                                    target: target.clone(),
                                    name: orig,
                                });
                            }
                        }
                        swc_core::ecma::ast::ExportSpecifier::Namespace(spec) => {
                            info.explicit_exports
                                .push(module_export_name_atom(&spec.name));
                        }
                        swc_core::ecma::ast::ExportSpecifier::Default(spec) => {
                            info.explicit_exports.push(spec.exported.sym.clone());
                        }
                    }
                }
            }
            ModuleDecl::ExportDefaultDecl(_) | ModuleDecl::ExportDefaultExpr(_) => {
                info.explicit_exports.push(Atom::from("default"));
            }
            ModuleDecl::ExportAll(export_all) => {
                match check_relative_ref(
                    filename,
                    &export_all.src,
                    "export",
                    filenames,
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

    let mut duplicates_seen: HashSet<Atom> = HashSet::new();
    let mut counted: HashSet<Atom> = HashSet::new();
    for name in &info.explicit_exports {
        if !counted.insert(name.clone()) && duplicates_seen.insert(name.clone()) {
            findings.push(OutputFinding {
                filename: filename.to_string(),
                kind: OutputFindingKind::DuplicateExport,
                message: format!("duplicate export \"{name}\""),
            });
        }
    }

    let mut const_collector = ConstBindingCollector {
        bindings: HashMap::new(),
    };
    module.visit_with(&mut const_collector);

    let mut ref_visitor = BodyVisitor {
        filename,
        filenames,
        unresolved_mark,
        writes: Vec::new(),
        dangling: Vec::new(),
    };
    module.visit_with(&mut ref_visitor);
    findings.extend(ref_visitor.dangling);

    for (id, name) in &ref_visitor.writes {
        if import_bindings.contains_key(id) {
            findings.push(OutputFinding {
                filename: filename.to_string(),
                kind: OutputFindingKind::AssignToImport,
                message: format!("assignment to imported binding \"{name}\""),
            });
        } else if const_collector.bindings.contains_key(id) {
            findings.push(OutputFinding {
                filename: filename.to_string(),
                kind: OutputFindingKind::AssignToConst,
                message: format!("assignment to const binding \"{name}\""),
            });
        }
    }

    Ok((info, findings))
}

/// Parse with the module goal; when that only fails on recoverable errors and
/// the file contains no import/export syntax, retry as a classic script
/// (single-file decompile output can legitimately be sloppy-mode code).
fn parse_for_validation(filename: &str, source: &str) -> Result<Module, String> {
    match parse_program(filename, source, true) {
        Ok(module) => Ok(module),
        Err(module_error) => match parse_program(filename, source, false) {
            Ok(module) if !has_module_syntax(&module) => Ok(module),
            _ => Err(module_error),
        },
    }
}

fn parse_program(filename: &str, source: &str, as_module: bool) -> Result<Module, String> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        FileName::Custom(filename.to_string()).into(),
        source.to_string(),
    );
    let syntax = Syntax::Es(EsSyntax {
        jsx: filename.ends_with(".jsx"),
        ..Default::default()
    });
    let lexer = Lexer::new(syntax, Default::default(), StringInput::from(&*fm), None);
    let mut parser = Parser::new_from(lexer);
    let parsed: Result<Program, _> = if as_module {
        parser.parse_module().map(Program::Module)
    } else {
        parser.parse_script().map(Program::Script)
    };
    let module = parsed.map_err(|error| format!("parse failed: {:?}", error.kind()))?;
    if let Some(error) = parser.take_errors().into_iter().next() {
        return Err(format!("parse failed: {:?}", error.kind()));
    }
    Ok(match module {
        Program::Module(module) => module,
        Program::Script(script) => Module {
            span: script.span,
            body: script.body.into_iter().map(ModuleItem::Stmt).collect(),
            shebang: script.shebang,
        },
    })
}

fn has_module_syntax(module: &Module) -> bool {
    module
        .body
        .iter()
        .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
}

fn export_decl_names(decl: &Decl) -> Vec<Atom> {
    match decl {
        Decl::Var(var) => {
            let mut names = Vec::new();
            for declarator in &var.decls {
                let ids: Vec<Id> = find_pat_ids(&declarator.name);
                names.extend(ids.into_iter().map(|(sym, _)| sym));
            }
            names
        }
        Decl::Fn(f) => vec![f.ident.sym.clone()],
        Decl::Class(c) => vec![c.ident.sym.clone()],
        _ => Vec::new(),
    }
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
    findings: &mut Vec<OutputFinding>,
) -> Option<String> {
    let spec = spec.value.as_str()?;
    if !(spec.starts_with("./") || spec.starts_with("../")) {
        return None;
    }
    match resolve_in_set(from_filename, spec, filenames) {
        Some(target) => Some(target),
        None => {
            findings.push(OutputFinding {
                filename: from_filename.to_string(),
                kind: OutputFindingKind::DanglingRelativeRef,
                message: format!("unresolved relative {context} \"{spec}\""),
            });
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
    unresolved_mark: Mark,
    writes: Vec<(Id, Atom)>,
    dangling: Vec<OutputFinding>,
}

impl BodyVisitor<'_> {
    fn record_write(&mut self, ident: &Ident) {
        self.writes.push((ident.to_id(), ident.sym.clone()));
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
                            &mut self.dangling,
                        );
                    }
                }
            }
        }
        call.visit_children_with(self);
    }
}
