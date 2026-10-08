//! Cocos Creator 2.x registration markers.
//!
//! Every project script in a Cocos Creator 2.x bundle is framed by
//! `cc._RF.push(module, uuid, script)` and `cc._RF.pop()` at the top level of
//! its factory. Passing `module` there does not touch any `exports` property
//! (`cocos_registration_frame` in docs/rewrite-assumptions.md), so rules that
//! otherwise treat a bare `module` as an escape of the export surface may skip
//! that one argument.

use crate::collections::HashSet;

use swc_core::common::{Mark, Span};
use swc_core::ecma::ast::{CallExpr, Callee, Expr, Ident, MemberProp, ModuleItem, Stmt};

use crate::utils::paren::strip_parens;

enum CcRfMarker {
    Push { skippable_span: Option<Span> },
    Pop,
}

/// Return the direct top-level `cc._RF.push` whose matching `pop` encloses
/// the current item. A push elsewhere in the AST is not evidence that its
/// bare `module` argument is the Cocos registration marker for this item.
pub(crate) fn enclosing_cc_rf_push_span<'a>(
    before: impl DoubleEndedIterator<Item = &'a ModuleItem>,
    after: impl Iterator<Item = &'a ModuleItem>,
    unresolved_mark: Mark,
) -> Option<Span> {
    let mut closed_frames = 0usize;
    let mut enclosing_push_span = None;

    for item in before.rev() {
        match direct_cc_rf_marker(item, unresolved_mark) {
            Some(CcRfMarker::Pop) => closed_frames += 1,
            Some(CcRfMarker::Push { .. }) if closed_frames > 0 => closed_frames -= 1,
            Some(CcRfMarker::Push { skippable_span }) => {
                enclosing_push_span = skippable_span;
                break;
            }
            None => {}
        }
    }

    let enclosing_push_span = enclosing_push_span?;
    let mut opened_frames = 0usize;
    for item in after {
        match direct_cc_rf_marker(item, unresolved_mark) {
            Some(CcRfMarker::Push { .. }) => opened_frames += 1,
            Some(CcRfMarker::Pop) if opened_frames == 0 => {
                return Some(enclosing_push_span);
            }
            Some(CcRfMarker::Pop) => opened_frames -= 1,
            None => {}
        }
    }

    None
}

/// The direct top-level `cc._RF.push(module, …)` calls that a later direct
/// top-level `cc._RF.pop()` closes, keyed by address. A push without its pop,
/// a pop without its push, or a marker nested in a function or a sequence is
/// not evidence of a registration frame.
pub(crate) fn framed_cc_rf_push_calls(
    items: &[ModuleItem],
    unresolved_mark: Mark,
) -> HashSet<*const CallExpr> {
    let mut framed = HashSet::default();
    // `None` marks a push whose first argument is not the free `module`: it
    // still opens a frame that a later pop closes.
    let mut open: Vec<Option<*const CallExpr>> = Vec::new();
    for item in items {
        let Some((marker, call)) = direct_cc_rf_marker_call(item, unresolved_mark) else {
            continue;
        };
        match marker {
            CcRfMarker::Push { skippable_span } => {
                open.push(skippable_span.map(|_| call as *const CallExpr));
            }
            CcRfMarker::Pop => {
                if let Some(Some(push)) = open.pop() {
                    framed.insert(push);
                }
            }
        }
    }
    framed
}

fn direct_cc_rf_marker(item: &ModuleItem, unresolved_mark: Mark) -> Option<CcRfMarker> {
    direct_cc_rf_marker_call(item, unresolved_mark).map(|(marker, _)| marker)
}

fn direct_cc_rf_marker_call(
    item: &ModuleItem,
    unresolved_mark: Mark,
) -> Option<(CcRfMarker, &CallExpr)> {
    let ModuleItem::Stmt(Stmt::Expr(expr_stmt)) = item else {
        return None;
    };
    let Expr::Call(call) = strip_parens(&expr_stmt.expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };

    if is_cc_rf_method_callee(callee, "push", unresolved_mark) {
        let marker = CcRfMarker::Push {
            skippable_span: first_arg_is_unresolved_module(call, unresolved_mark)
                .then_some(call.span),
        };
        return Some((marker, call));
    }
    if is_cc_rf_method_callee(callee, "pop", unresolved_mark) {
        return Some((CcRfMarker::Pop, call));
    }
    None
}

pub(crate) fn is_cc_rf_method_callee(callee: &Expr, method: &str, unresolved_mark: Mark) -> bool {
    let Expr::Member(method_member) = strip_parens(callee) else {
        return false;
    };
    let MemberProp::Ident(method_name) = &method_member.prop else {
        return false;
    };
    if method_name.sym != *method {
        return false;
    }
    let Expr::Member(rf) = strip_parens(&method_member.obj) else {
        return false;
    };
    let MemberProp::Ident(rf_name) = &rf.prop else {
        return false;
    };
    if rf_name.sym != "_RF" {
        return false;
    }
    let Expr::Ident(cc) = strip_parens(&rf.obj) else {
        return false;
    };
    is_unresolved_named(cc, "cc", unresolved_mark)
}

pub(crate) fn first_arg_is_unresolved_module(call: &CallExpr, unresolved_mark: Mark) -> bool {
    let Some(first) = call.args.first() else {
        return false;
    };
    if first.spread.is_some() {
        return false;
    }
    matches!(
        strip_parens(&first.expr),
        Expr::Ident(ident) if is_unresolved_named(ident, "module", unresolved_mark)
    )
}

fn is_unresolved_named(ident: &Ident, name: &str, unresolved_mark: Mark) -> bool {
    ident.sym == *name && ident.ctxt.outer() == unresolved_mark
}
