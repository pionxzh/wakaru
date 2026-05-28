use std::collections::HashMap;

use anyhow::anyhow;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, Mark, SourceMap, SyntaxContext, DUMMY_SP, GLOBALS};
use swc_core::ecma::ast::{
    ArrayLit, AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, BlockStmtOrExpr, CallExpr,
    Callee, Expr, ExprStmt, FnExpr, Function, Ident, IdentName, Lit, MemberExpr, MemberProp,
    Module, ModuleItem, ObjectLit, Pat, Prop, PropName, PropOrSpread, SeqExpr, SimpleAssignTarget,
    Stmt, Str, UnaryExpr, UnaryOp, VarDecl, VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};

use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::utils::replace_ident;
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use crate::rules::{apply_rules as run_rules, RulePipelineOptions};
use crate::unpacker::webpack4::{rewrite_require_n_accesses, RequireIdRewriter};
use crate::unpacker::{UnpackResult, UnpackedModule};

struct Webpack5RuntimeNormalizer {
    require_sym: Atom,
    exports_sym: Atom,
    unresolved_mark: Mark,
}

impl VisitMut for Webpack5RuntimeNormalizer {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Some(ctxt) = self.try_match_require_member(expr, "g") {
            *expr = Expr::Ident(Ident::new(Atom::from("global"), DUMMY_SP, ctxt));
        } else if let Some(ctxt) = self.try_match_require_member(expr, "amdO") {
            *expr = amd_define_detection_expr(ctxt);
        }
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        let mut new_items = Vec::with_capacity(items.len());
        for item in items.drain(..) {
            if let ModuleItem::Stmt(stmt) = item {
                if let Some(replacements) = self.try_convert_stmt(&stmt, true) {
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
            if let Some(replacements) = self.try_convert_stmt(&stmt, false) {
                new_stmts.extend(replacements);
            } else {
                new_stmts.push(stmt);
            }
        }
        *stmts = new_stmts;
    }
}

impl Webpack5RuntimeNormalizer {
    fn try_match_require_member(&self, expr: &Expr, expected_prop: &str) -> Option<SyntaxContext> {
        let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
            return None;
        };
        let Expr::Ident(obj_ident) = &**obj else {
            return None;
        };
        if obj_ident.sym != self.require_sym || obj_ident.ctxt.outer() != self.unresolved_mark {
            return None;
        }
        let MemberProp::Ident(prop_name) = prop else {
            return None;
        };
        if prop_name.sym.as_ref() != expected_prop {
            return None;
        }
        Some(obj_ident.ctxt)
    }

    fn try_convert_stmt(
        &self,
        stmt: &Stmt,
        allow_module_decorator_removal: bool,
    ) -> Option<Vec<Stmt>> {
        let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
            return None;
        };
        let stripped_expr = strip_parens(expr);
        if allow_module_decorator_removal {
            if self.is_module_decorator_assignment(stripped_expr) {
                return Some(vec![]);
            }
            if let Expr::Seq(seq) = stripped_expr {
                return self.try_remove_module_decorator_from_seq(seq);
            }
        }
        let Expr::Call(call) = stripped_expr else {
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
            "r" => {
                if call.args.len() != 1 {
                    return None;
                }
                let Expr::Ident(exports_arg) = &*call.args[0].expr else {
                    return None;
                };
                if exports_arg.sym != self.exports_sym
                    || exports_arg.ctxt.outer() != self.unresolved_mark
                {
                    return None;
                }
                Some(vec![])
            }
            "d" => None,
            _ => None,
        }
    }

    fn try_remove_module_decorator_from_seq(&self, seq: &SeqExpr) -> Option<Vec<Stmt>> {
        let mut changed = false;
        let mut exprs = Vec::with_capacity(seq.exprs.len());
        for expr in &seq.exprs {
            if self.is_module_decorator_assignment(strip_parens(expr)) {
                changed = true;
            } else {
                exprs.push(expr.clone());
            }
        }
        if !changed {
            return None;
        }
        match exprs.len() {
            0 => Some(vec![]),
            1 => Some(vec![Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: exprs.pop().expect("single seq expr"),
            })]),
            _ => Some(vec![Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: Box::new(Expr::Seq(SeqExpr {
                    span: seq.span,
                    exprs,
                })),
            })]),
        }
    }

    fn is_module_decorator_assignment(&self, expr: &Expr) -> bool {
        let Expr::Assign(AssignExpr {
            op: AssignOp::Assign,
            left,
            right,
            ..
        }) = expr
        else {
            return false;
        };
        let AssignTarget::Simple(SimpleAssignTarget::Ident(left_ident)) = left else {
            return false;
        };
        if left_ident.id.sym.as_ref() != "module" {
            return false;
        }
        let Expr::Call(call) = &**right else {
            return false;
        };
        if call.args.len() != 1 {
            return false;
        }
        let Callee::Expr(callee_expr) = &call.callee else {
            return false;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
            return false;
        };
        let Expr::Ident(callee_obj) = &**obj else {
            return false;
        };
        if callee_obj.sym != self.require_sym || callee_obj.ctxt.outer() != self.unresolved_mark {
            return false;
        }
        let MemberProp::Ident(prop_name) = prop else {
            return false;
        };
        if !matches!(prop_name.sym.as_ref(), "hmd" | "nmd") {
            return false;
        }
        let Expr::Ident(arg_ident) = &*call.args[0].expr else {
            return false;
        };
        arg_ident.sym.as_ref() == "module" && arg_ident.ctxt == left_ident.id.ctxt
    }
}

fn amd_define_detection_expr(ctxt: SyntaxContext) -> Expr {
    Expr::Bin(BinExpr {
        span: DUMMY_SP,
        op: BinaryOp::LogicalAnd,
        left: Box::new(Expr::Bin(BinExpr {
            span: DUMMY_SP,
            op: BinaryOp::EqEqEq,
            left: Box::new(Expr::Unary(UnaryExpr {
                span: DUMMY_SP,
                op: UnaryOp::TypeOf,
                arg: Box::new(Expr::Ident(Ident::new("define".into(), DUMMY_SP, ctxt))),
            })),
            right: Box::new(Expr::Lit(Lit::Str(Str {
                span: DUMMY_SP,
                value: "function".into(),
                raw: None,
            }))),
        })),
        right: Box::new(Expr::Member(MemberExpr {
            span: DUMMY_SP,
            obj: Box::new(Expr::Ident(Ident::new("define".into(), DUMMY_SP, ctxt))),
            prop: MemberProp::Ident(IdentName::new("amd".into(), DUMMY_SP)),
        })),
    })
}

pub fn detect_and_extract(source: &str) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::parse_es_module(source, "webpack5.js", cm.clone()).ok()?;
        detect_from_module(&module, cm)
    })
}

pub(super) fn detect_from_module(module: &Module, cm: Lrc<SourceMap>) -> Option<UnpackResult> {
    let span = tracing::info_span!("webpack5: detect_from_module");
    let _enter = span.enter();
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
    let span = tracing::info_span!("webpack5: detect_chunk_from_module");
    let _enter = span.enter();
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
    let span = tracing::info_span!(
        "webpack5: extract_modules_from_object",
        count = modules_object.props.len()
    );
    let _enter = span.enter();

    let mut module_entries = Vec::new();

    for prop in &modules_object.props {
        let (module_id, factory, body_stmts) = extract_module_from_prop(prop)?;
        let filename = if module_id.contains('/') || module_id.contains('.') {
            sanitize_filename(&module_id)
        } else {
            format!("module-{module_id}.js")
        };
        module_entries.push((module_id, factory, body_stmts, filename));
    }

    let id_to_filename: HashMap<usize, String> = module_entries
        .iter()
        .filter_map(|(id, _, _, filename)| id.parse::<usize>().ok().map(|n| (n, filename.clone())))
        .collect();

    let mut modules = Vec::new();

    for (module_id, factory, body_stmts, filename) in &module_entries {
        let code = emit_webpack5_module(factory, body_stmts.clone(), cm.clone(), &id_to_filename)?;
        modules.push(UnpackedModule {
            id: module_id.clone(),
            is_entry: false,
            code,
            filename: filename.clone(),
        });
    }

    Some(modules)
}

fn extract_webpack5_modules(
    bootstrap_body: &swc_core::ecma::ast::BlockStmt,
    cm: Lrc<SourceMap>,
) -> Option<UnpackResult> {
    let span = tracing::info_span!("webpack5: extract_modules");
    let _enter = span.enter();

    let modules_object = {
        let span = tracing::info_span!("webpack5: find modules object");
        let _enter = span.enter();
        let mut found: Option<&ObjectLit> = None;
        for stmt in &bootstrap_body.stmts {
            let Stmt::Decl(swc_core::ecma::ast::Decl::Var(var_decl)) = stmt else {
                continue;
            };
            let Some(object_lit) = extract_webpack_modules_object(var_decl) else {
                continue;
            };
            found = Some(object_lit);
            break;
        }
        found?
    };

    let module_entries = {
        let span = tracing::info_span!("webpack5: collect module entries");
        let _enter = span.enter();
        let mut entries = Vec::new();
        for prop in &modules_object.props {
            let (module_id, factory, body_stmts) = extract_module_from_prop(prop)?;
            let filename = if module_id.contains('/') || module_id.contains('.') {
                sanitize_filename(&module_id)
            } else {
                format!("module-{module_id}.js")
            };
            entries.push((module_id, factory, body_stmts, filename));
        }
        entries
    };

    let id_to_filename: HashMap<usize, String> = module_entries
        .iter()
        .filter_map(|(id, _, _, filename)| id.parse::<usize>().ok().map(|n| (n, filename.clone())))
        .collect();

    let mut modules = Vec::new();

    {
        let span = tracing::info_span!("webpack5: emit all modules", count = module_entries.len());
        let _enter = span.enter();
        for (module_id, factory, body_stmts, filename) in &module_entries {
            let code =
                emit_webpack5_module(factory, body_stmts.clone(), cm.clone(), &id_to_filename)?;
            modules.push(UnpackedModule {
                id: module_id.clone(),
                is_entry: false,
                code,
                filename: filename.clone(),
            });
        }
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
        run_rules(
            &mut synthetic_module,
            unresolved_mark,
            RulePipelineOptions::until("UnEsm"),
        );
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

    // Fallback: scan bootstrap for entry-module startup calls.
    if !has_trailing_entry {
        if let Some(entry_id) =
            find_require_s_entry(bootstrap_body).or_else(|| find_require_o_entry(bootstrap_body))
        {
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
    id_to_filename: &HashMap<usize, String>,
) -> Option<String> {
    let span = tracing::info_span!("webpack5: emit_module");
    let _enter = span.enter();

    let mut synthetic_module = build_module_from_stmts(body_stmts);

    let param_syms: Vec<Atom> = factory
        .params
        .iter()
        .filter_map(|p| match &p.pat {
            Pat::Ident(binding) => Some(binding.sym.clone()),
            _ => None,
        })
        .collect();

    let unresolved_mark = {
        let span = tracing::info_span!("webpack5: resolver");
        let _enter = span.enter();
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        synthetic_module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        unresolved_mark
    };

    {
        let span = tracing::info_span!("webpack5: normalize");
        let _enter = span.enter();
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

        let mut id_rewriter = RequireIdRewriter {
            require_sym: require_sym.clone(),
            unresolved_mark,
            id_to_filename,
        };
        synthetic_module.visit_mut_with(&mut id_rewriter);

        rewrite_require_n_accesses(&mut synthetic_module, require_sym.clone(), unresolved_mark);

        let mut normalizer = Webpack5RuntimeNormalizer {
            require_sym,
            exports_sym,
            unresolved_mark,
        };
        synthetic_module.visit_mut_with(&mut normalizer);
    }

    {
        let span = tracing::info_span!("webpack5: rules");
        let _enter = span.enter();
        run_rules(
            &mut synthetic_module,
            unresolved_mark,
            RulePipelineOptions::until("UnEsm"),
        );
    }

    {
        let span = tracing::info_span!("webpack5: fixer+emit");
        let _enter = span.enter();
        synthetic_module.visit_mut_with(&mut fixer(None));
        emit_module(&synthetic_module, cm).ok()
    }
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

/// Scan the bootstrap body for webpack 5 runtime startup:
/// `require.O(void 0, [chunkId], function() { return require(<id>); })`.
fn find_require_o_entry(body: &swc_core::ecma::ast::BlockStmt) -> Option<String> {
    for stmt in &body.stmts {
        if let Some(id) = find_require_o_in_stmt(stmt) {
            return Some(id);
        }
    }
    None
}

fn find_require_o_in_stmt(stmt: &Stmt) -> Option<String> {
    match stmt {
        Stmt::Expr(ExprStmt { expr, .. }) => find_require_o_in_expr(expr),
        Stmt::Decl(swc_core::ecma::ast::Decl::Var(var_decl)) => {
            for decl in &var_decl.decls {
                if let Some(init) = &decl.init {
                    if let Some(id) = find_require_o_in_expr(init) {
                        return Some(id);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn find_require_o_in_expr(expr: &Expr) -> Option<String> {
    match strip_parens(expr) {
        Expr::Call(call) => extract_require_o_entry_call(call),
        Expr::Assign(assign) => find_require_o_in_expr(&assign.right),
        Expr::Seq(seq) => seq
            .exprs
            .iter()
            .find_map(|expr| find_require_o_in_expr(expr)),
        _ => None,
    }
}

fn extract_require_o_entry_call(call: &CallExpr) -> Option<String> {
    let require_sym = extract_require_o_callee_sym(&call.callee)?;
    if call.args.len() < 3 {
        return None;
    }
    extract_require_call_from_callback(&call.args[2].expr, require_sym)
}

fn extract_require_o_callee_sym(callee: &Callee) -> Option<&Atom> {
    let Callee::Expr(callee_expr) = callee else {
        return None;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
        return None;
    };
    let MemberProp::Ident(prop_ident) = prop else {
        return None;
    };
    if prop_ident.sym.as_ref() != "O" {
        return None;
    }
    let Expr::Ident(require_ident) = &**obj else {
        return None;
    };
    Some(&require_ident.sym)
}

fn extract_require_call_from_callback(expr: &Expr, require_sym: &Atom) -> Option<String> {
    match strip_parens(expr) {
        Expr::Fn(fn_expr) => {
            let body = fn_expr.function.body.as_ref()?;
            extract_require_call_from_body(body, require_sym)
        }
        Expr::Arrow(arrow) => match &*arrow.body {
            BlockStmtOrExpr::BlockStmt(body) => extract_require_call_from_body(body, require_sym),
            BlockStmtOrExpr::Expr(expr) => extract_require_call_id(expr, require_sym),
        },
        _ => None,
    }
}

fn extract_require_call_from_body(
    body: &swc_core::ecma::ast::BlockStmt,
    require_sym: &Atom,
) -> Option<String> {
    if body.stmts.len() != 1 {
        return None;
    }
    let Stmt::Return(ret) = &body.stmts[0] else {
        return None;
    };
    let arg = ret.arg.as_ref()?;
    extract_require_call_id(arg, require_sym)
}

fn extract_require_call_id(expr: &Expr, require_sym: &Atom) -> Option<String> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    let Expr::Ident(callee_ident) = &**callee_expr else {
        return None;
    };
    if &callee_ident.sym != require_sym || call.args.len() != 1 {
        return None;
    }
    match &*call.args[0].expr {
        Expr::Lit(Lit::Num(n)) => Some(format!("{}", n.value as i64)),
        Expr::Lit(Lit::Str(s)) => Some(s.value.as_str().unwrap_or("unknown").to_string()),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_wp5_cjs_min_numeric_keys_and_method_shorthand() {
        // Minified webpack 5 CJS bundle with numeric keys and method shorthand syntax
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/bundles/webpack-gen/dist/wp5-cjs-min/bundle.js"
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
            "/tests/bundles/webpack-gen/dist/wp5-require-s/bundle.js"
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
    fn detects_wp5_require_o_entry() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/bundles/webpack-gen/dist/wp5-require-o/bundle.js"
        ))
        .expect("failed to read wp5-require-o fixture");

        assert!(
            source.contains(".O(void 0") && source.contains("=>"),
            "fixture should reproduce webpack 5 require.O arrow startup:\n{source}"
        );

        let result = detect_and_extract(&source).expect("wp5 require.O startup should be detected");
        let entry_modules: Vec<&str> = result
            .modules
            .iter()
            .filter(|m| m.is_entry)
            .map(|m| m.id.as_str())
            .collect();

        assert!(
            entry_modules.len() == 1,
            "expected require.O callback target to be marked as entry"
        );
        let entry_id = entry_modules[0];
        let entry = result
            .modules
            .iter()
            .find(|module| module.id == entry_id)
            .expect("entry id should refer to an extracted module");
        assert!(
            entry.code.contains("entry:"),
            "entry marker should come from require-o-entry.js:\n{}",
            entry.code
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
            "/tests/bundles/webpack-gen/dist/wp5-cjs/bundle.js"
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
