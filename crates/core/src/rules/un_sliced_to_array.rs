use std::collections::{HashMap, HashSet};

use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayPat, BinaryOp, BindingIdent, Callee, Decl, Expr, Lit, MemberProp, Module, ModuleItem, Pat,
    Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::transpiler_helper_utils::{
    tslib_helper_name_kind, tslib_member_helper_kind, tslib_require_member_name, BindingKey,
    LocalHelperContext, TranspilerHelperKind,
};

/// Detects and unwraps `_slicedToArray(expr, N)` helper calls.
///
/// Transforms:
///   `var _ref = _slicedToArray(expr, N)` → `var _ref = expr`
///   `var _ref = _slicedToArray(expr, 0)` → `var [] = expr`
///
/// The downstream `SmartInline` + destructuring rules handle converting
/// `var a = _ref[0]; var b = _ref[1]` → `const [a, b] = expr`.
pub struct UnSlicedToArray;

impl UnSlicedToArray {
    pub(crate) fn run_with_helpers(module: &mut Module, local_helpers: &LocalHelperContext) {
        run_un_sliced_to_array(module, local_helpers);
    }
}

impl VisitMut for UnSlicedToArray {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_sliced_to_array(module, &local_helpers);
    }
}

fn run_un_sliced_to_array(module: &mut Module, local_helpers: &LocalHelperContext) {
    let helpers = local_helpers.helpers_of_kind(TranspilerHelperKind::SlicedToArray);
    let tslib_namespaces = local_helpers.tslib_namespaces();
    let has_direct_tslib_calls =
        local_helpers.has_tslib_require_member_call(TranspilerHelperKind::SlicedToArray);
    if helpers.is_empty() && tslib_namespaces.is_empty() && !has_direct_tslib_calls {
        return;
    }
    module.visit_mut_children_with(&mut SlicedToArrayRewriter {
        helpers: &helpers,
        tslib_namespaces,
    });

    if helpers.is_empty() {
        return;
    }

    local_helpers.remove_helpers_with_dependencies(module, helpers);
}

struct SlicedToArrayRewriter<'a> {
    helpers: &'a HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &'a HashSet<BindingKey>,
}

impl VisitMut for SlicedToArrayRewriter<'_> {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        fold_sliced_to_array_module_item_groups(items, self.helpers, self.tslib_namespaces);
        for item in items {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
                continue;
            };
            rewrite_sliced_to_array_decls(&mut var.decls, self.helpers, self.tslib_namespaces);
        }
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        fold_sliced_to_array_stmt_groups(stmts, self.helpers, self.tslib_namespaces);
        for stmt in stmts {
            let Stmt::Decl(Decl::Var(var)) = stmt else {
                continue;
            };
            rewrite_sliced_to_array_decls(&mut var.decls, self.helpers, self.tslib_namespaces);
        }
    }
}

fn fold_sliced_to_array_module_item_groups(
    body: &mut Vec<ModuleItem>,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
) {
    let mut i = 0;
    while i < body.len() {
        try_fold_sliced_to_array_module_item_group(body, i, helpers, tslib_namespaces);
        i += 1;
    }
}

fn try_fold_sliced_to_array_module_item_group(
    body: &mut Vec<ModuleItem>,
    start: usize,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
) -> bool {
    let Some((ref_binding, source, length)) =
        extract_sliced_to_array_module_item(&body[start], helpers, tslib_namespaces)
    else {
        return false;
    };
    if length == 0 {
        return false;
    }

    let mut elems = Vec::with_capacity(length);
    for index in 0..length {
        let Some(item) = body.get(start + 1 + index) else {
            return false;
        };
        let Some(binding) = extract_ref_index_module_item(item, &ref_binding.id, index) else {
            return false;
        };
        if body
            .get(start + 2 + index)
            .is_some_and(|item| module_item_is_default_from_temp(item, &binding.id))
        {
            return false;
        }
        elems.push(Some(Pat::Ident(binding)));
    }
    if ident_used_in_items(&body[start + 1 + length..], &ref_binding.id) {
        return false;
    }

    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = &mut body[start] else {
        return false;
    };
    let Some(decl) = var.decls.first_mut() else {
        return false;
    };
    decl.name = Pat::Array(ArrayPat {
        span: DUMMY_SP,
        elems,
        optional: false,
        type_ann: None,
    });
    decl.init = Some(source);
    body.drain(start + 1..start + 1 + length);
    true
}

fn fold_sliced_to_array_stmt_groups(
    stmts: &mut Vec<Stmt>,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
) {
    let mut i = 0;
    while i < stmts.len() {
        try_fold_sliced_to_array_stmt_group(stmts, i, helpers, tslib_namespaces);
        i += 1;
    }
}

fn try_fold_sliced_to_array_stmt_group(
    stmts: &mut Vec<Stmt>,
    start: usize,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
) -> bool {
    let Some((ref_binding, source, length)) =
        extract_sliced_to_array_stmt(&stmts[start], helpers, tslib_namespaces)
    else {
        return false;
    };
    if length == 0 {
        return false;
    }

    let mut elems = Vec::with_capacity(length);
    for index in 0..length {
        let Some(stmt) = stmts.get(start + 1 + index) else {
            return false;
        };
        let Some(binding) = extract_ref_index_stmt(stmt, &ref_binding.id, index) else {
            return false;
        };
        if stmts
            .get(start + 2 + index)
            .is_some_and(|stmt| stmt_is_default_from_temp(stmt, &binding.id))
        {
            return false;
        }
        elems.push(Some(Pat::Ident(binding)));
    }
    if ident_used_in_stmts(&stmts[start + 1 + length..], &ref_binding.id) {
        return false;
    }

    let Stmt::Decl(Decl::Var(var)) = &mut stmts[start] else {
        return false;
    };
    let Some(decl) = var.decls.first_mut() else {
        return false;
    };
    decl.name = Pat::Array(ArrayPat {
        span: DUMMY_SP,
        elems,
        optional: false,
        type_ann: None,
    });
    decl.init = Some(source);
    stmts.drain(start + 1..start + 1 + length);
    true
}

fn extract_sliced_to_array_module_item(
    item: &ModuleItem,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
) -> Option<(BindingIdent, Box<Expr>, usize)> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    extract_sliced_to_array_decl(&var.decls[0], helpers, tslib_namespaces)
}

fn extract_sliced_to_array_stmt(
    stmt: &Stmt,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
) -> Option<(BindingIdent, Box<Expr>, usize)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    extract_sliced_to_array_decl(&var.decls[0], helpers, tslib_namespaces)
}

fn extract_ref_index_module_item(
    item: &ModuleItem,
    ref_ident: &swc_core::ecma::ast::Ident,
    index: usize,
) -> Option<BindingIdent> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    extract_ref_index_binding(&var.decls[0], ref_ident, index)
}

fn extract_ref_index_stmt(
    stmt: &Stmt,
    ref_ident: &swc_core::ecma::ast::Ident,
    index: usize,
) -> Option<BindingIdent> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    extract_ref_index_binding(&var.decls[0], ref_ident, index)
}

fn module_item_is_default_from_temp(item: &ModuleItem, temp: &swc_core::ecma::ast::Ident) -> bool {
    let ModuleItem::Stmt(stmt) = item else {
        return false;
    };
    stmt_is_default_from_temp(stmt, temp)
}

fn stmt_is_default_from_temp(stmt: &Stmt, temp: &swc_core::ecma::ast::Ident) -> bool {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return false;
    };
    if var.decls.len() != 1 {
        return false;
    }
    let Some(init) = &var.decls[0].init else {
        return false;
    };
    expr_is_default_from_temp(init, temp)
}

fn expr_is_default_from_temp(expr: &Expr, temp: &swc_core::ecma::ast::Ident) -> bool {
    let Expr::Cond(cond) = expr else {
        return false;
    };
    if !expr_is_equality_check_for_temp(cond.test.as_ref(), temp) {
        return false;
    }
    expr_is_temp(cond.alt.as_ref(), temp)
}

fn expr_is_equality_check_for_temp(expr: &Expr, temp: &swc_core::ecma::ast::Ident) -> bool {
    let Expr::Bin(bin) = expr else {
        return false;
    };
    if !matches!(bin.op, BinaryOp::EqEqEq | BinaryOp::EqEq) {
        return false;
    }
    expr_is_temp(bin.left.as_ref(), temp) || expr_is_temp(bin.right.as_ref(), temp)
}

fn expr_is_temp(expr: &Expr, temp: &swc_core::ecma::ast::Ident) -> bool {
    matches!(expr, Expr::Ident(id) if same_sliced_ref_ident(id, temp))
}

fn rewrite_sliced_to_array_decls(
    decls: &mut Vec<VarDeclarator>,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
) {
    let mut i = 0;
    while i < decls.len() {
        try_unwrap_sliced_to_array(&mut decls[i], helpers, tslib_namespaces);
        i += 1;
    }
}

fn extract_sliced_to_array_decl(
    decl: &VarDeclarator,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
) -> Option<(BindingIdent, Box<Expr>, usize)> {
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let init = decl.init.as_ref()?;
    let Expr::Call(call) = init.as_ref() else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };

    if !is_sliced_to_array_callee(callee, helpers, tslib_namespaces) {
        return None;
    }
    if call.args.len() != 2 {
        return None;
    }
    let Expr::Lit(Lit::Num(num)) = call.args[1].expr.as_ref() else {
        return None;
    };
    let length = numeric_length(num.value)?;
    Some((binding.clone(), call.args[0].expr.clone(), length))
}

fn extract_ref_index_binding(
    decl: &VarDeclarator,
    ref_ident: &swc_core::ecma::ast::Ident,
    index: usize,
) -> Option<BindingIdent> {
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let init = decl.init.as_ref()?;
    let Expr::Member(member) = init.as_ref() else {
        return None;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return None;
    };
    if !same_sliced_ref_ident(obj, ref_ident) {
        return None;
    }
    let MemberProp::Computed(computed) = &member.prop else {
        return None;
    };
    let Expr::Lit(Lit::Num(num)) = computed.expr.as_ref() else {
        return None;
    };
    (numeric_length(num.value)? == index).then(|| binding.clone())
}

fn try_unwrap_sliced_to_array(
    decl: &mut VarDeclarator,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
) {
    let Some(init) = &decl.init else { return };
    let Expr::Call(call) = init.as_ref() else {
        return;
    };
    let Callee::Expr(callee) = &call.callee else {
        return;
    };

    if !is_sliced_to_array_callee(callee, helpers, tslib_namespaces) {
        return;
    }

    // Must be exactly 2 args: (expr, numericLength)
    if call.args.len() != 2 {
        return;
    }

    let Expr::Lit(Lit::Num(num)) = call.args[1].expr.as_ref() else {
        return;
    };
    let Some(length) = numeric_length(num.value) else {
        return;
    };

    if length == 0 {
        // var [] = expr
        decl.name = Pat::Array(ArrayPat {
            span: DUMMY_SP,
            elems: vec![],
            optional: false,
            type_ann: None,
        });
        decl.init = Some(call.args[0].expr.clone());
    } else {
        // var _ref = expr (unwrap the helper call, keep the binding)
        decl.init = Some(call.args[0].expr.clone());
    }
}

fn is_sliced_to_array_callee(
    callee: &Expr,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    tslib_namespaces: &HashSet<BindingKey>,
) -> bool {
    if let Expr::Ident(id) = callee {
        return helpers.contains_key(&(id.sym.clone(), id.ctxt));
    }

    matches!(
        tslib_member_helper_kind(callee, tslib_namespaces)
            .or_else(|| tslib_helper_name_kind(tslib_require_member_name(callee)?)),
        Some(TranspilerHelperKind::SlicedToArray)
    )
}

fn numeric_length(value: f64) -> Option<usize> {
    if value < 0.0 || value.fract() != 0.0 || value > 64.0 {
        return None;
    }
    Some(value as usize)
}

fn same_sliced_ref_ident(
    obj: &swc_core::ecma::ast::Ident,
    ref_ident: &swc_core::ecma::ast::Ident,
) -> bool {
    obj.sym == ref_ident.sym
        && (obj.ctxt == ref_ident.ctxt
            || (obj.ctxt == SyntaxContext::empty() && ref_ident.ctxt != SyntaxContext::empty()))
}

fn ident_used_in_items(items: &[ModuleItem], target: &swc_core::ecma::ast::Ident) -> bool {
    let mut finder = IdentUseFinder {
        target,
        found: false,
    };
    for item in items {
        item.visit_with(&mut finder);
        if finder.found {
            return true;
        }
    }
    false
}

fn ident_used_in_stmts(stmts: &[Stmt], target: &swc_core::ecma::ast::Ident) -> bool {
    let mut finder = IdentUseFinder {
        target,
        found: false,
    };
    for stmt in stmts {
        stmt.visit_with(&mut finder);
        if finder.found {
            return true;
        }
    }
    false
}

struct IdentUseFinder<'a> {
    target: &'a swc_core::ecma::ast::Ident,
    found: bool,
}

impl Visit for IdentUseFinder<'_> {
    fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
        if same_sliced_ref_ident(ident, self.target) {
            self.found = true;
        }
    }
}
