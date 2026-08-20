use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap, Span, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayLit, ArrowFunctionBody, AssignExpr, AssignOp, AssignTarget, BindingIdent, CallExpr,
    Callee, Decl, EsVersion, ExportDecl, ExportDefaultExpr, ExportNamedSpecifier, ExportSpecifier,
    Expr, ExprOrSpread, ExprStmt, FnExpr, Function, FunctionBody, Ident, ImportDecl,
    ImportDefaultSpecifier, ImportNamedSpecifier, ImportSpecifier, ImportStarAsSpecifier, Lit,
    MemberExpr, MemberProp, MetaPropExpr, MetaPropKind, Module, ModuleDecl, ModuleExportName,
    ModuleItem, NamedExport, ObjectLit, ParenExpr, Pat, Prop, PropName, PropOrSpread, ReturnStmt,
    SimpleAssignTarget, Stmt, Str, UnaryOp, VarDecl, VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::js_names::is_reserved_binding_name;
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
        let code = emit_system_module(&register, filename.clone(), cm.clone())?;
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
    let declare = extract_function(call.args.get(declare_arg_idx)?.expr.as_ref())?;

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

fn emit_system_module(
    register: &SystemRegister,
    filename: String,
    cm: Lrc<SourceMap>,
) -> Option<String> {
    let export_sym = param_sym(&register.declare, 0)?;
    let context_sym = param_sym(&register.declare, 1);
    let body = register.declare.body.as_ref()?;
    let descriptor = extract_register_descriptor(body)?;
    let execute_body = descriptor.execute.body.as_ref()?;

    let imports = collect_imports(&register.deps, &descriptor.setters)?;
    let imported_locals = imports
        .iter()
        .flat_map(|import| import.local_names())
        .collect::<HashSet<_>>();

    let mut items = Vec::new();
    for import in &imports {
        items.extend(import.to_module_items());
    }
    items.extend(register.prelude.iter().cloned().map(ModuleItem::Stmt));

    let mut used_names = UsedIdentCollector::default();
    register.declare.visit_with(&mut used_names);
    for stmt in &register.prelude {
        stmt.visit_with(&mut used_names);
    }
    // Module-scope bindings only. `used_names` also contains nested IIFE idents,
    // which must not block `export const Name` when Name is free at module scope.
    let hoisted = outer_hoisted_stmts(body, &imported_locals);
    let mut module_bound_names = imported_locals.clone();
    for stmt in &register.prelude {
        collect_top_level_decl_names(stmt, &mut module_bound_names);
    }
    for stmt in &hoisted {
        collect_top_level_decl_names(stmt, &mut module_bound_names);
    }
    for stmt in &execute_body.stmts {
        collect_top_level_decl_names(stmt, &mut module_bound_names);
    }
    let mut transformer = SystemExecuteTransformer::new(
        export_sym,
        context_sym,
        used_names.names,
        module_bound_names,
    );
    for stmt in hoisted {
        transformer.push_stmt(stmt, &mut items);
    }

    for stmt in &execute_body.stmts {
        transformer.push_stmt(stmt.clone(), &mut items);
    }
    items.extend(transformer.export_items());

    let module = Module {
        span: DUMMY_SP,
        body: items,
        shebang: None,
    };
    emit_module(&module, filename, cm).ok()
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
    let Expr::Object(obj) = return_stmt else {
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

fn collect_top_level_decl_names(stmt: &Stmt, names: &mut HashSet<Atom>) {
    match stmt {
        Stmt::Decl(Decl::Var(var)) => {
            for decl in &var.decls {
                if let Some(name) = pat_single_ident(&decl.name) {
                    names.insert(name.clone());
                }
            }
        }
        Stmt::Decl(Decl::Fn(func)) => {
            names.insert(func.ident.sym.clone());
        }
        Stmt::Decl(Decl::Class(class)) => {
            names.insert(class.ident.sym.clone());
        }
        _ => {}
    }
}

#[derive(Default)]
struct ImportParts {
    source: String,
    default: Option<Atom>,
    namespace: Option<Atom>,
    named: Vec<(Atom, Atom)>,
}

impl ImportParts {
    fn local_names(&self) -> Vec<Atom> {
        let mut names = Vec::new();
        names.extend(self.default.clone());
        names.extend(self.namespace.clone());
        names.extend(self.named.iter().map(|(_, local)| local.clone()));
        names
    }

    fn to_module_items(&self) -> Vec<ModuleItem> {
        let mut items = Vec::new();
        let src = make_str(&self.source);
        if self.default.is_none() && self.namespace.is_none() && self.named.is_empty() {
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

        items
    }
}

fn collect_imports(deps: &[String], setters: &[Option<Function>]) -> Option<Vec<ImportParts>> {
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
        for stmt in &body.stmts {
            for (local, kind) in setter_assignments(stmt, &module_sym)? {
                match kind {
                    SetterImportKind::Default => parts.default = Some(local),
                    SetterImportKind::Named(imported) => parts.named.push((imported, local)),
                    SetterImportKind::Namespace => parts.namespace = Some(local),
                }
            }
        }
        imports.push(parts);
    }
    Some(imports)
}

enum SetterImportKind {
    Default,
    Named(Atom),
    Namespace,
}

fn setter_assignments(stmt: &Stmt, module_sym: &Atom) -> Option<Vec<(Atom, SetterImportKind)>> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    match expr_stmt.expr.as_ref() {
        Expr::Seq(seq) => seq
            .exprs
            .iter()
            .map(|expr| setter_assignment_expr(expr.as_ref(), module_sym))
            .collect(),
        expr => setter_assignment_expr(expr, module_sym).map(|assignment| vec![assignment]),
    }
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

struct SystemExecuteTransformer {
    export_sym: Atom,
    context_sym: Option<Atom>,
    exports: Vec<ExportBinding>,
    declared_exports: HashSet<(Atom, Atom)>,
    used_names: HashSet<Atom>,
    module_bound_names: HashSet<Atom>,
}

impl SystemExecuteTransformer {
    fn new(
        export_sym: Atom,
        context_sym: Option<Atom>,
        used_names: HashSet<Atom>,
        module_bound_names: HashSet<Atom>,
    ) -> Self {
        Self {
            export_sym,
            context_sym,
            exports: Vec::new(),
            declared_exports: HashSet::new(),
            used_names,
            module_bound_names,
        }
    }

    fn push_stmt(&mut self, mut stmt: Stmt, items: &mut Vec<ModuleItem>) {
        if let Some(export_items) = self.take_export_stmt(&stmt) {
            items.extend(export_items);
            return;
        }

        // `var x = (_export("A", v1), _export("B", v2))`: init is a Seq; the old rewrite only accepts Call.
        if let Stmt::Decl(Decl::Var(var)) = &stmt {
            if let Some(export_items) = self.take_var_export_seq_decls(var) {
                items.extend(export_items);
                return;
            }
        }

        if let Stmt::Decl(Decl::Var(var)) = &mut stmt {
            self.rewrite_var_exports(var);
        }

        stmt.visit_mut_with(self);
        items.push(ModuleItem::Stmt(stmt));
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
                    exported: if exported.as_ref() == local.as_ref() {
                        None
                    } else {
                        Some(ModuleExportName::Ident(ident(exported)))
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

    fn take_export_stmt(&mut self, stmt: &Stmt) -> Option<Vec<ModuleItem>> {
        let Stmt::Expr(expr_stmt) = stmt else {
            return None;
        };
        match expr_stmt.expr.as_ref() {
            Expr::Call(call) => {
                let export_call = parse_export_call(call, &self.export_sym)?;
                self.export_call_items(export_call)
            }
            Expr::Assign(assign) => self
                .export_member_assignment_items(assign)
                .or_else(|| self.take_ident_assign_export_seq(assign)),
            Expr::Seq(seq) => {
                let mut items = Vec::new();
                let mut saw_export = false;
                for expr in &seq.exprs {
                    let export_items = match expr.as_ref() {
                        Expr::Call(call) => parse_export_call(call, &self.export_sym)
                            .and_then(|export_call| self.export_call_items(export_call)),
                        Expr::Assign(assign) => self
                            .export_member_assignment_items(assign)
                            .or_else(|| self.take_ident_assign_export_seq(assign)),
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
        let ExportCall::Single { exported, value } = parse_export_call(call, &self.export_sym)?
        else {
            return None;
        };

        let is_default = exported.as_ref() == "default";
        if !is_default && !is_valid_ident_name(exported.as_ref()) {
            return None;
        }
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
        let mut value = value;
        value.visit_mut_with(self);
        let mut items = if exported_local.is_some() {
            self.add_export(local.clone(), exported);
            match value.as_ref() {
                Expr::Assign(_) => vec![ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                    span: DUMMY_SP,
                    expr: value,
                }))],
                Expr::Ident(_) => Vec::new(),
                _ => unreachable!("exported_value_local accepted an unsupported expression"),
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
            && !self.module_bound_names.contains(exported)
            && !self.export_name_is_declared(exported)
    }

    fn export_name_is_declared(&self, exported: &Atom) -> bool {
        self.declared_exports
            .iter()
            .any(|(local, name)| local == exported || name == exported)
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
                    init: Some(value),
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
                init: Some(value),
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
            ExportCall::Single { exported, value } => {
                if let Expr::Assign(assign) = value.as_ref() {
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
                    return Some(vec![ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(
                        ExportDefaultExpr {
                            span: DUMMY_SP,
                            expr: value,
                        },
                    ))]);
                }
                if is_valid_ident_name(exported.as_ref()) {
                    return Some(vec![self.export_const_item(exported, value)]);
                }
                None
            }
            ExportCall::Bulk(exports) => {
                let mut assignment_items = Vec::new();
                for (exported, value) in exports {
                    let local = exported_value_local(value.as_ref())?;
                    self.add_export(local, exported);
                    if matches!(value.as_ref(), Expr::Assign(_)) {
                        assignment_items.push(ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                            span: DUMMY_SP,
                            expr: value,
                        })));
                    }
                }
                Some(assignment_items)
            }
        }
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
                Expr::Call(call) => parse_export_call(call, &self.export_sym),
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
            items.push(ModuleItem::Stmt(stmt));
        }
        None
    }

    fn export_sequence_prefix_items(&mut self, export_call: ExportCall) -> Option<Vec<ModuleItem>> {
        match export_call {
            single @ ExportCall::Single { .. } => self
                .export_call_result_items(single)
                .map(|(items, _)| items),
            bulk @ ExportCall::Bulk(_) => self.export_call_items(bulk),
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
            let items = if matches!(value.as_ref(), Expr::Assign(_)) {
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
        if !is_default && !is_valid_ident_name(exported.as_ref()) {
            return None;
        }
        let can_use_exported_name = !is_default
            && !is_reserved_binding_name(exported.as_ref())
            && self.used_names.insert(exported.clone());
        if can_use_exported_name {
            self.declared_exports
                .insert((exported.clone(), exported.clone()));
            let result = Box::new(Expr::Ident(ident(exported.clone())));
            return Some((vec![named_export_decl_item(exported, value)], result));
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
                    }) = parse_export_call(call, &self.export_sym)
                    {
                        value.visit_mut_with(self);
                        self.add_export(local, exported);
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
                Expr::Call(call) if parse_export_call(call, &self.export_sym).is_some()
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
                Expr::Call(call) => parse_export_call(call, &self.export_sym),
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
            let Some(ExportCall::Single { exported, value }) =
                parse_export_call(call, &self.export_sym)
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
            if let Some(ExportCall::Single { exported, value }) =
                parse_export_call(call, &self.export_sym)
            {
                if let Some(local) = exported_value_local(value.as_ref()) {
                    self.add_export(local, exported);
                }
                *expr = export_replacement_expr(value);
                expr.visit_mut_children_with(self);
                return;
            }
        }

        expr.visit_mut_children_with(self);
    }
}

impl SystemExecuteTransformer {
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

fn named_export_item(local: Atom, exported: Atom) -> ModuleItem {
    ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
        span: DUMMY_SP,
        specifiers: vec![ExportSpecifier::Named(ExportNamedSpecifier {
            span: DUMMY_SP,
            orig: ModuleExportName::Ident(ident(local)),
            exported: Some(ModuleExportName::Ident(ident(exported))),
            is_type_only: false,
        })],
        src: None,
        type_only: false,
        with: None,
    }))
}

fn named_export_decl_item(exported: Atom, value: Box<Expr>) -> ModuleItem {
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
                    id: ident(exported),
                    type_ann: None,
                }),
                init: Some(value),
                definite: false,
            }],
        })),
    }))
}

enum ExportCall {
    Single { exported: Atom, value: Box<Expr> },
    Bulk(Vec<(Atom, Box<Expr>)>),
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
            _ => return None,
        }
    }
    Some(pairs)
}

fn exported_value_local(expr: &Expr) -> Option<Atom> {
    match expr {
        Expr::Ident(id) => Some(id.sym.clone()),
        Expr::Assign(assign) => assign.left.as_simple()?.as_ident().map(|id| id.sym.clone()),
        _ => None,
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
