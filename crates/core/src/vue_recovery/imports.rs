use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignTarget, BlockStmtOrExpr, Decl, DefaultDecl, ExportSpecifier, Expr,
    Function, MemberProp, Module, ModuleDecl, ModuleItem, ObjectLit, Pat, Prop, PropOrSpread,
    ReturnStmt, SimpleAssignTarget, Stmt, UpdateExpr, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::context::{call_callee_ident, unwrap_paren_expr};
use super::parse_module;
use super::syntax::{module_export_name, prop_name, string_lit};

pub(super) fn composable_ref_props_from_source(source: &str) -> HashMap<String, HashSet<Atom>> {
    let cm: Lrc<SourceMap> = Default::default();
    if let Ok(module) = parse_module(source, cm) {
        let exports = composable_ref_props_from_module(&module);
        if !exports.is_empty() {
            return exports;
        }
    }

    let Some(result) = crate::unpacker::unpack_bundle(source) else {
        return HashMap::new();
    };
    if result.modules.len() != 1 {
        return HashMap::new();
    }
    let Some(module) = result.modules.into_iter().next() else {
        return HashMap::new();
    };
    let cm: Lrc<SourceMap> = Default::default();
    parse_module(&module.code, cm)
        .map(|module| composable_ref_props_from_module(&module))
        .unwrap_or_default()
}

fn composable_ref_props_from_module(module: &Module) -> HashMap<String, HashSet<Atom>> {
    let local_functions = local_function_likes(module);
    let ref_returning_functions = ref_returning_function_bindings(&local_functions);
    let local_ref_props = local_functions
        .iter()
        .filter_map(|(binding, function)| {
            composable_ref_props_from_function(*function, &ref_returning_functions)
                .map(|props| (binding.clone(), props))
        })
        .collect::<HashMap<_, _>>();

    composable_ref_prop_exports(module, &local_ref_props, &ref_returning_functions)
}

#[derive(Clone, Copy)]
enum FunctionLike<'a> {
    Function(&'a Function),
    Arrow(&'a ArrowExpr),
}

fn local_function_likes(module: &Module) -> Vec<(Atom, FunctionLike<'_>)> {
    let mut functions = Vec::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(decl)) => {
                collect_decl_function_likes(decl, &mut functions);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                collect_decl_function_likes(&export.decl, &mut functions);
            }
            _ => {}
        }
    }
    functions
}

fn collect_decl_function_likes<'a>(decl: &'a Decl, functions: &mut Vec<(Atom, FunctionLike<'a>)>) {
    match decl {
        Decl::Fn(function) => {
            functions.push((
                function.ident.sym.clone(),
                FunctionLike::Function(&function.function),
            ));
        }
        Decl::Var(var) => {
            for declarator in &var.decls {
                let Pat::Ident(binding) = &declarator.name else {
                    continue;
                };
                let Some(init) = declarator.init.as_deref() else {
                    continue;
                };
                match unwrap_paren_expr(init) {
                    Expr::Arrow(arrow) => {
                        functions.push((binding.id.sym.clone(), FunctionLike::Arrow(arrow)));
                    }
                    Expr::Fn(function) => {
                        functions.push((
                            binding.id.sym.clone(),
                            FunctionLike::Function(&function.function),
                        ));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn ref_returning_function_bindings(functions: &[(Atom, FunctionLike<'_>)]) -> HashSet<Atom> {
    let mut ref_returning = HashSet::new();
    loop {
        let mut changed = false;
        for (binding, function) in functions {
            if ref_returning.contains(binding) {
                continue;
            }
            if function_returns_ref_like(*function, &ref_returning) {
                changed |= ref_returning.insert(binding.clone());
            }
        }
        if !changed {
            return ref_returning;
        }
    }
}

fn function_returns_ref_like(
    function: FunctionLike<'_>,
    ref_returning_functions: &HashSet<Atom>,
) -> bool {
    let value_writes = function_value_write_bindings(function);
    if value_writes.is_empty() && ref_returning_functions.is_empty() {
        return false;
    }

    function_return_exprs(function)
        .into_iter()
        .any(|expr| expr_returns_ref_like(&expr, &value_writes, ref_returning_functions))
}

fn function_value_write_bindings(function: FunctionLike<'_>) -> HashSet<Atom> {
    let mut collector = ValueWriteCollector {
        bindings: HashSet::new(),
    };
    match function {
        FunctionLike::Function(function) => {
            if let Some(body) = &function.body {
                body.visit_with(&mut collector);
            }
        }
        FunctionLike::Arrow(arrow) => {
            arrow.body.visit_with(&mut collector);
        }
    }
    collector.bindings
}

struct ValueWriteCollector {
    bindings: HashSet<Atom>,
}

impl Visit for ValueWriteCollector {
    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left {
            if member_prop_is_value(&member.prop) {
                if let Expr::Ident(object) = unwrap_paren_expr(member.obj.as_ref()) {
                    self.bindings.insert(object.sym.clone());
                }
            }
        }
        assign.right.visit_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        if let Expr::Member(member) = unwrap_paren_expr(update.arg.as_ref()) {
            if member_prop_is_value(&member.prop) {
                if let Expr::Ident(object) = unwrap_paren_expr(member.obj.as_ref()) {
                    self.bindings.insert(object.sym.clone());
                }
            }
        }
    }
}

fn function_return_exprs(function: FunctionLike<'_>) -> Vec<Expr> {
    match function {
        FunctionLike::Function(function) => function
            .body
            .as_ref()
            .map(|body| block_return_exprs(body.stmts.as_slice()))
            .unwrap_or_default(),
        FunctionLike::Arrow(arrow) => match arrow.body.as_ref() {
            BlockStmtOrExpr::BlockStmt(block) => block_return_exprs(block.stmts.as_slice()),
            BlockStmtOrExpr::Expr(expr) => vec![expr.as_ref().clone()],
        },
    }
}

fn block_return_exprs(stmts: &[Stmt]) -> Vec<Expr> {
    let mut collector = ReturnExprCollector { exprs: Vec::new() };
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
    collector.exprs
}

struct ReturnExprCollector {
    exprs: Vec<Expr>,
}

impl Visit for ReturnExprCollector {
    fn visit_return_stmt(&mut self, stmt: &ReturnStmt) {
        if let Some(expr) = &stmt.arg {
            self.exprs.push(expr.as_ref().clone());
        }
    }

    fn visit_function(&mut self, _function: &Function) {}

    fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
}

fn expr_returns_ref_like(
    expr: &Expr,
    value_writes: &HashSet<Atom>,
    ref_returning_functions: &HashSet<Atom>,
) -> bool {
    match unwrap_paren_expr(expr) {
        Expr::Ident(ident) => value_writes.contains(&ident.sym),
        Expr::Seq(seq) => seq
            .exprs
            .last()
            .is_some_and(|expr| expr_returns_ref_like(expr, value_writes, ref_returning_functions)),
        Expr::Call(call) => {
            call_callee_ident(call)
                .is_some_and(|callee| ref_returning_functions.contains(&callee.sym))
                || call.args.iter().any(|arg| {
                    matches!(
                        unwrap_paren_expr(arg.expr.as_ref()),
                        Expr::Ident(ident) if value_writes.contains(&ident.sym)
                    )
                })
        }
        Expr::Cond(cond) => {
            expr_returns_ref_like(cond.cons.as_ref(), value_writes, ref_returning_functions)
                && expr_returns_ref_like(cond.alt.as_ref(), value_writes, ref_returning_functions)
        }
        _ => false,
    }
}

fn composable_ref_props_from_function(
    function: FunctionLike<'_>,
    ref_returning_functions: &HashSet<Atom>,
) -> Option<HashSet<Atom>> {
    let local_ref_bindings = composable_local_ref_bindings(function, ref_returning_functions);
    if local_ref_bindings.is_empty() && ref_returning_functions.is_empty() {
        return None;
    }

    let mut ref_props = HashSet::new();
    for expr in function_return_exprs(function) {
        let Some(object) = returned_object_expr(&expr) else {
            continue;
        };
        ref_props.extend(object_ref_props(
            &object,
            &local_ref_bindings,
            ref_returning_functions,
        ));
    }
    (!ref_props.is_empty()).then_some(ref_props)
}

fn composable_local_ref_bindings(
    function: FunctionLike<'_>,
    ref_returning_functions: &HashSet<Atom>,
) -> HashSet<Atom> {
    let mut collector = RefLocalCollector {
        refs: HashSet::new(),
        ref_returning_functions,
    };
    match function {
        FunctionLike::Function(function) => {
            if let Some(body) = &function.body {
                body.visit_with(&mut collector);
            }
        }
        FunctionLike::Arrow(arrow) => {
            arrow.body.visit_with(&mut collector);
        }
    }
    collector.refs
}

struct RefLocalCollector<'a> {
    refs: HashSet<Atom>,
    ref_returning_functions: &'a HashSet<Atom>,
}

impl Visit for RefLocalCollector<'_> {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        let Some(init) = declarator.init.as_deref() else {
            return;
        };
        if !expr_is_ref_binding_init(init, &self.refs, self.ref_returning_functions) {
            return;
        }
        if let Pat::Ident(binding) = &declarator.name {
            self.refs.insert(binding.id.sym.clone());
        }
    }

    fn visit_function(&mut self, _function: &Function) {}

    fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}
}

fn expr_is_ref_binding_init(
    expr: &Expr,
    local_ref_bindings: &HashSet<Atom>,
    ref_returning_functions: &HashSet<Atom>,
) -> bool {
    match unwrap_paren_expr(expr) {
        Expr::Ident(ident) => local_ref_bindings.contains(&ident.sym),
        Expr::Seq(seq) => seq.exprs.last().is_some_and(|expr| {
            expr_is_ref_binding_init(expr, local_ref_bindings, ref_returning_functions)
        }),
        Expr::Call(call) => call_callee_ident(call)
            .is_some_and(|callee| ref_returning_functions.contains(&callee.sym)),
        _ => false,
    }
}

fn returned_object_expr(expr: &Expr) -> Option<ObjectLit> {
    match unwrap_paren_expr(expr) {
        Expr::Object(object) => Some(object.clone()),
        Expr::Seq(seq) => seq.exprs.last().and_then(|expr| returned_object_expr(expr)),
        _ => None,
    }
}

fn object_ref_props(
    object: &ObjectLit,
    local_ref_bindings: &HashSet<Atom>,
    ref_returning_functions: &HashSet<Atom>,
) -> HashSet<Atom> {
    let mut ref_props = HashSet::new();
    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            continue;
        };
        match prop.as_ref() {
            Prop::Shorthand(ident) if local_ref_bindings.contains(&ident.sym) => {
                ref_props.insert(ident.sym.clone());
            }
            Prop::KeyValue(key_value) => {
                let value = unwrap_paren_expr(key_value.value.as_ref());
                let is_ref_value = match value {
                    Expr::Ident(value) => local_ref_bindings.contains(&value.sym),
                    Expr::Call(call) => call_callee_ident(call)
                        .is_some_and(|callee| ref_returning_functions.contains(&callee.sym)),
                    _ => false,
                };
                if !is_ref_value {
                    continue;
                }
                if let Some(name) = prop_name(&key_value.key) {
                    ref_props.insert(Atom::from(name));
                }
            }
            _ => {}
        }
    }
    ref_props
}

fn composable_ref_prop_exports(
    module: &Module,
    local_ref_props: &HashMap<Atom, HashSet<Atom>>,
    ref_returning_functions: &HashSet<Atom>,
) -> HashMap<String, HashSet<Atom>> {
    let mut exports = HashMap::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::ExportDecl(export) => {
                collect_decl_ref_prop_exports(&export.decl, local_ref_props, &mut exports);
            }
            ModuleDecl::ExportDefaultExpr(export) => {
                collect_default_expr_ref_prop_export(
                    export.expr.as_ref(),
                    local_ref_props,
                    ref_returning_functions,
                    &mut exports,
                );
            }
            ModuleDecl::ExportDefaultDecl(export) => {
                collect_default_decl_ref_prop_export(
                    &export.decl,
                    ref_returning_functions,
                    &mut exports,
                );
            }
            ModuleDecl::ExportNamed(named) if named.src.is_none() => {
                for specifier in &named.specifiers {
                    let ExportSpecifier::Named(named) = specifier else {
                        continue;
                    };
                    let local = Atom::from(module_export_name(&named.orig));
                    let exported = named
                        .exported
                        .as_ref()
                        .map(module_export_name)
                        .unwrap_or_else(|| local.to_string());
                    if let Some(ref_props) = local_ref_props.get(&local) {
                        exports.insert(exported, ref_props.clone());
                    }
                }
            }
            _ => {}
        }
    }
    exports
}

fn collect_decl_ref_prop_exports(
    decl: &Decl,
    local_ref_props: &HashMap<Atom, HashSet<Atom>>,
    exports: &mut HashMap<String, HashSet<Atom>>,
) {
    match decl {
        Decl::Fn(function) => {
            if let Some(ref_props) = local_ref_props.get(&function.ident.sym) {
                exports.insert(function.ident.sym.to_string(), ref_props.clone());
            }
        }
        Decl::Var(var) => {
            for decl in &var.decls {
                let Pat::Ident(binding) = &decl.name else {
                    continue;
                };
                if let Some(ref_props) = local_ref_props.get(&binding.id.sym) {
                    exports.insert(binding.id.sym.to_string(), ref_props.clone());
                }
            }
        }
        _ => {}
    }
}

fn collect_default_expr_ref_prop_export(
    expr: &Expr,
    local_ref_props: &HashMap<Atom, HashSet<Atom>>,
    ref_returning_functions: &HashSet<Atom>,
    exports: &mut HashMap<String, HashSet<Atom>>,
) {
    match unwrap_paren_expr(expr) {
        Expr::Ident(ident) => {
            if let Some(ref_props) = local_ref_props.get(&ident.sym) {
                exports.insert("default".to_string(), ref_props.clone());
            }
        }
        Expr::Arrow(arrow) => {
            if let Some(ref_props) = composable_ref_props_from_function(
                FunctionLike::Arrow(arrow),
                ref_returning_functions,
            ) {
                exports.insert("default".to_string(), ref_props);
            }
        }
        Expr::Fn(function) => {
            if let Some(ref_props) = composable_ref_props_from_function(
                FunctionLike::Function(&function.function),
                ref_returning_functions,
            ) {
                exports.insert("default".to_string(), ref_props);
            }
        }
        _ => {}
    }
}

fn collect_default_decl_ref_prop_export(
    decl: &DefaultDecl,
    ref_returning_functions: &HashSet<Atom>,
    exports: &mut HashMap<String, HashSet<Atom>>,
) {
    if let DefaultDecl::Fn(function) = decl {
        if let Some(ref_props) = composable_ref_props_from_function(
            FunctionLike::Function(&function.function),
            ref_returning_functions,
        ) {
            exports.insert("default".to_string(), ref_props);
        }
    }
}

fn member_prop_is_value(prop: &MemberProp) -> bool {
    match prop {
        MemberProp::Ident(ident) => ident.sym.as_ref() == "value",
        MemberProp::Computed(computed) => {
            string_lit(computed.expr.as_ref()).as_deref() == Some("value")
        }
        MemberProp::PrivateName(_) => false,
    }
}
