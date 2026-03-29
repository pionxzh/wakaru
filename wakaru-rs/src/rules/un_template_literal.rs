use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    BinExpr, BinaryOp, CallExpr, Callee, Expr, Lit, MemberExpr, MemberProp, Tpl, TplElement,
};
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
        if let Some(next) = rewrite_plus_chain(expr) {
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

/// Convert `"str" + a + b` (binary `+` chains with at least one string literal)
/// into a template literal.
///
/// Safety: only transforms when at least one operand in the chain is a string
/// literal. All non-string elements that appear **before** the first string
/// literal are grouped into a single sub-expression to preserve arithmetic
/// semantics (e.g. `a + b + "c"` → `` `${a + b}c` `` not `` `${a}${b}c` ``).
fn rewrite_plus_chain(expr: &Expr) -> Option<Expr> {
    // Collect the flat left-associative operand list
    let mut operands: Vec<&Expr> = Vec::new();
    collect_add_chain(expr, &mut operands);

    // Must have at least 2 operands and at least one string literal
    if operands.len() < 2 {
        return None;
    }
    let first_str_idx = operands.iter().position(|e| is_str_lit(e))?;

    // Determine the span for the resulting template
    let span = match expr {
        Expr::Bin(b) => b.span,
        _ => DUMMY_SP,
    };

    // Build the parts list:
    // – everything before the first string literal is grouped into one Expr part
    //   (to avoid splitting arithmetic sub-expressions like `a + b`)
    // – from the first string literal onward, each element becomes Text or Expr
    let mut parts: Vec<Part> = Vec::new();

    if first_str_idx > 0 {
        let grouped = rebuild_add_chain(&operands[..first_str_idx]);
        parts.push(Part::Expr(grouped));
    }

    for op in &operands[first_str_idx..] {
        if let Expr::Lit(Lit::Str(s)) = op {
            parts.push(Part::Text(s.value.to_string_lossy().into_owned()));
        } else {
            parts.push(Part::Expr(Box::new((*op).clone())));
        }
    }

    // Must have at least one Expr part — a pure string-literal chain is not worth
    // converting (it would just be `\`constant\``).
    if !parts.iter().any(|p| matches!(p, Part::Expr(_))) {
        return None;
    }

    Some(Expr::Tpl(parts_to_template(parts, span)))
}

/// Flatten a left-associative `+` chain into individual operands.
fn collect_add_chain<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    if let Expr::Bin(BinExpr {
        op: BinaryOp::Add,
        left,
        right,
        ..
    }) = expr
    {
        collect_add_chain(left, out);
        out.push(right);
    } else {
        out.push(expr);
    }
}

fn is_str_lit(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(Lit::Str(_)))
}

/// Re-assemble a slice of expressions into a left-associative `+` chain.
fn rebuild_add_chain(exprs: &[&Expr]) -> Box<Expr> {
    debug_assert!(!exprs.is_empty());
    let mut acc = Box::new((*exprs[0]).clone());
    for e in &exprs[1..] {
        acc = Box::new(Expr::Bin(BinExpr {
            span: DUMMY_SP,
            op: BinaryOp::Add,
            left: acc,
            right: Box::new((*e).clone()),
        }));
    }
    acc
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
