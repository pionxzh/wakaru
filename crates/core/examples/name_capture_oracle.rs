//! Research oracle: after the rule pipeline, does any unresolved-marked
//! identifier reference sit inside a scope that declares the same emitted
//! name? Such a reference prints as text and is captured by the local
//! binding — a silent miscompile.
//!
//! Two more identifier-context checks run on the same walk:
//!
//! - *unmarked*: a reference with an empty `SyntaxContext`. The resolver never
//!   leaves one, so it is a rule that built the identifier from a string.
//! - *dangling*: a reference whose context is neither unresolved nor empty and
//!   matches no binding `(sym, ctxt)` declared anywhere in the module. A rule
//!   minted a fresh context for a binding and then rebuilt the reference (or
//!   the declaration) instead of cloning it. The text still prints, so
//!   snapshots do not see it; every `(sym, ctxt)`-keyed pass does.
//!
//! Also records, per module: `with` / direct `eval` presence, and for the
//! names the module-wide skip policy cares about (`undefined`, `Infinity`,
//! `NaN`) whether the module declares the name anywhere at all versus
//! whether an unresolved reference is actually captured. The gap between
//! those two numbers is what a scope-precise index would recover.
//!
//! Usage: name_capture_oracle <file-or-dir>... > out.jsonl
//! One JSON object per module on stdout; aggregate on stderr.
//!
//! `ORACLE_ATTRIBUTE=1` re-runs the pipeline rule by rule for every module
//! with an unmarked or dangling residual and reports the first rule after
//! which each residual appears (`attribution`: `[kind, name, line, rule]`).
//! Unmarked names are attributed once per corpus; dangling references are
//! attributed per module by name and line.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use swc_core::common::{
    sync::Lrc, FileName, Globals, Mark, SourceMap, Span, Spanned, SyntaxContext, GLOBALS,
};
use swc_core::ecma::ast::*;
use swc_core::ecma::atoms::Atom;
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};
use wakaru_core::{apply_rules, rule_names, DceMode, RewriteLevel, RulePipelineOptions};

const WATCHED: [&str; 3] = ["undefined", "Infinity", "NaN"];

#[derive(Default, Debug)]
struct Report {
    unresolved_refs: usize,
    unmarked_refs: usize,
    unmarked: Vec<(Atom, usize)>,
    /// Marked, non-unresolved reference that matches no declared binding
    /// `(sym, ctxt)` in the module.
    dangling: Vec<(Atom, usize)>,
    /// Unresolved reference whose emitted name an enclosing scope declares.
    captured: Vec<(Atom, usize, &'static str)>,
    jsx_tag_refs: usize,
    export_spec_unmarked: usize,
    /// Unresolved reference inside a `with` body.
    in_with: Vec<(Atom, usize)>,
    has_with: bool,
    has_direct_eval: bool,
    /// name -> (declared anywhere in module, unresolved refs, captured refs)
    watched: Vec<(Atom, bool, usize, usize)>,
}

struct Frame {
    names: HashSet<Atom>,
    in_with: bool,
    kind: &'static str,
}

struct Oracle<'a> {
    unresolved_mark: Mark,
    cm: &'a SourceMap,
    /// Line of the innermost enclosing statement with a real span, so a
    /// synthesized identifier (dummy span) still reports a usable position.
    enclosing_line: usize,
    frames: Vec<Frame>,
    report: Report,
    all_binding_names: HashSet<Atom>,
    /// Every binding the module declares, by resolver identity.
    all_binding_ids: HashSet<(Atom, SyntaxContext)>,
    /// Marked, non-unresolved references; matched against `all_binding_ids`
    /// once the walk is complete, since a reference may precede its
    /// declaration.
    marked_refs: Vec<(Atom, SyntaxContext, usize)>,
}

impl Oracle<'_> {
    fn line(&self, span: Span) -> usize {
        if span.is_dummy() {
            self.enclosing_line
        } else {
            self.cm.lookup_char_pos(span.lo).line
        }
    }

    fn push_kind(&mut self, kind: &'static str, names: HashSet<Atom>) {
        self.frames.push(Frame {
            names,
            in_with: false,
            kind,
        });
    }

    fn pop(&mut self) {
        self.frames.pop();
    }

    fn check_ref(&mut self, id: &Ident) {
        if id.ctxt.outer() == self.unresolved_mark {
            self.report.unresolved_refs += 1;
            let line = self.line(id.span);
            if let Some(frame) = self.frames.iter().rev().find(|f| f.names.contains(&id.sym)) {
                self.report
                    .captured
                    .push((id.sym.clone(), line, frame.kind));
            }
            if self.frames.iter().any(|f| f.in_with) {
                self.report.in_with.push((id.sym.clone(), line));
            }
        } else if id.ctxt.outer() == Mark::root() {
            self.report.unmarked_refs += 1;
            let line = self.line(id.span);
            self.report.unmarked.push((id.sym.clone(), line));
        } else {
            let line = self.line(id.span);
            self.marked_refs.push((id.sym.clone(), id.ctxt, line));
        }
    }

    fn declare(&mut self, id: &Ident) {
        self.all_binding_names.insert(id.sym.clone());
        self.all_binding_ids.insert((id.sym.clone(), id.ctxt));
    }

    /// Lexical declarations directly in a statement list (let/const/class,
    /// plus function declarations, which are block-scoped in modules).
    fn lexical_names(stmts: &[Stmt]) -> HashSet<Atom> {
        let mut names = HashSet::new();
        for stmt in stmts {
            match stmt {
                Stmt::Decl(Decl::Var(v)) if v.kind != VarDeclKind::Var => {
                    for d in &v.decls {
                        pat_names(&d.name, &mut names);
                    }
                }
                Stmt::Decl(Decl::Class(c)) => {
                    names.insert(c.ident.sym.clone());
                }
                Stmt::Decl(Decl::Fn(f)) => {
                    names.insert(f.ident.sym.clone());
                }
                _ => {}
            }
        }
        names
    }

    fn module_names(items: &[ModuleItem]) -> HashSet<Atom> {
        let mut names = HashSet::new();
        let mut stmts = Vec::new();
        for item in items {
            match item {
                ModuleItem::Stmt(s) => stmts.push(s.clone()),
                ModuleItem::ModuleDecl(ModuleDecl::Import(i)) => {
                    for s in &i.specifiers {
                        let local = match s {
                            ImportSpecifier::Named(n) => &n.local,
                            ImportSpecifier::Default(d) => &d.local,
                            ImportSpecifier::Namespace(n) => &n.local,
                        };
                        names.insert(local.sym.clone());
                    }
                }
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(e)) => {
                    stmts.push(Stmt::Decl(e.decl.clone()));
                }
                ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(e)) => match &e.decl {
                    DefaultDecl::Class(c) => {
                        if let Some(id) = &c.ident {
                            names.insert(id.sym.clone());
                        }
                    }
                    DefaultDecl::Fn(f) => {
                        if let Some(id) = &f.ident {
                            names.insert(id.sym.clone());
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        names.extend(Self::lexical_names(&stmts));
        let mut hoisted = HoistedVars::default();
        for s in &stmts {
            s.visit_with(&mut hoisted);
        }
        names.extend(hoisted.names);
        names
    }
}

/// `var` declarators and function declarations reachable without crossing a
/// function/class boundary (they hoist to the enclosing function scope).
#[derive(Default)]
struct HoistedVars {
    names: HashSet<Atom>,
}

impl Visit for HoistedVars {
    fn visit_var_decl(&mut self, v: &VarDecl) {
        if v.kind == VarDeclKind::Var {
            for d in &v.decls {
                pat_names(&d.name, &mut self.names);
            }
        }
        v.visit_children_with(self);
    }
    fn visit_fn_decl(&mut self, f: &FnDecl) {
        self.names.insert(f.ident.sym.clone());
    }
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
}

fn pat_names(pat: &Pat, out: &mut HashSet<Atom>) {
    match pat {
        Pat::Ident(b) => {
            out.insert(b.id.sym.clone());
        }
        Pat::Array(a) => {
            for e in a.elems.iter().flatten() {
                pat_names(e, out);
            }
        }
        Pat::Rest(r) => pat_names(&r.arg, out),
        Pat::Object(o) => {
            for p in &o.props {
                match p {
                    ObjectPatProp::KeyValue(kv) => pat_names(&kv.value, out),
                    ObjectPatProp::Assign(a) => {
                        out.insert(a.key.id.sym.clone());
                    }
                    ObjectPatProp::Rest(r) => pat_names(&r.arg, out),
                }
            }
        }
        Pat::Assign(a) => pat_names(&a.left, out),
        Pat::Invalid(_) | Pat::Expr(_) => {}
    }
}

fn function_scope_names(params: &[Pat], body: Option<&[Stmt]>) -> HashSet<Atom> {
    let mut names = HashSet::new();
    for p in params {
        pat_names(p, &mut names);
    }
    if let Some(stmts) = body {
        names.extend(Oracle::lexical_names(stmts));
        let mut hoisted = HoistedVars::default();
        for s in stmts {
            s.visit_with(&mut hoisted);
        }
        names.extend(hoisted.names);
    }
    names
}

impl Visit for Oracle<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        let saved = self.enclosing_line;
        if !s.span().is_dummy() {
            self.enclosing_line = self.cm.lookup_char_pos(s.span().lo).line;
        }
        s.visit_children_with(self);
        self.enclosing_line = saved;
    }

    fn visit_module(&mut self, m: &Module) {
        self.push_kind("module", Self::module_names(&m.body));
        m.visit_children_with(self);
        self.pop();
    }

    fn visit_script(&mut self, s: &Script) {
        self.push_kind("script", function_scope_names(&[], Some(&s.body)));
        s.visit_children_with(self);
        self.pop();
    }

    fn visit_ident(&mut self, id: &Ident) {
        self.check_ref(id);
    }

    fn visit_binding_ident(&mut self, b: &BindingIdent) {
        self.declare(&b.id);
        // Not a reference; do not descend into the ident.
        b.type_ann.visit_with(self);
    }

    fn visit_simple_assign_target(&mut self, t: &SimpleAssignTarget) {
        if let SimpleAssignTarget::Ident(b) = t {
            self.check_ref(&b.id);
        } else {
            t.visit_children_with(self);
        }
    }

    fn visit_jsx_element_name(&mut self, n: &JSXElementName) {
        match n {
            JSXElementName::Ident(id) if id.sym.starts_with(|c: char| c.is_ascii_lowercase()) => {
                self.report.jsx_tag_refs += 1;
            }
            JSXElementName::Ident(id) => self.check_ref(id),
            _ => n.visit_children_with(self),
        }
    }

    fn visit_labeled_stmt(&mut self, s: &LabeledStmt) {
        s.body.visit_with(self);
    }
    fn visit_break_stmt(&mut self, _: &BreakStmt) {}
    fn visit_continue_stmt(&mut self, _: &ContinueStmt) {}

    fn visit_import_decl(&mut self, i: &ImportDecl) {
        for s in &i.specifiers {
            let local = match s {
                ImportSpecifier::Named(n) => &n.local,
                ImportSpecifier::Default(d) => &d.local,
                ImportSpecifier::Namespace(n) => &n.local,
            };
            self.declare(local);
        }
    }

    fn visit_export_named_specifier(&mut self, s: &ExportNamedSpecifier) {
        if let ModuleExportName::Ident(id) = &s.orig {
            if id.ctxt == SyntaxContext::empty() {
                self.report.export_spec_unmarked += 1;
            } else {
                self.check_ref(id);
            }
        }
    }
    fn visit_export_namespace_specifier(&mut self, _: &ExportNamespaceSpecifier) {}
    fn visit_export_default_specifier(&mut self, s: &ExportDefaultSpecifier) {
        self.check_ref(&s.exported);
    }

    fn visit_prop_name(&mut self, p: &PropName) {
        if let PropName::Computed(c) = p {
            c.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, p: &MemberProp) {
        if let MemberProp::Computed(c) = p {
            c.visit_with(self);
        }
    }

    fn visit_function(&mut self, f: &Function) {
        let params: Vec<Pat> = f.params.iter().map(|p| p.pat.clone()).collect();
        let names = function_scope_names(&params, f.body.as_ref().map(|b| b.stmts.as_slice()));
        self.push_kind("fn", names);
        f.params.visit_with(self);
        f.decorators.visit_with(self);
        if let Some(body) = &f.body {
            // Body statements already contributed their lexical names.
            body.stmts.visit_with(self);
        }
        self.pop();
    }

    fn visit_arrow_expr(&mut self, a: &ArrowExpr) {
        let body_stmts = match &*a.body {
            ArrowFunctionBody::FunctionBody(b) => Some(b.stmts.as_slice()),
            ArrowFunctionBody::Expr(_) => None,
        };
        let names = function_scope_names(&a.params, body_stmts);
        self.push_kind("arrow", names);
        a.params.visit_with(self);
        match &*a.body {
            ArrowFunctionBody::FunctionBody(b) => b.stmts.visit_with(self),
            ArrowFunctionBody::Expr(e) => e.visit_with(self),
        }
        self.pop();
    }

    fn visit_fn_decl(&mut self, f: &FnDecl) {
        self.declare(&f.ident);
        f.function.visit_with(self);
    }

    fn visit_fn_expr(&mut self, f: &FnExpr) {
        let mut names = HashSet::new();
        if let Some(id) = &f.ident {
            names.insert(id.sym.clone());
            self.declare(id);
        }
        self.push_kind("fnexpr", names);
        f.function.visit_with(self);
        self.pop();
    }

    fn visit_class_decl(&mut self, c: &ClassDecl) {
        self.declare(&c.ident);
        let mut names = HashSet::new();
        names.insert(c.ident.sym.clone());
        self.push_kind("class", names);
        c.class.visit_with(self);
        self.pop();
    }

    fn visit_class_expr(&mut self, c: &ClassExpr) {
        let mut names = HashSet::new();
        if let Some(id) = &c.ident {
            names.insert(id.sym.clone());
            self.declare(id);
        }
        self.push_kind("class", names);
        c.class.visit_with(self);
        self.pop();
    }

    fn visit_block_stmt(&mut self, b: &BlockStmt) {
        self.push_kind("block", Self::lexical_names(&b.stmts));
        b.stmts.visit_with(self);
        self.pop();
    }

    fn visit_static_block(&mut self, b: &StaticBlock) {
        self.push_kind("static", function_scope_names(&[], Some(&b.body.stmts)));
        b.body.stmts.visit_with(self);
        self.pop();
    }

    fn visit_switch_stmt(&mut self, s: &SwitchStmt) {
        s.discriminant.visit_with(self);
        let stmts: Vec<Stmt> = s
            .cases
            .iter()
            .flat_map(|c| c.cons.iter().cloned())
            .collect();
        self.push_kind("switch", Self::lexical_names(&stmts));
        for c in &s.cases {
            c.test.visit_with(self);
            c.cons.visit_with(self);
        }
        self.pop();
    }

    fn visit_catch_clause(&mut self, c: &CatchClause) {
        let mut names = HashSet::new();
        if let Some(p) = &c.param {
            pat_names(p, &mut names);
        }
        self.push_kind("catch", names);
        c.param.visit_with(self);
        c.body.visit_with(self);
        self.pop();
    }

    fn visit_for_stmt(&mut self, f: &ForStmt) {
        let mut names = HashSet::new();
        if let Some(VarDeclOrExpr::VarDecl(v)) = &f.init {
            if v.kind != VarDeclKind::Var {
                for d in &v.decls {
                    pat_names(&d.name, &mut names);
                }
            }
        }
        self.push_kind("for", names);
        f.visit_children_with(self);
        self.pop();
    }

    fn visit_for_in_stmt(&mut self, f: &ForInStmt) {
        let mut names = HashSet::new();
        if let ForHead::VarDecl(v) = &f.left {
            if v.kind != VarDeclKind::Var {
                for d in &v.decls {
                    pat_names(&d.name, &mut names);
                }
            }
        }
        self.push_kind("for", names);
        f.visit_children_with(self);
        self.pop();
    }

    fn visit_for_of_stmt(&mut self, f: &ForOfStmt) {
        let mut names = HashSet::new();
        if let ForHead::VarDecl(v) = &f.left {
            if v.kind != VarDeclKind::Var {
                for d in &v.decls {
                    pat_names(&d.name, &mut names);
                }
            }
        }
        self.push_kind("for", names);
        f.visit_children_with(self);
        self.pop();
    }

    fn visit_with_stmt(&mut self, w: &WithStmt) {
        self.report.has_with = true;
        w.obj.visit_with(self);
        self.frames.push(Frame {
            names: HashSet::new(),
            in_with: true,
            kind: "with",
        });
        w.body.visit_with(self);
        self.pop();
    }

    fn visit_call_expr(&mut self, c: &CallExpr) {
        if let Callee::Expr(e) = &c.callee {
            if let Expr::Ident(id) = &**e {
                if id.sym == "eval" && id.ctxt.outer() == self.unresolved_mark {
                    self.report.has_direct_eval = true;
                }
            }
        }
        c.visit_children_with(self);
    }
}

fn run_oracle(module: &Module, unresolved_mark: Mark, cm: &SourceMap) -> Report {
    let mut oracle = Oracle {
        unresolved_mark,
        cm,
        enclosing_line: 0,
        frames: Vec::new(),
        report: Report::default(),
        all_binding_names: HashSet::new(),
        all_binding_ids: HashSet::new(),
        marked_refs: Vec::new(),
    };
    module.visit_with(&mut oracle);
    let mut report = oracle.report;
    for (sym, ctxt, line) in oracle.marked_refs {
        if !oracle.all_binding_ids.contains(&(sym.clone(), ctxt)) {
            report.dangling.push((sym, line));
        }
    }
    for name in WATCHED {
        let atom: Atom = name.into();
        let declared = oracle.all_binding_names.contains(&atom);
        // Count unresolved refs to this name by re-walking captured/in_with is
        // not enough; re-count from the visitor totals by name.
        let captured = report
            .captured
            .iter()
            .filter(|(n, _, _)| *n == atom)
            .count();
        report.watched.push((atom, declared, 0, captured));
    }
    report
}

/// Counts unresolved references per watched name (separate cheap pass).
struct WatchedRefs {
    unresolved_mark: Mark,
    counts: [usize; 3],
}

impl Visit for WatchedRefs {
    fn visit_ident(&mut self, id: &Ident) {
        if id.ctxt.outer() != self.unresolved_mark {
            return;
        }
        for (i, name) in WATCHED.iter().enumerate() {
            if id.sym == *name {
                self.counts[i] += 1;
            }
        }
    }
    fn visit_binding_ident(&mut self, _: &BindingIdent) {}
    fn visit_prop_name(&mut self, p: &PropName) {
        if let PropName::Computed(c) = p {
            c.visit_with(self);
        }
    }
    fn visit_member_prop(&mut self, p: &MemberProp) {
        if let MemberProp::Computed(c) = p {
            c.visit_with(self);
        }
    }
}

fn parse(cm: &Lrc<SourceMap>, path: &str, source: &str) -> Result<Program, String> {
    let file = cm.new_source_file(
        FileName::Custom(path.to_string()).into(),
        source.to_string(),
    );
    let syntax = Syntax::Es(EsSyntax {
        jsx: true,
        ..Default::default()
    });
    let lexer = Lexer::new(syntax, Default::default(), StringInput::from(&*file), None);
    let mut parser = Parser::new_from(lexer);
    match parser.parse_module() {
        Ok(m) => Ok(Program::Module(m)),
        Err(e) => {
            let lexer = Lexer::new(syntax, Default::default(), StringInput::from(&*file), None);
            Parser::new_from(lexer)
                .parse_script()
                .map(Program::Script)
                .map_err(|e2| format!("{e:?} / {e2:?}"))
        }
    }
}

/// Residuals present after the pipeline has run up to and including a rule:
/// unmarked reference names, and dangling references by name and line.
struct Residuals {
    unmarked: HashSet<Atom>,
    dangling: HashSet<(Atom, usize)>,
}

/// Re-runs parse → resolver → pipeline (stopping after `stop_after`) and
/// returns the residuals present at that point.
fn residuals_until(cm: &Lrc<SourceMap>, name: &str, source: &str, stop_after: &str) -> Residuals {
    let Ok(program) = parse(cm, name, source) else {
        return Residuals {
            unmarked: HashSet::new(),
            dangling: HashSet::new(),
        };
    };
    let mut module = match program {
        Program::Module(m) => m,
        Program::Script(s) => Module {
            span: s.span,
            body: s.body.into_iter().map(ModuleItem::Stmt).collect(),
            shebang: s.shebang,
        },
    };
    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
    apply_rules(
        &mut module,
        unresolved_mark,
        RulePipelineOptions::until(stop_after)
            .with_dce_mode(DceMode::TransformOnly)
            .with_rewrite_level(RewriteLevel::Standard)
            .with_current_filename(name),
    );
    let report = run_oracle(&module, unresolved_mark, cm);
    Residuals {
        unmarked: report.unmarked.into_iter().map(|(n, _)| n).collect(),
        dangling: report.dangling.into_iter().collect(),
    }
}

/// A path argument that cannot be read is an error: skipping it would report
/// zero modules and zero defects, which reads as a clean run.
fn collect_files(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if root.is_file() {
        out.push(root.to_path_buf());
        return Ok(());
    }
    let entries =
        fs::read_dir(root).map_err(|error| format!("cannot read {}: {error}", root.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else if path
            .extension()
            .is_some_and(|e| e == "js" || e == "mjs" || e == "cjs")
        {
            out.push(path);
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    for arg in env::args().skip(1) {
        collect_files(Path::new(&arg), &mut files)?;
    }
    files.sort();

    let mut totals = json!({
        "modules": 0,
        "parse_failed": 0,
        "scripts": 0,
        "input_captured_refs": 0,
        "output_captured_refs": 0,
        "output_unresolved_refs": 0,
        "output_unmarked_refs": 0,
        "input_dangling_refs": 0,
        "output_dangling_refs": 0,
        "modules_with_capture": 0,
        "modules_with_dangling": 0,
        "modules_with_with": 0,
        "modules_with_direct_eval": 0,
        "output_refs_in_with": 0,
        "watched": WATCHED.iter().map(|n| json!({"name": n, "modules_declaring": 0, "modules_declaring_and_referencing": 0, "unresolved_refs": 0, "captured_refs": 0, "modules_captured": 0})).collect::<Vec<_>>(),
    });

    let attribute = env::var("ORACLE_ATTRIBUTE").is_ok();
    let mut attributed: HashMap<Atom, &'static str> = HashMap::new();

    for path in &files {
        let source = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let name = path.to_string_lossy().to_string();
        let max_bytes: usize = env::var("ORACLE_MAX_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20 * 1024 * 1024);
        if source.len() > max_bytes {
            println!("{}", json!({"file": name, "skipped_bytes": source.len()}));
            continue;
        }
        let cm: Lrc<SourceMap> = Default::default();
        let result = GLOBALS.set(&Globals::new(), || {
            let program = match parse(&cm, &name, &source) {
                Ok(p) => p,
                Err(e) => return Err(e),
            };
            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();
            let mut module = match program {
                Program::Module(m) => m,
                Program::Script(s) => {
                    // Wrap as module for the pipeline; record it.
                    totals["scripts"] = json!(totals["scripts"].as_u64().unwrap() + 1);
                    Module {
                        span: s.span,
                        body: s.body.into_iter().map(ModuleItem::Stmt).collect(),
                        shebang: s.shebang,
                    }
                }
            };
            module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
            let input = run_oracle(&module, unresolved_mark, &cm);
            apply_rules(
                &mut module,
                unresolved_mark,
                RulePipelineOptions::default()
                    .with_dce_mode(DceMode::TransformOnly)
                    .with_rewrite_level(RewriteLevel::Standard)
                    .with_current_filename(&name),
            );
            let mut output = run_oracle(&module, unresolved_mark, &cm);
            let mut watched = WatchedRefs {
                unresolved_mark,
                counts: [0; 3],
            };
            module.visit_with(&mut watched);
            for (i, w) in output.watched.iter_mut().enumerate() {
                w.2 = watched.counts[i];
            }
            // Attribution: first rule after which each residual appears.
            let mut attribution: Vec<(&'static str, Atom, usize, &'static str)> = Vec::new();
            if attribute {
                let mut pending_unmarked: HashSet<Atom> = output
                    .unmarked
                    .iter()
                    .map(|(n, _)| n.clone())
                    .filter(|n| !attributed.contains_key(n))
                    .collect();
                let mut pending_dangling: HashSet<(Atom, usize)> =
                    output.dangling.iter().cloned().collect();
                if !pending_unmarked.is_empty() || !pending_dangling.is_empty() {
                    for rule in rule_names() {
                        let present = residuals_until(&cm, &name, &source, rule);
                        let found: Vec<Atom> = pending_unmarked
                            .iter()
                            .filter(|n| present.unmarked.contains(*n))
                            .cloned()
                            .collect();
                        for n in found {
                            pending_unmarked.remove(&n);
                            attributed.insert(n.clone(), rule);
                            attribution.push(("unmarked", n, 0, rule));
                        }
                        let found: Vec<(Atom, usize)> = pending_dangling
                            .iter()
                            .filter(|key| present.dangling.contains(*key))
                            .cloned()
                            .collect();
                        for (n, line) in found {
                            pending_dangling.remove(&(n.clone(), line));
                            attribution.push(("dangling", n, line, rule));
                        }
                        if pending_unmarked.is_empty() && pending_dangling.is_empty() {
                            break;
                        }
                    }
                    for n in pending_unmarked {
                        attributed.insert(n.clone(), "<unattributed>");
                        attribution.push(("unmarked", n, 0, "<unattributed>"));
                    }
                    for (n, line) in pending_dangling {
                        attribution.push(("dangling", n, line, "<unattributed>"));
                    }
                }
            }
            Ok((input, output, attribution))
        });

        totals["modules"] = json!(totals["modules"].as_u64().unwrap() + 1);
        let (input, output, attribution) = match result {
            Ok(r) => r,
            Err(e) => {
                totals["parse_failed"] = json!(totals["parse_failed"].as_u64().unwrap() + 1);
                println!("{}", json!({"file": name, "parse_error": e}));
                continue;
            }
        };

        let bump = |t: &mut serde_json::Value, key: &str, by: usize| {
            t[key] = json!(t[key].as_u64().unwrap() + by as u64);
        };
        bump(&mut totals, "input_captured_refs", input.captured.len());
        bump(&mut totals, "output_captured_refs", output.captured.len());
        bump(
            &mut totals,
            "output_unresolved_refs",
            output.unresolved_refs,
        );
        bump(&mut totals, "output_unmarked_refs", output.unmarked_refs);
        bump(&mut totals, "input_dangling_refs", input.dangling.len());
        bump(&mut totals, "output_dangling_refs", output.dangling.len());
        bump(&mut totals, "output_refs_in_with", output.in_with.len());
        if !output.captured.is_empty() {
            bump(&mut totals, "modules_with_capture", 1);
        }
        if !output.dangling.is_empty() {
            bump(&mut totals, "modules_with_dangling", 1);
        }
        if output.has_with {
            bump(&mut totals, "modules_with_with", 1);
        }
        if output.has_direct_eval {
            bump(&mut totals, "modules_with_direct_eval", 1);
        }
        for (i, (_, declared, refs, captured)) in output.watched.iter().enumerate() {
            let w = &mut totals["watched"][i];
            if *declared {
                w["modules_declaring"] = json!(w["modules_declaring"].as_u64().unwrap() + 1);
                if *refs > 0 {
                    w["modules_declaring_and_referencing"] =
                        json!(w["modules_declaring_and_referencing"].as_u64().unwrap() + 1);
                }
            }
            w["unresolved_refs"] = json!(w["unresolved_refs"].as_u64().unwrap() + *refs as u64);
            w["captured_refs"] = json!(w["captured_refs"].as_u64().unwrap() + *captured as u64);
            if *captured > 0 {
                w["modules_captured"] = json!(w["modules_captured"].as_u64().unwrap() + 1);
            }
        }

        println!(
            "{}",
            json!({
                "file": name,
                "input_captured": input.captured.iter().map(|(n, l, k)| json!([n.as_ref(), l, k])).collect::<Vec<_>>(),
                "output_captured": output.captured.iter().map(|(n, l, k)| json!([n.as_ref(), l, k])).collect::<Vec<_>>(),
                "jsx_tag_refs": output.jsx_tag_refs,
                "export_spec_unmarked": output.export_spec_unmarked,
                "bytes": source.len(),
                "output_in_with": output.in_with.iter().map(|(n, l)| json!([n.as_ref(), l])).collect::<Vec<_>>(),
                "unresolved_refs": output.unresolved_refs,
                "unmarked_refs": output.unmarked_refs,
                "unmarked": output.unmarked.iter().map(|(n, l)| json!([n.as_ref(), l])).collect::<Vec<_>>(),
                "input_dangling": input.dangling.iter().map(|(n, l)| json!([n.as_ref(), l])).collect::<Vec<_>>(),
                "output_dangling": output.dangling.iter().map(|(n, l)| json!([n.as_ref(), l])).collect::<Vec<_>>(),
                "has_with": output.has_with,
                "attribution": attribution.iter().map(|(k, n, l, r)| json!([k, n.as_ref(), l, r])).collect::<Vec<_>>(),
                "has_direct_eval": output.has_direct_eval,
                "watched": output.watched.iter().map(|(n, d, r, c)| json!({"name": n.as_ref(), "declared": d, "refs": r, "captured": c})).collect::<Vec<_>>(),
            })
        );
    }

    eprintln!("{}", serde_json::to_string_pretty(&totals)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_input_path_is_an_error() {
        let missing = env::temp_dir().join("name-capture-oracle-missing-input");
        let mut files = Vec::new();
        let error = collect_files(&missing, &mut files).expect_err("a missing path must fail");
        assert!(error.contains("cannot read"), "{error}");
        assert!(files.is_empty());
    }

    #[test]
    fn directory_input_collects_script_files() {
        let dir = env::temp_dir().join(format!("name-capture-oracle-{}", std::process::id()));
        fs::create_dir_all(dir.join("nested")).unwrap();
        fs::write(dir.join("a.js"), "").unwrap();
        fs::write(dir.join("nested/b.mjs"), "").unwrap();
        fs::write(dir.join("notes.txt"), "").unwrap();
        let mut files = Vec::new();
        collect_files(&dir, &mut files).expect("a readable directory should succeed");
        files.sort();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(files, [dir.join("a.js"), dir.join("nested/b.mjs")]);
    }
}
