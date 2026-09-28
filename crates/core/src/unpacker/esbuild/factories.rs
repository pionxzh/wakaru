//! Lazy-module helper detection and factory collection.

use swc_core::atoms::Atom;
use swc_core::common::{Span, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, CallExpr, Callee, Decl, ExportSpecifier, Expr, ExprStmt,
    Function, Module, ModuleDecl, ModuleExportName, ModuleItem, Pat, Prop, PropName, PropOrSpread,
    Stmt, Str, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::collections::HashSet;
use crate::unpacker::BindingId;
use crate::utils::paren::strip_parens;

use super::bindings::{collect_write_bindings, TopLevelRefCollector};

pub(super) struct Factory {
    /// Lazy helper used by this factory declaration.
    pub(super) helper_sym: Atom,
    /// Resolved top-level binding for the factory variable.
    pub(super) binding: BindingId,
    /// The declared variable name (e.g. `BO7`).
    pub(super) var_name: Atom,
    /// Derived filename: filepath string key when available, else `<var_name>.js`.
    pub(super) filename: String,
    /// CommonJS factory callback params: `(exports, module) => { ... }`.
    pub(super) cjs_params: Option<CjsFactoryParams>,
    /// The statements inside the factory function body (unresolved — for emission).
    pub(super) body_stmts: Vec<Stmt>,
    /// Location of the corresponding resolved declarator in the analysis AST.
    pub(super) analysis_location: FactoryLocation,
    /// Span of the factory's `var` declarator in the original bundle (provenance).
    pub(super) span: Span,
}

#[derive(Clone, Copy)]
pub(super) struct FactoryLocation {
    item_index: usize,
    declarator_index: usize,
}

#[derive(Clone)]
pub(super) struct CjsFactoryParams {
    pub(super) exports: Atom,
    pub(super) module: Option<Atom>,
}

// ---------------------------------------------------------------------------
// Helper detection
//
// esbuild emits lazy-module helpers as top-level `var` declarations whose RHS
// is an arrow function that takes ≤2 params and *returns* another function
// (either an arrow or a named `function` expression).  Both minified and
// non-minified forms share this shape:
//
//   Minified:     (q, K) => () => ...
//   Non-minified: (cb, mod) => function __require() { ... }
// ---------------------------------------------------------------------------

pub(super) fn collect_helper_syms(module: &Module) -> HashSet<Atom> {
    let mut syms = HashSet::default();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Some(init) = &decl.init else { continue };
            if is_lazy_helper(init) {
                if let Pat::Ident(bi) = &decl.name {
                    syms.insert(bi.id.sym.clone());
                }
            }
        }
    }
    syms
}

pub(super) fn has_factory_detection_evidence(module: &Module, helper_syms: &HashSet<Atom>) -> bool {
    // Keep this gate aligned with `factory_shape_helper_sym`,
    // `try_extract_factory`, and the `has_factories` acceptance check. The
    // owned detector relies on this being a conservative precondition before
    // it moves factory bodies and unwraps the final detection result.
    let commonjs_helper_syms = collect_commonjs_helper_syms(module);
    let mut factory_count = 0usize;
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Some(helper_sym) = factory_shape_helper_sym(decl, helper_syms) else {
                continue;
            };
            factory_count += 1;
            if commonjs_helper_syms.contains(&helper_sym) {
                return true;
            }
        }
    }
    factory_count >= 5
}

fn factory_shape_helper_sym(decl: &VarDeclarator, helper_syms: &HashSet<Atom>) -> Option<Atom> {
    // This preflight must stay no broader than both `try_extract_factory` and
    // `take_factory_body`; the owned path treats a collected factory body as
    // movable without another recoverable fallback.
    let Pat::Ident(_) = &decl.name else {
        return None;
    };
    let Expr::Call(call) = decl.init.as_deref()? else {
        return None;
    };
    let helper_sym = call_target_helper_sym(call, helper_syms)?;
    let [arg] = call.args.as_slice() else {
        return None;
    };
    if arg.spread.is_some() {
        return None;
    }
    let accepted = match arg.expr.as_ref() {
        Expr::Object(object) if object.props.len() == 1 => matches!(
            object.props.first(),
            Some(PropOrSpread::Prop(prop))
                if matches!(prop.as_ref(), Prop::Method(method) if method.function.body.is_some())
        ),
        Expr::Arrow(_) => true,
        Expr::Fn(function) => function.function.body.is_some(),
        _ => false,
    };
    accepted.then_some(helper_sym)
}

pub(super) fn collect_commonjs_helper_syms(module: &Module) -> HashSet<Atom> {
    let mut syms = HashSet::default();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Some(init) = &decl.init else { continue };
            if is_lazy_helper(init) && expr_mentions_exports_member(init) {
                if let Pat::Ident(bi) = &decl.name {
                    syms.insert(bi.id.sym.clone());
                }
            }
        }
    }
    syms
}

/// Returns `true` if `expr` matches the esbuild lazy-helper shape:
///   Arrow(≤2 params) → body is Arrow or named Fn expression
fn is_lazy_helper(expr: &Expr) -> bool {
    let Expr::Arrow(outer) = expr else {
        return false;
    };
    if outer.params.len() > 2 {
        return false;
    }
    let body_expr = match &*outer.body {
        ArrowFunctionBody::Expr(e) => e,
        ArrowFunctionBody::FunctionBody(_) => return false,
    };
    matches!(**body_expr, Expr::Arrow(_) | Expr::Fn(_))
}

fn expr_mentions_exports_member(expr: &Expr) -> bool {
    struct ExportsMemberVisitor {
        found: bool,
    }

    impl Visit for ExportsMemberVisitor {
        fn visit_member_expr(&mut self, expr: &swc_core::ecma::ast::MemberExpr) {
            if self.found {
                return;
            }
            if let swc_core::ecma::ast::MemberProp::Ident(prop) = &expr.prop {
                if prop.sym == *"exports" {
                    self.found = true;
                    return;
                }
            }
            expr.obj.visit_with(self);
            if let swc_core::ecma::ast::MemberProp::Computed(c) = &expr.prop {
                c.visit_with(self);
            }
        }

        fn visit_prop_name(&mut self, name: &PropName) {
            if self.found {
                return;
            }
            match name {
                PropName::Ident(id) if id.sym == *"exports" => {
                    self.found = true;
                }
                PropName::Computed(c) => c.visit_with(self),
                _ => {}
            }
        }
    }

    let mut visitor = ExportsMemberVisitor { found: false };
    expr.visit_with(&mut visitor);
    visitor.found
}

// ---------------------------------------------------------------------------
// Factory collection
//
// A factory is a top-level `var X = helper(fn_or_obj)` where `helper` is one
// of the detected lazy-helper symbols.
//
// Non-minified form uses an object literal whose key is the original file path:
//   var require_foo = __commonJS({ "src/foo.js"(exports, module) { … } })
//
// Minified form uses a plain arrow/function:
//   var BO7 = y(() => { … })
// ---------------------------------------------------------------------------

pub(super) struct PathCommentHints<'a> {
    pub(super) source: &'a str,
    source_start_pos: u32,
    hints: Vec<(usize, String)>,
}

impl<'a> PathCommentHints<'a> {
    pub(super) fn new(source: &'a str, source_start_pos: u32) -> Self {
        let mut hints = Vec::new();
        let mut offset = 0usize;
        for line in source.split_inclusive('\n') {
            let text = line.trim_end_matches(['\r', '\n']);
            if let Some(path) = parse_path_comment(text) {
                hints.push((offset + line.len(), sanitize_path_comment_hint(path)));
            }
            offset += line.len();
        }
        Self {
            source,
            source_start_pos,
            hints,
        }
    }

    fn hint_before(&self, abs_byte_pos: u32) -> Option<String> {
        let rel = abs_byte_pos.checked_sub(self.source_start_pos)? as usize;
        if rel > self.source.len() {
            return None;
        }
        let (code_start, filename) = self
            .hints
            .iter()
            .rev()
            .find(|(code_start, _)| *code_start <= rel)?;
        if self.source[*code_start..rel].trim().is_empty() {
            Some(filename.clone())
        } else {
            None
        }
    }
}

fn parse_path_comment(line: &str) -> Option<String> {
    let path = line.trim_start().strip_prefix("// ")?;
    if path.starts_with('#') || path.starts_with("===") {
        return None;
    }
    let normalized = path.replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    let valid_ext = [".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs", ".mts", ".cts"]
        .iter()
        .any(|ext| lower.ends_with(ext));
    valid_ext.then_some(normalized)
}

fn sanitize_path_comment_hint(path: String) -> String {
    let mut path = sanitize_path(path);
    if let Some(dot) = path.rfind('.') {
        path.replace_range(dot.., ".js");
    } else {
        path.push_str(".js");
    }
    path
}

pub(super) fn collect_factories(
    module: &Module,
    analysis_module: &Module,
    helper_syms: &HashSet<Atom>,
    commonjs_helper_syms: &HashSet<Atom>,
    filename_hints: Option<&PathCommentHints<'_>>,
) -> Vec<Factory> {
    let mut factories = Vec::new();
    for (item_index, (item, analysis_item)) in module
        .body
        .iter()
        .zip(analysis_module.body.iter())
        .enumerate()
    {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(analysis_var))) = analysis_item else {
            continue;
        };
        for (declarator_index, (decl, analysis_decl)) in
            var.decls.iter().zip(analysis_var.decls.iter()).enumerate()
        {
            if let Some(factory) = try_extract_factory(
                decl,
                analysis_decl,
                FactoryLocation {
                    item_index,
                    declarator_index,
                },
                var.span.lo.0,
                helper_syms,
                commonjs_helper_syms,
                filename_hints,
            ) {
                factories.push(factory);
            }
        }
    }
    factories
}

pub(super) fn collect_factories_owned(
    module: &mut Module,
    analysis_module: &Module,
    helper_syms: &HashSet<Atom>,
    commonjs_helper_syms: &HashSet<Atom>,
    filename_hints: Option<&PathCommentHints<'_>>,
) -> Vec<Factory> {
    let mut factories = Vec::new();
    for (item_index, (item, analysis_item)) in module
        .body
        .iter_mut()
        .zip(analysis_module.body.iter())
        .enumerate()
    {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(analysis_var))) = analysis_item else {
            continue;
        };
        let decl_start_abs = var.span.lo.0;
        for (declarator_index, (decl, analysis_decl)) in var
            .decls
            .iter_mut()
            .zip(analysis_var.decls.iter())
            .enumerate()
        {
            let Some(mut factory) = try_extract_factory(
                decl,
                analysis_decl,
                FactoryLocation {
                    item_index,
                    declarator_index,
                },
                decl_start_abs,
                helper_syms,
                commonjs_helper_syms,
                filename_hints,
            ) else {
                continue;
            };
            factory.body_stmts = take_factory_body(decl)
                .expect("a structurally collected factory must retain a movable body");
            factories.push(factory);
        }
    }
    factories
}

fn take_factory_body(decl: &mut VarDeclarator) -> Option<Vec<Stmt>> {
    // Keep accepted body shapes in lockstep with `factory_shape_helper_sym`
    // and `try_extract_factory`.
    let Expr::Call(call) = decl.init.as_deref_mut()? else {
        return None;
    };
    let [arg] = call.args.as_mut_slice() else {
        return None;
    };
    match arg.expr.as_mut() {
        Expr::Object(obj) if obj.props.len() == 1 => {
            let PropOrSpread::Prop(prop) = &mut obj.props[0] else {
                return None;
            };
            let Prop::Method(method) = prop.as_mut() else {
                return None;
            };
            Some(std::mem::take(&mut method.function.body.as_mut()?.stmts))
        }
        Expr::Arrow(arrow) => match arrow.body.as_mut() {
            ArrowFunctionBody::FunctionBody(block) => Some(std::mem::take(&mut block.stmts)),
            ArrowFunctionBody::Expr(expr) => {
                let expr = std::mem::replace(expr, Box::new(Expr::Invalid(Default::default())));
                Some(vec![Stmt::Expr(ExprStmt {
                    span: DUMMY_SP,
                    expr,
                })])
            }
        },
        Expr::Fn(fn_expr) => Some(std::mem::take(&mut fn_expr.function.body.as_mut()?.stmts)),
        _ => None,
    }
}

fn try_extract_factory(
    decl: &VarDeclarator,
    analysis_decl: &VarDeclarator,
    analysis_location: FactoryLocation,
    decl_start_abs: u32,
    helper_syms: &HashSet<Atom>,
    commonjs_helper_syms: &HashSet<Atom>,
    filename_hints: Option<&PathCommentHints<'_>>,
) -> Option<Factory> {
    // Keep accepted body shapes in lockstep with
    // `factory_shape_helper_sym` and `take_factory_body`.
    let Pat::Ident(var_ident) = &decl.name else {
        return None;
    };
    let init = decl.init.as_ref()?;
    let Expr::Call(call) = &**init else {
        return None;
    };

    // Callee must be one of the detected helpers.
    let helper_sym = call_target_helper_sym(call, helper_syms)?;
    let is_commonjs_factory = commonjs_helper_syms.contains(&helper_sym);

    if call.args.len() != 1 {
        return None;
    }

    let arg = &*call.args[0].expr;
    let var_name = var_ident.id.sym.clone();
    let hinted_filename = filename_hints.and_then(|hints| hints.hint_before(decl_start_abs));
    let binding = match &analysis_decl.name {
        Pat::Ident(bi) => (bi.id.sym.clone(), bi.id.ctxt),
        _ => return None,
    };

    match arg {
        // Non-minified: __commonJS({ "src/foo.js"(exports, module) { … } })
        Expr::Object(obj) if obj.props.len() == 1 => {
            use swc_core::ecma::ast::{Prop, PropOrSpread};
            if let PropOrSpread::Prop(prop) = &obj.props[0] {
                if let Prop::Method(method) = &**prop {
                    let filename = prop_key_str(&method.key)
                        .map(sanitize_path)
                        .or_else(|| hinted_filename.clone())
                        .unwrap_or_else(|| format!("{var_name}.js"));
                    let body_stmts = method.function.body.as_ref()?.stmts.clone();
                    return Some(Factory {
                        helper_sym,
                        binding,
                        var_name,
                        filename,
                        cjs_params: is_commonjs_factory
                            .then(|| function_cjs_params(&method.function))
                            .flatten(),
                        body_stmts,
                        analysis_location,
                        span: decl.span,
                    });
                }
            }
            None
        }

        // Minified arrow: y(() => { … }) or y(() => expr)
        Expr::Arrow(arrow) => {
            let body_stmts = arrow_body_stmts(arrow);
            let filename = hinted_filename.unwrap_or_else(|| format!("{var_name}.js"));
            Some(Factory {
                helper_sym,
                binding,
                var_name,
                filename,
                cjs_params: is_commonjs_factory
                    .then(|| arrow_cjs_params(arrow))
                    .flatten(),
                body_stmts,
                analysis_location,
                span: decl.span,
            })
        }

        // Minified function: m(function() { … })
        Expr::Fn(fn_expr) => {
            let body_stmts = fn_expr.function.body.as_ref()?.stmts.clone();
            let filename = hinted_filename.unwrap_or_else(|| format!("{var_name}.js"));
            Some(Factory {
                helper_sym,
                binding,
                var_name,
                filename,
                cjs_params: is_commonjs_factory
                    .then(|| function_cjs_params(&fn_expr.function))
                    .flatten(),
                body_stmts,
                analysis_location,
                span: decl.span,
            })
        }

        _ => None,
    }
}

pub(super) enum FactoryBodyRef<'a> {
    Stmts(&'a [Stmt]),
    Expr(&'a Expr),
}

pub(super) fn collect_factory_analysis_bindings(
    analysis_module: &Module,
    location: FactoryLocation,
    top_level_bindings: &HashSet<BindingId>,
) -> Option<(HashSet<BindingId>, HashSet<BindingId>)> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) =
        analysis_module.body.get(location.item_index)?
    else {
        return None;
    };
    let decl = var.decls.get(location.declarator_index)?;
    let Expr::Call(call) = decl.init.as_deref()? else {
        return None;
    };
    let [arg] = call.args.as_slice() else {
        return None;
    };

    let body = match arg.expr.as_ref() {
        Expr::Object(obj) if obj.props.len() == 1 => {
            let PropOrSpread::Prop(prop) = &obj.props[0] else {
                return None;
            };
            let Prop::Method(method) = prop.as_ref() else {
                return None;
            };
            FactoryBodyRef::Stmts(&method.function.body.as_ref()?.stmts)
        }
        Expr::Arrow(arrow) => match arrow.body.as_ref() {
            ArrowFunctionBody::FunctionBody(block) => FactoryBodyRef::Stmts(&block.stmts),
            ArrowFunctionBody::Expr(expr) => FactoryBodyRef::Expr(expr),
        },
        Expr::Fn(fn_expr) => FactoryBodyRef::Stmts(&fn_expr.function.body.as_ref()?.stmts),
        _ => return None,
    };

    Some(collect_factory_body_bindings(body, top_level_bindings))
}

pub(super) fn collect_factory_body_bindings(
    body: FactoryBodyRef<'_>,
    top_level_bindings: &HashSet<BindingId>,
) -> (HashSet<BindingId>, HashSet<BindingId>) {
    let mut references = TopLevelRefCollector {
        top_level_bindings,
        references: HashSet::default(),
    };
    let mut writes = HashSet::default();

    match body {
        FactoryBodyRef::Stmts(stmts) => {
            for stmt in stmts {
                stmt.visit_with(&mut references);
                collect_write_bindings(stmt, top_level_bindings, &mut writes);
            }
        }
        FactoryBodyRef::Expr(expr) => {
            expr.visit_with(&mut references);
            let stmt = Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: Box::new(expr.clone()),
            });
            collect_write_bindings(&stmt, top_level_bindings, &mut writes);
        }
    }

    (references.references, writes)
}

fn arrow_cjs_params(arrow: &ArrowExpr) -> Option<CjsFactoryParams> {
    if arrow.params.is_empty() {
        return None;
    }
    let exports_name = pat_ident_atom(&arrow.params[0])?;
    let module_name = arrow.params.get(1).and_then(pat_ident_atom);
    Some(CjsFactoryParams {
        exports: exports_name,
        module: module_name,
    })
}

fn function_cjs_params(function: &Function) -> Option<CjsFactoryParams> {
    if function.params.is_empty() {
        return None;
    }
    let exports_name = pat_ident_atom(&function.params[0].pat)?;
    let module_name = function
        .params
        .get(1)
        .and_then(|param| pat_ident_atom(&param.pat));
    Some(CjsFactoryParams {
        exports: exports_name,
        module: module_name,
    })
}

fn pat_ident_atom(pat: &Pat) -> Option<Atom> {
    match pat {
        Pat::Ident(ident) => Some(ident.id.sym.clone()),
        _ => None,
    }
}

fn call_target_helper_sym(call: &CallExpr, helper_syms: &HashSet<Atom>) -> Option<Atom> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(ident) = &**callee else {
        return None;
    };
    if helper_syms.contains(&ident.sym) {
        Some(ident.sym.clone())
    } else {
        None
    }
}

fn arrow_body_stmts(arrow: &ArrowExpr) -> Vec<Stmt> {
    match &*arrow.body {
        ArrowFunctionBody::FunctionBody(block) => block.stmts.clone(),
        ArrowFunctionBody::Expr(expr) => vec![Stmt::Expr(ExprStmt {
            span: Default::default(),
            expr: expr.clone(),
        })],
    }
}

fn prop_key_str(key: &swc_core::ecma::ast::PropName) -> Option<String> {
    use swc_core::ecma::ast::PropName;
    match key {
        PropName::Str(Str { value, .. }) => Some(value.as_str().unwrap_or("").to_string()),
        PropName::Ident(id) => Some(id.sym.to_string()),
        _ => None,
    }
}

/// Convert a source-map style path (`../src/foo.js`, `webpack:///src/foo.js`) to a
/// safe relative path suitable as a filename.
fn sanitize_path(raw: String) -> String {
    let s = raw
        .trim_start_matches("webpack://")
        .trim_start_matches("webpack:///")
        .trim_start_matches('/');
    crate::unpacker::sanitize_relative_path(s, "module.js")
}

pub(super) fn filter_helper_factory_declarators(
    item: &ModuleItem,
    helper_factory_syms: &HashSet<Atom>,
) -> Option<ModuleItem> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = item else {
        return Some(item.clone());
    };
    if var_decl
        .decls
        .iter()
        .all(|decl| is_helper_factory_declarator(decl, helper_factory_syms))
    {
        return None;
    }
    let mut filtered = var_decl.clone();
    filtered
        .decls
        .retain(|decl| !is_helper_factory_declarator(decl, helper_factory_syms));
    if filtered.decls.is_empty() {
        None
    } else {
        Some(ModuleItem::Stmt(Stmt::Decl(Decl::Var(filtered))))
    }
}

/// Local bindings named by `export { local as name }` (no `from`) or
/// `export default local`.
pub(super) fn locally_exported_atoms(body: &[ModuleItem]) -> HashSet<Atom> {
    let mut atoms = HashSet::default();
    for item in body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export)) if export.src.is_none() => {
                for specifier in &export.specifiers {
                    if let ExportSpecifier::Named(named) = specifier {
                        if let ModuleExportName::Ident(orig) = &named.orig {
                            atoms.insert(orig.sym.clone());
                        }
                    }
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export)) => {
                if let Expr::Ident(ident) = strip_parens(&export.expr) {
                    atoms.insert(ident.sym.clone());
                }
            }
            _ => {}
        }
    }
    atoms
}

pub(super) fn item_has_helper_factory_declarator(
    item: &ModuleItem,
    helper_factory_syms: &HashSet<Atom>,
) -> bool {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = item else {
        return false;
    };
    var_decl
        .decls
        .iter()
        .any(|decl| is_helper_factory_declarator(decl, helper_factory_syms))
}

fn is_helper_factory_declarator(decl: &VarDeclarator, helper_factory_syms: &HashSet<Atom>) -> bool {
    matches!(
        &decl.name,
        Pat::Ident(bi) if helper_factory_syms.contains(&bi.id.sym)
    )
}

// ---------------------------------------------------------------------------
// Scope-hoisted module extraction
//
// esbuild scope-hoists ESM modules into a flat top-level scope. Each
// scope-hoisted module is marked by:
//
//   var NS = {};
//   __export(NS, { exportName: () => localBinding, ... });
//   ... module code (var/function/class declarations) ...
//
// The `__export` helper is an arrow:
//   (target, all) => { for (var name in all) defProp(target, name, {get: all[name], ...}) }
//
// KNOWN LIMITATION (last-module boundary):
// For non-last modules, the next `var NS = {}; __export(NS, ...)` boundary
// cleanly delimits module code. For the last module, we use a three-phase
// heuristic: Phase 1 finds the last exported-binding declaration, Phase 2
// extends via reference closure (private helpers after exports), Phase 3
// includes trailing expression statements that reference module bindings.
//
// This can misattribute entry-level expressions that reference bindings
// from the last module. For example:
//   // constants.js (module side effect)
//   console.log(LABEL, VALUE);
//   // entry.js (entry code referencing same binding)
//   console.log("entry", VALUE);
//
// Both appear after the last export and reference `VALUE`. In minified
// production bundles there is no structural marker distinguishing them —
// the ambiguity is inherent. The misattribution is cosmetic (code lands
// in the wrong file) not functional (bindings remain accessible in the
// shared scope).
//
// We detect this helper, find all namespace+export pairs, and partition
// the top-level items into per-module groups.
// ---------------------------------------------------------------------------
