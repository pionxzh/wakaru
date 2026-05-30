use swc_core::atoms::Atom;
use swc_core::ecma::ast::{Callee, Expr, Lit};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::utils::paren::strip_parens;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EvalCallSource {
    NoSource,
    Known(String),
    Unknown,
}

#[derive(Default)]
pub(crate) struct DirectEvalAnalyzer {
    pub(crate) known_direct_eval_sources: Vec<String>,
    pub(crate) known_indirect_eval_sources: Vec<String>,
    pub(crate) unknown_direct_eval: bool,
    pub(crate) unknown_indirect_eval: bool,
}

impl Visit for DirectEvalAnalyzer {
    fn visit_call_expr(&mut self, expr: &swc_core::ecma::ast::CallExpr) {
        if let Some(source) = direct_eval_call_source(expr) {
            match source {
                EvalCallSource::NoSource => {}
                EvalCallSource::Known(source) => self.known_direct_eval_sources.push(source),
                EvalCallSource::Unknown => self.unknown_direct_eval = true,
            }
            visit_eval_args(expr, self);
            return;
        }

        if let Some(source) = indirect_eval_call_source(expr) {
            match source {
                EvalCallSource::NoSource => {}
                EvalCallSource::Known(source) => self.known_indirect_eval_sources.push(source),
                EvalCallSource::Unknown => self.unknown_indirect_eval = true,
            }
            visit_eval_args(expr, self);
            return;
        }

        expr.visit_children_with(self);
    }
}

fn visit_eval_args(expr: &swc_core::ecma::ast::CallExpr, visitor: &mut impl Visit) {
    for arg in &expr.args {
        arg.expr.visit_with(visitor);
    }
}

pub(crate) fn direct_eval_call_source(
    expr: &swc_core::ecma::ast::CallExpr,
) -> Option<EvalCallSource> {
    if !is_direct_eval_call(expr) {
        return None;
    }
    Some(eval_call_source(expr))
}

fn indirect_eval_call_source(expr: &swc_core::ecma::ast::CallExpr) -> Option<EvalCallSource> {
    if !is_indirect_eval_call(expr) {
        return None;
    }
    Some(eval_call_source(expr))
}

fn eval_call_source(expr: &swc_core::ecma::ast::CallExpr) -> EvalCallSource {
    if expr.args.is_empty() {
        return EvalCallSource::NoSource;
    }

    if expr.args.iter().any(|arg| arg.spread.is_some()) {
        return EvalCallSource::Unknown;
    }

    expr.args
        .first()
        .and_then(|arg| eval_static_string(arg.expr.as_ref()))
        .map(EvalCallSource::Known)
        .unwrap_or(EvalCallSource::Unknown)
}

pub(crate) fn is_direct_eval_call(expr: &swc_core::ecma::ast::CallExpr) -> bool {
    let Callee::Expr(callee) = &expr.callee else {
        return false;
    };
    matches!(strip_parens(callee.as_ref()), Expr::Ident(id) if id.sym == "eval")
}

fn is_indirect_eval_call(expr: &swc_core::ecma::ast::CallExpr) -> bool {
    let Callee::Expr(callee) = &expr.callee else {
        return false;
    };
    match strip_parens(callee.as_ref()) {
        Expr::Seq(seq) => {
            matches!(seq.exprs.last().map(|expr| expr.as_ref()), Some(Expr::Ident(id)) if id.sym == "eval")
        }
        Expr::Call(call) => is_object_wrapped_eval_call(call),
        _ => false,
    }
}

fn is_object_wrapped_eval_call(call: &swc_core::ecma::ast::CallExpr) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !matches!(strip_parens(callee.as_ref()), Expr::Ident(id) if id.sym == "Object") {
        return false;
    }
    let Some(arg) = call.args.first() else {
        return false;
    };
    arg.spread.is_none()
        && matches!(strip_parens(arg.expr.as_ref()), Expr::Ident(id) if id.sym == "eval")
}

fn eval_static_string(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(Lit::Str(s)) => s.value.as_str().map(|value| value.to_string()),
        Expr::Call(call) => eval_hidden_require_string(call),
        _ => None,
    }
}

fn eval_hidden_require_string(call: &swc_core::ecma::ast::CallExpr) -> Option<String> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    let swc_core::ecma::ast::MemberProp::Ident(prop) = &member.prop else {
        return None;
    };
    if prop.sym != "replace" || call.args.len() != 2 {
        return None;
    }

    let Expr::Lit(Lit::Str(base)) = member.obj.as_ref() else {
        return None;
    };
    let Expr::Lit(Lit::Regex(regex)) = call.args[0].expr.as_ref() else {
        return None;
    };
    let Expr::Lit(Lit::Str(replacement)) = call.args[1].expr.as_ref() else {
        return None;
    };

    // Known generated-code shape for hiding CommonJS require from bundlers:
    // `eval("quire".replace(/^/, "re"))`. Keep this exact and avoid general
    // constant folding; `String.prototype.replace` can be monkey-patched.
    if base.value.as_str() == Some("quire")
        && regex.exp.as_ref() == "^"
        && replacement.value.as_str() == Some("re")
    {
        return Some("require".to_string());
    }

    None
}

pub(crate) fn js_source_mentions_binding(source: &str, name: &Atom) -> bool {
    let name = name.as_ref();
    if name.is_empty() {
        return false;
    }

    let mut offset = 0;
    while let Some(index) = source[offset..].find(name) {
        let start = offset + index;
        let end = start + name.len();
        let before = source[..start].chars().next_back();
        let after = source[end..].chars().next();
        if !before.is_some_and(is_js_ident_part) && !after.is_some_and(is_js_ident_part) {
            return true;
        }
        offset = end;
    }

    false
}

fn is_js_ident_part(ch: char) -> bool {
    ch == '$' || ch == '_' || ch.is_ascii_alphanumeric()
}
