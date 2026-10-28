use crate::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, Mark, SourceMap, Span, Spanned, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::*;
use swc_core::ecma::codegen::Config;
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};

use crate::module_path::relative_import_specifier;
use crate::rules::rename_utils::{rename_bindings_in_module, BindingRename};
use crate::unpacker::umd_node_branch::{
    umd_node_call, HoistedNames, UmdArg, UmdNodeCall, UmdResult,
};
use crate::unpacker::{
    arrow_iife_call_with_async, deconflict_runtime_binding_renames, emit_module_with_positions,
    expr_has_function_level_special_bindings, function_level_returns, sanitize_relative_path,
    span_byte_range, stmts_have_function_level_special_bindings, BundleFormat, MappedCode,
    SourcePositions, UnpackResult, UnpackedModule,
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

pub(super) fn detect_from_module(
    module: &Module,
    cm: Lrc<SourceMap>,
    positions: SourcePositions,
) -> Option<UnpackResult> {
    detect_define_modules_from_module(module, cm.clone(), positions)
        .or_else(|| emit_plain_umd_module(module, cm, positions))
}

/// A file of only top-level `define(…)` calls (and directives), split into
/// one module per define.
pub(super) fn detect_define_modules_from_module(
    module: &Module,
    cm: Lrc<SourceMap>,
    positions: SourcePositions,
) -> Option<UnpackResult> {
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

    if defines.is_empty() || !only_defines {
        return None;
    }
    emit_define_modules(defines, cm, positions)
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

fn emit_define_modules(
    defines: Vec<AmdDefine<'_>>,
    cm: Lrc<SourceMap>,
    positions: SourcePositions,
) -> Option<UnpackResult> {
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
        let emitted = emit_module(module, cm.clone(), positions).ok()?;
        modules.push(UnpackedModule {
            id: define.id.clone(),
            is_entry: index == last_index,
            code: emitted.code,
            filename,
            source_ranges: span_byte_range(&cm, define.span).into_iter().collect(),
            inspection_context_ranges: Vec::new(),
            source_input: String::new(),
            generated_source_map: emitted.points,
            verbatim_source_offset: None,
            mapped_in_every_mode: false,
        });
    }

    Some(UnpackResult::new(modules, BundleFormat::Amd))
}

/// A file whose only statement is a UMD wrapper call becomes one module: the
/// factory body at module scope, with each parameter bound to the argument
/// Node's CommonJS branch passes. A wrapper whose Node branch is not a plain
/// factory call (`umd_node_branch`) is not lifted, and the file keeps its
/// wrapper.
fn emit_plain_umd_module(
    module: &Module,
    cm: Lrc<SourceMap>,
    positions: SourcePositions,
) -> Option<UnpackResult> {
    let mut node_call = None;
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) => {
                if is_directive_expr(expr) {
                    continue;
                }
                if node_call.is_some() {
                    return None;
                }
                node_call = Some((umd_node_call(expr)?, expr.span()));
            }
            ModuleItem::Stmt(Stmt::Empty(_)) => {}
            _ => return None,
        }
    }
    let (node_call, wrapper_span) = node_call?;

    let synthetic = umd_factory_to_module(&node_call)?;
    let emitted = emit_module(synthetic, cm.clone(), positions).ok()?;
    Some(UnpackResult::new(
        vec![UnpackedModule {
            id: "module".to_string(),
            is_entry: true,
            code: emitted.code,
            filename: "module.js".to_string(),
            source_ranges: span_byte_range(&cm, wrapper_span).into_iter().collect(),
            inspection_context_ranges: Vec::new(),
            source_input: String::new(),
            generated_source_map: emitted.points,
            verbatim_source_offset: None,
            mapped_in_every_mode: false,
        }],
        BundleFormat::Amd,
    ))
}

fn is_directive_expr(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Lit(Lit::Str(_)))
}

/// Lifts the factory of Node's UMD branch to module scope.
///
/// Each parameter becomes a top-level `var` bound to its argument, declared
/// in parameter order after the factory's directive prologue, so the
/// `require` calls run in the order the call evaluated them. A parameter the
/// call does not pass is `undefined`, as in the call. The generated code reads
/// the CommonJS `exports`, `module`, and `require` (and writes
/// `module.exports`), so a factory that declares one of those names, or binds
/// a parameter of that name to anything but the same global, is not lifted.
fn umd_factory_to_module(node_call: &UmdNodeCall<'_>) -> Option<Module> {
    let (params, body) = match node_call.factory {
        Expr::Fn(FnExpr { ident, function }) => {
            if function.is_async || function.is_generator {
                return None;
            }
            let body = function.body.as_ref()?;
            // The lift drops the function's own name binding.
            if let Some(ident) = ident {
                if stmts_mention_name(&body.stmts, &ident.sym) {
                    return None;
                }
            }
            let params = function
                .params
                .iter()
                .map(|param| plain_param_name(&param.pat))
                .collect::<Option<Vec<_>>>()?;
            let body = FactoryBody {
                stmts: body.stmts.clone(),
                returned_expr: None,
                is_async: false,
            };
            (params, body)
        }
        Expr::Arrow(arrow) => {
            if arrow.is_async || arrow.is_generator {
                return None;
            }
            let params = arrow
                .params
                .iter()
                .map(plain_param_name)
                .collect::<Option<Vec<_>>>()?;
            let body = match &*arrow.body {
                ArrowFunctionBody::FunctionBody(body) => FactoryBody {
                    stmts: body.stmts.clone(),
                    returned_expr: None,
                    is_async: false,
                },
                ArrowFunctionBody::Expr(expr) => FactoryBody {
                    stmts: Vec::new(),
                    returned_expr: Some(strip_parens(expr).clone()),
                    is_async: false,
                },
            };
            (params, body)
        }
        _ => return None,
    };
    let unique: HashSet<&Atom> = params.iter().collect();
    if unique.len() != params.len() {
        return None;
    }

    let FactoryBody {
        stmts: mut body_stmts,
        returned_expr,
        ..
    } = body;
    if stmts_have_function_level_special_bindings(&body_stmts)
        || returned_expr
            .as_ref()
            .is_some_and(expr_has_function_level_special_bindings)
    {
        return None;
    }

    let declared = HoistedNames::of_stmts(&body_stmts);
    if ["exports", "module", "require"]
        .iter()
        .any(|name| declared.names.contains(&Atom::from(*name)))
    {
        return None;
    }
    // In the factory a parameter is bound before its body's function
    // declarations replace it; at module scope the declarations hoist first
    // and the binding would overwrite them.
    if params
        .iter()
        .any(|param| declared.function_names.contains(param))
    {
        return None;
    }

    let (_, has_value_return) = function_level_returns(&body_stmts);
    let returns_value = has_value_return || returned_expr.is_some();
    // The call discards the factory's result; lifting a `return` would turn
    // it into `module.exports`.
    if node_call.result == UmdResult::Discarded && returns_value {
        return None;
    }

    let mut param_bindings = Vec::new();
    for (index, param) in params.iter().enumerate() {
        let arg = node_call.args.get(index);
        let same_global = match arg {
            Some(UmdArg::Exports) => Some("exports"),
            Some(UmdArg::Module) => Some("module"),
            Some(UmdArg::Require) => Some("require"),
            _ => None,
        };
        if matches!(param.as_ref(), "exports" | "module" | "require") {
            if same_global != Some(param.as_ref()) {
                return None;
            }
            continue;
        }
        param_bindings.push(match (arg, same_global) {
            (_, Some(global)) => ParamBinding::Global(param.clone(), global),
            (Some(UmdArg::RequireCall(specifier)), _) => ParamBinding::Var(
                param.clone(),
                Some(require_call_expr(specifier.to_string())),
            ),
            _ => ParamBinding::Var(param.clone(), None),
        });
    }
    let extra_requires = node_call
        .args
        .iter()
        .skip(params.len())
        .filter_map(|arg| match arg {
            UmdArg::RequireCall(specifier) => Some(Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: Box::new(require_call_expr(specifier.to_string())),
            })),
            _ => None,
        })
        .collect::<Vec<_>>();

    let prologue_len = body_stmts
        .iter()
        .take_while(
            |stmt| matches!(stmt, Stmt::Expr(ExprStmt { expr, .. }) if is_directive_expr(expr)),
        )
        .count();
    let rest = body_stmts.split_off(prologue_len);
    let prologue = body_stmts;
    let mut lowered = lower_factory_body(rest, false, &[])?;
    if let Some(expr) = returned_expr {
        lowered.push(module_exports_stmt(Box::new(expr)));
    } else if node_call.result == UmdResult::ModuleExports && !returns_value {
        // `module.exports = factory()` with a factory that returns nothing
        // sets `module.exports` to `undefined` after the body ran.
        lowered.push(module_exports_stmt(Box::new(Expr::Unary(UnaryExpr {
            span: DUMMY_SP,
            op: UnaryOp::Void,
            arg: Box::new(Expr::Lit(Lit::Num(Number {
                span: DUMMY_SP,
                value: 0.0,
                raw: None,
            }))),
        }))));
    }

    let assemble = |aliases: bool| {
        let mut stmts = prologue.clone();
        for binding in &param_bindings {
            match binding {
                ParamBinding::Var(name, init) => stmts.push(var_stmt(name.clone(), init.clone())),
                ParamBinding::Global(name, global) if aliases => {
                    stmts.push(mutable_alias_stmt(name.clone(), ident_expr(global)));
                }
                ParamBinding::Global(..) => {}
            }
        }
        stmts.extend(extra_requires.iter().cloned());
        stmts.extend(lowered.iter().cloned());
        Module {
            span: DUMMY_SP,
            body: stmts.into_iter().map(ModuleItem::Stmt).collect(),
            shebang: None,
        }
    };

    // A parameter that receives `exports`, `module`, or `require` takes the
    // global's name, so `e.x = …` reads as `exports.x = …` to the rules
    // after unpack. Without a binding statement the parameter is free in the
    // lifted module; the renamer gives it the global's name unless that
    // would change what another name resolves to, and then the parameter
    // keeps its name as an alias (`var e = exports`).
    let globals = param_bindings
        .iter()
        .filter_map(|binding| match binding {
            ParamBinding::Global(name, global) => Some((name.clone(), *global)),
            ParamBinding::Var(..) => None,
        })
        .collect::<Vec<_>>();
    if !globals.is_empty() {
        let mut module = assemble(false);
        let unresolved_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
        let unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
        let renames = globals
            .into_iter()
            .map(|(name, global)| BindingRename {
                old: (name, unresolved_ctxt),
                new: global.into(),
            })
            .collect::<Vec<_>>();
        if deconflict_runtime_binding_renames(&mut module, &renames, None) {
            rename_bindings_in_module(&mut module, &renames);
            return Some(module);
        }
    }
    Some(assemble(true))
}

/// How a lifted UMD factory parameter is bound at module scope.
enum ParamBinding {
    /// `var name = init;` (`var name;` when the call passes nothing).
    Var(Atom, Option<Expr>),
    /// The parameter receives the CommonJS global of this name.
    Global(Atom, &'static str),
}

fn plain_param_name(pat: &Pat) -> Option<Atom> {
    match pat {
        Pat::Ident(binding) => Some(binding.sym.clone()),
        _ => None,
    }
}

fn stmts_mention_name(stmts: &[Stmt], name: &Atom) -> bool {
    struct Finder<'a> {
        name: &'a Atom,
        found: bool,
    }
    impl Visit for Finder<'_> {
        fn visit_ident(&mut self, ident: &Ident) {
            self.found |= &ident.sym == self.name;
        }
    }
    let mut finder = Finder { name, found: false };
    stmts.visit_with(&mut finder);
    finder.found
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
                        stmts.push(mutable_alias_stmt(param.clone(), ident_expr(dep)));
                    }
                }
            }
            _ => {
                let specifier = dependency_specifier(dep, filename, module_id, id_to_filename);
                let require_call = require_call_expr(specifier);
                if let Some(param) = param {
                    stmts.push(mutable_alias_stmt(param.clone(), require_call));
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

/// AMD factory parameters are mutable function-scoped bindings. Preserve that
/// declaration contract when lifting them out of the factory; the normal rule
/// pipeline can still narrow an untouched binding or recover it as an import.
fn mutable_alias_stmt(name: Atom, value: Expr) -> Stmt {
    var_stmt(name, Some(value))
}

fn var_stmt(name: Atom, init: Option<Expr>) -> Stmt {
    Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent::from(Ident::new(
                name,
                DUMMY_SP,
                SyntaxContext::empty(),
            ))),
            init: init.map(Box::new),
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

fn emit_module(
    mut module: Module,
    cm: Lrc<SourceMap>,
    positions: SourcePositions,
) -> anyhow::Result<MappedCode> {
    apply_fixer(&mut module)?;
    emit_module_with_positions(&module, cm, Config::default(), positions)
}
