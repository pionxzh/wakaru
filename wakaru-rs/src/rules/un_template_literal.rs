use swc_core::ecma::ast::{CallExpr, Callee, Expr, Lit, MemberExpr, MemberProp, Tpl, TplElement};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnTemplateLiteral;

enum Part {
    Text(String),
    Expr(Box<Expr>),
}

impl VisitMut for UnTemplateLiteral {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Some(next) = rewrite_concat_chain(expr) {
            *expr = next;
            return;
        }

        expr.visit_mut_children_with(self);
    }
}

fn rewrite_concat_chain(expr: &Expr) -> Option<Expr> {
    let Expr::Call(call) = expr else {
        return None;
    };

    let mut parts = Vec::new();
    if !collect_concat_parts(call, &mut parts) {
        return None;
    }

    let tpl = parts_to_template(parts, call.span);
    Some(Expr::Tpl(tpl))
}

fn collect_concat_parts(call: &CallExpr, out: &mut Vec<Part>) -> bool {
    let Callee::Expr(callee_expr) = &call.callee else {
        return false;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
        return false;
    };
    if !matches!(prop, MemberProp::Ident(ident) if ident.sym == "concat") {
        return false;
    }

    match &**obj {
        Expr::Call(prev_call) => {
            if !collect_concat_parts(prev_call, out) {
                return false;
            }
        }
        Expr::Lit(Lit::Str(s)) => out.push(Part::Text(s.value.to_string_lossy().into_owned())),
        _ => return false,
    }

    for arg in &call.args {
        if arg.spread.is_some() {
            return false;
        }
        match &*arg.expr {
            Expr::Lit(Lit::Str(s)) => out.push(Part::Text(s.value.to_string_lossy().into_owned())),
            other => out.push(Part::Expr(Box::new(other.clone()))),
        }
    }

    true
}

fn parts_to_template(parts: Vec<Part>, span: swc_core::common::Span) -> Tpl {
    let mut quasis = Vec::new();
    let mut exprs = Vec::new();
    let mut current = String::new();

    for part in parts {
        match part {
            Part::Text(text) => current.push_str(&text),
            Part::Expr(expr) => {
                quasis.push(TplElement {
                    span,
                    tail: false,
                    cooked: Some(current.clone().into()),
                    raw: escape_template_raw(&current).into(),
                });
                current.clear();
                exprs.push(expr);
            }
        }
    }

    quasis.push(TplElement {
        span,
        tail: true,
        cooked: Some(current.clone().into()),
        raw: escape_template_raw(&current).into(),
    });

    Tpl {
        span,
        exprs,
        quasis,
    }
}

fn escape_template_raw(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
}
