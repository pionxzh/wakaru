//! Top-level comma sequences that a minifier merged out of separate
//! statements. Terser compresses code minified together with a bundle (a
//! banner, an injected snippet, a footer, concatenated scripts) into one
//! expression statement, `a(), (() => { … })();`, so the bundle statement no
//! longer stands at the top level where the detectors look for it. At
//! statement level `a, b, c;` evaluates exactly like `a; b; c;`, so detection
//! can run again with each element as its own statement.

use swc_core::common::{Span, Spanned};
use swc_core::ecma::ast::{
    AssignTarget, Callee, Expr, ExprStmt, Lit, Module, ModuleItem, OptChainBase, ParenExpr,
    SeqExpr, Stmt,
};

use crate::utils::paren::strip_parens;

/// Splits, in place, every top-level expression statement holding a comma
/// sequence (parenthesized or not) into one expression statement per
/// element, in order, each with its element's span. `None`, with the module
/// untouched, when there is no such statement.
///
/// The split moves nodes instead of copying the module. An input that is
/// still no bundle must keep its parsed form for the plain path, so the
/// caller undoes the split with [`SplitSequences::restore`].
pub(super) fn split_top_level_sequences(module: &mut Module) -> Option<SplitSequences> {
    if !module
        .body
        .iter()
        .any(|item| top_level_sequence(item).is_some())
    {
        return None;
    }
    let items = std::mem::take(&mut module.body);
    let mut body = Vec::with_capacity(items.len() + 8);
    let mut splits = Vec::new();
    for item in items {
        if top_level_sequence(&item).is_none() {
            body.push(item);
            continue;
        }
        let ModuleItem::Stmt(Stmt::Expr(ExprStmt {
            span: stmt_span,
            mut expr,
        })) = item
        else {
            unreachable!("a top-level sequence is an expression statement");
        };
        let mut parens = Vec::new();
        let seq = loop {
            match *expr {
                Expr::Paren(paren) => {
                    parens.push(paren.span);
                    expr = paren.expr;
                }
                Expr::Seq(seq) => break seq,
                _ => unreachable!("a top-level sequence strips to a sequence"),
            }
        };
        let mut wrapped = Vec::with_capacity(seq.exprs.len());
        let index = body.len();
        for mut element in seq.exprs {
            let span = element.span();
            wrapped.push(guard_statement_start(&mut element));
            body.push(ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                span,
                expr: element,
            })));
        }
        splits.push(Split {
            index,
            stmt_span,
            parens,
            seq_span: seq.span,
            wrapped,
        });
    }
    module.body = body;
    Some(SplitSequences(splits))
}

/// How to undo [`split_top_level_sequences`].
pub(super) struct SplitSequences(Vec<Split>);

struct Split {
    /// Position of the first element statement in the split body.
    index: usize,
    stmt_span: Span,
    /// Spans of the parentheses around the sequence, outermost first.
    parens: Vec<Span>,
    seq_span: Span,
    /// Per element, whether [`guard_statement_start`] added parentheses.
    wrapped: Vec<bool>,
}

impl SplitSequences {
    /// Rebuild each sequence statement exactly as it was parsed.
    pub(super) fn restore(self, module: &mut Module) {
        let mut items = std::mem::take(&mut module.body).into_iter();
        let mut body = Vec::with_capacity(items.len());
        // Position in the split body.
        let mut position = 0;
        for split in self.0 {
            while position < split.index {
                body.extend(items.next());
                position += 1;
            }
            position += split.wrapped.len();
            let mut exprs = Vec::with_capacity(split.wrapped.len());
            for wrapped in split.wrapped {
                let Some(ModuleItem::Stmt(Stmt::Expr(ExprStmt { mut expr, .. }))) = items.next()
                else {
                    unreachable!("split elements stay expression statements");
                };
                if wrapped {
                    unguard_statement_start(&mut expr);
                }
                exprs.push(expr);
            }
            let mut expr = Box::new(Expr::Seq(SeqExpr {
                span: split.seq_span,
                exprs,
            }));
            for span in split.parens.into_iter().rev() {
                expr = Box::new(Expr::Paren(ParenExpr { span, expr }));
            }
            body.push(ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                span: split.stmt_span,
                expr,
            })));
        }
        body.extend(items);
        module.body = body;
    }
}

fn top_level_sequence(item: &ModuleItem) -> Option<&SeqExpr> {
    let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
        return None;
    };
    match strip_parens(expr) {
        Expr::Seq(seq) => Some(seq),
        _ => None,
    }
}

/// An element that followed a comma now begins a statement. Parenthesize the
/// part a statement cannot begin with: a leading `function`, `class`, object
/// literal or object pattern (each would parse as a declaration, a block, or
/// fail), a leading `let` (a declaration), and a whole string literal, which
/// at the start of the file would become a directive such as `"use strict"`.
/// Only the leftmost operand is wrapped (`(function () {})()`), the form the
/// detectors already accept for a bundle wrapper. Returns whether it added
/// parentheses.
fn guard_statement_start(expr: &mut Box<Expr>) -> bool {
    if matches!(&**expr, Expr::Lit(Lit::Str(_))) {
        parenthesize(expr);
        return true;
    }
    // The left spine of a long `a + b + …` chain can be thousands deep; walk
    // it without recursion. The walk stops at the first parenthesized
    // operand, so the parentheses it adds are the first ones on the spine.
    let mut current = expr;
    loop {
        if begins_with_hazard(current) {
            parenthesize(current);
            return true;
        }
        current = match left_operand(current) {
            Some(next) => next,
            None => return false,
        };
    }
}

/// Remove the parentheses [`guard_statement_start`] added: the first ones on
/// the left spine.
fn unguard_statement_start(expr: &mut Box<Expr>) {
    let mut current = expr;
    loop {
        if let Expr::Paren(paren) = &mut **current {
            let inner =
                std::mem::replace(&mut paren.expr, Box::new(Expr::Invalid(Default::default())));
            *current = inner;
            return;
        }
        current = match left_operand(current) {
            Some(next) => next,
            None => return,
        };
    }
}

/// The operand whose first token is the expression's first token.
fn left_operand(expr: &mut Box<Expr>) -> Option<&mut Box<Expr>> {
    Some(match &mut **expr {
        Expr::Call(call) => match &mut call.callee {
            Callee::Expr(callee) => callee,
            _ => return None,
        },
        Expr::Member(member) => &mut member.obj,
        Expr::Bin(bin) => &mut bin.left,
        Expr::Cond(cond) => &mut cond.test,
        Expr::TaggedTpl(tagged) => &mut tagged.tag,
        Expr::Update(update) if !update.prefix => &mut update.arg,
        Expr::OptChain(chain) => match &mut *chain.base {
            OptChainBase::Member(member) => &mut member.obj,
            OptChainBase::Call(call) => &mut call.callee,
        },
        Expr::Assign(assign) => match &mut assign.left {
            AssignTarget::Simple(swc_core::ecma::ast::SimpleAssignTarget::Member(member)) => {
                &mut member.obj
            }
            _ => return None,
        },
        Expr::Seq(seq) => seq.exprs.first_mut()?,
        _ => return None,
    })
}

fn begins_with_hazard(expr: &Expr) -> bool {
    match expr {
        Expr::Fn(_) | Expr::Class(_) | Expr::Object(_) => true,
        Expr::Ident(ident) => ident.sym == "let",
        Expr::Assign(assign) => matches!(
            &assign.left,
            AssignTarget::Pat(swc_core::ecma::ast::AssignTargetPat::Object(_))
        ),
        _ => false,
    }
}

fn parenthesize(expr: &mut Box<Expr>) {
    let span = expr.span();
    let inner = std::mem::replace(&mut **expr, Expr::Invalid(Default::default()));
    **expr = Expr::Paren(ParenExpr {
        span,
        expr: Box::new(inner),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::common::{sync::Lrc, SourceMap, GLOBALS};
    use swc_core::ecma::codegen::Config;

    fn split(source: &str) -> Option<String> {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = super::super::parse_es_module(source, "input.js", cm.clone()).unwrap();
            let parsed = module.clone();
            let split = split_top_level_sequences(&mut module)?;
            let emitted = super::super::emit_module_with_positions(
                &module,
                cm,
                Config::default().with_minify(true),
                super::super::SourcePositions::Discard,
            )
            .unwrap();
            split.restore(&mut module);
            assert!(module == parsed, "restore must rebuild the parsed module");
            Some(emitted.code)
        })
    }

    #[test]
    fn module_without_a_top_level_sequence_is_not_split() {
        assert_eq!(split("a();\nfunction f() { return b(), c(); }"), None);
    }

    #[test]
    fn each_element_becomes_a_statement_in_order() {
        assert_eq!(
            split("a(), (b = 1), c.d = 2;\nvar g;\n((e(), (f(), h())));\ni(), j();").as_deref(),
            Some("a();(b=1);c.d=2;var g;e();(f(),h());i();j();")
        );
    }

    #[test]
    fn statement_start_hazards_get_parentheses() {
        assert_eq!(
            split(r#"a(), function () {}(), class {}.x, { b: 1 }.b, { c } = d;"#).as_deref(),
            Some("a();(function(){})();(class{}).x;({b:1}).b;({c}=d);")
        );
    }

    #[test]
    fn a_leading_string_does_not_become_a_directive() {
        assert_eq!(
            split(r#""use strict", a();"#).as_deref(),
            Some(r#"("use strict");a();"#)
        );
    }

    #[test]
    fn plain_input_with_top_level_sequences_is_left_as_parsed() {
        // A minified script that is no bundle: the retry runs on a copy, and
        // the parsed module that the plain path reuses must not change.
        // shape: hypothetical (Terser output of a plain script).
        let source = r#""use strict";var a=1;!function(){a++}(),window.b=function(){return a},a>1&&console.log(a),function(e){e.c=2}(window);"#;
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = super::super::parse_es_module(source, "input.js", cm.clone()).unwrap();
            let parsed = module.clone();
            let detected = super::super::detect_parsed_source(
                &mut module,
                cm,
                source,
                false,
                super::super::SourcePositions::Discard,
            );
            assert!(detected.is_none());
            assert!(module == parsed, "the parsed module must stay as parsed");
        });
    }
}
