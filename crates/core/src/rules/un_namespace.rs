//! Recover simple TypeScript namespace IIFEs as stable-alias blocks.
//!
//! TypeScript lowers runtime namespace members to an augmentation IIFE:
//!
//! ```js
//! (function (namespace) {
//!     namespace.value = createValue();
//! })(Helpers || (Helpers = {}));
//! ```
//!
//! Replacing that with `Helpers = { value: createValue() }` is not generally
//! equivalent: namespaces can be augmented more than once, and the existing
//! value can be a proxy or otherwise observable object. This rule instead
//! removes only the generated call boundary while preserving its stable alias:
//!
//! ```js
//! {
//!     const namespace = Helpers || (Helpers = {});
//!     namespace.value = createValue();
//! }
//! ```
//!
//! The initial implementation deliberately accepts only sequential static
//! member assignments. Function-scoped declarations, direct eval, lexical
//! `this`/`arguments`/`new.target`, and alias reassignment keep the IIFE.

use swc_core::common::{Spanned, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BinExpr, BinaryOp, BindingIdent, BlockStmt, BlockStmtOrExpr, CallExpr,
    Callee, Decl, Expr, ExprStmt, FnExpr, Function, Ident, MemberProp, MetaPropExpr, MetaPropKind,
    Pat, SimpleAssignTarget, Stmt, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_uses::{BindingId, BindingUseIndex};
use crate::utils::paren::strip_parens;

use super::eval_utils::is_direct_eval_call;

#[derive(Default)]
pub struct UnNamespace;

impl VisitMut for UnNamespace {
    fn visit_mut_stmt(&mut self, stmt: &mut Stmt) {
        stmt.visit_mut_children_with(self);

        if let Some(block) = build_namespace_block(stmt) {
            *stmt = Stmt::Block(block);
        }
    }
}

fn build_namespace_block(stmt: &Stmt) -> Option<BlockStmt> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = strip_unary_bang(strip_parens(expr)) else {
        return None;
    };
    let (binding, body) = namespace_callee(call)?;
    let binding_id = (binding.id.sym.clone(), binding.id.ctxt);

    let init = namespace_initializer(call)?;
    if body.stmts.is_empty()
        || !body
            .stmts
            .iter()
            .all(|stmt| is_namespace_member_assignment(stmt, &binding_id))
        || BindingUseIndex::collect_stmts(&body.stmts).has_direct_write(&binding_id)
        || has_function_scope_hazard(body)
    {
        return None;
    }

    let mut stmts = Vec::with_capacity(body.stmts.len() + 1);
    stmts.push(alias_declaration(binding, init));
    stmts.extend(body.stmts.iter().cloned());

    Some(BlockStmt {
        span: stmt.span(),
        ctxt: Default::default(),
        stmts,
    })
}

fn namespace_callee(call: &CallExpr) -> Option<(BindingIdent, &BlockStmt)> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };

    match strip_parens(callee) {
        Expr::Fn(FnExpr {
            ident: None,
            function,
        }) => function_namespace_parts(function),
        Expr::Arrow(arrow) if !arrow.is_async && !arrow.is_generator && arrow.params.len() == 1 => {
            let Pat::Ident(binding) = &arrow.params[0] else {
                return None;
            };
            let BlockStmtOrExpr::BlockStmt(body) = arrow.body.as_ref() else {
                return None;
            };
            Some((binding.clone(), body))
        }
        _ => None,
    }
}

fn function_namespace_parts(function: &Function) -> Option<(BindingIdent, &BlockStmt)> {
    if function.is_async || function.is_generator || function.params.len() != 1 {
        return None;
    }
    let param = &function.params[0];
    if !param.decorators.is_empty() {
        return None;
    }
    let Pat::Ident(binding) = &param.pat else {
        return None;
    };
    Some((binding.clone(), function.body.as_ref()?))
}

fn namespace_initializer(call: &CallExpr) -> Option<Box<Expr>> {
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return None;
    }
    let init = call.args[0].expr.clone();
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalOr,
        left,
        right,
        ..
    }) = strip_parens(&init)
    else {
        return None;
    };
    let Expr::Ident(read) = strip_parens(left) else {
        return None;
    };
    let Expr::Assign(assign) = strip_parens(right) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(write)) = &assign.left else {
        return None;
    };
    if !same_binding(read, &write.id)
        || !matches!(strip_parens(&assign.right), Expr::Object(object) if object.props.is_empty())
    {
        return None;
    }
    Some(init)
}

fn is_namespace_member_assignment(stmt: &Stmt, binding: &BindingId) -> bool {
    let Stmt::Expr(expr_stmt) = stmt else {
        return false;
    };
    let Expr::Assign(assign) = strip_parens(&expr_stmt.expr) else {
        return false;
    };
    if assign.op != AssignOp::Assign {
        return false;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return false;
    };
    let Expr::Ident(object) = strip_parens(&member.obj) else {
        return false;
    };
    matches!(&member.prop, MemberProp::Ident(_))
        && object.sym == binding.0
        && object.ctxt == binding.1
}

fn alias_declaration(binding: BindingIdent, init: Box<Expr>) -> Stmt {
    Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(binding),
            init: Some(init),
            definite: false,
        }],
    })))
}

fn strip_unary_bang(expr: &Expr) -> &Expr {
    if let Expr::Unary(unary) = expr {
        if unary.op == swc_core::ecma::ast::UnaryOp::Bang {
            return strip_parens(&unary.arg);
        }
    }
    expr
}

fn same_binding(left: &Ident, right: &Ident) -> bool {
    left.sym == right.sym && left.ctxt == right.ctxt
}

fn has_function_scope_hazard(body: &BlockStmt) -> bool {
    let mut eval = DirectEvalFinder::default();
    body.visit_with(&mut eval);
    if eval.found {
        return true;
    }

    let mut finder = FunctionScopeHazardFinder::default();
    body.visit_with(&mut finder);
    finder.found
}

#[derive(Default)]
struct DirectEvalFinder {
    found: bool,
}

impl Visit for DirectEvalFinder {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if is_direct_eval_call(call) {
            self.found = true;
            return;
        }
        call.visit_children_with(self);
    }
}

#[derive(Default)]
struct FunctionScopeHazardFinder {
    found: bool,
}

impl Visit for FunctionScopeHazardFinder {
    // Nested ordinary functions provide their own `this`, `arguments`,
    // and `new.target`. Arrow functions deliberately use the default recursion
    // because they capture those from the namespace IIFE. Direct eval is
    // checked separately across all nested scopes because evaluated source can
    // still assign the outer namespace alias by name.
    fn visit_function(&mut self, _: &Function) {}

    fn visit_this_expr(&mut self, _: &swc_core::ecma::ast::ThisExpr) {
        self.found = true;
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == *"arguments" {
            self.found = true;
        }
    }

    fn visit_meta_prop_expr(&mut self, meta: &MetaPropExpr) {
        if meta.kind == MetaPropKind::NewTarget {
            self.found = true;
        }
    }
}
