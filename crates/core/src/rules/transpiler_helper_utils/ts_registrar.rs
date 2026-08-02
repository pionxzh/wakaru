//! Proof engine for TypeScript helper export registrars.
//!
//! Some bundled tslib variants register helpers through a registrar function
//! (`function r(name, value) { exports[name] = value; }`) instead of exporting
//! them directly. The registrar may be declared inline, produced by a factory,
//! or threaded through a callback parameter of a callable call. These matchers
//! prove that a binding is such a registrar so `facts::collect_module_facts`
//! can attribute registered helper exports.

use std::collections::HashSet;

use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, AssignTarget, BlockStmtOrExpr, Callee, Expr, Function,
    MemberExpr, MemberProp, Module, ReturnStmt, SimpleAssignTarget, Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::rules::helper_matcher::{
    binding_key, binding_key_from_ident_pat, expr_matches_binding, static_member_prop_name,
    BindingKey,
};
use crate::utils::paren::strip_parens;

pub(crate) fn collect_ts_helper_export_registrars(module: &Module) -> HashSet<BindingKey> {
    struct RegistrarCollector {
        registrars: HashSet<BindingKey>,
    }

    impl Visit for RegistrarCollector {
        fn visit_fn_decl(&mut self, fn_decl: &swc_core::ecma::ast::FnDecl) {
            if function_is_ts_helper_export_registrar(&fn_decl.function) {
                self.registrars.insert(binding_key(&fn_decl.ident));
            }
            fn_decl.visit_children_with(self);
        }

        fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
            if declarator
                .init
                .as_deref()
                .is_some_and(expr_is_ts_helper_export_registrar)
            {
                if let Some(key) = binding_key_from_ident_pat(&declarator.name) {
                    self.registrars.insert(key);
                }
            }
            declarator.visit_children_with(self);
        }

        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if assign.op == AssignOp::Assign
                && expr_is_ts_helper_export_registrar(strip_parens(assign.right.as_ref()))
            {
                if let Some(key) = assign_target_binding_key(&assign.left) {
                    self.registrars.insert(key);
                }
            }
            assign.visit_children_with(self);
        }

        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            self.registrars
                .extend(callback_registrars_from_callable_call(call));
            call.visit_children_with(self);
        }
    }

    let mut collector = RegistrarCollector {
        registrars: HashSet::new(),
    };
    module.visit_with(&mut collector);
    collector.registrars
}

pub(crate) fn is_ts_helper_export_registration_call(
    call: &swc_core::ecma::ast::CallExpr,
    registrars: &HashSet<BindingKey>,
) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Ident(callee) = strip_parens(callee.as_ref()) else {
        return false;
    };
    registrars.contains(&binding_key(callee))
}

fn expr_is_ts_helper_export_registrar(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Fn(fn_expr) => function_is_ts_helper_export_registrar(&fn_expr.function),
        Expr::Arrow(arrow) => arrow_is_ts_helper_export_registrar(arrow),
        _ => false,
    }
}

fn function_is_ts_helper_export_registrar(function: &Function) -> bool {
    if function.params.len() < 2 {
        return false;
    }
    let Some(name_key) = binding_key_from_ident_pat(&function.params[0].pat) else {
        return false;
    };
    let Some(value_key) = binding_key_from_ident_pat(&function.params[1].pat) else {
        return false;
    };
    let Some(body) = &function.body else {
        return false;
    };
    body_writes_helper_export(&body.stmts, &name_key, &value_key)
}

fn arrow_is_ts_helper_export_registrar(arrow: &ArrowExpr) -> bool {
    if arrow.params.len() < 2 {
        return false;
    }
    let Some(name_key) = binding_key_from_ident_pat(&arrow.params[0]) else {
        return false;
    };
    let Some(value_key) = binding_key_from_ident_pat(&arrow.params[1]) else {
        return false;
    };
    match arrow.body.as_ref() {
        BlockStmtOrExpr::BlockStmt(body) => {
            body_writes_helper_export(&body.stmts, &name_key, &value_key)
        }
        BlockStmtOrExpr::Expr(expr) => expr_writes_helper_export(expr, &name_key, &value_key),
    }
}

fn callback_registrars_from_callable_call(
    call: &swc_core::ecma::ast::CallExpr,
) -> HashSet<BindingKey> {
    let Callee::Expr(callee) = &call.callee else {
        return HashSet::new();
    };
    let Some(params) = callable_param_keys(strip_parens(callee.as_ref())) else {
        return HashSet::new();
    };

    let mut registrars = HashSet::new();
    for (index, arg) in call.args.iter().enumerate() {
        if arg.spread.is_some() {
            continue;
        }
        let Some(callback_param) = callback_registrar_param(arg.expr.as_ref()) else {
            continue;
        };
        let Some(Some(param)) = params.get(index) else {
            continue;
        };
        if callable_passes_registrar_to_param(strip_parens(callee.as_ref()), param) {
            registrars.insert(callback_param);
        }
    }
    registrars
}

fn callable_param_keys(expr: &Expr) -> Option<Vec<Option<BindingKey>>> {
    match expr {
        Expr::Fn(fn_expr) => Some(
            fn_expr
                .function
                .params
                .iter()
                .map(|param| binding_key_from_ident_pat(&param.pat))
                .collect(),
        ),
        Expr::Arrow(arrow) => Some(
            arrow
                .params
                .iter()
                .map(binding_key_from_ident_pat)
                .collect(),
        ),
        _ => None,
    }
}

fn callback_registrar_param(expr: &Expr) -> Option<BindingKey> {
    match strip_parens(expr) {
        Expr::Fn(fn_expr) => fn_expr
            .function
            .params
            .first()
            .and_then(|param| binding_key_from_ident_pat(&param.pat)),
        Expr::Arrow(arrow) => arrow.params.first().and_then(binding_key_from_ident_pat),
        _ => None,
    }
}

fn callable_passes_registrar_to_param(expr: &Expr, param: &BindingKey) -> bool {
    match expr {
        Expr::Fn(fn_expr) => fn_expr
            .function
            .body
            .as_ref()
            .is_some_and(|body| body_passes_registrar_to_param(&body.stmts, param)),
        Expr::Arrow(arrow) => match arrow.body.as_ref() {
            BlockStmtOrExpr::BlockStmt(body) => body_passes_registrar_to_param(&body.stmts, param),
            BlockStmtOrExpr::Expr(expr) => expr_passes_registrar_to_param(expr, param),
        },
        _ => false,
    }
}

fn body_passes_registrar_to_param(stmts: &[Stmt], param: &BindingKey) -> bool {
    let factories = collect_ts_helper_export_registrar_factories(stmts);

    struct RegistrarArgumentFinder<'a> {
        param: &'a BindingKey,
        factories: &'a HashSet<BindingKey>,
        found: bool,
    }

    impl Visit for RegistrarArgumentFinder<'_> {
        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if self.found {
                return;
            }
            if call_passes_registrar_to_param(call, self.param, self.factories) {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }

        fn visit_function(&mut self, _: &Function) {}

        fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    }

    let mut finder = RegistrarArgumentFinder {
        param,
        factories: &factories,
        found: false,
    };
    stmts.visit_with(&mut finder);
    finder.found
}

fn expr_passes_registrar_to_param(expr: &Expr, param: &BindingKey) -> bool {
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    call_passes_registrar_to_param(call, param, &HashSet::new())
}

fn call_passes_registrar_to_param(
    call: &swc_core::ecma::ast::CallExpr,
    param: &BindingKey,
    factories: &HashSet<BindingKey>,
) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !expr_matches_binding(strip_parens(callee.as_ref()), param) {
        return false;
    }
    let Some(first_arg) = call.args.first() else {
        return false;
    };
    first_arg.spread.is_none()
        && expr_is_ts_helper_export_registrar_value(first_arg.expr.as_ref(), factories)
}

fn expr_is_ts_helper_export_registrar_value(expr: &Expr, factories: &HashSet<BindingKey>) -> bool {
    if expr_is_ts_helper_export_registrar(expr) {
        return true;
    }
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    match strip_parens(callee.as_ref()) {
        Expr::Ident(id) => {
            factories.contains(&binding_key(id))
                && factory_call_arguments_are_registrar_safe(call, factories)
        }
        Expr::Fn(fn_expr) => {
            function_returns_ts_helper_export_registrar(&fn_expr.function)
                && factory_call_arguments_are_registrar_safe(call, factories)
        }
        Expr::Arrow(arrow) => {
            arrow_returns_ts_helper_export_registrar(arrow)
                && factory_call_arguments_are_registrar_safe(call, factories)
        }
        _ => false,
    }
}

fn factory_call_arguments_are_registrar_safe(
    call: &swc_core::ecma::ast::CallExpr,
    factories: &HashSet<BindingKey>,
) -> bool {
    call.args.iter().skip(1).all(|arg| {
        arg.spread.is_none()
            && expr_is_ts_helper_export_registrar_value(arg.expr.as_ref(), factories)
    })
}

fn collect_ts_helper_export_registrar_factories(stmts: &[Stmt]) -> HashSet<BindingKey> {
    struct FactoryCollector {
        factories: HashSet<BindingKey>,
    }

    impl Visit for FactoryCollector {
        fn visit_fn_decl(&mut self, fn_decl: &swc_core::ecma::ast::FnDecl) {
            if function_returns_ts_helper_export_registrar(&fn_decl.function) {
                self.factories.insert(binding_key(&fn_decl.ident));
            }
        }

        fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
            if declarator
                .init
                .as_deref()
                .is_some_and(expr_returns_ts_helper_export_registrar)
            {
                if let Some(key) = binding_key_from_ident_pat(&declarator.name) {
                    self.factories.insert(key);
                }
            }
        }

        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if assign.op == AssignOp::Assign
                && expr_returns_ts_helper_export_registrar(strip_parens(assign.right.as_ref()))
            {
                if let Some(key) = assign_target_binding_key(&assign.left) {
                    self.factories.insert(key);
                }
            }
            assign.visit_children_with(self);
        }
    }

    let mut collector = FactoryCollector {
        factories: HashSet::new(),
    };
    stmts.visit_with(&mut collector);
    collector.factories
}

fn expr_returns_ts_helper_export_registrar(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Fn(fn_expr) => function_returns_ts_helper_export_registrar(&fn_expr.function),
        Expr::Arrow(arrow) => arrow_returns_ts_helper_export_registrar(arrow),
        _ => false,
    }
}

fn function_returns_ts_helper_export_registrar(function: &Function) -> bool {
    let Some(target_key) = function
        .params
        .first()
        .and_then(|param| binding_key_from_ident_pat(&param.pat))
    else {
        return false;
    };
    let Some(body) = &function.body else {
        return false;
    };
    body.stmts
        .iter()
        .any(|stmt| return_stmt_returns_target_registrar(stmt, &target_key))
}

fn arrow_returns_ts_helper_export_registrar(arrow: &ArrowExpr) -> bool {
    let Some(target_key) = arrow.params.first().and_then(binding_key_from_ident_pat) else {
        return false;
    };
    match arrow.body.as_ref() {
        BlockStmtOrExpr::BlockStmt(body) => body
            .stmts
            .iter()
            .any(|stmt| return_stmt_returns_target_registrar(stmt, &target_key)),
        BlockStmtOrExpr::Expr(expr) => expr_is_target_ts_helper_export_registrar(expr, &target_key),
    }
}

fn return_stmt_returns_target_registrar(stmt: &Stmt, target_key: &BindingKey) -> bool {
    let Stmt::Return(ReturnStmt {
        arg: Some(expr), ..
    }) = stmt
    else {
        return false;
    };
    expr_is_target_ts_helper_export_registrar(expr, target_key)
}

fn expr_is_target_ts_helper_export_registrar(expr: &Expr, target_key: &BindingKey) -> bool {
    match strip_parens(expr) {
        Expr::Fn(fn_expr) => {
            function_is_target_ts_helper_export_registrar(&fn_expr.function, target_key)
        }
        Expr::Arrow(arrow) => arrow_is_target_ts_helper_export_registrar(arrow, target_key),
        _ => false,
    }
}

fn function_is_target_ts_helper_export_registrar(
    function: &Function,
    target_key: &BindingKey,
) -> bool {
    if function.params.len() < 2 {
        return false;
    }
    let Some(name_key) = binding_key_from_ident_pat(&function.params[0].pat) else {
        return false;
    };
    let Some(value_key) = binding_key_from_ident_pat(&function.params[1].pat) else {
        return false;
    };
    let Some(body) = &function.body else {
        return false;
    };
    body_writes_target_helper_export(&body.stmts, target_key, &name_key, &value_key)
}

fn arrow_is_target_ts_helper_export_registrar(arrow: &ArrowExpr, target_key: &BindingKey) -> bool {
    if arrow.params.len() < 2 {
        return false;
    }
    let Some(name_key) = binding_key_from_ident_pat(&arrow.params[0]) else {
        return false;
    };
    let Some(value_key) = binding_key_from_ident_pat(&arrow.params[1]) else {
        return false;
    };
    match arrow.body.as_ref() {
        BlockStmtOrExpr::BlockStmt(body) => {
            body_writes_target_helper_export(&body.stmts, target_key, &name_key, &value_key)
        }
        BlockStmtOrExpr::Expr(expr) => {
            expr_writes_target_helper_export(expr, target_key, &name_key, &value_key)
        }
    }
}

fn body_writes_target_helper_export(
    stmts: &[Stmt],
    target_key: &BindingKey,
    name_key: &BindingKey,
    value_key: &BindingKey,
) -> bool {
    struct TargetExportWriteFinder<'a> {
        target_key: &'a BindingKey,
        name_key: &'a BindingKey,
        value_key: &'a BindingKey,
        found: bool,
    }

    impl Visit for TargetExportWriteFinder<'_> {
        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if is_target_helper_export_assignment(
                assign,
                self.target_key,
                self.name_key,
                self.value_key,
            ) {
                self.found = true;
                return;
            }
            assign.visit_children_with(self);
        }

        fn visit_function(&mut self, _: &Function) {}

        fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    }

    let mut finder = TargetExportWriteFinder {
        target_key,
        name_key,
        value_key,
        found: false,
    };
    stmts.visit_with(&mut finder);
    finder.found
}

fn expr_writes_target_helper_export(
    expr: &Expr,
    target_key: &BindingKey,
    name_key: &BindingKey,
    value_key: &BindingKey,
) -> bool {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return false;
    };
    is_target_helper_export_assignment(assign, target_key, name_key, value_key)
}

fn is_target_helper_export_assignment(
    assign: &AssignExpr,
    target_key: &BindingKey,
    name_key: &BindingKey,
    value_key: &BindingKey,
) -> bool {
    if assign.op != AssignOp::Assign || !expr_produces_registered_value(&assign.right, value_key) {
        return false;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return false;
    };
    member_of_target_indexed_by(member, target_key, name_key)
}

fn expr_produces_registered_value(expr: &Expr, value_key: &BindingKey) -> bool {
    match strip_parens(expr) {
        expr if expr_matches_binding(expr, value_key) => true,
        Expr::Cond(cond) => {
            expr_produces_registered_value(&cond.cons, value_key)
                && expr_produces_registered_value(&cond.alt, value_key)
        }
        Expr::Call(call) => call.args.iter().any(|arg| {
            arg.spread.is_none() && expr_matches_binding(strip_parens(&arg.expr), value_key)
        }),
        _ => false,
    }
}

fn body_writes_helper_export(
    stmts: &[swc_core::ecma::ast::Stmt],
    name_key: &BindingKey,
    value_key: &BindingKey,
) -> bool {
    struct ExportWriteFinder<'a> {
        name_key: &'a BindingKey,
        value_key: &'a BindingKey,
        found: bool,
    }

    impl Visit for ExportWriteFinder<'_> {
        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if is_helper_export_assignment(assign, self.name_key, self.value_key) {
                self.found = true;
                return;
            }
            assign.visit_children_with(self);
        }

        fn visit_function(&mut self, _: &Function) {}

        fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    }

    let mut finder = ExportWriteFinder {
        name_key,
        value_key,
        found: false,
    };
    stmts.visit_with(&mut finder);
    finder.found
}

fn expr_writes_helper_export(expr: &Expr, name_key: &BindingKey, value_key: &BindingKey) -> bool {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return false;
    };
    is_helper_export_assignment(assign, name_key, value_key)
}

fn is_helper_export_assignment(
    assign: &AssignExpr,
    name_key: &BindingKey,
    value_key: &BindingKey,
) -> bool {
    if assign.op != AssignOp::Assign
        || !expr_matches_binding(strip_parens(&assign.right), value_key)
    {
        return false;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return false;
    };
    exports_member_indexed_by(member, name_key)
}

fn exports_member_indexed_by(member: &MemberExpr, name_key: &BindingKey) -> bool {
    if !matches!(
        &member.prop,
        MemberProp::Computed(computed) if expr_matches_binding(strip_parens(&computed.expr), name_key)
    ) {
        return false;
    }
    is_exports_object(strip_parens(&member.obj))
}

fn member_of_target_indexed_by(
    member: &MemberExpr,
    target_key: &BindingKey,
    name_key: &BindingKey,
) -> bool {
    if !matches!(
        &member.prop,
        MemberProp::Computed(computed) if expr_matches_binding(strip_parens(&computed.expr), name_key)
    ) {
        return false;
    }
    expr_matches_binding(strip_parens(&member.obj), target_key)
}

fn is_exports_object(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Ident(id) => id.sym.as_ref() == "exports",
        Expr::Member(member) => {
            matches!(strip_parens(&member.obj), Expr::Ident(id) if id.sym.as_ref() == "module")
                && static_member_prop_name(&member.prop) == Some("exports")
        }
        _ => false,
    }
}

fn assign_target_binding_key(target: &AssignTarget) -> Option<BindingKey> {
    let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = target else {
        return None;
    };
    Some(binding_key(&binding.id))
}
