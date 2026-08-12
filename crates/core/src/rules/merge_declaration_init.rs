//! Merge a bare declaration with its first assignment.
//!
//! Transpilers and minifiers frequently hoist declarations and then assign them
//! later, producing split forms like:
//!
//! ```js
//! let response;
//! response = await fetch_user(id);
//! ```
//!
//! This rule folds the bare `let`/`var` declaration into its first
//! statement-level assignment **in the same statement list**. It also handles
//! an exactly-adjacent top-level declaration and literal initializer, including
//! `export let Enum; Enum = { ... };`. In standard mode, it only folds inert
//! right-hand sides. Aggressive mode also folds broader generated-code shapes
//! such as `let response; response = await fetch_user(id);`.
//!
//! It runs late (after `UnDestructuring`/`SmartInline`) so it does not disturb
//! the assignment-form temporaries those rules rely on. `VarDeclToLetConst` has
//! already run, so this rule batches the `let` bindings it merges and performs
//! one module-wide post-merge write analysis before promoting eligible bindings
//! to `const`. Modules with no merged `let` skip that analysis entirely, and no
//! binding triggers its own AST scan. This avoids rerunning the broader early
//! declaration-kind rule.
//!
//! ## Safety
//!
//! The merge only fires when these structural guards pass:
//! - the declaration is a single bare `let`/`var` binding (no initializer);
//! - the first statement-level assignment to that binding is a simple `=` in the
//!   same statement list (not nested in a branch/loop/closure);
//! - top-level module merging is exactly adjacent and recursively literal-only;
//! - only other bare declarations appear between the declaration and that
//!   assignment (calls, branches, function declarations, and initialized
//!   declarations may observe declaration timing or closure state);
//! - the assignment's right-hand side does not reference the binding itself.
//!
//! Standard mode additionally requires an inert right-hand side so the merge
//! cannot change whether the binding is initialized while evaluating that RHS
//! (for example through a call, `await`, or direct `eval`). Aggressive mode
//! relaxes that RHS guard for statement-list merges. A merged `let` is promoted
//! to `const` only when the completed module has neither another resolved direct
//! write nor direct `eval` that can name the binding. The module-wide eval check
//! is intentionally conservative for bindings inside nested functions.
//!
//! Matching is by [`BindingId`] (name + `SyntaxContext`), so same-named bindings
//! in different scopes are never conflated.

use std::collections::HashSet;

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, Decl, EmptyStmt, Expr, Ident, Lit, Module, ModuleDecl, ModuleItem, Pat,
    Prop, PropName, PropOrSpread, SimpleAssignTarget, Stmt, UnaryOp, VarDecl, VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_uses::BindingUseIndex;

use super::{decl_utils::BindingId, RewriteLevel};
use super::{eval_utils::js_source_mentions_binding, eval_utils::DirectEvalAnalyzer};

pub struct MergeDeclarationInit {
    level: RewriteLevel,
    merged_let_bindings: HashSet<BindingId>,
}

impl MergeDeclarationInit {
    pub fn new(level: RewriteLevel) -> Self {
        Self {
            level,
            merged_let_bindings: HashSet::new(),
        }
    }
}

impl Default for MergeDeclarationInit {
    fn default() -> Self {
        Self::new(RewriteLevel::Standard)
    }
}

impl VisitMut for MergeDeclarationInit {
    fn visit_mut_module(&mut self, module: &mut Module) {
        debug_assert!(self.merged_let_bindings.is_empty());
        module.visit_mut_children_with(self);
        promote_merged_lets(module, &self.merged_let_bindings);
        self.merged_let_bindings.clear();
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        self.merged_let_bindings
            .extend(merge_module_item_list(items));
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        self.merged_let_bindings
            .extend(merge_stmt_list(stmts, self.level));
    }
}

fn merge_module_item_list(items: &mut Vec<ModuleItem>) -> Vec<BindingId> {
    let mut merged_lets = Vec::new();
    let mut i = 0;
    while i + 1 < items.len() {
        let Some(id) = module_bare_decl_binding(&items[i]) else {
            i += 1;
            continue;
        };
        if module_assignment_target(&items[i + 1]).as_ref() != Some(&id)
            || module_assignment_rhs_references(&items[i + 1], &id)
            || !module_assignment_rhs_is_literal(&items[i + 1]).unwrap_or(false)
        {
            i += 1;
            continue;
        }

        let merged_let = module_decl_kind(&items[i]) == Some(VarDeclKind::Let);
        let rhs = take_module_assignment_rhs(&mut items[i + 1]);
        let mut declaration = std::mem::replace(
            &mut items[i],
            ModuleItem::Stmt(Stmt::Empty(EmptyStmt { span: DUMMY_SP })),
        );
        let var = module_var_decl_mut(&mut declaration)
            .expect("module_bare_decl_binding guarantees a variable declaration");
        var.decls[0].init = Some(rhs);
        items[i + 1] = declaration;
        items.remove(i);
        if merged_let {
            merged_lets.push(id);
        }
    }
    merged_lets
}

fn merge_stmt_list(stmts: &mut Vec<Stmt>, level: RewriteLevel) -> Vec<BindingId> {
    let mut merged_lets = Vec::new();
    let mut i = 0;
    while i < stmts.len() {
        let Some(id) = bare_decl_binding(&stmts[i]) else {
            i += 1;
            continue;
        };
        let assignment =
            (i + 1..stmts.len()).find(|&j| assignment_target(&stmts[j]) == Some(id.clone()));
        let Some(j) = assignment else {
            i += 1;
            continue;
        };
        let between = &stmts[i + 1..j];
        if !only_bare_declarations(between)
            || slice_references(between, &id)
            || assignment_rhs_references(&stmts[j], &id)
            || (level < RewriteLevel::Aggressive
                && !assignment_rhs_is_standard_safe(&stmts[j]).unwrap_or(false))
        {
            i += 1;
            continue;
        }

        let merged_let = stmt_decl_kind(&stmts[i]) == Some(VarDeclKind::Let);
        let rhs = take_assignment_rhs(&mut stmts[j]);
        let mut var = take_var_decl(&mut stmts[i]);
        var.decls[0].init = Some(rhs);
        stmts[j] = Stmt::Decl(Decl::Var(var));
        stmts.remove(i);
        if merged_let {
            merged_lets.push(id);
        }
        // Elements shifted left by one; re-examine the same index. The merged
        // declaration now has an initializer, so it won't be matched again.
    }
    merged_lets
}

fn only_bare_declarations(stmts: &[Stmt]) -> bool {
    stmts.iter().all(|stmt| bare_decl_binding(stmt).is_some())
}

/// The binding of a bare `let`/`var X;` (single declarator, no initializer).
fn bare_decl_binding(stmt: &Stmt) -> Option<BindingId> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    bare_var_binding(var)
}

fn bare_var_binding(var: &VarDecl) -> Option<BindingId> {
    if var.kind == VarDeclKind::Const || var.decls.len() != 1 {
        return None;
    }
    let declarator = &var.decls[0];
    if declarator.init.is_some() {
        return None;
    }
    let Pat::Ident(binding) = &declarator.name else {
        return None;
    };
    Some((binding.id.sym.clone(), binding.id.ctxt))
}

fn module_bare_decl_binding(item: &ModuleItem) -> Option<BindingId> {
    module_var_decl(item).and_then(bare_var_binding)
}

fn module_var_decl(item: &ModuleItem) -> Option<&VarDecl> {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => Some(var),
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
            let Decl::Var(var) = &export.decl else {
                return None;
            };
            Some(var)
        }
        _ => None,
    }
}

fn module_var_decl_mut(item: &mut ModuleItem) -> Option<&mut VarDecl> {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => Some(var),
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
            let Decl::Var(var) = &mut export.decl else {
                return None;
            };
            Some(var)
        }
        _ => None,
    }
}

fn module_decl_kind(item: &ModuleItem) -> Option<VarDeclKind> {
    module_var_decl(item).map(|var| var.kind)
}

fn stmt_decl_kind(stmt: &Stmt) -> Option<VarDeclKind> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    Some(var.kind)
}

/// The binding targeted by a statement-level simple assignment `X = expr;`.
fn assignment_target(stmt: &Stmt) -> Option<BindingId> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = &*expr_stmt.expr else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &assign.left else {
        return None;
    };
    Some((binding.id.sym.clone(), binding.id.ctxt))
}

fn assignment_rhs_references(stmt: &Stmt, id: &BindingId) -> bool {
    let Some(rhs) = assignment_rhs(stmt) else {
        return false;
    };
    let mut finder = RefFinder { id, found: false };
    rhs.visit_with(&mut finder);
    finder.found
}

fn assignment_rhs_is_standard_safe(stmt: &Stmt) -> Option<bool> {
    Some(expr_is_inert_initializer(assignment_rhs(stmt)?))
}

fn module_assignment_target(item: &ModuleItem) -> Option<BindingId> {
    let ModuleItem::Stmt(stmt) = item else {
        return None;
    };
    assignment_target(stmt)
}

fn module_assignment_rhs_references(item: &ModuleItem, id: &BindingId) -> bool {
    let ModuleItem::Stmt(stmt) = item else {
        return false;
    };
    assignment_rhs_references(stmt, id)
}

fn module_assignment_rhs_is_literal(item: &ModuleItem) -> Option<bool> {
    let ModuleItem::Stmt(stmt) = item else {
        return None;
    };
    Some(expr_is_literal_initializer(assignment_rhs(stmt)?))
}

fn assignment_rhs(stmt: &Stmt) -> Option<&Expr> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = &*expr_stmt.expr else {
        return None;
    };
    Some(assign.right.as_ref())
}

fn expr_is_inert_initializer(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(_) | Expr::Fn(_) | Expr::Arrow(_) => true,
        Expr::Object(obj) => obj.props.is_empty(),
        Expr::Array(array) => array.elems.is_empty(),
        Expr::Paren(paren) => expr_is_inert_initializer(&paren.expr),
        _ => false,
    }
}

fn expr_is_literal_initializer(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(_) => true,
        Expr::Unary(unary)
            if matches!(unary.op, UnaryOp::Plus | UnaryOp::Minus)
                && matches!(unary.arg.as_ref(), Expr::Lit(Lit::Num(_))) =>
        {
            true
        }
        Expr::Object(obj) => obj.props.iter().all(|prop| {
            let PropOrSpread::Prop(prop) = prop else {
                return false;
            };
            let Prop::KeyValue(prop) = prop.as_ref() else {
                return false;
            };
            prop_name_is_literal(&prop.key) && expr_is_literal_initializer(&prop.value)
        }),
        Expr::Array(array) => {
            array.elems.iter().flatten().all(|element| {
                element.spread.is_none() && expr_is_literal_initializer(&element.expr)
            })
        }
        Expr::Paren(paren) => expr_is_literal_initializer(&paren.expr),
        _ => false,
    }
}

fn prop_name_is_literal(name: &PropName) -> bool {
    match name {
        PropName::Ident(_) | PropName::Str(_) | PropName::Num(_) | PropName::BigInt(_) => true,
        PropName::Computed(computed) => computed_prop_key_is_primitive_literal(&computed.expr),
    }
}

fn computed_prop_key_is_primitive_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(Lit::Str(_) | Lit::Bool(_) | Lit::Null(_) | Lit::Num(_) | Lit::BigInt(_)) => true,
        Expr::Unary(unary)
            if matches!(unary.op, UnaryOp::Plus | UnaryOp::Minus)
                && matches!(unary.arg.as_ref(), Expr::Lit(Lit::Num(_))) =>
        {
            true
        }
        Expr::Paren(paren) => computed_prop_key_is_primitive_literal(&paren.expr),
        _ => false,
    }
}

/// Take the right-hand side out of a statement known to be `X = expr;`.
fn take_assignment_rhs(stmt: &mut Stmt) -> Box<Expr> {
    let taken = std::mem::replace(stmt, Stmt::Empty(EmptyStmt { span: DUMMY_SP }));
    let Stmt::Expr(expr_stmt) = taken else {
        unreachable!("assignment_target guarantees an ExprStmt")
    };
    let Expr::Assign(assign) = *expr_stmt.expr else {
        unreachable!("assignment_target guarantees an AssignExpr")
    };
    assign.right
}

fn take_module_assignment_rhs(item: &mut ModuleItem) -> Box<Expr> {
    let ModuleItem::Stmt(stmt) = item else {
        unreachable!("module_assignment_target guarantees a statement")
    };
    take_assignment_rhs(stmt)
}

fn promote_merged_lets(module: &mut Module, candidates: &HashSet<BindingId>) {
    if candidates.is_empty() {
        return;
    }

    let writes = BindingUseIndex::collect_direct_write_bindings(module);
    let mut eval = DirectEvalAnalyzer::default();
    module.visit_with(&mut eval);

    let eligible = candidates
        .iter()
        .filter(|id| {
            !writes.contains(*id)
                && !eval.unknown_direct_eval
                && !eval
                    .known_direct_eval_sources
                    .iter()
                    .any(|source| js_source_mentions_binding(source, &id.0))
        })
        .cloned()
        .collect::<HashSet<_>>();

    if !eligible.is_empty() {
        module.visit_mut_with(&mut MergedLetPromoter {
            eligible: &eligible,
        });
    }
}

struct MergedLetPromoter<'a> {
    eligible: &'a HashSet<BindingId>,
}

impl VisitMut for MergedLetPromoter<'_> {
    fn visit_mut_var_decl(&mut self, var: &mut VarDecl) {
        if var.kind == VarDeclKind::Let && var.decls.len() == 1 {
            if let Pat::Ident(binding) = &var.decls[0].name {
                let id = (binding.id.sym.clone(), binding.id.ctxt);
                if self.eligible.contains(&id) {
                    var.kind = VarDeclKind::Const;
                }
            }
        }
        var.visit_mut_children_with(self);
    }
}

/// Take the boxed `VarDecl` out of a statement known to be a bare declaration.
fn take_var_decl(stmt: &mut Stmt) -> Box<VarDecl> {
    let taken = std::mem::replace(stmt, Stmt::Empty(EmptyStmt { span: DUMMY_SP }));
    let Stmt::Decl(Decl::Var(var)) = taken else {
        unreachable!("bare_decl_binding guarantees a VarDecl")
    };
    var
}

fn slice_references(stmts: &[Stmt], id: &BindingId) -> bool {
    let mut finder = RefFinder { id, found: false };
    for stmt in stmts {
        stmt.visit_with(&mut finder);
        if finder.found {
            return true;
        }
    }
    false
}

struct RefFinder<'a> {
    id: &'a BindingId,
    found: bool,
}

impl Visit for RefFinder<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == self.id.0 && ident.ctxt == self.id.1 {
            self.found = true;
        }
    }
}
