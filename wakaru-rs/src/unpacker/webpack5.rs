use anyhow::anyhow;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, Span, SyntaxContext, GLOBALS};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, CallExpr, Callee, Expr, ExprStmt, FnExpr, Function, Ident,
    IdentName, MemberExpr, MemberProp, Module, ModuleItem, ObjectLit, Pat, SimpleAssignTarget,
    Stmt, VarDecl, VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
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
        if call.args.len() != 2 {
            return None;
        }

        let Expr::Object(defs) = &*call.args[1].expr else {
            return None;
        };
        let span = stmt.span();
        let exports_ident = Ident::new(self.exports_sym.clone(), span, Default::default());
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

        Some(assignments)
    }
}

pub fn detect_and_extract(source: &str) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = parse_es_module(source, cm.clone()).ok()?;

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
    })
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
        let swc_core::ecma::ast::PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let swc_core::ecma::ast::Prop::KeyValue(key_value) = &**prop else {
            return None;
        };
        let swc_core::ecma::ast::PropName::Str(name) = &key_value.key else {
            return None;
        };
        let module_id = name.value.as_str()?.to_string();
        let filename = sanitize_filename(&module_id);

        let Some((factory, body_stmts)) = extract_factory(&key_value.value) else {
            return None;
        };
        let code = emit_webpack5_module(&factory, body_stmts, cm.clone())?;
        modules.push(UnpackedModule {
            id: module_id,
            is_entry: false,
            code,
            filename,
        });
    }

    if let Some(entry_body) = bootstrap_body
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
    }

    if modules.is_empty() {
        return None;
    }

    Some(UnpackResult { modules })
}

fn extract_webpack_modules_object(var_decl: &VarDecl) -> Option<&ObjectLit> {
    if var_decl.decls.len() != 1 {
        return None;
    }
    let VarDeclarator {
        init: Some(init), ..
    } = &var_decl.decls[0]
    else {
        return None;
    };
    let Expr::Object(object_lit) = strip_parens(init) else {
        return None;
    };
    if object_lit.props.is_empty() {
        return None;
    }
    for prop in &object_lit.props {
        let swc_core::ecma::ast::PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let swc_core::ecma::ast::Prop::KeyValue(key_value) = &**prop else {
            return None;
        };
        if !matches!(key_value.key, swc_core::ecma::ast::PropName::Str(_)) {
            return None;
        }
        if extract_factory(&key_value.value).is_none() {
            return None;
        }
    }
    Some(object_lit)
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

fn strip_parens<'a>(expr: &'a Expr) -> &'a Expr {
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
    module_id.trim_start_matches("./").to_string()
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

fn parse_es_module(source: &str, cm: Lrc<SourceMap>) -> anyhow::Result<Module> {
    let fm = cm.new_source_file(
        FileName::Custom("webpack5.js".to_string()).into(),
        source.to_string(),
    );
    let lexer = Lexer::new(
        Syntax::Es(EsSyntax {
            jsx: true,
            ..Default::default()
        }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    parser
        .parse_module()
        .map_err(|e| anyhow!("parse error: {e:?}"))
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
