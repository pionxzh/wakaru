use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayLit, ArrowExpr, ArrowFunctionBody, CallExpr, Callee, Constructor, Expr, ExprOrSpread,
    Function, FunctionBody, MemberProp, ParamOrTsParamProp, Pat,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::{binding_id, ident_matches_binding, BindingId};

use super::arg_rest::find_rest_array_copy_proof;
use super::eval_utils::has_dynamic_scope_construct;
use super::helper_matcher::count_binding_refs;
use super::RewriteLevel;

/// Flattens array-literal `.concat(...)` calls into one array literal.
///
/// At Minimal and Standard, every argument must be an array literal. Aggressive
/// also treats unknown arguments as arrays, preserving the old generated-code
/// heuristic for Babel loose / `iterableIsArray` output under the
/// `concat_arguments_are_arrays` assumption.
///
/// Handles:
/// - `[a].concat([b, c])` → `[a, b, c]`
/// - `[a].concat([b], [c])` → `[a, b, c]`
/// - Aggressive: `[a].concat(items)` → `[a, ...items]`
///
/// Only transforms when the receiver is an **array literal** — variable
/// receivers like `arr.concat(other)` are left as-is since `concat` may be
/// overridden or the receiver may not be a plain array.
///
/// `concat` only spreads Array / `@@isConcatSpreadable`; spread iterates any
/// iterable and throws on non-iterables. Flattening nested array literals is
/// concat-faithful for ordinary arrays; patched `Array.prototype.concat` or
/// `Symbol.isConcatSpreadable` can still differ. This remains a generated-code
/// heuristic, with unknown values restricted to Aggressive.
pub struct UnArrayConcatSpread {
    level: RewriteLevel,
}

impl UnArrayConcatSpread {
    pub fn new() -> Self {
        Self::new_with_level(RewriteLevel::Standard)
    }

    pub fn new_with_level(level: RewriteLevel) -> Self {
        Self { level }
    }
}

impl Default for UnArrayConcatSpread {
    fn default() -> Self {
        Self::new()
    }
}

impl VisitMut for UnArrayConcatSpread {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };

        if let Some(new_arr) = try_simplify_array_concat(call, self.level, None) {
            *expr = Expr::Array(new_arr);
        }
    }
}

/// A proof-aware second pass for arrays created by rest parameters or canonical
/// Babel/TypeScript `arguments` copy loops.
///
/// The early concat pass cannot treat an identifier as an Array. This pass runs
/// after parameter-shape cleanup and immediately before class recovery, while
/// the exact rest-copy shape is still available. It only accepts a binding when
/// every use after initialization is a direct concat argument, so reassignment,
/// escape, and `Symbol.isConcatSpreadable` mutation all fail closed.
pub struct UnArrayConcatSpreadRest {
    unresolved_mark: Mark,
    level: RewriteLevel,
}

impl UnArrayConcatSpreadRest {
    pub fn new(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            unresolved_mark,
            level,
        }
    }
}

impl VisitMut for UnArrayConcatSpreadRest {
    fn visit_mut_function(&mut self, function: &mut Function) {
        function.visit_mut_children_with(self);
        if self.level < RewriteLevel::Standard {
            return;
        }

        let rest_bindings = function
            .params
            .iter()
            .filter_map(|param| rest_binding(&param.pat))
            .collect::<Vec<_>>();
        let fixed_param_count = function.params.len();
        let copy = function.body.as_ref().and_then(|body| {
            find_rest_array_copy_proof(body, fixed_param_count, self.unresolved_mark)
        });

        let Some(body) = &mut function.body else {
            return;
        };
        for binding in rest_bindings {
            recover_proven_array_concat(body, &binding, 0, 0);
        }
        if let Some(copy) = copy {
            let binding = binding_id(&copy.binding);
            recover_proven_array_concat(body, &binding, copy.start_stmt, copy.ready_stmt);
        }
    }

    fn visit_mut_constructor(&mut self, constructor: &mut Constructor) {
        constructor.visit_mut_children_with(self);
        if self.level < RewriteLevel::Standard {
            return;
        }

        let rest_bindings = constructor
            .params
            .iter()
            .filter_map(|param| match param {
                ParamOrTsParamProp::Param(param) => rest_binding(&param.pat),
                ParamOrTsParamProp::TsParamProp(_) => None,
            })
            .collect::<Vec<_>>();
        let fixed_param_count = constructor.params.len();
        let copy = constructor.body.as_ref().and_then(|body| {
            find_rest_array_copy_proof(body, fixed_param_count, self.unresolved_mark)
        });

        let Some(body) = &mut constructor.body else {
            return;
        };
        for binding in rest_bindings {
            recover_proven_array_concat(body, &binding, 0, 0);
        }
        if let Some(copy) = copy {
            let binding = binding_id(&copy.binding);
            recover_proven_array_concat(body, &binding, copy.start_stmt, copy.ready_stmt);
        }
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        arrow.visit_mut_children_with(self);
        if self.level < RewriteLevel::Standard {
            return;
        }

        let rest_bindings = arrow
            .params
            .iter()
            .filter_map(rest_binding)
            .collect::<Vec<_>>();
        let ArrowFunctionBody::FunctionBody(body) = arrow.body.as_mut() else {
            return;
        };
        for binding in rest_bindings {
            recover_proven_array_concat(body, &binding, 0, 0);
        }
    }
}

fn rest_binding(pat: &Pat) -> Option<BindingId> {
    let Pat::Rest(rest) = pat else {
        return None;
    };
    let Pat::Ident(binding) = rest.arg.as_ref() else {
        return None;
    };
    Some(binding_id(&binding.id))
}

fn recover_proven_array_concat(
    body: &mut FunctionBody,
    binding: &BindingId,
    proof_start: usize,
    ready_stmt: usize,
) {
    if proof_start > ready_stmt
        || ready_stmt > body.stmts.len()
        || has_dynamic_scope_construct(body)
    {
        return;
    }

    // A `var` copy binding exists before its loop as `undefined`. Any resolved
    // pre-copy reference means the later Array initialization is not sufficient
    // proof for all uses of the binding.
    let prefix_refs = body.stmts[..proof_start]
        .iter()
        .map(|stmt| count_binding_refs(stmt, binding))
        .sum::<usize>();
    if prefix_refs != 0 {
        return;
    }

    let suffix = &body.stmts[ready_stmt..];
    let total_refs = suffix
        .iter()
        .map(|stmt| count_binding_refs(stmt, binding))
        .sum::<usize>();
    if total_refs == 0 {
        return;
    }

    let mut counter = ProvenConcatRefCounter { binding, count: 0 };
    for stmt in suffix {
        stmt.visit_with(&mut counter);
    }
    if counter.count != total_refs {
        return;
    }

    let mut rewriter = ProvenConcatRewriter { binding };
    for stmt in &mut body.stmts[ready_stmt..] {
        stmt.visit_mut_with(&mut rewriter);
    }
}

struct ProvenConcatRefCounter<'a> {
    binding: &'a BindingId,
    count: usize,
}

impl Visit for ProvenConcatRefCounter<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if concat_args_use_only_proven_array(call, self.binding) {
            self.count += call
                .args
                .iter()
                .filter(|arg| {
                    matches!(arg.expr.as_ref(), Expr::Ident(id) if ident_matches_binding(id, self.binding))
                })
                .count();
        }
        call.visit_children_with(self);
    }
}

struct ProvenConcatRewriter<'a> {
    binding: &'a BindingId,
}

impl VisitMut for ProvenConcatRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        if let Some(new_arr) =
            try_simplify_array_concat(call, RewriteLevel::Standard, Some(self.binding))
        {
            *expr = Expr::Array(new_arr);
        }
    }
}

fn concat_args_use_only_proven_array(call: &CallExpr, binding: &BindingId) -> bool {
    array_concat_receiver(call).is_some()
        && !call.args.is_empty()
        && call.args.iter().all(|arg| {
            arg.spread.is_none()
                && match arg.expr.as_ref() {
                    Expr::Array(_) => true,
                    Expr::Ident(id) => ident_matches_binding(id, binding),
                    _ => false,
                }
        })
}

/// Try to convert `[elems].concat(args...)` into a single array literal.
fn try_simplify_array_concat(
    call: &CallExpr,
    level: RewriteLevel,
    proven_array: Option<&BindingId>,
) -> Option<ArrayLit> {
    let receiver_arr = array_concat_receiver(call)?;

    if call.args.is_empty() {
        return None;
    }

    // `[].concat(...arr)` flattens sub-arrays via concat's built-in behavior,
    // but `[...arr]` does not.
    if call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }

    let mut elems: Vec<Option<ExprOrSpread>> = receiver_arr.elems.clone();

    for arg in &call.args {
        match arg.expr.as_ref() {
            Expr::Array(arr) => elems.extend(arr.elems.iter().cloned()),
            Expr::Ident(id)
                if proven_array.is_some_and(|binding| ident_matches_binding(id, binding)) =>
            {
                elems.push(Some(ExprOrSpread {
                    spread: Some(DUMMY_SP),
                    expr: arg.expr.clone(),
                }));
            }
            _ if level >= RewriteLevel::Aggressive => {
                elems.push(Some(ExprOrSpread {
                    spread: Some(DUMMY_SP),
                    expr: arg.expr.clone(),
                }));
            }
            _ => return None,
        }
    }

    Some(ArrayLit {
        span: DUMMY_SP,
        elems,
    })
}

fn array_concat_receiver(call: &CallExpr) -> Option<&ArrayLit> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    let MemberProp::Ident(prop) = &member.prop else {
        return None;
    };
    if prop.sym != "concat" {
        return None;
    }
    let Expr::Array(receiver) = member.obj.as_ref() else {
        return None;
    };
    Some(receiver)
}
