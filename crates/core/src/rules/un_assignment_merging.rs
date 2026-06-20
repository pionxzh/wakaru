use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, Expr, ExprStmt, Ident, ModuleItem, SimpleAssignTarget, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

pub struct UnAssignmentMerging;

impl VisitMut for UnAssignmentMerging {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        let original = std::mem::take(items);
        let mut out = Vec::with_capacity(original.len());
        for item in original {
            match item {
                ModuleItem::Stmt(stmt) => {
                    let expanded = split_chained_assignment(stmt);
                    out.extend(expanded.into_iter().map(ModuleItem::Stmt));
                }
                other => out.push(other),
            }
        }
        *items = out;
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);

        let original = std::mem::take(stmts);
        let mut out = Vec::with_capacity(original.len());
        for stmt in original {
            out.extend(split_chained_assignment(stmt));
        }
        *stmts = out;
    }
}

/// Returns true if the statement is a chained assignment with a simple final value,
/// meaning it should be split.
fn should_split(stmt: &Stmt) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    let Expr::Assign(a) = &**expr else {
        return false;
    };
    if a.op != AssignOp::Assign {
        return false;
    }
    // Must be chained: right side is also an assignment
    let Expr::Assign(inner) = &*a.right else {
        return false;
    };
    if inner.op != AssignOp::Assign {
        return false;
    }
    // Walk to the final (non-assignment) value
    let mut cur: &Expr = a.right.as_ref();
    while let Expr::Assign(a2) = cur {
        if a2.op != AssignOp::Assign {
            return false;
        }
        cur = &a2.right;
    }
    is_simple_value(cur) && targets_can_be_split(a)
}

/// A "simple" value is an identifier or a primitive literal.
/// Regex literals are excluded: each evaluation creates a new object,
/// so cloning would break identity and shared `lastIndex` state.
fn is_simple_value(expr: &Expr) -> bool {
    match expr {
        Expr::Ident(_) => true,
        Expr::Lit(lit) => !matches!(lit, swc_core::ecma::ast::Lit::Regex(_)),
        _ => false,
    }
}

type BindingKey = (Atom, SyntaxContext);

fn targets_can_be_split(assign: &AssignExpr) -> bool {
    let mut assigned_bindings = HashSet::new();
    let mut current = assign;

    loop {
        if let Some(binding) = target_ident_binding(&current.left) {
            assigned_bindings.insert(binding);
        }

        match current.right.as_ref() {
            Expr::Assign(next) if next.op == AssignOp::Assign => {
                current = next;
            }
            _ => break,
        }
    }

    let mut current = assign;
    loop {
        if !target_reference_bindings(&current.left).is_disjoint(&assigned_bindings) {
            return false;
        }

        match current.right.as_ref() {
            Expr::Assign(next) if next.op == AssignOp::Assign => {
                current = next;
            }
            _ => return true,
        }
    }
}

fn target_ident_binding(target: &AssignTarget) -> Option<BindingKey> {
    match target {
        AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
            Some((binding.id.sym.clone(), binding.id.ctxt))
        }
        _ => None,
    }
}

fn target_reference_bindings(target: &AssignTarget) -> HashSet<BindingKey> {
    if matches!(target, AssignTarget::Simple(SimpleAssignTarget::Ident(_))) {
        return HashSet::new();
    }

    let mut collector = IdentReferenceCollector {
        references: HashSet::new(),
    };
    target.visit_with(&mut collector);
    collector.references
}

struct IdentReferenceCollector {
    references: HashSet<BindingKey>,
}

impl Visit for IdentReferenceCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        self.references.insert((ident.sym.clone(), ident.ctxt));
    }
}

/// Splits a chained assignment statement into individual assignment statements,
/// if applicable. Otherwise returns the statement unchanged (wrapped in a Vec).
fn split_chained_assignment(stmt: Stmt) -> Vec<Stmt> {
    if !should_split(&stmt) {
        return vec![stmt];
    }

    // Destructure the statement to collect all targets and the final value
    let Stmt::Expr(ExprStmt { span, expr }) = stmt else {
        unreachable!("should_split ensures this is an ExprStmt");
    };
    let Expr::Assign(top_assign) = *expr else {
        unreachable!("should_split ensures this is an AssignExpr");
    };

    let mut assignments: Vec<AssignTarget> = Vec::new();
    let mut current = top_assign;

    loop {
        assignments.push(current.left);
        match *current.right {
            Expr::Assign(next) if next.op == AssignOp::Assign => {
                current = next;
            }
            final_expr => {
                // This is the final (simple) value
                let final_value = Box::new(final_expr);
                return assignments
                    .into_iter()
                    .map(|target| {
                        Stmt::Expr(ExprStmt {
                            span,
                            expr: Box::new(Expr::Assign(AssignExpr {
                                span: DUMMY_SP,
                                op: AssignOp::Assign,
                                left: target,
                                right: final_value.clone(),
                            })),
                        })
                    })
                    .collect();
            }
        }
    }
}
