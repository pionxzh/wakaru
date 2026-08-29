use std::collections::HashMap;

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap, Span, Spanned, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::*;
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};

use crate::module_path::relative_import_specifier;
use crate::unpacker::wrappers::body_looks_like_umd_wrapper;
use crate::unpacker::{
    arrow_iife_call_with_async, expr_has_function_level_special_bindings, function_level_returns,
    sanitize_relative_path, span_byte_range, stmts_have_function_level_special_bindings,
    BundleFormat, UnpackResult, UnpackedModule,
};
use crate::utils::paren::strip_parens;
use crate::utils::swc_safety::apply_fixer;

struct AmdDefine<'a> {
    id: String,
    deps: Vec<String>,
    factory: &'a Expr,
    is_anonymous: bool,
    span: Span,
}

pub(super) fn detect_from_module(module: &Module, cm: Lrc<SourceMap>) -> Option<UnpackResult> {
    let mut defines = Vec::new();
    let mut only_defines = true;
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) => {
                if is_directive_expr(expr) {
                    continue;
                }
                if let Some(define) = parse_define_call(expr) {
                    defines.push(define);
                } else {
                    only_defines = false;
                }
            }
            ModuleItem::Stmt(Stmt::Empty(_)) => {}
            _ => only_defines = false,
        }
    }

    if !defines.is_empty() && only_defines {
        return emit_define_modules(defines, cm);
    }

    emit_plain_umd_module(module, cm)
}

fn parse_define_call(expr: &Expr) -> Option<AmdDefine<'_>> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    let Expr::Ident(callee_ident) = strip_parens(callee_expr) else {
        return None;
    };
    if callee_ident.sym.as_ref() != "define" {
        return None;
    }

    let mut index = 0;
    let mut is_anonymous = true;
    let arg = call.args.get(index)?;
    let id = if let Some(id) = string_lit_value(&arg.expr) {
        index += 1;
        is_anonymous = false;
        id
    } else {
        "module".to_string()
    };

    let deps = if let Some(arg) = call.args.get(index) {
        if let Expr::Array(array) = strip_parens(&arg.expr) {
            index += 1;
            array
                .elems
                .iter()
                .map(|elem| {
                    let elem = elem.as_ref()?;
                    if elem.spread.is_some() {
                        return None;
                    }
                    string_lit_value(&elem.expr)
                })
                .collect::<Option<Vec<_>>>()?
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let factory = call.args.get(index)?.expr.as_ref();
    Some(AmdDefine {
        id,
        deps,
        factory,
        is_anonymous,
        span: call.span,
    })
}

fn string_lit_value(expr: &Expr) -> Option<String> {
    match strip_parens(expr) {
        Expr::Lit(Lit::Str(s)) => Some(s.value.as_str().unwrap_or("").to_string()),
        _ => None,
    }
}

fn emit_define_modules(defines: Vec<AmdDefine<'_>>, cm: Lrc<SourceMap>) -> Option<UnpackResult> {
    let named_count = defines.iter().filter(|define| !define.is_anonymous).count();
    let allow_anonymous = defines.len() == 1 && named_count == 0;
    if named_count == 0 && !allow_anonymous {
        return None;
    }

    let id_to_filename: HashMap<String, String> = defines
        .iter()
        .filter(|define| !define.is_anonymous)
        .map(|define| (define.id.clone(), amd_id_to_filename(&define.id)))
        .collect();

    let last_index = defines.len().saturating_sub(1);
    let mut modules = Vec::new();
    for (index, define) in defines.iter().enumerate() {
        if define.is_anonymous && !allow_anonymous {
            return None;
        }
        let filename = if define.is_anonymous {
            "module.js".to_string()
        } else {
            amd_id_to_filename(&define.id)
        };
        let module = factory_to_module(
            define.factory,
            &define.deps,
            &filename,
            &define.id,
            &id_to_filename,
        )?;
        modules.push(UnpackedModule {
            id: define.id.clone(),
            is_entry: index == last_index,
            code: emit_module(module, cm.clone()).ok()?,
            filename,
            source_ranges: span_byte_range(&cm, define.span).into_iter().collect(),
            inspection_context_ranges: Vec::new(),
            source_input: String::new(),
            generated_source_map: Vec::new(),
        });
    }

    Some(UnpackResult::new(modules, BundleFormat::Amd))
}

fn emit_plain_umd_module(module: &Module, cm: Lrc<SourceMap>) -> Option<UnpackResult> {
    let mut factory = None;
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) => {
                if is_directive_expr(expr) {
                    continue;
                }
                if factory.is_some() {
                    return None;
                }
                factory = Some((umd_factory_arg(expr)?, expr.span()));
            }
            ModuleItem::Stmt(Stmt::Empty(_)) => {}
            _ => return None,
        }
    }
    let (factory, wrapper_span) = factory?;

    let synthetic = factory_to_module(factory, &[], "module.js", "module", &HashMap::new())?;
    Some(UnpackResult::new(
        vec![UnpackedModule {
            id: "module".to_string(),
            is_entry: true,
            code: emit_module(synthetic, cm.clone()).ok()?,
            filename: "module.js".to_string(),
            source_ranges: span_byte_range(&cm, wrapper_span).into_iter().collect(),
            inspection_context_ranges: Vec::new(),
            source_input: String::new(),
            generated_source_map: Vec::new(),
        }],
        BundleFormat::Amd,
    ))
}

fn is_directive_expr(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Lit(Lit::Str(_)))
}

fn umd_factory_arg(expr: &Expr) -> Option<&Expr> {
    let call = top_level_call(expr)?;
    let (wrapper_params, wrapper_body) = wrapper_callee_parts(&call.callee)?;
    let factory_sym = wrapper_params.get(1)?;
    if !body_looks_like_umd_wrapper(wrapper_body, factory_sym) {
        return None;
    }

    let factory_arg = call.args.get(1)?;
    if factory_arg.spread.is_some() {
        return None;
    }
    Some(strip_parens(&factory_arg.expr))
}

fn top_level_call(expr: &Expr) -> Option<&CallExpr> {
    match strip_parens(expr) {
        Expr::Call(call) => Some(call),
        Expr::Unary(unary) if matches!(unary.op, UnaryOp::Bang) => match strip_parens(&unary.arg) {
            Expr::Call(call) => Some(call),
            _ => None,
        },
        _ => None,
    }
}

fn wrapper_callee_parts(callee: &Callee) -> Option<(Vec<Atom>, &FunctionBody)> {
    let Callee::Expr(callee_expr) = callee else {
        return None;
    };
    match strip_parens(callee_expr) {
        Expr::Fn(FnExpr { function, .. }) => {
            Some((function_params(&function.params), function.body.as_ref()?))
        }
        Expr::Arrow(arrow) => {
            let ArrowFunctionBody::FunctionBody(body) = &*arrow.body else {
                return None;
            };
            Some((pat_params(&arrow.params), body))
        }
        _ => None,
    }
}

fn factory_to_module(
    factory: &Expr,
    deps: &[String],
    filename: &str,
    module_id: &str,
    id_to_filename: &HashMap<String, String>,
) -> Option<Module> {
    match strip_parens(factory) {
        Expr::Fn(FnExpr { function, .. }) => {
            let params = function_params(&function.params);
            let body = function.body.as_ref()?;
            module_from_factory_parts(
                FactoryBody {
                    stmts: body.stmts.clone(),
                    returned_expr: None,
                    is_async: function.is_async,
                },
                deps,
                &params,
                filename,
                module_id,
                id_to_filename,
            )
        }
        Expr::Arrow(arrow) => {
            let params = pat_params(&arrow.params);
            let body = match &*arrow.body {
                ArrowFunctionBody::FunctionBody(body) => FactoryBody {
                    stmts: body.stmts.clone(),
                    returned_expr: None,
                    is_async: arrow.is_async,
                },
                ArrowFunctionBody::Expr(expr) => FactoryBody {
                    stmts: Vec::new(),
                    returned_expr: Some(strip_parens(expr).clone()),
                    is_async: arrow.is_async,
                },
            };
            module_from_factory_parts(body, deps, &params, filename, module_id, id_to_filename)
        }
        expr => module_from_factory_parts(
            FactoryBody {
                stmts: Vec::new(),
                returned_expr: Some(expr.clone()),
                is_async: false,
            },
            deps,
            &[],
            filename,
            module_id,
            id_to_filename,
        ),
    }
}

fn function_params(params: &[Param]) -> Vec<Atom> {
    params
        .iter()
        .filter_map(|param| pat_name(&param.pat))
        .collect()
}

fn pat_params(params: &[Pat]) -> Vec<Atom> {
    params.iter().filter_map(pat_name).collect()
}

fn pat_name(pat: &Pat) -> Option<Atom> {
    match pat {
        Pat::Ident(binding) => Some(binding.sym.clone()),
        _ => None,
    }
}

/// The executable parts of one AMD factory: its statements, the expression an
/// expression-bodied arrow returns, and whether the factory itself is async.
struct FactoryBody {
    stmts: Vec<Stmt>,
    returned_expr: Option<Expr>,
    is_async: bool,
}

fn module_from_factory_parts(
    body: FactoryBody,
    deps: &[String],
    params: &[Atom],
    filename: &str,
    module_id: &str,
    id_to_filename: &HashMap<String, String>,
) -> Option<Module> {
    let FactoryBody {
        stmts: body_stmts,
        returned_expr,
        is_async: factory_is_async,
    } = body;
    // Lifting the factory body to module scope (or into a restored arrow
    // boundary) changes what `this`, `arguments`, and `new.target` resolve to.
    // Fail closed so the whole-bundle fallback preserves the original
    // semantics and never emits top-level `new.target`.
    if stmts_have_function_level_special_bindings(&body_stmts)
        || returned_expr
            .as_ref()
            .is_some_and(expr_has_function_level_special_bindings)
    {
        return None;
    }
    let mut stmts = dependency_stmts(deps, params, filename, module_id, id_to_filename);
    let body_stmts = lower_factory_body(body_stmts, factory_is_async, deps)?;
    stmts.extend(body_stmts);
    if let Some(expr) = returned_expr {
        stmts.push(module_exports_stmt(Box::new(expr)));
    }
    Some(Module {
        span: DUMMY_SP,
        body: stmts.into_iter().map(ModuleItem::Stmt).collect(),
        shebang: None,
    })
}

fn lower_factory_body(
    mut stmts: Vec<Stmt>,
    factory_is_async: bool,
    deps: &[String],
) -> Option<Vec<Stmt>> {
    let (return_count, _) = function_level_returns(&stmts);
    let terminal_return_index = stmts.iter().rposition(|stmt| !is_hoist_only_var(stmt));

    if let Some(index) = terminal_return_index {
        if let Stmt::Return(return_stmt) = &mut stmts[index] {
            if return_stmt.arg.is_none() {
                stmts.remove(index);
            } else if return_count == 1 {
                let arg = return_stmt.arg.take().expect("value return checked above");
                stmts[index] = module_exports_stmt(arg);
                return Some(stmts);
            }
        }
    }

    let (return_count, has_value_return) = function_level_returns(&stmts);
    if return_count == 0 {
        return Some(stmts);
    }
    if deps
        .iter()
        .any(|dep| matches!(dep.as_str(), "exports" | "module"))
    {
        return None;
    }

    // The factory's own async flag is authoritative: a body can contain
    // await forms (such as `for await`) without any AwaitExpr node.
    let call = arrow_iife_call_with_async(stmts, factory_is_async);
    Some(vec![if has_value_return {
        module_exports_stmt(Box::new(call))
    } else {
        Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(call),
        })
    }])
}

fn is_hoist_only_var(stmt: &Stmt) -> bool {
    let Stmt::Decl(Decl::Var(var_decl)) = stmt else {
        return false;
    };
    var_decl.kind == VarDeclKind::Var && var_decl.decls.iter().all(|decl| decl.init.is_none())
}

fn dependency_stmts(
    deps: &[String],
    params: &[Atom],
    filename: &str,
    module_id: &str,
    id_to_filename: &HashMap<String, String>,
) -> Vec<Stmt> {
    let mut stmts = Vec::new();
    for (index, dep) in deps.iter().enumerate() {
        let param = params.get(index);
        match dep.as_str() {
            "require" | "exports" | "module" => {
                if let Some(param) = param {
                    if param.as_ref() != dep {
                        stmts.push(const_alias_stmt(param.clone(), ident_expr(dep)));
                    }
                }
            }
            _ => {
                let specifier = dependency_specifier(dep, filename, module_id, id_to_filename);
                let require_call = require_call_expr(specifier);
                if let Some(param) = param {
                    stmts.push(const_alias_stmt(param.clone(), require_call));
                } else {
                    stmts.push(Stmt::Expr(ExprStmt {
                        span: DUMMY_SP,
                        expr: Box::new(require_call),
                    }));
                }
            }
        }
    }
    stmts
}

fn const_alias_stmt(name: Atom, value: Expr) -> Stmt {
    Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent::from(Ident::new(
                name,
                DUMMY_SP,
                SyntaxContext::empty(),
            ))),
            init: Some(Box::new(value)),
            definite: false,
        }],
    })))
}

fn ident_expr(name: &str) -> Expr {
    Expr::Ident(Ident::new(name.into(), DUMMY_SP, SyntaxContext::empty()))
}

fn require_call_expr(specifier: String) -> Expr {
    Expr::Call(CallExpr {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        callee: Callee::Expr(Box::new(ident_expr("require"))),
        type_args: None,
        args: vec![ExprOrSpread {
            spread: None,
            expr: Box::new(Expr::Lit(Lit::Str(Str {
                span: DUMMY_SP,
                value: specifier.into(),
                raw: None,
            }))),
        }],
    })
}

fn module_exports_stmt(value: Box<Expr>) -> Stmt {
    Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Member(MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(ident_expr("module")),
                prop: MemberProp::Ident(IdentName::new("exports".into(), DUMMY_SP)),
            })),
            right: value,
        })),
    })
}

fn amd_id_to_filename(id: &str) -> String {
    let mut filename = sanitize_relative_path(id, "module");
    if !filename.ends_with(".js") {
        filename.push_str(".js");
    }
    filename
}

fn dependency_specifier(
    dep: &str,
    filename: &str,
    module_id: &str,
    id_to_filename: &HashMap<String, String>,
) -> String {
    // Bare AMD IDs are package/external specifiers unless another named define
    // in this bundle proves that the dependency is internal. Turning an
    // unknown bare ID into `./<id>.js` changes package resolution semantics.
    if !dep.starts_with('.') && !id_to_filename.contains_key(dep) {
        return dep.to_string();
    }

    let target = id_to_filename
        .get(dep)
        .cloned()
        .unwrap_or_else(|| amd_id_to_filename(&resolve_amd_id(module_id, dep)));
    relative_import_specifier(filename, &target)
}

fn resolve_amd_id(from_id: &str, dep: &str) -> String {
    if !dep.starts_with('.') {
        return dep.to_string();
    }
    let mut parts: Vec<&str> = from_id.split('/').collect();
    parts.pop();
    for part in dep.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    parts.join("/")
}

fn emit_module(mut module: Module, cm: Lrc<SourceMap>) -> anyhow::Result<String> {
    apply_fixer(&mut module)?;
    let mut output = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: Config::default(),
            comments: None,
            cm: cm.clone(),
            wr: JsWriter::new(cm, "\n", &mut output, None),
        };
        emitter.emit_module(&module)?;
    }
    Ok(String::from_utf8(output)?)
}
