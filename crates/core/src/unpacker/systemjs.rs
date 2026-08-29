use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{
    sync::Lrc, Globals, Mark, SourceMap, Span, SyntaxContext, DUMMY_SP, GLOBALS,
};
use swc_core::ecma::ast::{
    ArrayLit, ArrowFunctionBody, AssignExpr, AssignOp, AssignTarget, AssignTargetPat, BindingIdent,
    CallExpr, Callee, ComputedPropName, Decl, EsVersion, ExportDecl, ExportDefaultExpr,
    ExportNamedSpecifier, ExportSpecifier, Expr, ExprOrSpread, ExprStmt, FnExpr, Function,
    FunctionBody, Ident, ImportDecl, ImportDefaultSpecifier, ImportNamedSpecifier, ImportSpecifier,
    ImportStarAsSpecifier, KeyValueProp, Lit, MemberExpr, MemberProp, MetaPropExpr, MetaPropKind,
    Module, ModuleDecl, ModuleExportName, ModuleItem, NamedExport, ObjectLit, OptChainBase,
    ParenExpr, Pat, Prop, PropName, PropOrSpread, ReturnStmt, SimpleAssignTarget, Stmt, Str,
    UnaryOp, VarDecl, VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::js_names::{is_reserved_binding_name, is_valid_identifier_name};
use crate::unpacker::{span_byte_range, BundleFormat, UnpackResult, UnpackedModule};

pub(super) fn detect_from_module(module: &Module, cm: Lrc<SourceMap>) -> Option<UnpackResult> {
    let mut registers = Vec::new();

    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
            continue;
        };
        match register_from_expr(expr.as_ref()) {
            RegisterParse::Register(register) => {
                registers.push(register);
                continue;
            }
            RegisterParse::Invalid => return None,
            RegisterParse::NotRegister => {}
        }
        match registers_from_iife_expr(expr.as_ref()) {
            IifeRegisterParse::Registers(wrapped_registers) => {
                registers.extend(wrapped_registers);
            }
            IifeRegisterParse::Invalid => return None,
            IifeRegisterParse::NotIife => {}
        }
    }

    if registers.is_empty() {
        return None;
    }

    let multiple = registers.len() > 1;
    let mut seen = HashSet::new();
    let mut modules = Vec::new();
    for (idx, register) in registers.into_iter().enumerate() {
        let register_range = span_byte_range(&cm, register.span);
        if let Some(mut result) = try_unpack_dynamic_export_bundle(&register, cm.clone()) {
            // The nested bundle was re-parsed from emitted code, so its
            // spans are meaningless here; attribute the whole register call.
            for module in &mut result.modules {
                module.source_ranges = register_range.into_iter().collect();
            }
            modules.extend(result.modules);
            continue;
        }

        let filename = filename_for_register(register.name.as_deref(), idx, multiple, &mut seen);
        let code = emit_system_module(&register, filename.clone(), cm.clone(), multiple)?;
        let is_entry = idx == 0;
        modules.push(UnpackedModule {
            id: register.name.unwrap_or_else(|| idx.to_string()),
            is_entry,
            code,
            filename,
            source_ranges: register_range.into_iter().collect(),
            inspection_context_ranges: Vec::new(),
            source_input: String::new(),
            generated_source_map: Vec::new(),
        });
    }

    Some(UnpackResult::new(modules, BundleFormat::SystemJs))
}

fn try_unpack_dynamic_export_bundle(
    register: &SystemRegister,
    cm: Lrc<SourceMap>,
) -> Option<UnpackResult> {
    let export_sym = param_sym(&register.declare, 0)?;
    let body = register.declare.body.as_ref()?;
    let descriptor = extract_register_descriptor(body)?;
    let execute_body = descriptor.execute.body.as_ref()?;
    let expr = dynamic_export_expr(execute_body, &export_sym)?;
    let source = emit_expr_module(expr, cm).ok()?;
    crate::unpacker::try_unpack_bundle(&source).ok().flatten()
}

fn dynamic_export_expr<'a>(body: &'a FunctionBody, export_sym: &Atom) -> Option<&'a Expr> {
    if body.stmts.len() != 1 {
        return None;
    }
    let Stmt::Expr(expr_stmt) = &body.stmts[0] else {
        return None;
    };
    let Expr::Call(call) = expr_stmt.expr.as_ref() else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !matches!(callee.as_ref(), Expr::Ident(id) if id.sym == *export_sym) {
        return None;
    }
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return None;
    }
    if matches!(call.args[0].expr.as_ref(), Expr::Object(_)) {
        return None;
    }
    Some(call.args[0].expr.as_ref())
}

struct SystemRegister {
    name: Option<String>,
    deps: Vec<String>,
    declare: Function,
    /// Span of the whole `System.register(...)` call (provenance).
    span: Span,
    prelude: Vec<Stmt>,
}

fn is_system_register_call(call: &CallExpr) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    if !matches!(member.obj.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "System") {
        return false;
    }
    member_prop_name(&member.prop).is_some_and(|name| name == "register")
}

fn parse_register_call(call: &CallExpr) -> Option<SystemRegister> {
    let first = call.args.first()?;
    let (name, deps_arg_idx, declare_arg_idx) = match first.expr.as_ref() {
        Expr::Lit(Lit::Str(name)) => (Some(name.value.to_string_lossy().to_string()), 1, 2),
        _ => (None, 0, 1),
    };

    let deps = extract_string_array(call.args.get(deps_arg_idx)?.expr.as_ref())?;
    let declare = extract_declare_function(call.args.get(declare_arg_idx)?.expr.as_ref())?;

    Some(SystemRegister {
        name,
        deps,
        declare,
        span: call.span,
        prelude: Vec::new(),
    })
}

enum RegisterParse {
    Register(SystemRegister),
    Invalid,
    NotRegister,
}

fn register_from_expr(expr: &Expr) -> RegisterParse {
    let Expr::Call(call) = expr else {
        return RegisterParse::NotRegister;
    };
    if !is_system_register_call(call) {
        return RegisterParse::NotRegister;
    }
    parse_register_call(call)
        .map(RegisterParse::Register)
        .unwrap_or(RegisterParse::Invalid)
}

enum IifeRegisterParse {
    Registers(Vec<SystemRegister>),
    Invalid,
    NotIife,
}

fn registers_from_iife_expr(expr: &Expr) -> IifeRegisterParse {
    let Some(body) = iife_body(expr) else {
        return IifeRegisterParse::NotIife;
    };
    let mut registers = Vec::new();

    for (idx, stmt) in body.stmts.iter().enumerate() {
        let Stmt::Expr(expr_stmt) = stmt else {
            continue;
        };
        let mut register = match register_from_expr(expr_stmt.expr.as_ref()) {
            RegisterParse::Register(register) => register,
            RegisterParse::Invalid => return IifeRegisterParse::Invalid,
            RegisterParse::NotRegister => continue,
        };
        register.prelude = body.stmts[..idx]
            .iter()
            .filter(|stmt| {
                !matches!(stmt, Stmt::Expr(expr) if is_use_strict(expr) || is_register_expr_stmt(expr))
            })
            .cloned()
            .collect();
        registers.push(register);
    }

    IifeRegisterParse::Registers(registers)
}

fn is_register_expr_stmt(expr: &ExprStmt) -> bool {
    matches!(
        register_from_expr(expr.expr.as_ref()),
        RegisterParse::Register(_) | RegisterParse::Invalid
    )
}

fn iife_body(expr: &Expr) -> Option<&FunctionBody> {
    match expr {
        Expr::Paren(paren) => iife_body(paren.expr.as_ref()),
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => iife_body(unary.arg.as_ref()),
        Expr::Call(call) => match &call.callee {
            Callee::Expr(callee) => match callee.as_ref() {
                Expr::Fn(function) => function.function.body.as_ref(),
                Expr::Arrow(arrow) => match arrow.body.as_ref() {
                    ArrowFunctionBody::FunctionBody(body) => Some(body),
                    ArrowFunctionBody::Expr(_) => None,
                },
                Expr::Paren(paren) => iife_body(paren.expr.as_ref()),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

fn extract_string_array(expr: &Expr) -> Option<Vec<String>> {
    let Expr::Array(ArrayLit { elems, .. }) = expr else {
        return None;
    };
    let mut values = Vec::new();
    for elem in elems {
        let ExprOrSpread { expr, spread: None } = elem.as_ref()? else {
            return None;
        };
        let Expr::Lit(Lit::Str(value)) = expr.as_ref() else {
            return None;
        };
        values.push(value.value.to_string_lossy().to_string());
    }
    Some(values)
}

fn extract_function(expr: &Expr) -> Option<Function> {
    match expr {
        Expr::Fn(FnExpr { function, .. }) => Some(*function.clone()),
        Expr::Paren(paren) => extract_function(paren.expr.as_ref()),
        Expr::Arrow(arrow) => {
            let ArrowFunctionBody::FunctionBody(body) = arrow.body.as_ref() else {
                return None;
            };
            let params = arrow
                .params
                .iter()
                .cloned()
                .map(|pat| swc_core::ecma::ast::Param {
                    span: DUMMY_SP,
                    decorators: vec![],
                    pat,
                })
                .collect();
            Some(Function {
                params,
                decorators: vec![],
                span: DUMMY_SP,
                ctxt: Default::default(),
                this_param: None,
                body: Some(body.clone()),
                is_generator: arrow.is_generator,
                is_async: arrow.is_async,
                type_params: None,
                return_type: None,
            })
        }
        _ => None,
    }
}

/// Declare factories may be expression-body arrows (`() => ({ execute() {} })`).
/// Shared `extract_function` stays block-body-only so setter/execute arrows
/// that return a value are not rewritten into a top-level `return`.
fn extract_declare_function(expr: &Expr) -> Option<Function> {
    match expr {
        Expr::Fn(FnExpr { function, .. }) => Some(*function.clone()),
        Expr::Paren(paren) => extract_declare_function(paren.expr.as_ref()),
        Expr::Arrow(arrow) => {
            let body = match arrow.body.as_ref() {
                ArrowFunctionBody::FunctionBody(body) => body.clone(),
                ArrowFunctionBody::Expr(value) => FunctionBody {
                    span: DUMMY_SP,
                    stmts: vec![Stmt::Return(ReturnStmt {
                        span: DUMMY_SP,
                        arg: Some(value.clone()),
                    })],
                },
            };
            let params = arrow
                .params
                .iter()
                .cloned()
                .map(|pat| swc_core::ecma::ast::Param {
                    span: DUMMY_SP,
                    decorators: vec![],
                    pat,
                })
                .collect();
            Some(Function {
                params,
                decorators: vec![],
                span: DUMMY_SP,
                ctxt: Default::default(),
                this_param: None,
                body: Some(body),
                is_generator: arrow.is_generator,
                is_async: arrow.is_async,
                type_params: None,
                return_type: None,
            })
        }
        _ => None,
    }
}

fn emit_system_module(
    register: &SystemRegister,
    filename: String,
    cm: Lrc<SourceMap>,
    multiple: bool,
) -> Option<String> {
    // Missing `_export` is optional. A present but unreadable first param
    // (rest / destructure / default) stays fail-closed.
    let export_sym = match register.declare.params.first() {
        None => None,
        Some(param) => Some(pat_single_ident(&param.pat).cloned()?),
    };
    let context_sym = param_sym(&register.declare, 1);
    let body = register.declare.body.as_ref()?;
    let descriptor = extract_register_descriptor(body)?;
    let execute_body = descriptor.execute.body.as_ref()?;
    let export_call_spans = export_sym
        .as_ref()
        .map(|export_sym| collect_export_call_spans(&register.declare, export_sym))
        .unwrap_or_default();

    let imports = collect_imports(&register.deps, &descriptor.setters, export_sym.as_ref())?;
    let assigned_import_locals = imports
        .iter()
        .flat_map(|import| import.assigned_local_names())
        .collect::<HashSet<_>>();
    let inferred_import_locals = imports
        .iter()
        .flat_map(|import| import.leftover_named.iter().cloned())
        .collect::<Vec<_>>();

    let mut used_names = UsedIdentCollector::default();
    register.declare.visit_with(&mut used_names);
    for stmt in &register.prelude {
        stmt.visit_with(&mut used_names);
    }
    // A leftover member read has lost its original local alias. Do not let
    // that guessed local hide a real outer declaration before collision
    // checks have seen it.
    let hoisted = outer_hoisted_stmts(body, &assigned_import_locals);
    let mut lifted_stmts =
        Vec::with_capacity(register.prelude.len() + hoisted.len() + execute_body.stmts.len());
    lifted_stmts.extend(register.prelude.iter().cloned());
    lifted_stmts.extend(hoisted.iter().cloned());
    lifted_stmts.extend(execute_body.stmts.iter().cloned());

    let mut module_bound_names = assigned_import_locals;
    collect_lifted_decl_names(&lifted_stmts, &mut module_bound_names);
    for inferred in &inferred_import_locals {
        if !module_bound_names.insert(inferred.clone()) {
            return None;
        }
    }
    let export_name_usage = match export_sym.as_ref() {
        Some(export_sym) => collect_export_name_usage(
            &lifted_stmts,
            export_sym,
            &export_call_spans,
            &module_bound_names,
        ),
        None => ExportNameUsageSummary {
            mutable: Vec::new(),
            seen: Vec::new(),
        },
    };
    let ExportNameUsageSummary {
        mutable: mutable_export_names,
        seen: seen_export_names,
    } = export_name_usage;
    let mut direct_binding_candidates = match export_sym.as_ref() {
        Some(export_sym) => {
            collect_member_export_binding_names(&lifted_stmts, export_sym, &export_call_spans)
        }
        None => HashSet::new(),
    };
    direct_binding_candidates.extend(seen_export_names.into_iter().filter(|name| {
        name.as_ref() != "default"
            && Ident::verify_symbol(name.as_ref()).is_ok()
            && !is_reserved_binding_name(name.as_ref())
    }));
    direct_binding_candidates.extend(inferred_import_locals.iter().cloned());
    let unresolved_analysis = if !direct_binding_candidates.is_empty()
        && (direct_binding_candidates
            .iter()
            .any(|name| used_names.names.contains(name))
            || used_names.names.iter().any(|name| name.as_ref() == "eval"))
    {
        analyze_unresolved_names(&lifted_stmts)
    } else {
        UnresolvedNameAnalysis::default()
    };
    if inferred_import_locals
        .iter()
        .any(|name| unresolved_analysis.names.contains(name))
    {
        return None;
    }

    let mut items = Vec::new();
    for import in &imports {
        items.extend(import.to_module_items());
    }
    items.extend(register.prelude.iter().cloned().map(ModuleItem::Stmt));

    let mut transformer = SystemExecuteTransformer::new(
        export_sym,
        context_sym,
        used_names.names,
        module_bound_names,
        unresolved_analysis.names,
        unresolved_analysis.has_direct_eval,
        export_call_spans,
    );
    items.extend(transformer.prepare_mutable_exports(mutable_export_names));
    for stmt in hoisted {
        transformer.push_stmt(stmt, &mut items);
    }

    for stmt in &execute_body.stmts {
        transformer.push_stmt(stmt.clone(), &mut items);
    }
    // Expression-position object `_export` cannot be dropped without
    // changing the value. Keep the register rather than leftover `e({`.
    if transformer.leftover_export_call {
        if multiple {
            return original_register_code(register, &cm);
        }
        return None;
    }
    // Any unlowerable top-level object export keeps the register whole. A
    // reconstructable sibling does not make it safe to drop public names from
    // the object call. Lone register → None (Plain fallback); a sibling →
    // original text so `detect_from_module` does not `?` the rest of the bundle.
    if transformer.unlowerable_export {
        if multiple {
            return original_register_code(register, &cm);
        }
        return None;
    }
    items.extend(transformer.export_items());

    let module = Module {
        span: DUMMY_SP,
        body: items,
        shebang: None,
    };
    emit_module(&module, filename, cm).ok()
}

fn original_register_code(register: &SystemRegister, cm: &SourceMap) -> Option<String> {
    let (start, end) = span_byte_range(cm, register.span)?;
    let file = cm.lookup_byte_offset(register.span.lo).sf;
    file.src
        .get(start as usize..end as usize)
        .map(str::to_string)
}

struct RegisterDescriptor {
    setters: Vec<Option<Function>>,
    execute: Function,
}

fn extract_register_descriptor(body: &FunctionBody) -> Option<RegisterDescriptor> {
    let return_stmt = body.stmts.iter().find_map(|stmt| match stmt {
        Stmt::Return(ReturnStmt { arg: Some(arg), .. }) => Some(arg.as_ref()),
        _ => None,
    })?;
    // Expression-body arrows keep the grouping paren: `() => ({ execute() {} })`.
    let Expr::Object(obj) = strip_paren_expr(return_stmt) else {
        return None;
    };

    let mut setters = None;
    let mut execute = None;
    for prop in &obj.props {
        let PropOrSpread::Prop(prop) = prop else {
            continue;
        };
        match prop.as_ref() {
            Prop::KeyValue(key_value) => match prop_name(&key_value.key).as_deref() {
                Some("setters") => setters = Some(extract_setters(key_value.value.as_ref())?),
                Some("execute") => execute = Some(extract_function(key_value.value.as_ref())?),
                _ => {}
            },
            Prop::Method(method) if prop_name(&method.key).as_deref() == Some("execute") => {
                execute = Some(*method.function.clone());
            }
            _ => {}
        }
    }

    Some(RegisterDescriptor {
        setters: setters.unwrap_or_default(),
        execute: execute?,
    })
}

fn extract_setters(expr: &Expr) -> Option<Vec<Option<Function>>> {
    let Expr::Array(array) = expr else {
        return None;
    };
    let mut setters = Vec::new();
    for elem in &array.elems {
        let Some(ExprOrSpread { expr, spread: None }) = elem else {
            setters.push(None);
            continue;
        };
        if matches!(expr.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "undefined")
            || matches!(expr.as_ref(), Expr::Lit(Lit::Null(_)))
        {
            setters.push(None);
            continue;
        }
        setters.push(Some(extract_function(expr.as_ref())?));
    }
    Some(setters)
}

fn outer_hoisted_stmts(body: &FunctionBody, imported_locals: &HashSet<Atom>) -> Vec<Stmt> {
    let mut out = Vec::new();
    for stmt in &body.stmts {
        match stmt {
            Stmt::Return(_) => {}
            Stmt::Expr(expr) if is_use_strict(expr) => {}
            Stmt::Decl(Decl::Var(var)) => {
                let mut var = *var.clone();
                var.decls.retain(|decl| {
                    !pat_single_ident(&decl.name).is_some_and(|name| {
                        imported_locals.contains(name) || name.as_ref() == "__moduleName"
                    })
                });
                if !var.decls.is_empty() {
                    out.push(Stmt::Decl(Decl::Var(Box::new(var))));
                }
            }
            _ => out.push(stmt.clone()),
        }
    }
    out
}

/// Collect bindings that will share the recovered module's top-level region.
/// Nested function/class bodies keep their own scopes and are deliberately not
/// traversed. Block-scoped declarations are conservatively included: choosing
/// an alias is harmless, while missing a function-scoped `var` is not.
fn collect_lifted_decl_names(stmts: &[Stmt], names: &mut HashSet<Atom>) {
    let mut collector = LiftedDeclNameCollector { names };
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
}

struct LiftedDeclNameCollector<'a> {
    names: &'a mut HashSet<Atom>,
}

impl Visit for LiftedDeclNameCollector<'_> {
    fn visit_binding_ident(&mut self, binding: &BindingIdent) {
        self.names.insert(binding.id.sym.clone());
    }

    fn visit_fn_decl(&mut self, function: &swc_core::ecma::ast::FnDecl) {
        self.names.insert(function.ident.sym.clone());
    }

    fn visit_class_decl(&mut self, class: &swc_core::ecma::ast::ClassDecl) {
        self.names.insert(class.ident.sym.clone());
    }

    fn visit_function(&mut self, _function: &Function) {}

    fn visit_arrow_expr(&mut self, _arrow: &swc_core::ecma::ast::ArrowExpr) {}

    fn visit_class(&mut self, _class: &swc_core::ecma::ast::Class) {}
}

/// Resolve a clone of the statements in the scope they will occupy after
/// lifting. Any identifier still carrying `unresolved_mark` is a free/global
/// read that a newly introduced module binding would capture.
fn analyze_unresolved_names(stmts: &[Stmt]) -> UnresolvedNameAnalysis {
    let globals = Globals::new();
    GLOBALS.set(&globals, || {
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        let probe_fn = Function {
            params: Vec::new(),
            decorators: Vec::new(),
            span: DUMMY_SP,
            ctxt: SyntaxContext::empty(),
            this_param: None,
            body: Some(FunctionBody {
                span: DUMMY_SP,
                stmts: stmts.to_vec(),
            }),
            is_generator: false,
            is_async: true,
            type_params: None,
            return_type: None,
        };
        let mut probe = Module {
            span: DUMMY_SP,
            body: vec![ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: Box::new(Expr::Fn(FnExpr {
                    ident: None,
                    function: Box::new(probe_fn),
                })),
            }))],
            shebang: None,
        };
        probe.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        let mut collector = UnresolvedNameCollector {
            unresolved_ctxt: SyntaxContext::empty().apply_mark(unresolved_mark),
            names: HashSet::new(),
            has_direct_eval: false,
        };
        probe.visit_with(&mut collector);
        UnresolvedNameAnalysis {
            names: collector.names,
            has_direct_eval: collector.has_direct_eval,
        }
    })
}

#[derive(Default)]
struct UnresolvedNameAnalysis {
    names: HashSet<Atom>,
    has_direct_eval: bool,
}

struct UnresolvedNameCollector {
    unresolved_ctxt: SyntaxContext,
    names: HashSet<Atom>,
    has_direct_eval: bool,
}

impl Visit for UnresolvedNameCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.ctxt == self.unresolved_ctxt {
            self.names.insert(ident.sym.clone());
        }
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        // `(eval)(x)` is still a direct eval; only aliasing or optional
        // calls make it indirect.
        if matches!(
            &call.callee,
            Callee::Expr(callee)
                if matches!(
                    strip_paren_expr(callee),
                    Expr::Ident(ident)
                        if ident.sym.as_ref() == "eval" && ident.ctxt == self.unresolved_ctxt
                )
        ) {
            self.has_direct_eval = true;
        }
        call.visit_children_with(self);
    }
}

#[derive(Default)]
struct ExportNameUse {
    count: usize,
    shared_local: Option<Atom>,
    needs_mutable_binding: bool,
}

struct ExportNameUsageSummary {
    mutable: Vec<Atom>,
    /// Every exported name in first-seen order (two-arg and object keys).
    /// Each of these may become a direct ESM binding, so each is a candidate
    /// for the free-name analysis.
    seen: Vec<Atom>,
}

/// Resolve the original declare function and retain only calls whose callee is
/// the callback parameter binding. Resolving before lifting the execute body is
/// important: its locals belong to a nested function in the source and can
/// shadow the outer callback even though they later become module statements.
fn collect_export_call_spans(declare: &Function, export_sym: &Atom) -> HashSet<Span> {
    let globals = Globals::new();
    GLOBALS.set(&globals, || {
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        let mut probe = Module {
            span: DUMMY_SP,
            body: vec![ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: Box::new(Expr::Fn(FnExpr {
                    ident: None,
                    function: Box::new(declare.clone()),
                })),
            }))],
            shebang: None,
        };
        probe.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        let export_ctxt = match &probe.body[0] {
            ModuleItem::Stmt(Stmt::Expr(expr_stmt)) => match expr_stmt.expr.as_ref() {
                Expr::Fn(function) => match &function.function.params[0].pat {
                    Pat::Ident(binding) => binding.id.ctxt,
                    _ => unreachable!("the export callback is always an identifier parameter"),
                },
                _ => unreachable!("the export probe is always a function expression"),
            },
            _ => unreachable!("the export probe always contains one expression statement"),
        };
        let mut collector = ExportCallSpanCollector {
            export_sym,
            export_ctxt,
            spans: HashSet::new(),
        };
        probe.visit_with(&mut collector);
        collector.spans
    })
}

struct ExportCallSpanCollector<'a> {
    export_sym: &'a Atom,
    export_ctxt: SyntaxContext,
    spans: HashSet<Span>,
}

impl Visit for ExportCallSpanCollector<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if matches!(
            &call.callee,
            Callee::Expr(callee)
                if matches!(
                    callee.as_ref(),
                    Expr::Ident(ident)
                        if ident.sym == *self.export_sym && ident.ctxt == self.export_ctxt
                )
        ) {
            self.spans.insert(call.span);
        }
        call.visit_children_with(self);
    }
}

/// A repeated SystemJS export can reuse an existing ESM live binding only
/// when every update is backed by the same module-scope local. Different,
/// computed, or free values need one canonical mutable binding instead.
fn collect_export_name_usage(
    stmts: &[Stmt],
    export_sym: &Atom,
    export_call_spans: &HashSet<Span>,
    module_bound_names: &HashSet<Atom>,
) -> ExportNameUsageSummary {
    let mut collector = ExportNameUseCollector {
        export_sym,
        export_call_spans,
        uses: HashMap::new(),
        order: Vec::new(),
    };
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
    let mutable = collector
        .order
        .iter()
        .filter(|name| {
            collector.uses.get(*name).is_some_and(|usage| {
                usage.count > 1
                    && (usage.needs_mutable_binding
                        || !usage
                            .shared_local
                            .as_ref()
                            .is_some_and(|local| module_bound_names.contains(local)))
            })
        })
        .cloned()
        .collect();
    ExportNameUsageSummary {
        mutable,
        seen: collector.order,
    }
}

struct ExportNameUseCollector<'a> {
    export_sym: &'a Atom,
    export_call_spans: &'a HashSet<Span>,
    uses: HashMap<Atom, ExportNameUse>,
    order: Vec<Atom>,
}

impl ExportNameUseCollector<'_> {
    fn record(&mut self, exported: Atom, local: Option<Atom>) {
        let usage = self.uses.entry(exported.clone()).or_default();
        if usage.count == 0 {
            self.order.push(exported);
            usage.needs_mutable_binding = local.is_none();
            usage.shared_local = local;
        } else if usage.shared_local.as_ref() != local.as_ref() {
            usage.needs_mutable_binding = true;
        }
        usage.count += 1;
    }
}

impl Visit for ExportNameUseCollector<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if self.export_call_spans.contains(&call.span) {
            match parse_export_call(call, self.export_sym) {
                Some(ExportCall::Single { exported, value }) => {
                    self.record(exported, exported_value_local(value.as_ref()));
                }
                Some(ExportCall::Bulk(exports)) => {
                    for (exported, value) in exports {
                        self.record(exported, exported_value_local(value.as_ref()));
                    }
                }
                None => {}
            }
        }
        call.visit_children_with(self);
    }
}

fn collect_member_export_binding_names(
    stmts: &[Stmt],
    export_sym: &Atom,
    export_call_spans: &HashSet<Span>,
) -> HashSet<Atom> {
    let mut collector = MemberExportBindingNameCollector {
        export_sym,
        export_call_spans,
        names: HashSet::new(),
    };
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
    collector.names
}

struct MemberExportBindingNameCollector<'a> {
    export_sym: &'a Atom,
    export_call_spans: &'a HashSet<Span>,
    names: HashSet<Atom>,
}

impl Visit for MemberExportBindingNameCollector<'_> {
    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        let exported = assign
            .left
            .as_simple()
            .and_then(SimpleAssignTarget::as_member)
            .and_then(|member| match member.obj.as_ref() {
                Expr::Call(call) => Some(call),
                _ => None,
            })
            .filter(|call| self.export_call_spans.contains(&call.span))
            .and_then(|call| parse_export_call(call, self.export_sym))
            .and_then(|export_call| match export_call {
                ExportCall::Single { exported, value }
                    if exported.as_ref() != "default"
                        && exported_value_local(value.as_ref()).is_none()
                        && Ident::verify_symbol(exported.as_ref()).is_ok()
                        && !is_reserved_binding_name(exported.as_ref()) =>
                {
                    Some(exported)
                }
                _ => None,
            });
        if let Some(exported) = exported {
            self.names.insert(exported);
        }
        assign.visit_children_with(self);
    }
}

#[derive(Default)]
struct ImportParts {
    source: String,
    default: Option<Atom>,
    namespace: Option<Atom>,
    named: Vec<(Atom, Atom)>,
    /// Leftover `module.Name` gets. Not the same as `Name = module.Name`.
    leftover_named: Vec<Atom>,
    /// Live `export { local as Name }` from a setter assignment + `_export`.
    reexports: Vec<(Atom, Atom)>,
    /// `export { imported as Name } from "dep"` — no synthesized local.
    reexports_from: Vec<(Atom, Atom)>,
}

impl ImportParts {
    fn assigned_local_names(&self) -> Vec<Atom> {
        let mut names = Vec::new();
        names.extend(self.default.clone());
        names.extend(self.namespace.clone());
        names.extend(self.named.iter().map(|(_, local)| local.clone()));
        names
    }

    fn local_names(&self) -> Vec<Atom> {
        let mut names = self.assigned_local_names();
        names.extend(self.leftover_named.iter().cloned());
        names
    }

    fn has_import_bindings(&self) -> bool {
        self.default.is_some()
            || self.namespace.is_some()
            || !self.named.is_empty()
            || !self.leftover_named.is_empty()
    }

    fn to_module_items(&self) -> Vec<ModuleItem> {
        let mut items = Vec::new();
        let src = make_str(&self.source);
        if !self.has_import_bindings() {
            if self.reexports_from.is_empty() && self.reexports.is_empty() {
                items.push(ModuleItem::ModuleDecl(ModuleDecl::Import(ImportDecl {
                    span: DUMMY_SP,
                    specifiers: vec![],
                    src: Box::new(src),
                    type_only: false,
                    with: None,
                    phase: Default::default(),
                })));
                return items;
            }
            items.extend(self.reexport_from_items());
            items.extend(self.reexport_items());
            return items;
        }

        if let Some(namespace) = &self.namespace {
            items.push(ModuleItem::ModuleDecl(ModuleDecl::Import(ImportDecl {
                span: DUMMY_SP,
                specifiers: vec![ImportSpecifier::Namespace(ImportStarAsSpecifier {
                    span: DUMMY_SP,
                    local: ident(namespace.clone()),
                })],
                src: Box::new(src.clone()),
                type_only: false,
                with: None,
                phase: Default::default(),
            })));
        }

        let mut specifiers = Vec::new();
        if let Some(default) = &self.default {
            specifiers.push(ImportSpecifier::Default(ImportDefaultSpecifier {
                span: DUMMY_SP,
                local: ident(default.clone()),
            }));
        }
        specifiers.extend(self.named.iter().map(|(imported, local)| {
            ImportSpecifier::Named(ImportNamedSpecifier {
                span: DUMMY_SP,
                local: ident(local.clone()),
                imported: (imported != local).then(|| {
                    ModuleExportName::Ident(Ident::new(
                        imported.clone(),
                        DUMMY_SP,
                        Default::default(),
                    ))
                }),
                is_type_only: false,
            })
        }));
        specifiers.extend(self.leftover_named.iter().filter_map(|imported| {
            if self.named.iter().any(|(name, _)| name == imported) {
                return None;
            }
            Some(ImportSpecifier::Named(ImportNamedSpecifier {
                span: DUMMY_SP,
                local: ident(imported.clone()),
                imported: None,
                is_type_only: false,
            }))
        }));

        if !specifiers.is_empty() {
            items.push(ModuleItem::ModuleDecl(ModuleDecl::Import(ImportDecl {
                span: DUMMY_SP,
                specifiers,
                src: Box::new(src),
                type_only: false,
                with: None,
                phase: Default::default(),
            })));
        }

        items.extend(self.reexport_items());
        items.extend(self.reexport_from_items());
        items
    }

    fn reexport_items(&self) -> Vec<ModuleItem> {
        self.reexports
            .iter()
            .map(|(local, exported)| named_reexport_item(local.clone(), exported.clone()))
            .collect()
    }

    fn reexport_from_items(&self) -> Vec<ModuleItem> {
        self.reexports_from
            .iter()
            .map(|(imported, exported)| {
                named_export_from_item(imported.clone(), exported.clone(), &self.source)
            })
            .collect()
    }
}

fn collect_imports(
    deps: &[String],
    setters: &[Option<Function>],
    export_sym: Option<&Atom>,
) -> Option<Vec<ImportParts>> {
    let mut imports = Vec::new();
    for (idx, dep) in deps.iter().enumerate() {
        let mut parts = ImportParts {
            source: dep.clone(),
            ..Default::default()
        };
        let Some(Some(setter)) = setters.get(idx) else {
            imports.push(parts);
            continue;
        };
        let module_sym = param_sym(setter, 0);
        let body = setter.body.as_ref()?;
        if body.stmts.is_empty() {
            imports.push(parts);
            continue;
        }
        let module_sym = module_sym?;
        let mut pending_reexports = Vec::new();
        for part in collect_setter_parts(&body.stmts, &module_sym, export_sym)? {
            match part {
                SetterPart::Import(local, kind) => match kind {
                    SetterImportKind::Default => parts.default = Some(local),
                    SetterImportKind::Named(imported) => {
                        // An assigned leftover of the same specifier is the
                        // DCE twin of this write. Drop the leftover get only.
                        if let Some(idx) = parts
                            .leftover_named
                            .iter()
                            .position(|name| name == &imported)
                        {
                            parts.leftover_named.remove(idx);
                        }
                        if let Some((_, existing)) =
                            parts.named.iter().find(|(name, _)| *name == imported)
                        {
                            if *existing == local {
                                // same assigned binding twice
                            } else if parts.local_names().iter().any(|name| name == &local) {
                                return None;
                            } else {
                                parts.named.push((imported, local));
                            }
                        } else if parts.local_names().iter().any(|name| name == &local) {
                            return None;
                        } else {
                            parts.named.push((imported, local));
                        }
                    }
                    SetterImportKind::UnusedNamed(imported) => {
                        let already = parts.named.iter().any(|(name, _)| name == &imported)
                            || parts.leftover_named.iter().any(|name| name == &imported);
                        if already {
                            // leftover get of an already-imported name
                        } else if parts.local_names().iter().any(|name| name == &imported) {
                            // Invented leftover local collides (`module.n`
                            // beside `n = module.cclegacy`).
                            return None;
                        } else {
                            parts.leftover_named.push(imported);
                        }
                    }
                    SetterImportKind::Namespace => parts.namespace = Some(local),
                },
                SetterPart::Reexport { exported, value } => {
                    pending_reexports.push((exported, value));
                }
            }
        }
        for (exported, value) in pending_reexports {
            apply_setter_reexport(&mut parts, &module_sym, exported, value.as_ref())?;
        }
        imports.push(parts);
    }
    Some(imports)
}

enum SetterImportKind {
    Default,
    Named(Atom),
    /// Terser leftover `module.Name` with no assigned local.
    UnusedNamed(Atom),
    Namespace,
}

enum SetterPart {
    Import(Atom, SetterImportKind),
    Reexport { exported: Atom, value: Box<Expr> },
}

fn collect_setter_parts(
    stmts: &[Stmt],
    module_sym: &Atom,
    export_sym: Option<&Atom>,
) -> Option<Vec<SetterPart>> {
    let mut parts = Vec::new();
    let mut temp_name: Option<Atom> = None;
    let mut temp_pairs: Vec<(Atom, Box<Expr>)> = Vec::new();
    let mut temp_exported = false;
    let mut hoisted_temp: Option<Atom> = None;

    for stmt in stmts {
        if let Some(name) = empty_object_binding(stmt) {
            // One empty-object temp per setter. A second decl is not a proven shape.
            // Reusing the module or `_export` ident makes later member matches lie.
            if temp_name.is_some()
                || name == *module_sym
                || export_sym.is_some_and(|export_sym| name == *export_sym)
            {
                return None;
            }
            temp_name = Some(name);
            continue;
        }
        if let Some(name) = uninitialized_var_binding(stmt) {
            // Minifiers hoist `var n;` and then write `(n = {}).Foo = module.Foo`.
            if hoisted_temp.is_some()
                || temp_name.is_some()
                || name == *module_sym
                || export_sym.is_some_and(|export_sym| name == *export_sym)
            {
                return None;
            }
            hoisted_temp = Some(name);
            continue;
        }

        let exprs: Vec<&Expr> = match stmt {
            Stmt::Expr(expr_stmt) => match expr_stmt.expr.as_ref() {
                Expr::Seq(seq) => seq.exprs.iter().map(|expr| expr.as_ref()).collect(),
                expr => vec![expr],
            },
            _ => return None,
        };

        for expr in exprs {
            if temp_name.is_none() {
                if let Some(name) = empty_object_ident_assign(expr) {
                    if !can_start_named_temp(&name, module_sym, export_sym, hoisted_temp.as_ref()) {
                        return None;
                    }
                    temp_name = Some(name);
                    continue;
                }
                if let Some((name, key, value)) = named_temp_assign_with_empty_init(expr) {
                    if !can_start_named_temp(&name, module_sym, export_sym, hoisted_temp.as_ref()) {
                        return None;
                    }
                    temp_name = Some(name);
                    temp_pairs.push((key, value));
                    continue;
                }
            }

            if let Some(name) = &temp_name {
                if !temp_exported {
                    if let Some(pair) = named_temp_object_assign(expr, name) {
                        temp_pairs.push(pair);
                        continue;
                    }
                    if is_export_temp_ident(expr, export_sym, name) {
                        // Setter param shadowing `_export` is a namespace call, not a re-export.
                        if export_sym.is_some_and(|export_sym| module_sym == export_sym) {
                            return None;
                        }
                        // An empty temp is `_export({})`, which is not a proven setter shape.
                        if temp_pairs.is_empty() {
                            return None;
                        }
                        for (exported, value) in temp_pairs.drain(..) {
                            parts.push(SetterPart::Reexport { exported, value });
                        }
                        temp_exported = true;
                        continue;
                    }
                }
            }

            // Rebinding the temp as a setter import is not the named-object spelling.
            if let Some(name) = &temp_name {
                if setter_assignment_expr(expr, module_sym).is_some_and(|(local, _)| local == *name)
                {
                    return None;
                }
            }

            parts.push(setter_expr_part(expr, module_sym, export_sym)?);
        }
    }

    if temp_name.is_some() && !temp_exported {
        return None;
    }
    if hoisted_temp.is_some() && temp_name.is_none() {
        return None;
    }
    Some(parts)
}

fn can_start_named_temp(
    name: &Atom,
    module_sym: &Atom,
    export_sym: Option<&Atom>,
    hoisted_temp: Option<&Atom>,
) -> bool {
    if name == module_sym || export_sym.is_some_and(|export_sym| name == export_sym) {
        return false;
    }
    match hoisted_temp {
        None => false,
        Some(hoisted) => hoisted == name,
    }
}

fn uninitialized_var_binding(stmt: &Stmt) -> Option<Atom> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    if decl.init.is_some() {
        return None;
    }
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    Some(binding.id.sym.clone())
}

fn empty_object_ident_assign(expr: &Expr) -> Option<Atom> {
    let Expr::Assign(assign) = expr else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &assign.left else {
        return None;
    };
    let Expr::Object(object) = assign.right.as_ref() else {
        return None;
    };
    object.props.is_empty().then(|| binding.id.sym.clone())
}

fn named_temp_assign_with_empty_init(expr: &Expr) -> Option<(Atom, Atom, Box<Expr>)> {
    let Expr::Assign(assign) = expr else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return None;
    };
    let Expr::Assign(init) = strip_paren_expr(member.obj.as_ref()) else {
        return None;
    };
    if init.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &init.left else {
        return None;
    };
    let Expr::Object(object) = init.right.as_ref() else {
        return None;
    };
    if !object.props.is_empty() {
        return None;
    }
    let key = member_prop_name(&member.prop)?;
    Some((binding.id.sym.clone(), key, assign.right.clone()))
}

fn empty_object_binding(stmt: &Stmt) -> Option<Atom> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let Expr::Object(object) = decl.init.as_deref()? else {
        return None;
    };
    object.props.is_empty().then(|| binding.id.sym.clone())
}

fn named_temp_object_assign(expr: &Expr, temp: &Atom) -> Option<(Atom, Box<Expr>)> {
    let Expr::Assign(assign) = expr else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return None;
    };
    if !member_obj_ident(member, temp) {
        return None;
    }
    let key = member_prop_name(&member.prop)?;
    Some((key, assign.right.clone()))
}

fn is_export_temp_ident(expr: &Expr, export_sym: Option<&Atom>, temp: &Atom) -> bool {
    let Some(export_sym) = export_sym else {
        return false;
    };
    let Expr::Call(call) = expr else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !matches!(callee.as_ref(), Expr::Ident(id) if id.sym == *export_sym) {
        return false;
    }
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return false;
    }
    matches!(call.args[0].expr.as_ref(), Expr::Ident(id) if id.sym == *temp)
}

fn setter_expr_part(
    expr: &Expr,
    module_sym: &Atom,
    export_sym: Option<&Atom>,
) -> Option<SetterPart> {
    if let Some((local, kind)) = setter_assignment_expr(expr, module_sym) {
        return Some(SetterPart::Import(local, kind));
    }
    // Terser drops `a = module.Name` but keeps the getter. Recognize that
    // before requiring `_export`, including factories with no export param.
    if let Some((local, kind)) = unused_setter_member_import(expr, module_sym) {
        return Some(SetterPart::Import(local, kind));
    }
    let export_sym = export_sym?;
    // A setter parameter that shadows `_export` makes `e(...)` a call on the
    // module namespace, not a re-export. Keep the whole module fail-closed.
    if module_sym == export_sym {
        return None;
    }
    let Expr::Call(call) = expr else {
        return None;
    };
    let ExportCall::Single { exported, value } = parse_export_call(call, export_sym)? else {
        return None;
    };
    Some(SetterPart::Reexport { exported, value })
}

fn apply_setter_reexport(
    parts: &mut ImportParts,
    module_sym: &Atom,
    exported: Atom,
    value: &Expr,
) -> Option<()> {
    match value {
        Expr::Ident(id) if parts.local_names().iter().any(|name| name == &id.sym) => {
            parts.reexports.push((id.sym.clone(), exported));
            Some(())
        }
        Expr::Member(member) if member_obj_ident(member, module_sym) => {
            let imported = member_prop_name(&member.prop)?;
            if imported.as_ref() == "default" {
                if let Some(local) = &parts.default {
                    parts.reexports.push((local.clone(), exported));
                } else {
                    parts.reexports_from.push((Atom::from("default"), exported));
                }
                return Some(());
            }
            if let Some((_, local)) = parts.named.iter().find(|(name, _)| name == &imported) {
                parts.reexports.push((local.clone(), exported));
            } else {
                parts.reexports_from.push((imported, exported));
            }
            Some(())
        }
        _ => None,
    }
}

fn unused_setter_member_import(expr: &Expr, module_sym: &Atom) -> Option<(Atom, SetterImportKind)> {
    let Expr::Member(member) = strip_paren_expr(expr) else {
        return None;
    };
    if !member_obj_ident(member, module_sym) {
        return None;
    }
    let imported = member_prop_name(&member.prop)?;
    // Unused `module.default` is a default import, not this named shape.
    // Illegal / reserved keys would print as Ident and emit invalid ESM.
    if imported.as_ref() == "default"
        || !is_valid_identifier_name(imported.as_ref())
        || is_reserved_binding_name(imported.as_ref())
    {
        return None;
    }
    Some((imported.clone(), SetterImportKind::UnusedNamed(imported)))
}

fn setter_assignment_expr(expr: &Expr, module_sym: &Atom) -> Option<(Atom, SetterImportKind)> {
    let Expr::Assign(assign) = expr else {
        return None;
    };
    let left = assign.left.as_simple()?.as_ident()?.sym.clone();
    match assign.right.as_ref() {
        Expr::Ident(id) if id.sym == *module_sym => Some((left, SetterImportKind::Namespace)),
        Expr::Member(member) if member_obj_ident(member, module_sym) => {
            let imported = member_prop_name(&member.prop)?;
            if imported.as_ref() == "default" {
                Some((left, SetterImportKind::Default))
            } else {
                Some((left, SetterImportKind::Named(imported)))
            }
        }
        _ => None,
    }
}

struct MutableExportRewriter<'a> {
    export_sym: Option<&'a Atom>,
    bindings: &'a HashMap<Atom, Atom>,
    export_call_spans: &'a HashSet<Span>,
}

impl VisitMut for MutableExportRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        let replacement = match expr {
            Expr::Call(call) if self.export_call_spans.contains(&call.span) => {
                parse_optional_export_call(call, self.export_sym).and_then(|export_call| {
                    let ExportCall::Single {
                        exported,
                        mut value,
                    } = export_call
                    else {
                        return None;
                    };
                    let local = self.bindings.get(&exported)?.clone();
                    value.visit_mut_with(self);
                    // `_export(name, value)` evaluates to `value`. The assignment
                    // has the same result; grouping keeps member/callee precedence.
                    Some(Expr::Paren(ParenExpr {
                        span: DUMMY_SP,
                        expr: Box::new(assign_local_expr(local, value)),
                    }))
                })
            }
            _ => None,
        };
        if let Some(replacement) = replacement {
            *expr = replacement;
            return;
        }
        expr.visit_mut_children_with(self);
    }
}

struct SystemExecuteTransformer {
    export_sym: Option<Atom>,
    context_sym: Option<Atom>,
    exports: Vec<ExportBinding>,
    declared_exports: HashSet<(Atom, Atom)>,
    used_names: HashSet<Atom>,
    module_bound_names: HashSet<Atom>,
    unresolved_names: HashSet<Atom>,
    has_direct_eval: bool,
    /// Calls proven by resolver to target the declare callback, rather than a
    /// nested binding that happens to have the same minified name.
    export_call_spans: HashSet<Span>,
    mutable_export_bindings: HashMap<Atom, Atom>,
    /// Side-effect-free live-binding declarations needed by expression-position exports.
    pending_expr_export_decls: Vec<ModuleItem>,
    /// Top-level `_export({...})` that `export_call_items` could not lower.
    unlowerable_export: bool,
    /// Expression-position object `_export` left in place; the module must
    /// stay fail-closed so the free call is not emitted as ESM.
    leftover_export_call: bool,
}

impl SystemExecuteTransformer {
    fn new(
        export_sym: Option<Atom>,
        context_sym: Option<Atom>,
        used_names: HashSet<Atom>,
        module_bound_names: HashSet<Atom>,
        unresolved_names: HashSet<Atom>,
        has_direct_eval: bool,
        export_call_spans: HashSet<Span>,
    ) -> Self {
        Self {
            export_sym,
            context_sym,
            exports: Vec::new(),
            declared_exports: HashSet::new(),
            used_names,
            module_bound_names,
            unresolved_names,
            has_direct_eval,
            export_call_spans,
            mutable_export_bindings: HashMap::new(),
            pending_expr_export_decls: Vec::new(),
            unlowerable_export: false,
            leftover_export_call: false,
        }
    }

    fn prepare_mutable_exports(&mut self, exported_names: Vec<Atom>) -> Vec<ModuleItem> {
        let mut items = Vec::new();
        for exported in exported_names {
            let (_, declarations) = self.ensure_mutable_export_binding(exported);
            items.extend(declarations);
        }
        items
    }

    fn ensure_mutable_export_binding(&mut self, exported: Atom) -> (Atom, Vec<ModuleItem>) {
        if let Some(local) = self.mutable_export_bindings.get(&exported) {
            return (local.clone(), Vec::new());
        }

        let use_direct_binding =
            exported.as_ref() != "default" && self.can_bind_export_name(&exported);
        let local = if use_direct_binding {
            self.module_bound_names.insert(exported.clone());
            self.used_names.insert(exported.clone());
            exported.clone()
        } else {
            self.fresh_export_name()
        };

        self.mutable_export_bindings
            .insert(exported.clone(), local.clone());
        self.declared_exports
            .insert((local.clone(), exported.clone()));
        // Only the declaration is lifted. Each original `_export` call is
        // rewritten to an assignment at the same expression position.
        let declarations = if use_direct_binding {
            vec![export_let_item(local.clone())]
        } else {
            vec![
                let_binding_item(local.clone()),
                named_export_item(local.clone(), exported),
            ]
        };
        (local, declarations)
    }

    fn push_stmt(&mut self, mut stmt: Stmt, items: &mut Vec<ModuleItem>) {
        let mut mutable_export_rewriter = MutableExportRewriter {
            export_sym: self.export_sym.as_ref(),
            bindings: &self.mutable_export_bindings,
            export_call_spans: &self.export_call_spans,
        };
        stmt.visit_mut_with(&mut mutable_export_rewriter);

        if let Some(export_items) = self.take_export_stmt(&stmt) {
            items.extend(self.take_pending_expr_export_decls());
            items.extend(export_items);
            return;
        }

        // `var x = (_export("A", v1), _export("B", v2))`: init is a Seq; the old rewrite only accepts Call.
        if let Stmt::Decl(Decl::Var(var)) = &stmt {
            if let Some(export_items) = self.take_var_export_seq_decls(var) {
                items.extend(self.take_pending_expr_export_decls());
                items.extend(export_items);
                return;
            }
        }

        if let Stmt::Decl(Decl::Var(var)) = &mut stmt {
            self.rewrite_var_exports(var);
        }

        stmt.visit_mut_with(self);
        items.extend(self.take_pending_expr_export_decls());
        items.push(ModuleItem::Stmt(stmt));
    }

    fn take_pending_expr_export_decls(&mut self) -> Vec<ModuleItem> {
        std::mem::take(&mut self.pending_expr_export_decls)
    }

    fn export_items(self) -> Vec<ModuleItem> {
        if self.exports.is_empty() {
            return Vec::new();
        }
        let specifiers: Vec<_> = self
            .exports
            .into_iter()
            .filter(|binding| {
                !self
                    .declared_exports
                    .contains(&(binding.local.clone(), binding.exported.clone()))
            })
            .map(|binding| {
                let local = binding.local;
                let exported = binding.exported;
                ExportSpecifier::Named(ExportNamedSpecifier {
                    span: DUMMY_SP,
                    orig: ModuleExportName::Ident(ident(local.clone())),
                    exported: if exported.as_ref() == local.as_ref()
                        && is_valid_ident_name(exported.as_ref())
                    {
                        None
                    } else {
                        Some(export_name_node(&exported))
                    },
                    is_type_only: false,
                })
            })
            .collect();
        if specifiers.is_empty() {
            return Vec::new();
        }
        vec![ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(
            NamedExport {
                span: DUMMY_SP,
                specifiers,
                src: None,
                type_only: false,
                with: None,
            },
        ))]
    }

    fn is_export_callee(&self, call: &CallExpr) -> bool {
        let Some(export_sym) = self.export_sym.as_ref() else {
            return false;
        };
        self.export_call_spans.contains(&call.span)
            && matches!(
                &call.callee,
                Callee::Expr(callee)
                    if matches!(callee.as_ref(), Expr::Ident(id) if id.sym == *export_sym)
            )
    }

    fn parse_execute_export_call(&self, call: &CallExpr) -> Option<ExportCall> {
        if !self.is_export_callee(call) {
            return None;
        }
        parse_optional_export_call(call, self.export_sym.as_ref())
    }

    /// Lower a top-level `_export(...)`. An unlowerable Bulk / unreadable
    /// object is consumed and marks the module fail-closed, so the partial ESM
    /// output is discarded rather than leaving a free call. `_export(n)` and
    /// other unparsed execute shapes are left in place (`for-in` / `export *`
    /// stay out of scope). A Single that `export_call_items` cannot take is
    /// left for `visit_mut_expr`.
    fn take_parsed_export_call(&mut self, call: &CallExpr) -> Option<Vec<ModuleItem>> {
        if !self.is_export_callee(call) {
            return None;
        }
        match self.parse_execute_export_call(call) {
            Some(ExportCall::Bulk(exports)) => {
                match self.export_call_items(ExportCall::Bulk(exports)) {
                    Some(items) => Some(items),
                    None => {
                        self.unlowerable_export = true;
                        Some(Vec::new())
                    }
                }
            }
            Some(single) => self.export_call_items(single),
            None => {
                if export_call_object_arg(call) {
                    self.unlowerable_export = true;
                    Some(Vec::new())
                } else {
                    None
                }
            }
        }
    }

    fn take_export_stmt(&mut self, stmt: &Stmt) -> Option<Vec<ModuleItem>> {
        let Stmt::Expr(expr_stmt) = stmt else {
            return None;
        };
        match strip_paren_expr(expr_stmt.expr.as_ref()) {
            Expr::Call(call) => self.take_parsed_export_call(call),
            Expr::Assign(assign) => self
                .export_member_assignment_items(assign)
                .or_else(|| self.take_ident_assign_export(assign)),
            Expr::Seq(seq) => {
                let mut items = Vec::new();
                let mut saw_export = false;
                for expr in &seq.exprs {
                    let export_items = match strip_paren_expr(expr) {
                        Expr::Call(call) => self.take_parsed_export_call(call),
                        Expr::Assign(assign) => self
                            .export_member_assignment_items(assign)
                            .or_else(|| self.take_ident_assign_export(assign)),
                        _ => None,
                    };
                    if let Some(export_items) = export_items {
                        items.extend(export_items);
                        saw_export = true;
                    } else {
                        let mut stmt = Stmt::Expr(ExprStmt {
                            span: DUMMY_SP,
                            expr: expr.clone(),
                        });
                        stmt.visit_mut_with(self);
                        parenthesize_lifted_stmt_expr(&mut stmt);
                        items.extend(self.take_pending_expr_export_decls());
                        items.push(ModuleItem::Stmt(stmt));
                    }
                }
                saw_export.then_some(items)
            }
            _ => None,
        }
    }

    fn export_member_assignment_items(
        &mut self,
        assign: &swc_core::ecma::ast::AssignExpr,
    ) -> Option<Vec<ModuleItem>> {
        let member = assign.left.as_simple()?.as_member()?;
        let Expr::Call(call) = member.obj.as_ref() else {
            return None;
        };
        let ExportCall::Single { exported, value } = self.parse_execute_export_call(call)? else {
            return None;
        };

        let is_default = exported.as_ref() == "default";
        if !is_default && !is_valid_ident_name(exported.as_ref()) {
            return None;
        }
        let mut value = value;
        value.visit_mut_with(self);
        let exported_local = exported_value_local(value.as_ref());
        let local = exported_local
            .clone()
            .unwrap_or_else(|| self.bind_member_export_local(&exported, is_default));

        let mut assignment = assign.clone();
        let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &mut assignment.left else {
            return None;
        };
        *member.obj = Expr::Ident(ident(local.clone()));

        let mut assignment_stmt = Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(Expr::Assign(assignment)),
        });
        assignment_stmt.visit_mut_with(self);
        let mut items = if exported_local.is_some() {
            self.add_export(local.clone(), exported);
            if exported_value_is_assignment(value.as_ref()) {
                vec![ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                    span: DUMMY_SP,
                    expr: value,
                }))]
            } else {
                Vec::new()
            }
        } else if is_default {
            // Initialize the export before computed keys or the assignment RHS can re-enter.
            vec![
                self.binding_item(local.clone(), value),
                ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(ExportDefaultExpr {
                    span: DUMMY_SP,
                    expr: Box::new(Expr::Ident(ident(local.clone()))),
                })),
            ]
        } else if local.as_ref() == exported.as_ref() {
            vec![self.export_const_item(local.clone(), value)]
        } else {
            let mut items = vec![self.binding_item(local.clone(), value)];
            // Name already emitted as `export const` / named export: keep the
            // second value on a synthetic local, do not repeat the specifier.
            if !self.export_name_is_declared(&exported) {
                self.declared_exports
                    .insert((local.clone(), exported.clone()));
                items.push(named_export_item(local.clone(), exported));
            }
            items
        };
        items.push(ModuleItem::Stmt(assignment_stmt));
        Some(items)
    }

    /// Bind a free, legal Identifier to the export name. Reserved words and
    /// module-scope collisions keep the `__systemjs_export` alias path.
    fn bind_member_export_local(&mut self, exported: &Atom, is_default: bool) -> Atom {
        if !is_default && self.can_bind_export_name(exported) {
            self.module_bound_names.insert(exported.clone());
            self.used_names.insert(exported.clone());
            exported.clone()
        } else {
            self.fresh_export_name()
        }
    }

    fn can_bind_export_name(&self, exported: &Atom) -> bool {
        Ident::verify_symbol(exported.as_ref()).is_ok()
            && !is_reserved_binding_name(exported.as_ref())
            && !self.module_bound_names.contains(exported)
            && !self.unresolved_names.contains(exported)
            && !self.has_direct_eval
            && !self.export_name_is_declared(exported)
    }

    fn export_name_is_declared(&self, exported: &Atom) -> bool {
        self.declared_exports
            .iter()
            .any(|(_, name)| name == exported)
            || self
                .exports
                .iter()
                .any(|binding| &binding.exported == exported)
    }

    fn export_const_item(&mut self, name: Atom, value: Box<Expr>) -> ModuleItem {
        self.declared_exports.insert((name.clone(), name.clone()));
        self.module_bound_names.insert(name.clone());
        self.used_names.insert(name.clone());
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
            span: DUMMY_SP,
            decl: Decl::Var(Box::new(VarDecl {
                span: DUMMY_SP,
                ctxt: Default::default(),
                kind: swc_core::ecma::ast::VarDeclKind::Const,
                declare: false,
                decls: vec![VarDeclarator {
                    span: DUMMY_SP,
                    name: Pat::Ident(BindingIdent {
                        id: ident(name),
                        type_ann: None,
                    }),
                    init: Some(emit_exported_value(value)),
                    definite: false,
                }],
            })),
        }))
    }

    fn binding_item(&self, local: Atom, value: Box<Expr>) -> ModuleItem {
        let decl = Decl::Var(Box::new(VarDecl {
            span: DUMMY_SP,
            ctxt: Default::default(),
            kind: swc_core::ecma::ast::VarDeclKind::Const,
            declare: false,
            decls: vec![VarDeclarator {
                span: DUMMY_SP,
                name: Pat::Ident(BindingIdent {
                    id: ident(local),
                    type_ann: None,
                }),
                init: Some(emit_exported_value(value)),
                definite: false,
            }],
        }));
        ModuleItem::Stmt(Stmt::Decl(decl))
    }

    fn fresh_export_name(&mut self) -> Atom {
        let base = Atom::from("__systemjs_export");
        if self.used_names.insert(base.clone()) {
            self.module_bound_names.insert(base.clone());
            return base;
        }
        let mut suffix = 2u32;
        loop {
            let candidate = Atom::from(format!("__systemjs_export_{suffix}"));
            if self.used_names.insert(candidate.clone()) {
                self.module_bound_names.insert(candidate.clone());
                return candidate;
            }
            suffix += 1;
        }
    }

    fn export_call_items(&mut self, export_call: ExportCall) -> Option<Vec<ModuleItem>> {
        match export_call {
            ExportCall::Single {
                exported,
                mut value,
            } => {
                // A producer may nest aliases, for example Babel emits
                // `_export("b", _export("a", x = make()))`. Peel the inner
                // call before deciding which local backs the outer export.
                value.visit_mut_with(self);
                if let Expr::Assign(assign) = strip_paren_expr(value.as_ref()) {
                    let local = assign.left.as_simple()?.as_ident()?.sym.clone();
                    self.add_export(local, exported);
                    return Some(vec![ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                        span: DUMMY_SP,
                        expr: value,
                    }))]);
                }
                if let Some(local) = exported_value_local(value.as_ref()) {
                    self.add_export(local, exported);
                    return Some(Vec::new());
                }
                if exported.as_ref() == "default" {
                    if default_export_needs_binding(value.as_ref()) {
                        // SWC's fixer can remove the only parens that distinguish
                        // these values from a default function/class declaration.
                        // A local initializer is unambiguously expression context.
                        let local = self.fresh_export_name();
                        return Some(vec![
                            self.binding_item(local.clone(), value),
                            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(
                                ExportDefaultExpr {
                                    span: DUMMY_SP,
                                    expr: Box::new(Expr::Ident(ident(local))),
                                },
                            )),
                        ]);
                    }
                    // Function-callee IIFEs must stay expressions. Bare
                    // `export default function () {}()` is a SyntaxError.
                    return Some(vec![ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(
                        ExportDefaultExpr {
                            span: DUMMY_SP,
                            expr: emit_exported_value(value),
                        },
                    ))]);
                }
                if is_valid_ident_name(exported.as_ref()) && self.can_bind_export_name(&exported) {
                    return Some(vec![self.export_const_item(exported, value)]);
                }
                // Reserved, already taken, not an Identifier, or observable
                // elsewhere: keep the alias path. Illegal ident names still
                // become `export { local as "foo-bar" }` so the public name
                // is not dropped by peeling `_export`.
                let local = self.fresh_export_name();
                self.declared_exports
                    .insert((local.clone(), exported.clone()));
                Some(vec![
                    self.binding_item(local.clone(), value),
                    named_export_item(local, exported),
                ])
            }
            ExportCall::Bulk(exports) => {
                // Empty `_export({})` is not a proven execute shape.
                // Check every pair before committing: a later void 0 must
                // not leave earlier `add_export` / `export const` in place.
                if exports.is_empty()
                    || !exports
                        .iter()
                        .all(|(exported, value)| self.bulk_value_is_restorable(exported, value))
                {
                    self.unlowerable_export = true;
                    return None;
                }
                let export_len = self.exports.len();
                let declared_exports = self.declared_exports.clone();
                let used_names = self.used_names.clone();
                let module_bound_names = self.module_bound_names.clone();
                let mut items = Vec::new();
                for (exported, value) in exports {
                    match self.lower_bulk_export_value(exported, value) {
                        Some(part) => items.extend(part),
                        None => {
                            self.exports.truncate(export_len);
                            self.declared_exports = declared_exports;
                            self.used_names = used_names;
                            self.module_bound_names = module_bound_names;
                            self.unlowerable_export = true;
                            return None;
                        }
                    }
                }
                Some(items)
            }
        }
    }

    fn take_ident_assign_export(&mut self, assign: &AssignExpr) -> Option<Vec<ModuleItem>> {
        self.take_ident_assign_export_seq(assign)
            .or_else(|| self.take_ident_assign_single_export(assign))
    }

    fn take_ident_assign_single_export(&mut self, assign: &AssignExpr) -> Option<Vec<ModuleItem>> {
        if assign.op != AssignOp::Assign {
            return None;
        }
        let target = assign.left.as_simple()?.as_ident()?.clone();
        let Expr::Call(call) = strip_paren_expr(assign.right.as_ref()) else {
            return None;
        };
        let export_call = self.parse_execute_export_call(call)?;
        let (export_items, result) = self.export_call_result_items(export_call)?;
        let mut items = export_items;
        items.push(assign_ident_item(target, result));
        Some(items)
    }

    fn take_ident_assign_export_seq(&mut self, assign: &AssignExpr) -> Option<Vec<ModuleItem>> {
        if assign.op != AssignOp::Assign {
            return None;
        }
        let target = assign.left.as_simple()?.as_ident()?.clone();
        let exprs = self.as_export_seq(assign.right.as_ref())?;
        let last_idx = exprs.len().checked_sub(1)?;
        let mut items = Vec::new();
        for (idx, part) in exprs.iter().enumerate() {
            let export_call = match strip_paren_expr(part) {
                Expr::Call(call) => self.parse_execute_export_call(call),
                _ => None,
            };
            if idx == last_idx {
                if let Some(export_call) = export_call {
                    if let Some((export_items, result)) = self.export_call_result_items(export_call)
                    {
                        items.extend(export_items);
                        items.push(assign_ident_item(target, result));
                        return Some(items);
                    }
                }
                let mut last = part.clone();
                last.visit_mut_with(self);
                items.push(assign_ident_item(target, last));
                return Some(items);
            }
            if let Some(export_call) = export_call {
                if let Some(export_items) = self.export_sequence_prefix_items(export_call) {
                    items.extend(export_items);
                    continue;
                }
            }
            let mut stmt = Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: part.clone(),
            });
            stmt.visit_mut_with(self);
            parenthesize_lifted_stmt_expr(&mut stmt);
            items.push(ModuleItem::Stmt(stmt));
        }
        None
    }

    fn export_sequence_prefix_items(&mut self, export_call: ExportCall) -> Option<Vec<ModuleItem>> {
        match export_call {
            single @ ExportCall::Single { .. } => self
                .export_call_result_items(single)
                .map(|(items, _)| items),
            bulk @ ExportCall::Bulk(_) => match self.export_call_items(bulk) {
                Some(items) => Some(items),
                None => {
                    self.unlowerable_export = true;
                    Some(Vec::new())
                }
            },
        }
    }

    fn export_call_result_items(
        &mut self,
        export_call: ExportCall,
    ) -> Option<(Vec<ModuleItem>, Box<Expr>)> {
        let ExportCall::Single {
            exported,
            mut value,
        } = export_call
        else {
            return None;
        };
        value.visit_mut_with(self);

        if let Some(local) = exported_value_local(value.as_ref()) {
            self.add_export(local.clone(), exported);
            let result = Box::new(Expr::Ident(ident(local)));
            let items = if exported_value_is_assignment(value.as_ref()) {
                vec![ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                    span: DUMMY_SP,
                    expr: value,
                }))]
            } else {
                Vec::new()
            };
            return Some((items, result));
        }

        let is_default = exported.as_ref() == "default";
        if !is_default && self.can_bind_export_name(&exported) {
            let result = Box::new(Expr::Ident(ident(exported.clone())));
            return Some((vec![self.export_const_item(exported, value)], result));
        }

        let local = self.fresh_export_name();
        let mut items = vec![self.binding_item(local.clone(), value)];
        if is_default {
            items.push(ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(
                ExportDefaultExpr {
                    span: DUMMY_SP,
                    expr: Box::new(Expr::Ident(ident(local.clone()))),
                },
            )));
        } else {
            self.declared_exports
                .insert((local.clone(), exported.clone()));
            items.push(named_export_item(local.clone(), exported));
        }
        Some((items, Box::new(Expr::Ident(ident(local)))))
    }

    fn take_var_export_seq_decls(&mut self, var: &VarDecl) -> Option<Vec<ModuleItem>> {
        let has_seq = var.decls.iter().any(|decl| {
            decl.init
                .as_ref()
                .is_some_and(|init| self.as_export_seq(init).is_some())
        });
        if !has_seq {
            return None;
        }

        let mut items = Vec::new();
        for decl in &var.decls {
            if let (Some(local), Some(init)) =
                (pat_single_ident(&decl.name).cloned(), decl.init.as_ref())
            {
                if let Some((prefix, value)) =
                    self.take_bound_export_seq(init.as_ref(), Some(local.clone()))
                {
                    items.extend(prefix);
                    items.push(var_declarator_item(
                        var,
                        VarDeclarator {
                            span: decl.span,
                            name: decl.name.clone(),
                            init: Some(value),
                            definite: decl.definite,
                        },
                    ));
                    continue;
                }

                if let Expr::Call(call) = strip_paren_expr(init.as_ref()) {
                    if let Some(ExportCall::Single {
                        exported,
                        mut value,
                    }) = self.parse_execute_export_call(call)
                    {
                        value.visit_mut_with(self);
                        self.add_export(local, exported);
                        items.push(var_declarator_item(
                            var,
                            VarDeclarator {
                                span: decl.span,
                                name: decl.name.clone(),
                                init: Some(emit_exported_value(value)),
                                definite: decl.definite,
                            },
                        ));
                        continue;
                    }
                }
            }

            let mut leftover = decl.clone();
            leftover.visit_mut_with(self);
            items.push(var_declarator_item(var, leftover));
        }
        Some(items)
    }

    fn as_export_seq<'a>(&self, expr: &'a Expr) -> Option<&'a [Box<Expr>]> {
        let Expr::Seq(seq) = strip_paren_expr(expr) else {
            return None;
        };
        let has_export = seq.exprs.iter().any(|part| {
            matches!(
                strip_paren_expr(part),
                Expr::Call(call) if self.parse_execute_export_call(call).is_some()
            )
        });
        has_export.then_some(seq.exprs.as_slice())
    }

    /// Split an `_export` comma sequence into prefix export items plus the last value.
    /// When the last item is `_export(name, v)` and `bind` is set, `add_export(bind, name)` and return `v`.
    fn take_bound_export_seq(
        &mut self,
        expr: &Expr,
        bind: Option<Atom>,
    ) -> Option<(Vec<ModuleItem>, Box<Expr>)> {
        let exprs = self.as_export_seq(expr)?;
        let last_idx = exprs.len().checked_sub(1)?;
        let mut items = Vec::new();
        for (idx, part) in exprs.iter().enumerate() {
            let export_call = match strip_paren_expr(part) {
                Expr::Call(call) => self.parse_execute_export_call(call),
                _ => None,
            };
            if idx == last_idx {
                if let (Some(local), Some(ExportCall::Single { exported, value })) =
                    (bind.as_ref(), export_call)
                {
                    let mut value = value;
                    value.visit_mut_with(self);
                    self.add_export(local.clone(), exported);
                    return Some((items, value));
                }
                let mut last = part.clone();
                last.visit_mut_with(self);
                return Some((items, last));
            }
            if let Some(export_call) = export_call {
                if let Some(export_items) = self.export_sequence_prefix_items(export_call) {
                    items.extend(export_items);
                    continue;
                }
            }
            let mut stmt = Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: part.clone(),
            });
            stmt.visit_mut_with(self);
            parenthesize_lifted_stmt_expr(&mut stmt);
            items.push(ModuleItem::Stmt(stmt));
        }
        None
    }

    fn rewrite_var_exports(&mut self, var: &mut Box<VarDecl>) {
        for decl in &mut var.decls {
            let Some(init) = &mut decl.init else {
                continue;
            };
            let Expr::Call(call) = init.as_ref() else {
                continue;
            };
            let Some(ExportCall::Single { exported, value }) = self.parse_execute_export_call(call)
            else {
                continue;
            };
            let Some(local) = pat_single_ident(&decl.name).cloned() else {
                continue;
            };
            self.add_export(local, exported);
            *init = value;
        }
    }

    fn add_export(&mut self, local: Atom, exported: Atom) {
        if self
            .declared_exports
            .contains(&(local.clone(), exported.clone()))
        {
            return;
        }
        if self
            .exports
            .iter()
            .any(|existing| existing.local == local && existing.exported == exported)
        {
            return;
        }
        self.exports.push(ExportBinding { local, exported });
    }

    /// Ident values must already be module locals. Pretty-printed `undefined`
    /// and free globals (`window`) are not restorable — that would invent
    /// `export { undefined as Name }`. Assignments still name the target.
    fn restorable_bulk_local(&self, value: &Expr) -> Option<Atom> {
        let local = exported_value_local(value)?;
        if exported_value_is_assignment(value) || self.module_bound_names.contains(&local) {
            Some(local)
        } else {
            None
        }
    }

    fn bulk_value_is_restorable(&self, exported: &Atom, value: &Expr) -> bool {
        if self.mutable_export_bindings.contains_key(exported) {
            return true;
        }
        if self.restorable_bulk_local(value).is_some() {
            return true;
        }
        is_function_or_class_expr(value) && exported.as_ref() != "default"
    }

    /// Lower one `_export({ Name: value })` pair. Ident / assign reuse
    /// `add_export`; anonymous functions reuse `export_const_item`. A name
    /// already prepared as a live binding becomes an assignment.
    fn lower_bulk_export_value(
        &mut self,
        exported: Atom,
        mut value: Box<Expr>,
    ) -> Option<Vec<ModuleItem>> {
        value.visit_mut_with(self);
        if let Some(local) = self.mutable_export_bindings.get(&exported).cloned() {
            if local != exported {
                value = preserve_bulk_export_inferred_name(&exported, value);
            }
            return Some(vec![ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: Box::new(assign_local_expr(local, value)),
            }))]);
        }
        if let Some(local) = self.restorable_bulk_local(value.as_ref()) {
            self.add_export(local, exported);
            if exported_value_is_assignment(value.as_ref()) {
                return Some(vec![ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                    span: DUMMY_SP,
                    expr: value,
                }))]);
            }
            return Some(Vec::new());
        }
        // Same values Single already binds with `export const`. Keep the
        // inner function/class ident on the expression (#209), do not hoist it.
        if !is_function_or_class_expr(value.as_ref()) {
            return None;
        }
        if exported.as_ref() == "default" {
            return None;
        }
        if is_valid_ident_name(exported.as_ref()) && self.can_bind_export_name(&exported) {
            return Some(vec![self.export_const_item(exported, value)]);
        }
        let local = self.fresh_export_name();
        value = preserve_bulk_export_inferred_name(&exported, value);
        self.declared_exports
            .insert((local.clone(), exported.clone()));
        Some(vec![
            self.binding_item(local.clone(), value),
            named_export_item(local, exported),
        ])
    }
}

impl VisitMut for SystemExecuteTransformer {
    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        if self.is_context_import(call) {
            call.callee = Callee::Import(swc_core::ecma::ast::Import {
                span: DUMMY_SP,
                phase: Default::default(),
            });
        }
        call.visit_mut_children_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Member(member) = expr {
            if self.is_context_meta(member) {
                *expr = Expr::MetaProp(MetaPropExpr {
                    span: DUMMY_SP,
                    kind: MetaPropKind::ImportMeta,
                });
                return;
            }
        }

        if let Expr::Call(call) = expr {
            if self.is_export_callee(call) {
                if let Some(ExportCall::Single { exported, value }) =
                    parse_optional_export_call(call, self.export_sym.as_ref())
                {
                    let mut value = value;
                    value.visit_mut_with(self);
                    if let Some(local) = exported_value_local(value.as_ref()) {
                        self.add_export(local, exported);
                        *expr = export_replacement_expr(value);
                        return;
                    }

                    // Lifting an initialized declaration would evaluate `value`
                    // before its containing condition, call arguments, or other
                    // siblings. Lift only a side-effect-free declaration and keep
                    // the assignment exactly where `_export` appeared.
                    let (local, declarations) = self.ensure_mutable_export_binding(exported);
                    self.pending_expr_export_decls.extend(declarations);
                    *expr = Expr::Paren(ParenExpr {
                        span: DUMMY_SP,
                        expr: Box::new(assign_local_expr(local, value)),
                    });
                    return;
                }
                // Object `_export` in `&&` / call args / assignment RHS is not a
                // top-level drop. Mark leftover so emit stays fail-closed.
                if self.is_expression_unlowerable_export(call) {
                    self.leftover_export_call = true;
                }
            }
        }

        expr.visit_mut_children_with(self);
    }
}

impl SystemExecuteTransformer {
    fn is_expression_unlowerable_export(&self, call: &CallExpr) -> bool {
        if !self.is_export_callee(call) {
            return false;
        }
        match parse_optional_export_call(call, self.export_sym.as_ref()) {
            Some(ExportCall::Bulk(_)) => true,
            None => export_call_object_arg(call),
            Some(ExportCall::Single { .. }) => false,
        }
    }

    fn is_context_import(&self, call: &CallExpr) -> bool {
        let Some(context_sym) = &self.context_sym else {
            return false;
        };
        let Callee::Expr(callee) = &call.callee else {
            return false;
        };
        let Expr::Member(member) = callee.as_ref() else {
            return false;
        };
        member_obj_ident(member, context_sym)
            && member_prop_name(&member.prop).is_some_and(|prop| prop.as_ref() == "import")
    }

    fn is_context_meta(&self, member: &MemberExpr) -> bool {
        let Some(context_sym) = &self.context_sym else {
            return false;
        };
        member_obj_ident(member, context_sym)
            && member_prop_name(&member.prop).is_some_and(|prop| prop.as_ref() == "meta")
    }
}

#[derive(Clone)]
struct ExportBinding {
    local: Atom,
    exported: Atom,
}

fn export_name_node(name: &Atom) -> ModuleExportName {
    if is_valid_ident_name(name.as_ref()) {
        ModuleExportName::Ident(ident(name.clone()))
    } else {
        ModuleExportName::Str(make_str(name.as_ref()))
    }
}

fn named_export_item(local: Atom, exported: Atom) -> ModuleItem {
    ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
        span: DUMMY_SP,
        specifiers: vec![ExportSpecifier::Named(ExportNamedSpecifier {
            span: DUMMY_SP,
            orig: ModuleExportName::Ident(ident(local)),
            exported: Some(export_name_node(&exported)),
            is_type_only: false,
        })],
        src: None,
        type_only: false,
        with: None,
    }))
}

fn named_reexport_item(local: Atom, exported: Atom) -> ModuleItem {
    ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
        span: DUMMY_SP,
        specifiers: vec![ExportSpecifier::Named(ExportNamedSpecifier {
            span: DUMMY_SP,
            orig: ModuleExportName::Ident(ident(local.clone())),
            exported: (local.as_ref() != exported.as_ref()).then(|| export_name_node(&exported)),
            is_type_only: false,
        })],
        src: None,
        type_only: false,
        with: None,
    }))
}

fn named_export_from_item(imported: Atom, exported: Atom, source: &str) -> ModuleItem {
    ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
        span: DUMMY_SP,
        specifiers: vec![ExportSpecifier::Named(ExportNamedSpecifier {
            span: DUMMY_SP,
            orig: export_name_node(&imported),
            exported: (imported.as_ref() != exported.as_ref()).then(|| export_name_node(&exported)),
            is_type_only: false,
        })],
        src: Some(Box::new(make_str(source))),
        type_only: false,
        with: None,
    }))
}

fn let_binding_item(local: Atom) -> ModuleItem {
    ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: swc_core::ecma::ast::VarDeclKind::Let,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent {
                id: ident(local),
                type_ann: None,
            }),
            init: None,
            definite: false,
        }],
    }))))
}

fn export_let_item(name: Atom) -> ModuleItem {
    ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
        span: DUMMY_SP,
        decl: Decl::Var(Box::new(VarDecl {
            span: DUMMY_SP,
            ctxt: Default::default(),
            kind: swc_core::ecma::ast::VarDeclKind::Let,
            declare: false,
            decls: vec![VarDeclarator {
                span: DUMMY_SP,
                name: Pat::Ident(BindingIdent {
                    id: ident(name),
                    type_ann: None,
                }),
                init: None,
                definite: false,
            }],
        })),
    }))
}

enum ExportCall {
    Single { exported: Atom, value: Box<Expr> },
    Bulk(Vec<(Atom, Box<Expr>)>),
}

fn parse_optional_export_call(call: &CallExpr, export_sym: Option<&Atom>) -> Option<ExportCall> {
    parse_export_call(call, export_sym?)
}

fn export_call_object_arg(call: &CallExpr) -> bool {
    call.args.len() == 1
        && call.args[0].spread.is_none()
        && matches!(
            strip_paren_expr(call.args[0].expr.as_ref()),
            Expr::Object(_)
        )
}

fn parse_export_call(call: &CallExpr, export_sym: &Atom) -> Option<ExportCall> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    if !matches!(callee.as_ref(), Expr::Ident(id) if id.sym == *export_sym) {
        return None;
    }

    if call.args.len() == 2 {
        let exported = string_lit_arg(&call.args[0])?;
        let value = call.args[1].expr.clone();
        return Some(ExportCall::Single { exported, value });
    }

    if call.args.len() == 1 {
        let Expr::Object(object) = call.args[0].expr.as_ref() else {
            return None;
        };
        return Some(ExportCall::Bulk(object_export_pairs(object)?));
    }

    None
}

fn object_export_pairs(object: &ObjectLit) -> Option<Vec<(Atom, Box<Expr>)>> {
    let mut pairs = Vec::new();
    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        match prop.as_ref() {
            Prop::Shorthand(id) => pairs.push((id.sym.clone(), Box::new(Expr::Ident(id.clone())))),
            Prop::KeyValue(kv) => pairs.push((prop_name(&kv.key)?.into(), kv.value.clone())),
            // Pretty-printers rewrite `assert: function () {}` as a method.
            Prop::Method(method) => pairs.push((
                prop_name(&method.key)?.into(),
                Box::new(Expr::Fn(FnExpr {
                    ident: None,
                    function: method.function.clone(),
                })),
            )),
            _ => return None,
        }
    }
    Some(pairs)
}

fn exported_value_local(expr: &Expr) -> Option<Atom> {
    match strip_paren_expr(expr) {
        Expr::Ident(id) => Some(id.sym.clone()),
        Expr::Assign(assign) => assign.left.as_simple()?.as_ident().map(|id| id.sym.clone()),
        _ => None,
    }
}

fn is_function_or_class_expr(expr: &Expr) -> bool {
    matches!(
        strip_paren_expr(expr),
        Expr::Fn(_) | Expr::Arrow(_) | Expr::Class(_)
    )
}

/// Object-literal properties perform NamedEvaluation for anonymous functions,
/// arrows, and classes. When a public export name cannot be used as the local
/// ESM binding, retain that observable `.name` by evaluating the value through
/// a computed property with the original key.
fn preserve_bulk_export_inferred_name(exported: &Atom, value: Box<Expr>) -> Box<Expr> {
    let is_anonymous = match strip_paren_expr(value.as_ref()) {
        Expr::Fn(function) => function.ident.is_none(),
        Expr::Arrow(_) => true,
        Expr::Class(class) => class.ident.is_none(),
        _ => false,
    };
    if !is_anonymous {
        return value;
    }

    let property_key = || ComputedPropName {
        span: DUMMY_SP,
        expr: Box::new(Expr::Lit(Lit::Str(make_str(exported.as_ref())))),
    };
    Box::new(Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: Box::new(Expr::Object(ObjectLit {
            span: DUMMY_SP,
            props: vec![PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                key: PropName::Computed(property_key()),
                value,
            })))],
        })),
        prop: MemberProp::Computed(property_key()),
    }))
}

fn emit_exported_value(value: Box<Expr>) -> Box<Expr> {
    Box::new(export_replacement_expr(value))
}

fn default_export_needs_binding(value: &Expr) -> bool {
    let value = strip_paren_expr(value);
    match value {
        // Reprinting these as declarations would hoist their names into module
        // scope instead of keeping the names local to the expressions.
        Expr::Fn(function) => function.ident.is_some(),
        Expr::Class(class) => class.ident.is_some(),
        // The direct IIFE case is safely handled by emit_exported_value. Once
        // another operation is appended, SWC can discard the outer parens.
        _ => starts_with_function_or_class(value) && !is_function_callee_iife(value),
    }
}

fn starts_with_function_or_class(expr: &Expr) -> bool {
    match expr {
        Expr::Fn(_) | Expr::Class(_) => true,
        Expr::Bin(binary) => starts_with_function_or_class(binary.left.as_ref()),
        Expr::Call(call) => matches!(
            &call.callee,
            Callee::Expr(callee) if starts_with_function_or_class(callee.as_ref())
        ),
        Expr::Member(member) => starts_with_function_or_class(member.obj.as_ref()),
        Expr::Cond(condition) => starts_with_function_or_class(condition.test.as_ref()),
        Expr::TaggedTpl(tagged) => starts_with_function_or_class(tagged.tag.as_ref()),
        Expr::Paren(paren) => starts_with_function_or_class(paren.expr.as_ref()),
        Expr::OptChain(chain) => match chain.base.as_ref() {
            OptChainBase::Member(member) => starts_with_function_or_class(member.obj.as_ref()),
            OptChainBase::Call(call) => starts_with_function_or_class(call.callee.as_ref()),
        },
        Expr::TsAs(as_expr) => starts_with_function_or_class(as_expr.expr.as_ref()),
        Expr::TsNonNull(non_null) => starts_with_function_or_class(non_null.expr.as_ref()),
        Expr::TsInstantiation(instantiation) => {
            starts_with_function_or_class(instantiation.expr.as_ref())
        }
        Expr::TsSatisfies(satisfies) => starts_with_function_or_class(satisfies.expr.as_ref()),
        _ => false,
    }
}

fn exported_value_is_assignment(expr: &Expr) -> bool {
    matches!(strip_paren_expr(expr), Expr::Assign(_))
}

// A comma-sequence operand is already in expression context. Once lifted to a
// statement, a function/class/object-headed expression can be reinterpreted as
// a declaration or block, and a leading string can become a directive.
fn parenthesize_lifted_stmt_expr(stmt: &mut Stmt) {
    let Stmt::Expr(expr_stmt) = stmt else {
        return;
    };
    if !lifted_stmt_expr_needs_parens(expr_stmt.expr.as_ref()) {
        return;
    }
    let inner = expr_stmt.expr.clone();
    *expr_stmt.expr = Expr::Paren(ParenExpr {
        span: DUMMY_SP,
        expr: inner,
    });
}

fn lifted_stmt_expr_needs_parens(expr: &Expr) -> bool {
    let mut unwrapped = expr;
    while let Expr::Paren(paren) = unwrapped {
        unwrapped = paren.expr.as_ref();
    }
    matches!(unwrapped, Expr::Lit(Lit::Str(_))) || starts_with_forbidden_stmt_head(expr)
}

fn starts_with_forbidden_stmt_head(expr: &Expr) -> bool {
    match expr {
        Expr::Fn(_) | Expr::Class(_) | Expr::Object(_) => true,
        Expr::Bin(binary) => starts_with_forbidden_stmt_head(binary.left.as_ref()),
        Expr::Call(call) => matches!(
            &call.callee,
            Callee::Expr(callee) if starts_with_forbidden_stmt_head(callee.as_ref())
        ),
        Expr::Member(member) => starts_with_forbidden_stmt_head(member.obj.as_ref()),
        Expr::Cond(condition) => starts_with_forbidden_stmt_head(condition.test.as_ref()),
        Expr::Seq(sequence) => sequence
            .exprs
            .first()
            .is_some_and(|first| starts_with_forbidden_stmt_head(first.as_ref())),
        Expr::TaggedTpl(tagged) => starts_with_forbidden_stmt_head(tagged.tag.as_ref()),
        Expr::Assign(assign) => assign_target_starts_with_forbidden_stmt_head(&assign.left),
        Expr::OptChain(chain) => match chain.base.as_ref() {
            OptChainBase::Member(member) => starts_with_forbidden_stmt_head(member.obj.as_ref()),
            OptChainBase::Call(call) => starts_with_forbidden_stmt_head(call.callee.as_ref()),
        },
        Expr::Update(update) if !update.prefix => {
            starts_with_forbidden_stmt_head(update.arg.as_ref())
        }
        // Add one defensive layer even when the source supplied parentheses:
        // the later SWC fixer may remove the original layer as redundant.
        Expr::Paren(paren) => starts_with_forbidden_stmt_head(paren.expr.as_ref()),
        Expr::TsAs(as_expr) => starts_with_forbidden_stmt_head(as_expr.expr.as_ref()),
        Expr::TsConstAssertion(assertion) => {
            starts_with_forbidden_stmt_head(assertion.expr.as_ref())
        }
        Expr::TsTypeAssertion(assertion) => {
            starts_with_forbidden_stmt_head(assertion.expr.as_ref())
        }
        Expr::TsNonNull(non_null) => starts_with_forbidden_stmt_head(non_null.expr.as_ref()),
        Expr::TsInstantiation(instantiation) => {
            starts_with_forbidden_stmt_head(instantiation.expr.as_ref())
        }
        Expr::TsSatisfies(satisfies) => starts_with_forbidden_stmt_head(satisfies.expr.as_ref()),
        _ => false,
    }
}

fn assign_target_starts_with_forbidden_stmt_head(target: &AssignTarget) -> bool {
    match target {
        AssignTarget::Pat(AssignTargetPat::Object(_)) => true,
        AssignTarget::Pat(_) => false,
        AssignTarget::Simple(target) => match target {
            SimpleAssignTarget::Member(member) => {
                starts_with_forbidden_stmt_head(member.obj.as_ref())
            }
            SimpleAssignTarget::OptChain(chain) => match chain.base.as_ref() {
                OptChainBase::Member(member) => {
                    starts_with_forbidden_stmt_head(member.obj.as_ref())
                }
                OptChainBase::Call(call) => starts_with_forbidden_stmt_head(call.callee.as_ref()),
            },
            SimpleAssignTarget::Paren(paren) => {
                starts_with_forbidden_stmt_head(paren.expr.as_ref())
            }
            SimpleAssignTarget::TsAs(as_expr) => {
                starts_with_forbidden_stmt_head(as_expr.expr.as_ref())
            }
            SimpleAssignTarget::TsSatisfies(satisfies) => {
                starts_with_forbidden_stmt_head(satisfies.expr.as_ref())
            }
            SimpleAssignTarget::TsNonNull(non_null) => {
                starts_with_forbidden_stmt_head(non_null.expr.as_ref())
            }
            SimpleAssignTarget::TsTypeAssertion(assertion) => {
                starts_with_forbidden_stmt_head(assertion.expr.as_ref())
            }
            SimpleAssignTarget::TsInstantiation(instantiation) => {
                starts_with_forbidden_stmt_head(instantiation.expr.as_ref())
            }
            SimpleAssignTarget::Ident(_)
            | SimpleAssignTarget::SuperProp(_)
            | SimpleAssignTarget::Invalid(_) => false,
        },
    }
}

fn export_replacement_expr(value: Box<Expr>) -> Expr {
    if matches!(value.as_ref(), Expr::Assign(_)) || is_function_callee_iife(value.as_ref()) {
        Expr::Paren(ParenExpr {
            span: DUMMY_SP,
            expr: value,
        })
    } else {
        *value
    }
}

fn is_function_callee_iife(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    matches!(
        &call.callee,
        Callee::Expr(callee)
            if matches!(callee.as_ref(), Expr::Fn(_) | Expr::Arrow(_))
    )
}

#[derive(Default)]
struct UsedIdentCollector {
    names: HashSet<Atom>,
}

impl Visit for UsedIdentCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        self.names.insert(ident.sym.clone());
    }
}

fn string_lit_arg(arg: &ExprOrSpread) -> Option<Atom> {
    if arg.spread.is_some() {
        return None;
    }
    let Expr::Lit(Lit::Str(s)) = arg.expr.as_ref() else {
        return None;
    };
    Some(Atom::from(s.value.as_str()?))
}

fn param_sym(function: &Function, idx: usize) -> Option<Atom> {
    let param = function.params.get(idx)?;
    pat_single_ident(&param.pat).cloned()
}

fn assign_ident_item(target: BindingIdent, value: Box<Expr>) -> ModuleItem {
    ModuleItem::Stmt(Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Ident(target)),
            right: value,
        })),
    }))
}

fn assign_local_expr(local: Atom, value: Box<Expr>) -> Expr {
    Expr::Assign(AssignExpr {
        span: DUMMY_SP,
        op: AssignOp::Assign,
        left: AssignTarget::Simple(SimpleAssignTarget::Ident(BindingIdent {
            id: ident(local),
            type_ann: None,
        })),
        right: value,
    })
}

fn var_declarator_item(var: &VarDecl, decl: VarDeclarator) -> ModuleItem {
    ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: var.span,
        ctxt: var.ctxt,
        kind: var.kind,
        declare: var.declare,
        decls: vec![decl],
    }))))
}

fn strip_paren_expr(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => strip_paren_expr(paren.expr.as_ref()),
        other => other,
    }
}

fn pat_single_ident(pat: &Pat) -> Option<&Atom> {
    match pat {
        Pat::Ident(binding) => Some(&binding.id.sym),
        _ => None,
    }
}

fn is_use_strict(expr: &ExprStmt) -> bool {
    matches!(expr.expr.as_ref(), Expr::Lit(Lit::Str(s)) if s.value.as_str() == Some("use strict"))
}

fn member_obj_ident(member: &MemberExpr, sym: &Atom) -> bool {
    matches!(member.obj.as_ref(), Expr::Ident(id) if id.sym == *sym)
}

fn member_prop_name(prop: &MemberProp) -> Option<Atom> {
    match prop {
        MemberProp::Ident(id) => Some(id.sym.clone()),
        MemberProp::Computed(computed) => match computed.expr.as_ref() {
            Expr::Lit(Lit::Str(s)) => Some(Atom::from(s.value.as_str()?)),
            _ => None,
        },
        MemberProp::PrivateName(_) => None,
    }
}

fn prop_name(prop: &PropName) -> Option<String> {
    match prop {
        PropName::Ident(id) => Some(id.sym.to_string()),
        PropName::Str(s) => Some(s.value.to_string_lossy().to_string()),
        PropName::Num(n) if n.value.fract() == 0.0 => Some((n.value as i64).to_string()),
        _ => None,
    }
}

fn ident(sym: Atom) -> Ident {
    Ident::new(sym, DUMMY_SP, Default::default())
}

fn make_str(value: &str) -> Str {
    Str {
        span: DUMMY_SP,
        value: value.into(),
        raw: None,
    }
}

fn filename_for_register(
    name: Option<&str>,
    idx: usize,
    multiple: bool,
    seen: &mut HashSet<String>,
) -> String {
    let base = match name {
        Some(name) => sanitize_filename(name),
        None if multiple => format!("module-{idx}.js"),
        None => "entry.js".to_string(),
    };
    dedup_filename(&base, seen)
}

fn sanitize_filename(module_id: &str) -> String {
    let mut filename = crate::unpacker::sanitize_relative_path(module_id, "unknown");
    if !filename
        .rsplit('/')
        .next()
        .is_some_and(|leaf| leaf.contains('.'))
    {
        filename.push_str(".js");
    }
    filename
}

fn dedup_filename(filename: &str, seen: &mut HashSet<String>) -> String {
    super::emit_esm::dedup_filename(filename, seen, super::emit_esm::FilenameDedupStyle::Flat)
}

fn is_valid_ident_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first == '$' || first.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric())
}

fn emit_module(module: &Module, filename: String, cm: Lrc<SourceMap>) -> anyhow::Result<String> {
    let _fm = cm.new_source_file(
        swc_core::common::FileName::Custom(filename).into(),
        String::new(),
    );
    let mut output = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: Config::default()
                .with_minify(false)
                .with_target(EsVersion::EsNext),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm.clone(), "\n", &mut output, None),
        };
        emitter.emit_module(module)?;
    }
    String::from_utf8(output).map_err(|e| anyhow::anyhow!("{e}"))
}

fn emit_expr_module(expr: &Expr, cm: Lrc<SourceMap>) -> anyhow::Result<String> {
    let module = Module {
        span: DUMMY_SP,
        body: vec![ModuleItem::Stmt(Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(expr.clone()),
        }))],
        shebang: None,
    };
    emit_module(&module, "systemjs-inner-bundle.js".to_string(), cm)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unpack(source: &str) -> UnpackResult {
        let cm: Lrc<SourceMap> = Default::default();
        let module = crate::unpacker::parse_es_module(source, "system.js", cm.clone()).unwrap();
        detect_from_module(&module, cm).expect("should detect System.register")
    }

    #[test]
    fn anonymous_register_uses_entry_filename() {
        let result = unpack(
            r#"
System.register([], function (exports) {
  return {
    execute: function () {
      const value = exports("value", 1);
    }
  };
});
"#,
        );

        assert_eq!(result.modules.len(), 1);
        assert_eq!(result.modules[0].filename, "entry.js");
        assert!(result.modules[0].code.contains("const value = 1"));
        assert!(result.modules[0].code.contains("export { value };"));
    }

    #[test]
    fn named_register_sanitizes_filename() {
        let result = unpack(
            r#"
System.register("../chunks/main", [], function (exports) {
  return { execute: function () { exports("default", 1); } };
});
"#,
        );

        assert_eq!(result.modules[0].filename, "chunks/main.js");
    }

    #[test]
    fn object_method_execute_reconstructs_module() {
        let result = unpack(
            r#"
System.register(["./dep.js"], (_export) => {
  let value;
  let label;
  return {
    setters: [
      (module) => {
        value = module.value, label = module.label;
      }
    ],
    execute () {
      _export("default", `${label}:${value + 1}`);
    }
  };
});
"#,
        );

        assert_eq!(result.modules.len(), 1);
        assert_eq!(result.modules[0].filename, "entry.js");
        assert!(result.modules[0]
            .code
            .contains(r#"import { value, label } from "./dep.js";"#));
        assert!(result.modules[0]
            .code
            .contains("export default `${label}:${value + 1}`;"));
    }

    #[test]
    fn iife_wrapped_register_preserves_helper_prelude() {
        let result = unpack(
            r#"
!function () {
  "use strict";
  function decorate(value) {
    return value + "!";
  }
  System.register([], function (_export) {
    return {
      execute: function () {
        _export("default", decorate("ready"));
      }
    };
  });
}();
"#,
        );

        assert_eq!(result.modules.len(), 1);
        assert_eq!(result.modules[0].filename, "entry.js");
        assert!(result.modules[0].code.contains("function decorate(value)"));
        assert!(result.modules[0]
            .code
            .contains("export default decorate(\"ready\");"));
        assert!(!result.modules[0].code.contains("use strict"));
    }

    #[test]
    fn iife_wrapped_multiple_registers_do_not_copy_prior_registers_into_prelude() {
        let result = unpack(
            r#"
!function () {
  function decorate(value) {
    return value + "!";
  }
  System.register("first", [], function (_export) {
    return {
      execute: function () {
        _export("default", decorate("one"));
      }
    };
  });
  System.register("second", [], function (_export) {
    return {
      execute: function () {
        _export("default", decorate("two"));
      }
    };
  });
}();
"#,
        );

        assert_eq!(result.modules.len(), 2);
        assert_eq!(result.modules[1].filename, "second.js");
        assert!(result.modules[1].code.contains("function decorate(value)"));
        assert!(result.modules[1]
            .code
            .contains("export default decorate(\"two\");"));
        assert!(
            !result.modules[1].code.contains("System.register"),
            "later modules must not copy prior register calls as prelude:\n{}",
            result.modules[1].code
        );
        assert!(
            !result.modules[1].code.contains("decorate(\"one\")"),
            "later modules must not copy prior register execute bodies:\n{}",
            result.modules[1].code
        );
    }

    #[test]
    fn null_setters_reconstruct_imports() {
        let result = unpack(
            r#"
!function () {
  function decorate(value) {
    return value;
  }
  System.register(["./side-effect.js", "./dep.js"], function (_export) {
    var component;
    return {
      setters: [
        null,
        function (module) {
          component = module.component;
        }
      ],
      execute: function () {
        const button = _export("V", decorate(component));
      }
    };
  });
}();
"#,
        );

        assert_eq!(result.modules.len(), 1);
        assert!(result.modules[0]
            .code
            .contains(r#"import "./side-effect.js";"#));
        assert!(result.modules[0]
            .code
            .contains(r#"import { component } from "./dep.js";"#));
        assert!(result.modules[0]
            .code
            .contains("const button = decorate(component);"));
        assert!(result.modules[0].code.contains("export { button as V };"));
    }

    #[test]
    fn sequence_export_calls_reconstruct_each_export() {
        let result = unpack(
            r#"
System.register([], function (_export) {
  function make(value) {
    return { value };
  }
  return {
    execute: function () {
      _export("C", make(1)), _export("_", make(2));
    }
  };
});
"#,
        );

        assert_eq!(result.modules.len(), 1);
        assert!(result.modules[0].code.contains("export const C = make(1);"));
        assert!(result.modules[0].code.contains("export const _ = make(2);"));
    }

    #[test]
    fn mixed_sequence_side_effects_preserve_direct_export_value() {
        let result = unpack(
            r#"
System.register([], function (_export) {
  return {
    execute: function () {
      style.textContent = ".badge{}", document.head.appendChild(style), _export("_", defineComponent({
        __name: "TeamBadge"
      }));
    }
  };
});
"#,
        );

        assert_eq!(result.modules.len(), 1);
        assert!(result.modules[0]
            .code
            .contains("style.textContent = \".badge{}\";"));
        assert!(result.modules[0]
            .code
            .contains("document.head.appendChild(style);"));
        assert!(result.modules[0]
            .code
            .contains("export const _ = defineComponent({"));
    }

    #[test]
    fn assignment_export_in_logical_member_object_is_parenthesized() {
        let result = unpack(
            r#"
System.register([], function (_export) {
  var Kind;
  return {
    execute: function () {
      (Kind || _export("Kind", Kind = {})).Ready = "Ready";
    }
  };
});
"#,
        );

        assert_eq!(result.modules.len(), 1);
        assert!(result.modules[0]
            .code
            .contains("(Kind || (Kind = {})).Ready = \"Ready\";"));
        assert!(result.modules[0].code.contains("export { Kind };"));
    }

    #[test]
    fn ignores_unrelated_top_level_expressions() {
        let result = unpack(
            r#"
console.log("loading");
System.register([], function (_export) {
  return {
    execute: function () {
      _export("default", 1);
    }
  };
});
"#,
        );

        assert_eq!(result.modules.len(), 1);
        assert!(result.modules[0].code.contains("export default 1;"));
    }

    #[test]
    fn named_register_does_not_create_traversal_from_overlapping_dots() {
        let result = unpack(
            r#"
System.register("....//chunks/main", [], function (exports) {
  return { execute: function () { exports("default", 1); } };
});
"#,
        );

        assert_eq!(result.modules[0].filename, "..../chunks/main.js");
    }
}
