use std::collections::HashMap;

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrayPat, Callee, Decl, Expr, Lit, Module, ModuleItem, Pat, Stmt, VarDeclarator,
};
use swc_core::ecma::visit::VisitMut;

use super::babel_helper_utils::{
    collect_helpers, helpers_with_remaining_refs, remove_helper_declarations, BabelHelperKind,
    BindingKey,
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
            .into_iter()
            .filter(|(_, kind)| *kind == BabelHelperKind::SlicedToArray)
            .collect();
        if helpers.is_empty() {
            return;
        }

        // Walk all var declarators and unwrap slicedToArray calls
        for item in &mut module.body {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
                continue;
            };
            for decl in &mut var.decls {
                try_unwrap_sliced_to_array(decl, &helpers);
            }
        }

        // Only remove declaration if no untransformed calls remain
        let remaining = helpers_with_remaining_refs(module, &helpers);
        let safe_to_remove: HashMap<BindingKey, BabelHelperKind> = helpers
            .into_iter()
            .filter(|(key, _)| !remaining.contains(key))
            .collect();
        if !safe_to_remove.is_empty() {
            remove_helper_declarations(&mut module.body, &safe_to_remove);
        }
    }
}

fn try_unwrap_sliced_to_array(
    decl: &mut VarDeclarator,
    helpers: &HashMap<BindingKey, BabelHelperKind>,
) {
    let Some(init) = &decl.init else { return };
    let Expr::Call(call) = init.as_ref() else { return };
    let Callee::Expr(callee) = &call.callee else { return };
    let Expr::Ident(id) = callee.as_ref() else { return };

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
    let length = num.value as usize;

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
