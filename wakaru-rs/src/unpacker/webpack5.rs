use anyhow::anyhow;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, Mark, SourceMap, Span, SyntaxContext, GLOBALS};
use swc_core::ecma::ast::{
    ArrayLit, AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, CallExpr, Callee, Expr,
    ExprStmt, FnExpr, Function, Ident, IdentName, Lit, MemberExpr, MemberProp, Module, ModuleItem,
    ObjectLit, Pat, Prop, PropName, PropOrSpread, SimpleAssignTarget, Stmt, VarDecl, VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};

use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::utils::replace_ident;
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use crate::rules::apply_default_rules;
use crate::unpacker::{UnpackResult, UnpackedModule};

struct Webpack5RuntimeNormalizer {
    require_sym: Atom,
    exports_sym: Atom,
    unresolved_mark: Mark,
}

impl VisitMut for Webpack5RuntimeNormalizer {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        let mut new_items = Vec::with_capacity(items.len());
        for item in items.drain(..) {
            if let ModuleItem::Stmt(stmt) = item {
                if let Some(replacements) = self.try_convert_stmt(&stmt) {
                    new_items.extend(replacements.into_iter().map(ModuleItem::Stmt));
                } else {
                    new_items.push(ModuleItem::Stmt(stmt));
                }
            } else {
                new_items.push(item);
            }
        }
        *items = new_items;
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);

        let mut new_stmts = Vec::with_capacity(stmts.len());
        for stmt in stmts.drain(..) {
            if let Some(replacements) = self.try_convert_stmt(&stmt) {
                new_stmts.extend(replacements);
            } else {
                new_stmts.push(stmt);
            }
        }
        *stmts = new_stmts;
    }
}

impl Webpack5RuntimeNormalizer {
    fn try_convert_stmt(&self, stmt: &Stmt) -> Option<Vec<Stmt>> {
        let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
            return None;
        };
        let Expr::Call(call) = &**expr else {
            return None;
        };
        let Callee::Expr(callee_expr) = &call.callee else {
            return None;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
            return None;
        };
        let Expr::Ident(callee_obj) = &**obj else {
            return None;
        };
        if callee_obj.sym != self.require_sym || callee_obj.ctxt.outer() != self.unresolved_mark {
            return None;
        }
        let MemberProp::Ident(prop_name) = prop else {
            return None;
        };

        match prop_name.sym.as_ref() {
            "r" => Some(vec![]),
            "d" => self.convert_require_d(stmt, call),
            _ => None,
        }
    }

    fn convert_require_d(&self, stmt: &Stmt, call: &CallExpr) -> Option<Vec<Stmt>> {
        let span = stmt.span();
        let exports_ident = Ident::new(self.exports_sym.clone(), span, Default::default());

        // Webpack5 form: require.d(exports, { key: getter, ... })
        if call.args.len() == 2 {
            let Expr::Object(defs) = &*call.args[1].expr else {
                return None;
            };
            let mut assignments = Vec::new();

            for prop in &defs.props {
                let swc_core::ecma::ast::PropOrSpread::Prop(prop) = prop else {
                    return None;
                };
                let swc_core::ecma::ast::Prop::KeyValue(key_value) = &**prop else {
                    return None;
                };
                let export_name = match &key_value.key {
                    swc_core::ecma::ast::PropName::Ident(name) => name.sym.clone(),
                    swc_core::ecma::ast::PropName::Str(name) => {
                        Atom::from(name.value.as_str().unwrap_or(""))
                    }
                    _ => return None,
                };
                let value = extract_getter_value(&key_value.value)?;
                assignments.push(build_member_assign(
                    exports_ident.clone(),
                    export_name,
                    value,
                    span,
                ));
            }

            return Some(assignments);
        }

        // Webpack4 form: require.d(exports, "name", getter)
        if call.args.len() == 3 {
            let Expr::Lit(Lit::Str(name_str)) = &*call.args[1].expr else {
                return None;
            };
            let export_name: Atom = name_str.value.as_str().map(Atom::from).unwrap_or_else(|| {
                Atom::from(name_str.value.to_string_lossy().into_owned().as_str())
            });
            let value = extract_getter_value(&call.args[2].expr)?;
            return Some(vec![build_member_assign(
                exports_ident,
                export_name,
                value,
                span,
            )]);
        }

        None
    }
}

pub fn detect_and_extract(source: &str) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::parse_es_module(source, "webpack5.js", cm.clone()).ok()?;
        detect_from_module(&module, cm)
    })
}

pub(super) fn detect_from_module(module: &Module, cm: Lrc<SourceMap>) -> Option<UnpackResult> {
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
            continue;
        };
        let Some(bootstrap_body) = extract_iife_body(expr) else {
            continue;
        };
        if let Some(result) = extract_webpack5_modules(bootstrap_body, cm.clone()) {
            return Some(result);
        }
    }
    None
}

/// Detect and extract modules from a webpack5 JSONP chunk format:
/// `(self.webpackChunk_N_E = self.webpackChunk_N_E || []).push([[chunkIds], {modules}])`
pub fn detect_and_extract_chunk(source: &str) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::parse_es_module(source, "webpack5.js", cm.clone()).ok()?;
        detect_chunk_from_module(&module, cm)
    })
}

pub(super) fn detect_chunk_from_module(
    module: &Module,
    cm: Lrc<SourceMap>,
) -> Option<UnpackResult> {
    let mut all_modules = Vec::new();

    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
            continue;
        };
        if let Some(modules_object) = extract_chunk_push_modules(expr) {
            let extracted = extract_modules_from_object(modules_object, cm.clone())?;
            all_modules.extend(extracted);
        }
    }

    if all_modules.is_empty() {
        return None;
    }

    Some(UnpackResult {
        modules: all_modules,
    })
}

/// Match the pattern: `(self.X = self.X || []).push([[ids], {modules}])`
/// or `(window["X"] = window["X"] || []).push([[ids], {modules}])`
fn extract_chunk_push_modules(expr: &Expr) -> Option<&ObjectLit> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    // callee is (self.X = self.X || []).push
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
        return None;
    };
    let MemberProp::Ident(push_ident) = prop else {
        return None;
    };
    if push_ident.sym.as_ref() != "push" {
        return None;
    }

    // obj is (self.X = self.X || []) — a parenthesized assignment
    let obj = strip_parens(obj);
    let Expr::Assign(AssignExpr {
        op: AssignOp::Assign,
        right,
        ..
    }) = obj
    else {
        return None;
    };

    // right side: self.X || []
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalOr,
        right: or_right,
        ..
    }) = &**right
    else {
        return None;
    };
    // Verify right side of || is an empty array
    let Expr::Array(ArrayLit { elems, .. }) = &**or_right else {
        return None;
    };
    if !elems.is_empty() {
        return None;
    }

    // push argument: [[chunkIds], {modules}, ...]
    if call.args.is_empty() {
        return None;
    }
    let push_arg = &call.args[0].expr;
    let Expr::Array(ArrayLit {
        elems: push_elems, ..
    }) = &**push_arg
    else {
        return None;
    };
    // Must have at least 2 elements: [chunkIds, modulesObject]
    if push_elems.len() < 2 {
        return None;
    }
    // First element: array of chunk IDs
    let Some(Some(first)) = push_elems.first() else {
        return None;
    };
    if !matches!(&*first.expr, Expr::Array(_)) {
        return None;
    }
    // Second element: modules object
    let Some(Some(second)) = push_elems.get(1) else {
        return None;
    };
    let Expr::Object(modules_object) = &*second.expr else {
        return None;
    };

    // Validate that the object contains function properties
    if modules_object.props.is_empty() {
        return None;
    }
    for prop in &modules_object.props {
        extract_module_from_prop(prop)?;
    }

    Some(modules_object)
}

/// Extract modules from an ObjectLit where keys are module IDs and values are factory functions.
/// Used by both entry bundles and JSONP chunks.
fn extract_modules_from_object(
    modules_object: &ObjectLit,
    cm: Lrc<SourceMap>,
) -> Option<Vec<UnpackedModule>> {
    let mut modules = Vec::new();

    for prop in &modules_object.props {
        let (module_id, factory, body_stmts) = extract_module_from_prop(prop)?;
        let filename = if module_id.contains('/') || module_id.contains('.') {
            sanitize_filename(&module_id)
        } else {
            format!("module-{module_id}.js")
        };

        let code = emit_webpack5_module(&factory, body_stmts, cm.clone())?;
        modules.push(UnpackedModule {
            id: module_id,
            is_entry: false,
            code,
            filename,
        });
    }

    Some(modules)
}

fn extract_webpack5_modules(
    bootstrap_body: &swc_core::ecma::ast::BlockStmt,
    cm: Lrc<SourceMap>,
) -> Option<UnpackResult> {
    let mut modules_object: Option<&ObjectLit> = None;

    for stmt in &bootstrap_body.stmts {
        let Stmt::Decl(swc_core::ecma::ast::Decl::Var(var_decl)) = stmt else {
            continue;
        };
        let Some(object_lit) = extract_webpack_modules_object(var_decl) else {
            continue;
        };
        modules_object = Some(object_lit);
        break;
    }

    let modules_object = modules_object?;
    let mut modules = Vec::new();

    for prop in &modules_object.props {
        let (module_id, factory, body_stmts) = extract_module_from_prop(prop)?;
        let filename = if module_id.contains('/') || module_id.contains('.') {
            sanitize_filename(&module_id)
        } else {
            format!("module-{module_id}.js")
        };

        let code = emit_webpack5_module(&factory, body_stmts, cm.clone())?;
        modules.push(UnpackedModule {
            id: module_id,
            is_entry: false,
            code,
            filename,
        });
    }

    // Check for trailing IIFE entry point
    let has_trailing_entry = if let Some(entry_body) = bootstrap_body
        .stmts
        .last()
        .and_then(extract_iife_stmt_body)
        .map(|body| body.stmts.clone())
    {
        let mut synthetic_module = build_module_from_stmts(entry_body);
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        synthetic_module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        apply_default_rules(&mut synthetic_module, unresolved_mark);
        synthetic_module.visit_mut_with(&mut fixer(None));
        let code = emit_module(&synthetic_module, cm.clone()).ok()?;
        modules.push(UnpackedModule {
            id: "entry".to_string(),
            is_entry: true,
            code,
            filename: "entry.js".to_string(),
        });
        true
    } else {
        false
    };

    // Fallback: scan bootstrap for `__webpack_require__(__webpack_require__.s = <id>)`
    if !has_trailing_entry {
        if let Some(entry_id) = find_require_s_entry(bootstrap_body) {
            for module in &mut modules {
                if module.id == entry_id {
                    module.is_entry = true;
                    break;
                }
            }
        }
    }

    if modules.is_empty() {
        return None;
    }

    Some(UnpackResult { modules })
}

/// Extract a string module ID from any `PropName` variant.
fn extract_module_id_from_prop_name(key: &PropName) -> Option<String> {
    match key {
        PropName::Str(s) => Some(s.value.as_str().unwrap_or("unknown").to_string()),
        PropName::Num(n) => Some(format!("{}", n.value as i64)),
        PropName::Ident(i) => Some(i.sym.to_string()),
        _ => None,
    }
}

/// Extract the factory function from a prop, handling both `Prop::KeyValue` and `Prop::Method`.
/// Returns `(module_id, factory_function, body_stmts)`.
fn extract_module_from_prop(prop: &PropOrSpread) -> Option<(String, Function, Vec<Stmt>)> {
    let PropOrSpread::Prop(prop) = prop else {
        return None;
    };
    match &**prop {
        Prop::KeyValue(key_value) => {
            let module_id = extract_module_id_from_prop_name(&key_value.key)?;
            let (factory, body_stmts) = extract_factory(&key_value.value)?;
            Some((module_id, factory, body_stmts))
        }
        Prop::Method(method) => {
            let module_id = extract_module_id_from_prop_name(&method.key)?;
            let body = method.function.body.as_ref()?.stmts.clone();
            Some((module_id, *method.function.clone(), body))
        }
        _ => None,
    }
}

fn extract_webpack_modules_object(var_decl: &VarDecl) -> Option<&ObjectLit> {
    for decl in &var_decl.decls {
        let VarDeclarator {
            init: Some(init), ..
        } = decl
        else {
            continue;
        };
        let Expr::Object(object_lit) = strip_parens(init) else {
            continue;
        };
        if object_lit.props.is_empty() {
            continue;
        }
        let all_valid = object_lit
            .props
            .iter()
            .all(|prop| extract_module_from_prop(prop).is_some());
        if !all_valid {
            continue;
        }
        return Some(object_lit);
    }
    None
}

fn extract_factory(expr: &Expr) -> Option<(Function, Vec<Stmt>)> {
    match strip_parens(expr) {
        Expr::Fn(FnExpr { function, .. }) => {
            let body = function.body.as_ref()?.stmts.clone();
            Some((*function.clone(), body))
        }
        Expr::Arrow(arrow) => {
            let swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(body) = &*arrow.body else {
                return None;
            };
            let params = arrow
                .params
                .iter()
                .cloned()
                .map(|pat| swc_core::ecma::ast::Param {
                    span: Default::default(),
                    decorators: vec![],
                    pat,
                })
                .collect();
            Some((
                Function {
                    params,
                    decorators: vec![],
                    span: Default::default(),
                    ctxt: Default::default(),
                    body: Some(body.clone()),
                    is_generator: arrow.is_generator,
                    is_async: arrow.is_async,
                    type_params: None,
                    return_type: None,
                },
                body.stmts.clone(),
            ))
        }
        _ => None,
    }
}

fn strip_parens(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => strip_parens(&paren.expr),
        _ => expr,
    }
}

fn emit_webpack5_module(
    factory: &Function,
    body_stmts: Vec<Stmt>,
    cm: Lrc<SourceMap>,
) -> Option<String> {
    let mut synthetic_module = build_module_from_stmts(body_stmts);

    let param_syms: Vec<Atom> = factory
        .params
        .iter()
        .filter_map(|p| match &p.pat {
            Pat::Ident(binding) => Some(binding.sym.clone()),
            _ => None,
        })
        .collect();

    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    synthetic_module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

    let unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
    for (idx, target) in ["module", "exports", "require"].iter().enumerate() {
        let Some(old_sym) = param_syms.get(idx) else {
            continue;
        };
        if old_sym.as_ref() == *target {
            continue;
        }
        let from_id = (old_sym.clone(), unresolved_ctxt);
        let to_ident = Ident::new(Atom::from(*target), Default::default(), unresolved_ctxt);
        replace_ident(&mut synthetic_module, from_id, &to_ident);
    }

    let exports_sym = Atom::from("exports");
    let require_sym = Atom::from("require");
    let mut normalizer = Webpack5RuntimeNormalizer {
        require_sym,
        exports_sym,
        unresolved_mark,
    };
    synthetic_module.visit_mut_with(&mut normalizer);

    apply_default_rules(&mut synthetic_module, unresolved_mark);
    synthetic_module.visit_mut_with(&mut fixer(None));

    emit_module(&synthetic_module, cm).ok()
}

fn extract_iife_stmt_body(stmt: &Stmt) -> Option<&swc_core::ecma::ast::BlockStmt> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    extract_iife_body(expr)
}

fn extract_iife_body(expr: &Expr) -> Option<&swc_core::ecma::ast::BlockStmt> {
    match expr {
        Expr::Call(call) => extract_callee_body(&call.callee),
        Expr::Unary(unary) if matches!(unary.op, swc_core::ecma::ast::UnaryOp::Bang) => {
            let Expr::Call(call) = &*unary.arg else {
                return None;
            };
            extract_callee_body(&call.callee)
        }
        _ => None,
    }
}

fn extract_callee_body(callee: &Callee) -> Option<&swc_core::ecma::ast::BlockStmt> {
    let Callee::Expr(callee_expr) = callee else {
        return None;
    };
    match &**callee_expr {
        Expr::Fn(FnExpr { function, .. }) => function.body.as_ref(),
        Expr::Arrow(arrow) => match &*arrow.body {
            swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(body) => Some(body),
            _ => None,
        },
        Expr::Paren(paren) => match &*paren.expr {
            Expr::Fn(FnExpr { function, .. }) => function.body.as_ref(),
            Expr::Arrow(arrow) => match &*arrow.body {
                swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(body) => Some(body),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

fn sanitize_filename(module_id: &str) -> String {
    let stripped = module_id.trim_start_matches("./");
    let sanitized = stripped.replace("../", "").replace("..\\", "");
    if sanitized.is_empty() {
        "unknown.js".to_string()
    } else {
        sanitized
    }
}

/// Scan the bootstrap body for `__webpack_require__(__webpack_require__.s = <id>)` and return
/// the entry module ID as a string.
fn find_require_s_entry(body: &swc_core::ecma::ast::BlockStmt) -> Option<String> {
    for stmt in &body.stmts {
        if let Some(id) = find_require_s_in_stmt(stmt) {
            return Some(id);
        }
    }
    None
}

/// Check a single statement for the `require(require.s = <id>)` pattern.
/// Matches both standalone expression statements and `var x = require(require.s = <id>)`.
fn find_require_s_in_stmt(stmt: &Stmt) -> Option<String> {
    match stmt {
        Stmt::Expr(ExprStmt { expr, .. }) => find_require_s_in_expr(expr),
        Stmt::Decl(swc_core::ecma::ast::Decl::Var(var_decl)) => {
            for decl in &var_decl.decls {
                if let Some(init) = &decl.init {
                    if let Some(id) = find_require_s_in_expr(init) {
                        return Some(id);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Check an expression for `require(require.s = <id>)` and extract the module ID.
fn find_require_s_in_expr(expr: &Expr) -> Option<String> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if call.args.len() != 1 {
        return None;
    }
    // The argument should be `require.s = <id>`
    let Expr::Assign(assign) = &*call.args[0].expr else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    // Left side: require.s (a member expression)
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return None;
    };
    let MemberProp::Ident(prop_ident) = &member.prop else {
        return None;
    };
    if prop_ident.sym.as_ref() != "s" {
        return None;
    }
    // Right side: the module ID (numeric literal or string)
    match &*assign.right {
        Expr::Lit(Lit::Num(n)) => Some(format!("{}", n.value as i64)),
        Expr::Lit(Lit::Str(s)) => Some(s.value.as_str().unwrap_or("unknown").to_string()),
        _ => None,
    }
}

fn extract_getter_value(expr: &Expr) -> Option<Box<Expr>> {
    match expr {
        Expr::Fn(fn_expr) => {
            let body = fn_expr.function.body.as_ref()?;
            if body.stmts.len() == 1 {
                if let Stmt::Return(ret) = &body.stmts[0] {
                    return ret.arg.clone();
                }
            }
            None
        }
        Expr::Arrow(arrow_expr) => match &*arrow_expr.body {
            swc_core::ecma::ast::BlockStmtOrExpr::Expr(expr) => Some(expr.clone()),
            swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(block) => {
                if block.stmts.len() == 1 {
                    if let Stmt::Return(ret) = &block.stmts[0] {
                        return ret.arg.clone();
                    }
                }
                None
            }
        },
        _ => None,
    }
}

fn build_member_assign(obj_ident: Ident, prop_name: Atom, val: Box<Expr>, span: Span) -> Stmt {
    let member = MemberExpr {
        span,
        obj: Box::new(Expr::Ident(obj_ident)),
        prop: MemberProp::Ident(IdentName::new(prop_name, span)),
    };

    let assign = AssignExpr {
        span,
        op: AssignOp::Assign,
        left: AssignTarget::Simple(SimpleAssignTarget::Member(member)),
        right: val,
    };

    Stmt::Expr(ExprStmt {
        span,
        expr: Box::new(Expr::Assign(assign)),
    })
}

fn build_module_from_stmts(stmts: Vec<Stmt>) -> Module {
    Module {
        span: Default::default(),
        body: stmts.into_iter().map(ModuleItem::Stmt).collect(),
        shebang: None,
    }
}

fn emit_module(module: &Module, cm: Lrc<SourceMap>) -> anyhow::Result<String> {
    let mut output = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: Config::default().with_minify(false),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm.clone(), "\n", &mut output, None),
        };
        emitter
            .emit_module(module)
            .map_err(|e| anyhow!("emit error: {e:?}"))?;
    }
    String::from_utf8(output).map_err(|e| anyhow!("utf8 error: {e}"))
}

trait StmtSpan {
    fn span(&self) -> Span;
}

impl StmtSpan for Stmt {
    fn span(&self) -> Span {
        match self {
            Stmt::Expr(expr) => expr.span,
            Stmt::Block(block) => block.span,
            Stmt::Return(ret) => ret.span,
            Stmt::If(if_stmt) => if_stmt.span,
            Stmt::Throw(throw_stmt) => throw_stmt.span,
            Stmt::Decl(decl) => match decl {
                swc_core::ecma::ast::Decl::Var(var) => var.span,
                swc_core::ecma::ast::Decl::Fn(func) => func.function.span,
                swc_core::ecma::ast::Decl::Class(class) => class.class.span,
                _ => Default::default(),
            },
            _ => Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_wp5_cjs_min_numeric_keys_and_method_shorthand() {
        // Minified webpack 5 CJS bundle with numeric keys and method shorthand syntax
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/webpack-gen/dist/wp5-cjs-min/bundle.js"
        ))
        .expect("failed to read wp5-cjs-min fixture");

        let result = detect_and_extract(&source).expect("wp5-cjs-min should be detected");
        let module_ids: Vec<&str> = result.modules.iter().map(|m| m.id.as_str()).collect();

        // Should extract modules with numeric IDs
        assert!(
            result.modules.len() >= 2,
            "expected at least 2 modules, got {}: {:?}",
            result.modules.len(),
            module_ids
        );
        // Numeric module IDs should be present
        assert!(
            module_ids.iter().any(|id| id.parse::<i64>().is_ok()),
            "expected numeric module IDs, got {:?}",
            module_ids
        );
    }

    #[test]
    fn detects_wp5_require_s_entry() {
        // Webpack 5 bundle using __webpack_require__(__webpack_require__.s = 2) for entry
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/webpack-gen/dist/wp5-require-s/bundle.js"
        ))
        .expect("failed to read wp5-require-s fixture");

        let result = detect_and_extract(&source).expect("wp5-require-s should be detected");
        let module_ids: Vec<&str> = result.modules.iter().map(|m| m.id.as_str()).collect();

        assert!(
            result.modules.len() >= 2,
            "expected at least 2 modules, got {}: {:?}",
            result.modules.len(),
            module_ids
        );
        // Module "2" should be marked as entry
        let entry_modules: Vec<&str> = result
            .modules
            .iter()
            .filter(|m| m.is_entry)
            .map(|m| m.id.as_str())
            .collect();
        assert!(
            entry_modules.contains(&"2"),
            "expected module '2' to be marked as entry, entries: {:?}",
            entry_modules
        );
    }

    #[test]
    fn sanitize_filename_strips_dot_slash() {
        assert_eq!(sanitize_filename("./src/index.js"), "src/index.js");
    }

    #[test]
    fn sanitize_filename_strips_path_traversal() {
        assert_eq!(sanitize_filename("../../../etc/passwd"), "etc/passwd");
        assert_eq!(sanitize_filename("./../../foo.js"), "foo.js");
    }

    #[test]
    fn sanitize_filename_strips_backslash_traversal() {
        assert_eq!(
            sanitize_filename("./\\..\\node_modules\\debug\\src\\index"),
            "\\node_modules\\debug\\src\\index"
        );
    }

    #[test]
    fn sanitize_filename_empty_after_strip() {
        assert_eq!(sanitize_filename("../"), "unknown.js");
        assert_eq!(sanitize_filename("./"), "unknown.js");
    }

    #[test]
    fn sanitize_filename_preserves_normal_paths() {
        assert_eq!(sanitize_filename("src/utils.js"), "src/utils.js");
        assert_eq!(sanitize_filename("index.js"), "index.js");
    }

    #[test]
    fn detects_wp5_cjs_string_keys() {
        // Non-minified webpack 5 CJS bundle with string keys (and method shorthand)
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/webpack-gen/dist/wp5-cjs/bundle.js"
        ))
        .expect("failed to read wp5-cjs fixture");

        let result = detect_and_extract(&source).expect("wp5-cjs should be detected");
        let module_ids: Vec<&str> = result.modules.iter().map(|m| m.id.as_str()).collect();

        // Should have 3 modules: 2 library modules + 1 entry
        assert_eq!(
            result.modules.len(),
            3,
            "expected 3 modules, got {}: {:?}",
            result.modules.len(),
            module_ids
        );
        assert!(
            result.modules.iter().any(|m| m.is_entry),
            "expected an entry module, got {:?}",
            module_ids
        );
    }
}
