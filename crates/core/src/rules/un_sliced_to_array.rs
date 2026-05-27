use std::collections::HashMap;

use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayPat, BindingIdent, Callee, Decl, Expr, Lit, MemberProp, Module, ModuleItem, Pat, Stmt,
    VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitWith};

use super::babel_helper_utils::{
    collect_helper_dependencies, collect_helpers, helpers_with_remaining_refs,
    remove_helper_declarations, BabelHelperKind, BindingKey,
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

impl VisitMut for UnSlicedToArray {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let all_helpers = collect_helpers(module);
        let helpers: HashMap<BindingKey, BabelHelperKind> = all_helpers
            .iter()
            .filter(|(_, kind)| **kind == BabelHelperKind::SlicedToArray)
            .map(|(key, kind)| (key.clone(), *kind))
            .collect();
        if helpers.is_empty() {
            return;
        }
        fold_sliced_to_array_stmt_groups(&mut module.body, &helpers);

        // Walk all var declarators and unwrap slicedToArray calls
        for item in &mut module.body {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
                continue;
            };
            rewrite_sliced_to_array_decls(&mut var.decls, &helpers);
        }

        // Only remove root helpers whose calls were fully transformed. Dependencies
        // referenced by retained helpers must stay with those helpers.
        let remaining_roots = helpers_with_remaining_refs(module, &helpers);
        let removable_roots = helpers
            .iter()
            .filter(|(key, _)| !remaining_roots.contains(*key))
            .map(|(key, kind)| (key.clone(), *kind))
            .collect::<HashMap<_, _>>();
        let helper_dependencies = collect_helper_dependencies(module, &removable_roots);
        let removable_helpers = removable_roots
            .into_iter()
            .chain(helper_dependencies)
            .collect::<HashMap<_, _>>();
        let remaining = helpers_with_remaining_refs(module, &removable_helpers);
        let safe_to_remove: HashMap<BindingKey, BabelHelperKind> = removable_helpers
            .into_iter()
            .filter(|(key, _)| !remaining.contains(key))
            .collect();
        if !safe_to_remove.is_empty() {
            remove_helper_declarations(&mut module.body, &safe_to_remove);
        }
    }
}

fn fold_sliced_to_array_stmt_groups(
    body: &mut Vec<ModuleItem>,
    helpers: &HashMap<BindingKey, BabelHelperKind>,
) {
    let mut i = 0;
    while i < body.len() {
        try_fold_sliced_to_array_stmt_group(body, i, helpers);
        i += 1;
    }
}

fn try_fold_sliced_to_array_stmt_group(
    body: &mut Vec<ModuleItem>,
    start: usize,
    helpers: &HashMap<BindingKey, BabelHelperKind>,
) -> bool {
    let Some((ref_binding, source, length)) = extract_sliced_to_array_item(&body[start], helpers)
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
        let Some(binding) = extract_ref_index_item(item, &ref_binding.id, index) else {
            return false;
        };
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

fn extract_sliced_to_array_item(
    item: &ModuleItem,
    helpers: &HashMap<BindingKey, BabelHelperKind>,
) -> Option<(BindingIdent, Box<Expr>, usize)> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    extract_sliced_to_array_decl(&var.decls[0], helpers)
}

fn extract_ref_index_item(
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

fn rewrite_sliced_to_array_decls(
    decls: &mut Vec<VarDeclarator>,
    helpers: &HashMap<BindingKey, BabelHelperKind>,
) {
    let mut i = 0;
    while i < decls.len() {
        try_unwrap_sliced_to_array(&mut decls[i], helpers);
        i += 1;
    }
}

fn extract_sliced_to_array_decl(
    decl: &VarDeclarator,
    helpers: &HashMap<BindingKey, BabelHelperKind>,
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
    let Expr::Ident(id) = callee.as_ref() else {
        return None;
    };

    if !helpers.contains_key(&(id.sym.clone(), id.ctxt)) {
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
    helpers: &HashMap<BindingKey, BabelHelperKind>,
) {
    let Some(init) = &decl.init else { return };
    let Expr::Call(call) = init.as_ref() else {
        return;
    };
    let Callee::Expr(callee) = &call.callee else {
        return;
    };
    let Expr::Ident(id) = callee.as_ref() else {
        return;
    };

    if !helpers.contains_key(&(id.sym.clone(), id.ctxt)) {
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
