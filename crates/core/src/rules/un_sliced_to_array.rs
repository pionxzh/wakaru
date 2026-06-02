use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayPat, BinaryOp, BindingIdent, Callee, Decl, Expr, Lit, MemberProp, Module, ModuleItem, Pat,
    Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::transpiler_helper_utils::{
    extract_inline_sliced_to_array_call, LocalHelperContext, TranspilerHelperKind,
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

struct SlicedExtraction {
    ref_binding: BindingIdent,
    source: Box<Expr>,
    source_ref: Option<swc_core::ecma::ast::Ident>,
    length: Option<usize>,
}

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
    if helpers.is_empty()
        && tslib_namespaces.is_empty()
        && !has_direct_tslib_calls
        && !has_inline_sliced_to_array_candidate(module)
    {
        return;
    }
    module.visit_mut_children_with(&mut SlicedToArrayRewriter { local_helpers });

    if helpers.is_empty() {
        return;
    }

    local_helpers.remove_helpers_with_dependencies(module, helpers);
}

fn has_inline_sliced_to_array_candidate(module: &Module) -> bool {
    struct Scan {
        found: bool,
    }

    impl Visit for Scan {
        fn visit_expr(&mut self, expr: &Expr) {
            if self.found {
                return;
            }
            if extract_inline_sliced_to_array_call(expr).is_some() {
                self.found = true;
                return;
            }
            expr.visit_children_with(self);
        }
    }

    let mut scan = Scan { found: false };
    module.visit_with(&mut scan);
    scan.found
}

struct SlicedToArrayRewriter<'a> {
    local_helpers: &'a LocalHelperContext,
}

impl VisitMut for SlicedToArrayRewriter<'_> {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        fold_sliced_to_array_module_item_groups(items, self.local_helpers);
        for item in items {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
                continue;
            };
            rewrite_sliced_to_array_decls(&mut var.decls, self.local_helpers);
        }
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        fold_sliced_to_array_stmt_groups(stmts, self.local_helpers);
        for stmt in stmts {
            let Stmt::Decl(Decl::Var(var)) = stmt else {
                continue;
            };
            rewrite_sliced_to_array_decls(&mut var.decls, self.local_helpers);
        }
    }
}

fn fold_sliced_to_array_module_item_groups(
    body: &mut Vec<ModuleItem>,
    local_helpers: &LocalHelperContext,
) {
    let mut i = 0;
    while i < body.len() {
        try_fold_sliced_to_array_module_item_group(body, i, local_helpers);
        i += 1;
    }
}

fn try_fold_sliced_to_array_module_item_group(
    body: &mut Vec<ModuleItem>,
    start: usize,
    local_helpers: &LocalHelperContext,
) -> bool {
    let Some(extraction) = extract_sliced_to_array_module_item(&body[start], local_helpers) else {
        return false;
    };
    if extraction.length == Some(0) {
        return false;
    }
    if module_item_sliced_ref_is_unreferenced(body, start, &extraction.ref_binding.id) {
        let Some(length) = extraction.length else {
            return false;
        };
        if sliced_source_ref_is_used_in_items(&body[start + 1..], &extraction) {
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
            elems: vec![None; length],
            optional: false,
            type_ann: None,
        });
        decl.init = Some(extraction.source);
        return true;
    }

    let known_length = extraction.length;
    let mut elems = Vec::with_capacity(known_length.unwrap_or(2));
    let max_len = known_length.unwrap_or(64);
    for index in 0..max_len {
        let Some(item) = body.get(start + 1 + index) else {
            break;
        };
        let Some(binding) = extract_ref_index_module_item(item, &extraction.ref_binding.id, index)
        else {
            if known_length.is_some() {
                return false;
            }
            break;
        };
        if body
            .get(start + 2 + index)
            .is_some_and(|item| module_item_is_default_from_temp(item, &binding.id))
        {
            return false;
        }
        elems.push(Some(Pat::Ident(binding)));
    }
    let length = elems.len();
    if length == 0 {
        return false;
    }
    if known_length.is_some_and(|known| known != length) {
        return false;
    }
    if ident_used_in_items(&body[start + 1 + length..], &extraction.ref_binding.id) {
        return false;
    }
    if sliced_source_ref_is_used_in_items(&body[start + 1 + length..], &extraction) {
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
    decl.init = Some(extraction.source);
    body.drain(start + 1..start + 1 + length);
    true
}

fn fold_sliced_to_array_stmt_groups(stmts: &mut Vec<Stmt>, local_helpers: &LocalHelperContext) {
    let mut i = 0;
    while i < stmts.len() {
        try_fold_sliced_to_array_stmt_group(stmts, i, local_helpers);
        i += 1;
    }
}

fn try_fold_sliced_to_array_stmt_group(
    stmts: &mut Vec<Stmt>,
    start: usize,
    local_helpers: &LocalHelperContext,
) -> bool {
    let Some(extraction) = extract_sliced_to_array_stmt(&stmts[start], local_helpers) else {
        return false;
    };
    if extraction.length == Some(0) {
        return false;
    }
    if stmt_sliced_ref_is_unreferenced(stmts, start, &extraction.ref_binding.id) {
        let Some(length) = extraction.length else {
            return false;
        };
        if sliced_source_ref_is_used_in_stmts(&stmts[start + 1..], &extraction) {
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
            elems: vec![None; length],
            optional: false,
            type_ann: None,
        });
        decl.init = Some(extraction.source);
        return true;
    }

    let known_length = extraction.length;
    let mut elems = Vec::with_capacity(known_length.unwrap_or(2));
    let max_len = known_length.unwrap_or(64);
    for index in 0..max_len {
        let Some(stmt) = stmts.get(start + 1 + index) else {
            break;
        };
        let Some(binding) = extract_ref_index_stmt(stmt, &extraction.ref_binding.id, index) else {
            if known_length.is_some() {
                return false;
            }
            break;
        };
        if stmts
            .get(start + 2 + index)
            .is_some_and(|stmt| stmt_is_default_from_temp(stmt, &binding.id))
        {
            return false;
        }
        elems.push(Some(Pat::Ident(binding)));
    }
    let length = elems.len();
    if length == 0 {
        return false;
    }
    if known_length.is_some_and(|known| known != length) {
        return false;
    }
    if ident_used_in_stmts(&stmts[start + 1 + length..], &extraction.ref_binding.id) {
        return false;
    }
    if sliced_source_ref_is_used_in_stmts(&stmts[start + 1 + length..], &extraction) {
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
    decl.init = Some(extraction.source);
    stmts.drain(start + 1..start + 1 + length);
    true
}

fn extract_sliced_to_array_module_item(
    item: &ModuleItem,
    local_helpers: &LocalHelperContext,
) -> Option<SlicedExtraction> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    extract_sliced_to_array_decl(&var.decls[0], local_helpers)
}

fn extract_sliced_to_array_stmt(
    stmt: &Stmt,
    local_helpers: &LocalHelperContext,
) -> Option<SlicedExtraction> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    extract_sliced_to_array_decl(&var.decls[0], local_helpers)
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

fn module_item_sliced_ref_is_unreferenced(
    body: &[ModuleItem],
    start: usize,
    ref_ident: &swc_core::ecma::ast::Ident,
) -> bool {
    body.get(start + 1)
        .is_none_or(|_| !ident_used_in_items(&body[start + 1..], ref_ident))
}

fn stmt_sliced_ref_is_unreferenced(
    stmts: &[Stmt],
    start: usize,
    ref_ident: &swc_core::ecma::ast::Ident,
) -> bool {
    stmts
        .get(start + 1)
        .is_none_or(|_| !ident_used_in_stmts(&stmts[start + 1..], ref_ident))
}

fn rewrite_sliced_to_array_decls(
    decls: &mut Vec<VarDeclarator>,
    local_helpers: &LocalHelperContext,
) {
    let mut i = 0;
    while i < decls.len() {
        try_unwrap_sliced_to_array(&mut decls[i], local_helpers);
        i += 1;
    }
}

fn extract_sliced_to_array_decl(
    decl: &VarDeclarator,
    local_helpers: &LocalHelperContext,
) -> Option<SlicedExtraction> {
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let init = decl.init.as_ref()?;
    let Expr::Call(call) = init.as_ref() else {
        return extract_inline_sliced_to_array_call(init.as_ref()).map(|inline_call| {
            SlicedExtraction {
                ref_binding: binding.clone(),
                source: inline_call.source,
                source_ref: inline_call.source_ref,
                length: inline_call.length,
            }
        });
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };

    if !local_helpers.is_helper_callee(callee, TranspilerHelperKind::SlicedToArray) {
        return None;
    }
    if call.args.len() != 2 {
        return None;
    }
    let Expr::Lit(Lit::Num(num)) = call.args[1].expr.as_ref() else {
        return None;
    };
    let length = numeric_length(num.value)?;
    Some(SlicedExtraction {
        ref_binding: binding.clone(),
        source: call.args[0].expr.clone(),
        source_ref: None,
        length: Some(length),
    })
}

fn sliced_source_ref_is_used_in_items(items: &[ModuleItem], extraction: &SlicedExtraction) -> bool {
    extraction
        .source_ref
        .as_ref()
        .is_some_and(|source_ref| ident_used_in_items(items, source_ref))
}

fn sliced_source_ref_is_used_in_stmts(stmts: &[Stmt], extraction: &SlicedExtraction) -> bool {
    extraction
        .source_ref
        .as_ref()
        .is_some_and(|source_ref| ident_used_in_stmts(stmts, source_ref))
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

fn try_unwrap_sliced_to_array(decl: &mut VarDeclarator, local_helpers: &LocalHelperContext) {
    let Some(init) = &decl.init else { return };
    let Expr::Call(call) = init.as_ref() else {
        return;
    };
    let Callee::Expr(callee) = &call.callee else {
        return;
    };

    if !local_helpers.is_helper_callee(callee, TranspilerHelperKind::SlicedToArray) {
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
    fn visit_binding_ident(&mut self, _: &BindingIdent) {}

    fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
        if same_sliced_ref_ident(ident, self.target) {
            self.found = true;
        }
    }
}
