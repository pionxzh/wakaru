use swc_core::ecma::ast::Expr;

pub fn strip_parens(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => strip_parens(&paren.expr),
        _ => expr,
    }
}

pub fn strip_parens_owned(expr: Expr) -> Expr {
    match expr {
        Expr::Paren(paren) => strip_parens_owned(*paren.expr),
        other => other,
    }
}

pub fn strip_parens_mut(expr: &mut Box<Expr>) -> &mut Expr {
    let mut current = expr.as_mut();
    loop {
        match current {
            Expr::Paren(paren) => current = paren.expr.as_mut(),
            _ => return current,
        }
    }
}
