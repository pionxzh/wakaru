use swc_core::common::{Mark, SyntaxContext};
use swc_core::ecma::ast::{BinExpr, BinaryOp, Expr, Ident, Lit, UnaryExpr, UnaryOp};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

/// Rewrites minifier output `1 / 0` and `-1 / 0` back to `Infinity` and
/// `-Infinity`.
///
/// The synthesized `Infinity` carries the unresolved mark, the codebase idiom
/// for a generated reference to a global: later scope-aware rules and renamers
/// then see it as a global reference rather than an unmarked name. Printed
/// JavaScript is still name-based, so a binding named `Infinity` in scope
/// would capture it — that exposure is tracked separately.
pub struct UnInfinity {
    unresolved_ctxt: SyntaxContext,
}

impl UnInfinity {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self {
            unresolved_ctxt: SyntaxContext::empty().apply_mark(unresolved_mark),
        }
    }

    fn infinity(&self, span: swc_core::common::Span) -> Expr {
        Expr::Ident(Ident::new("Infinity".into(), span, self.unresolved_ctxt))
    }
}

impl VisitMut for UnInfinity {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Bin(BinExpr {
            op: BinaryOp::Div,
            left,
            right,
            span,
        }) = expr
        {
            if !matches!(&**right, Expr::Lit(Lit::Num(num)) if num.value == 0.0) {
                return;
            }

            if matches!(&**left, Expr::Lit(Lit::Num(num)) if num.value == 1.0) {
                *expr = self.infinity(*span);
                return;
            }

            if matches!(&**left, Expr::Unary(UnaryExpr { op: UnaryOp::Minus, arg, .. }) if matches!(&**arg, Expr::Lit(Lit::Num(num)) if num.value == 1.0))
            {
                *expr = Expr::Unary(UnaryExpr {
                    span: *span,
                    op: UnaryOp::Minus,
                    arg: Box::new(self.infinity(*span)),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, FileName, Globals, SourceMap, GLOBALS};
    use swc_core::ecma::ast::{Decl, ModuleItem, Stmt};
    use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
    use swc_core::ecma::transforms::base::resolver;

    use super::*;

    #[test]
    fn synthesized_infinity_carries_the_unresolved_mark() {
        GLOBALS.set(&Globals::new(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let file = cm.new_source_file(
                FileName::Custom("fixture.js".into()).into(),
                "const a = 1 / 0, b = -1 / 0;".to_string(),
            );
            let lexer = Lexer::new(
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                StringInput::from(&*file),
                None,
            );
            let mut module = Parser::new_from(lexer).parse_module().expect("parse");
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));

            module.visit_mut_with(&mut UnInfinity::new(unresolved_mark));

            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = &module.body[0] else {
                panic!("expected a var declaration");
            };
            let a = var.decls[0].init.as_deref().expect("init");
            let Expr::Ident(a) = a else {
                panic!("expected `Infinity`, got {a:?}");
            };
            assert_eq!(a.sym.as_ref(), "Infinity");
            assert_eq!(a.ctxt.outer(), unresolved_mark);

            let b = var.decls[1].init.as_deref().expect("init");
            let Expr::Unary(UnaryExpr { arg, .. }) = b else {
                panic!("expected `-Infinity`, got {b:?}");
            };
            let Expr::Ident(b) = arg.as_ref() else {
                panic!("expected `Infinity` operand, got {arg:?}");
            };
            assert_eq!(b.ctxt.outer(), unresolved_mark);
        });
    }
}
