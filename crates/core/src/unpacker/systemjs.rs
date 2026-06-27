use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap, Span, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayLit, BindingIdent, BlockStmt, BlockStmtOrExpr, CallExpr, Callee, Decl, EsVersion,
    ExportDecl, ExportDefaultExpr, ExportNamedSpecifier, ExportSpecifier, Expr, ExprOrSpread,
    ExprStmt, FnExpr, Function, Ident, ImportDecl, ImportDefaultSpecifier, ImportNamedSpecifier,
    ImportSpecifier, ImportStarAsSpecifier, Lit, MemberExpr, MemberProp, MetaPropExpr,
    MetaPropKind, Module, ModuleDecl, ModuleExportName, ModuleItem, NamedExport, ObjectLit, Pat,
    Prop, PropName, PropOrSpread, ReturnStmt, Stmt, Str, VarDecl, VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use crate::unpacker::{span_byte_range, BundleFormat, UnpackResult, UnpackedModule};

pub(super) fn detect_from_module(module: &Module, cm: Lrc<SourceMap>) -> Option<UnpackResult> {
    let mut registers = Vec::new();

    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
            continue;
        };
        let Expr::Call(call) = expr.as_ref() else {
            continue;
        };
        if !is_system_register_call(call) {
            continue;
        }
        let register = parse_register_call(call)?;
        registers.push(register);
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

fn dynamic_export_expr<'a>(body: &'a BlockStmt, export_sym: &Atom) -> Option<&'a Expr> {
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
    })
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
            let BlockStmtOrExpr::BlockStmt(body) = arrow.body.as_ref() else {
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

    let mut transformer = SystemExecuteTransformer::new(export_sym, context_sym);
    for stmt in outer_hoisted_stmts(body, &imported_locals) {
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

fn extract_register_descriptor(body: &BlockStmt) -> Option<RegisterDescriptor> {
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
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            continue;
        };
        match prop_name(&key_value.key).as_deref() {
            Some("setters") => setters = Some(extract_setters(key_value.value.as_ref())?),
            Some("execute") => execute = Some(extract_function(key_value.value.as_ref())?),
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
        if matches!(expr.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "undefined") {
            setters.push(None);
            continue;
        }
        setters.push(Some(extract_function(expr.as_ref())?));
    }
    Some(setters)
}

fn outer_hoisted_stmts(body: &BlockStmt, imported_locals: &HashSet<Atom>) -> Vec<Stmt> {
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
            let (local, kind) = setter_assignment(stmt, &module_sym)?;
            match kind {
                SetterImportKind::Default => parts.default = Some(local),
                SetterImportKind::Named(imported) => parts.named.push((imported, local)),
                SetterImportKind::Namespace => parts.namespace = Some(local),
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

fn setter_assignment(stmt: &Stmt, module_sym: &Atom) -> Option<(Atom, SetterImportKind)> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr_stmt.expr.as_ref() else {
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
}

impl SystemExecuteTransformer {
    fn new(export_sym: Atom, context_sym: Option<Atom>) -> Self {
        Self {
            export_sym,
            context_sym,
            exports: Vec::new(),
        }
    }

    fn push_stmt(&mut self, mut stmt: Stmt, items: &mut Vec<ModuleItem>) {
        if let Some(export_items) = self.take_export_stmt(&stmt) {
            items.extend(export_items);
            return;
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
        let specifiers = self
            .exports
            .into_iter()
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
        let Expr::Call(call) = expr_stmt.expr.as_ref() else {
            return None;
        };
        let export_call = parse_export_call(call, &self.export_sym)?;
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
                    return Some(vec![ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(
                        ExportDecl {
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
                        },
                    ))]);
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
                *expr = *value;
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
    if seen.insert(filename.to_ascii_lowercase()) {
        return filename.to_string();
    }
    let (stem, ext) = match filename.rfind('.') {
        Some(i) => (&filename[..i], &filename[i + 1..]),
        None => (filename, "js"),
    };
    let mut n = 2u32;
    loop {
        let candidate = format!("{stem}_{n}.{ext}");
        if seen.insert(candidate.to_ascii_lowercase()) {
            return candidate;
        }
        n += 1;
    }
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
