use anyhow::anyhow;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, SyntaxContext, GLOBALS};
use swc_core::ecma::ast::{
    ArrayLit, Callee, Expr, ExprOrSpread, ExprStmt, FnExpr, Function, Lit, Module, ModuleItem,
    Number, Pat, Stmt,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::utils::replace_ident;
use swc_core::ecma::visit::VisitMutWith;

use crate::rules::apply_default_rules;
use crate::unpacker::{UnpackResult, UnpackedModule};

pub fn detect_and_extract(source: &str) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = parse_es_module(source, cm.clone()).ok()?;

        for item in &module.body {
            let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
                continue;
            };
            let Expr::Call(outer_call) = &**expr else {
                continue;
            };
            let Callee::Expr(callee_expr) = &outer_call.callee else {
                continue;
            };
            let Expr::Call(_) = &**callee_expr else {
                continue;
            };

            if let Some(result) = extract_browserify_modules(outer_call, cm.clone()) {
                return Some(result);
            }
        }
        None
    })
}

fn extract_browserify_modules(
    call: &swc_core::ecma::ast::CallExpr,
    cm: Lrc<SourceMap>,
) -> Option<UnpackResult> {
    if call.args.len() != 3 {
        return None;
    }

    let Expr::Array(entry_array) = &*call.args[2].expr else {
        return None;
    };
    let entry_ids = extract_entry_ids(entry_array)?;

    let Expr::Object(modules_obj) = &*call.args[0].expr else {
        return None;
    };

    let mut modules = Vec::new();

    for prop in &modules_obj.props {
        let swc_core::ecma::ast::PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let swc_core::ecma::ast::Prop::KeyValue(key_value) = &**prop else {
            return None;
        };

        let module_id = match &key_value.key {
            swc_core::ecma::ast::PropName::Num(Number { value, .. })
                if value.fract() == 0.0 && *value >= 0.0 =>
            {
                *value as usize
            }
            _ => return None,
        };

        let Expr::Array(module_parts) = &*key_value.value else {
            return None;
        };
        let (factory, body_stmts) = extract_factory(module_parts)?;

        let code = emit_browserify_module(&factory, body_stmts, cm.clone())?;
        let is_entry = entry_ids.contains(&module_id);
        let filename = if is_entry && entry_ids.len() == 1 {
            "entry.js".to_string()
        } else if is_entry {
            format!("entry-{module_id}.js")
        } else {
            format!("module-{module_id}.js")
        };

        modules.push(UnpackedModule {
            id: module_id.to_string(),
            is_entry,
            code,
            filename,
        });
    }

    if modules.is_empty() {
        return None;
    }

    Some(UnpackResult { modules })
}

fn extract_entry_ids(entry_array: &ArrayLit) -> Option<Vec<usize>> {
    let mut ids = Vec::new();
    for elem in &entry_array.elems {
        let Some(ExprOrSpread { expr, .. }) = elem else {
            return None;
        };
        let Expr::Lit(Lit::Num(Number { value, .. })) = &**expr else {
            return None;
        };
        if value.fract() != 0.0 || *value < 0.0 {
            return None;
        }
        ids.push(*value as usize);
    }
    Some(ids)
}

fn extract_factory(module_parts: &ArrayLit) -> Option<(Function, Vec<Stmt>)> {
    if module_parts.elems.len() != 2 {
        return None;
    }
    let Some(ExprOrSpread { expr, .. }) = &module_parts.elems[0] else {
        return None;
    };

    match &**expr {
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

fn emit_browserify_module(
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
    for (idx, target) in ["require", "module", "exports"].iter().enumerate() {
        let Some(old_sym) = param_syms.get(idx) else {
            continue;
        };
        if old_sym.as_ref() == *target {
            continue;
        }
        let from_id = (old_sym.clone(), unresolved_ctxt);
        let to_ident = swc_core::ecma::ast::Ident::new(
            Atom::from(*target),
            Default::default(),
            unresolved_ctxt,
        );
        replace_ident(&mut synthetic_module, from_id, &to_ident);
    }

    apply_default_rules(&mut synthetic_module, unresolved_mark);
    synthetic_module.visit_mut_with(&mut fixer(None));

    emit_module(&synthetic_module, cm).ok()
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
        FileName::Custom("browserify.js".to_string()).into(),
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
