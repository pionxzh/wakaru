use std::collections::HashSet;

use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, BlockStmt, Decl, Expr, ExprStmt, ForHead, ForInStmt, ForOfStmt,
    ForStmt, IfStmt, Invalid, Lit, MemberExpr, ModuleDecl, ModuleItem, ParenExpr, Pat, Prop,
    PropName, PropOrSpread, ReturnStmt, SeqExpr, SimpleAssignTarget, Stmt, SwitchStmt, ThrowStmt,
    UnaryOp, VarDecl, VarDeclKind, VarDeclOrExpr, VarDeclarator,
};
use swc_core::ecma::utils::{ExprCtx, ExprExt};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::decl_utils::BindingId;
use super::RewriteLevel;

pub struct SimplifySequence {
    unresolved_mark: Mark,
    level: RewriteLevel,
}

impl SimplifySequence {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self::new_with_level(unresolved_mark, RewriteLevel::Standard)
    }

    pub fn new_with_level(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            unresolved_mark,
            level,
        }
    }
}

impl VisitMut for SimplifySequence {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        let old_items = std::mem::take(items);
        let mut new_items = Vec::with_capacity(old_items.len());

        let mut future_lexical = collect_lexical_decl_ids_from_module_items(&old_items);

        for item in old_items {
            match item {
                ModuleItem::Stmt(stmt) => {
                    let declared = collect_lexical_decl_ids_from_stmt(&stmt);
                    for stmt in split_stmt(stmt, self.level) {
                        if !is_pure_no_op_stmt(&stmt, self.unresolved_mark, &future_lexical) {
                            new_items.push(ModuleItem::Stmt(stmt));
                        }
                    }
                    remove_ids(&mut future_lexical, &declared);
                }
                ModuleItem::ModuleDecl(decl) => {
                    let declared = collect_lexical_decl_ids_from_module_decl(&decl);
                    new_items.push(ModuleItem::ModuleDecl(decl));
                    remove_ids(&mut future_lexical, &declared);
                }
            }
        }

        *items = new_items;
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);

        let old_stmts = std::mem::take(stmts);
        let mut new_stmts = Vec::with_capacity(old_stmts.len());

        let mut future_lexical = collect_lexical_decl_ids_from_stmts(&old_stmts);

        for stmt in old_stmts {
            let declared = collect_lexical_decl_ids_from_stmt(&stmt);
            for s in split_stmt(stmt, self.level) {
                if !is_pure_no_op_stmt(&s, self.unresolved_mark, &future_lexical) {
                    new_stmts.push(s);
                }
            }
            remove_ids(&mut future_lexical, &declared);
        }

        *stmts = new_stmts;
    }
}

/// Returns true for expression statements that are provably side-effect-free.
/// String literals are intentionally excluded because they may be directive prologues
/// (e.g., "use strict") handled by a later pass.
fn is_pure_no_op_stmt(
    stmt: &Stmt,
    unresolved_mark: Mark,
    future_lexical: &HashSet<BindingId>,
) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    // Never drop string literals — may be "use strict" directives
    if matches!(expr.as_ref(), Expr::Lit(Lit::Str(_))) {
        return false;
    }
    // Never drop function/arrow/class expressions — they can represent
    // intentional wrapper patterns, and class evaluation can throw.
    if is_fn_arrow_or_class(expr) {
        return false;
    }
    // Identifier reads are observable: unresolved identifiers throw ReferenceError,
    // and lexical bindings can throw before initialization (TDZ).
    if is_observable_ident_read(expr, unresolved_mark, future_lexical) {
        return false;
    }
    if is_observable_typeof(expr, unresolved_mark) {
        return false;
    }
    if is_this_read(expr) {
        return false;
    }
    // Computed object literal keys perform ToPropertyKey even when the key
    // expression itself looks pure, and that coercion can throw.
    if has_computed_object_literal_key(expr) {
        return false;
    }
    let unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
    let ctx = ExprCtx {
        unresolved_ctxt,
        is_unresolved_ref_safe: false,
        in_strict: false,
        remaining_depth: 4,
    };
    !expr.may_have_side_effects(ctx)
}

fn is_observable_typeof(expr: &Expr, unresolved_mark: Mark) -> bool {
    match expr {
        Expr::Unary(unary) if unary.op == UnaryOp::TypeOf => {
            let Expr::Ident(ident) = unary.arg.as_ref() else {
                return false;
            };
            let unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
            ident.ctxt != unresolved_ctxt
        }
        Expr::Paren(paren) => is_observable_typeof(&paren.expr, unresolved_mark),
        _ => false,
    }
}

fn is_this_read(expr: &Expr) -> bool {
    match expr {
        Expr::This(_) => true,
        Expr::Paren(paren) => is_this_read(&paren.expr),
        _ => false,
    }
}

fn is_fn_arrow_or_class(expr: &Expr) -> bool {
    match expr {
        Expr::Fn(_) | Expr::Arrow(_) | Expr::Class(_) => true,
        Expr::Paren(paren) => is_fn_arrow_or_class(&paren.expr),
        _ => false,
    }
}

fn is_observable_ident_read(
    expr: &Expr,
    unresolved_mark: Mark,
    future_lexical: &HashSet<BindingId>,
) -> bool {
    match expr {
        Expr::Ident(ident) => {
            let unresolved_ctxt = SyntaxContext::empty().apply_mark(unresolved_mark);
            if ident.ctxt == unresolved_ctxt {
                return ident.sym.as_ref() != "undefined";
            }
            future_lexical.contains(&(ident.sym.clone(), ident.ctxt))
        }
        Expr::Paren(paren) => {
            is_observable_ident_read(&paren.expr, unresolved_mark, future_lexical)
        }
        _ => false,
    }
}

fn collect_lexical_decl_ids_from_module_items(items: &[ModuleItem]) -> HashSet<BindingId> {
    let mut ids = HashSet::new();
    for item in items {
        match item {
            ModuleItem::Stmt(stmt) => collect_lexical_decl_ids_from_stmt_into(stmt, &mut ids),
            ModuleItem::ModuleDecl(decl) => {
                collect_lexical_decl_ids_from_module_decl_into(decl, &mut ids)
            }
        }
    }
    ids
}

fn collect_lexical_decl_ids_from_stmts(stmts: &[Stmt]) -> HashSet<BindingId> {
    let mut ids = HashSet::new();
    for stmt in stmts {
        collect_lexical_decl_ids_from_stmt_into(stmt, &mut ids);
    }
    ids
}

fn collect_lexical_decl_ids_from_module_decl(decl: &ModuleDecl) -> HashSet<BindingId> {
    let mut ids = HashSet::new();
    collect_lexical_decl_ids_from_module_decl_into(decl, &mut ids);
    ids
}

fn collect_lexical_decl_ids_from_module_decl_into(decl: &ModuleDecl, ids: &mut HashSet<BindingId>) {
    if let ModuleDecl::ExportDecl(export) = decl {
        collect_lexical_decl_ids_from_decl(&export.decl, ids);
    }
}

fn collect_lexical_decl_ids_from_stmt(stmt: &Stmt) -> HashSet<BindingId> {
    let mut ids = HashSet::new();
    collect_lexical_decl_ids_from_stmt_into(stmt, &mut ids);
    ids
}

fn collect_lexical_decl_ids_from_stmt_into(stmt: &Stmt, ids: &mut HashSet<BindingId>) {
    if let Stmt::Decl(decl) = stmt {
        collect_lexical_decl_ids_from_decl(decl, ids);
    }
}

fn collect_lexical_decl_ids_from_decl(decl: &Decl, ids: &mut HashSet<BindingId>) {
    match decl {
        Decl::Var(var) if matches!(var.kind, VarDeclKind::Let | VarDeclKind::Const) => {
            for decl in &var.decls {
                collect_binding_ids_from_pat(&decl.name, ids);
            }
        }
        Decl::Class(class) => {
            ids.insert((class.ident.sym.clone(), class.ident.ctxt));
        }
        _ => {}
    }
}

fn collect_binding_ids_from_pat(pat: &Pat, ids: &mut HashSet<BindingId>) {
    match pat {
        Pat::Ident(ident) => {
            ids.insert((ident.id.sym.clone(), ident.id.ctxt));
        }
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_binding_ids_from_pat(elem, ids);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                        collect_binding_ids_from_pat(&kv.value, ids);
                    }
                    swc_core::ecma::ast::ObjectPatProp::Assign(assign) => {
                        ids.insert((assign.key.sym.clone(), assign.key.ctxt));
                    }
                    swc_core::ecma::ast::ObjectPatProp::Rest(rest) => {
                        collect_binding_ids_from_pat(&rest.arg, ids);
                    }
                }
            }
        }
        Pat::Rest(rest) => collect_binding_ids_from_pat(&rest.arg, ids),
        Pat::Assign(assign) => collect_binding_ids_from_pat(&assign.left, ids),
        _ => {}
    }
}

fn remove_ids(ids: &mut HashSet<BindingId>, remove: &HashSet<BindingId>) {
    for id in remove {
        ids.remove(id);
    }
}

fn has_computed_object_literal_key(expr: &Expr) -> bool {
    match expr {
        Expr::Object(obj) => obj.props.iter().any(prop_or_spread_has_computed_key),
        Expr::Paren(paren) => has_computed_object_literal_key(&paren.expr),
        _ => false,
    }
}

fn prop_or_spread_has_computed_key(prop: &PropOrSpread) -> bool {
    match prop {
        PropOrSpread::Spread(_) => false,
        PropOrSpread::Prop(prop) => prop_has_computed_key(prop),
    }
}

fn prop_has_computed_key(prop: &Prop) -> bool {
    match prop {
        Prop::KeyValue(kv) => matches!(kv.key, PropName::Computed(_)),
        Prop::Getter(getter) => matches!(getter.key, PropName::Computed(_)),
        Prop::Setter(setter) => matches!(setter.key, PropName::Computed(_)),
        Prop::Method(method) => matches!(method.key, PropName::Computed(_)),
        Prop::Shorthand(_) | Prop::Assign(_) => false,
    }
}

fn split_stmt(stmt: Stmt, level: RewriteLevel) -> Vec<Stmt> {
    match stmt {
        Stmt::Expr(ExprStmt { span, expr }) => {
            // Check assignment-member pattern: (a = expr)[prop] = val
            if let Some(stmts) = try_split_assign_member(&expr, span) {
                return stmts;
            }
            match *expr {
                Expr::Seq(SeqExpr { exprs, .. }) => exprs
                    .into_iter()
                    .map(|expr| Stmt::Expr(ExprStmt { span, expr }))
                    .collect(),
                Expr::Paren(paren) => split_expr_stmt_paren(paren, span),
                other => vec![Stmt::Expr(ExprStmt {
                    span,
                    expr: Box::new(other),
                })],
            }
        }
        Stmt::Return(ReturnStmt {
            span,
            arg: Some(arg),
        }) => split_return(span, arg),
        Stmt::Throw(ThrowStmt { span, arg }) => split_throw(span, arg),
        Stmt::If(if_stmt) => split_if(if_stmt, level),
        Stmt::Switch(switch_stmt) => split_switch(switch_stmt),
        Stmt::Decl(Decl::Var(var)) => split_var_decl(var, level),
        Stmt::For(for_stmt) => split_for_stmt(for_stmt, level),
        Stmt::ForIn(for_in_stmt) => split_for_in_stmt(for_in_stmt),
        Stmt::ForOf(for_of_stmt) => split_for_of_stmt(for_of_stmt),
        _ => vec![stmt],
    }
}

fn split_expr_stmt_paren(paren: ParenExpr, span: swc_core::common::Span) -> Vec<Stmt> {
    match *paren.expr {
        Expr::Seq(SeqExpr { exprs, .. }) => exprs
            .into_iter()
            .map(|expr| Stmt::Expr(ExprStmt { span, expr }))
            .collect(),
        inner => vec![Stmt::Expr(ExprStmt {
            span,
            expr: Box::new(Expr::Paren(ParenExpr {
                expr: Box::new(inner),
                ..paren
            })),
        })],
    }
}

// ---------------------------------------------------------------------------
// Assignment-member pattern: (a = expr)[prop] = val  →  a = expr; a[prop] = val
// ---------------------------------------------------------------------------

fn try_split_assign_member(expr: &Expr, span: swc_core::common::Span) -> Option<Vec<Stmt>> {
    let Expr::Assign(outer) = expr else {
        return None;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &outer.left else {
        return None;
    };
    // member.obj should be a (possibly paren-wrapped) assignment expr
    let obj = strip_paren(&member.obj);
    let Expr::Assign(inner) = obj else {
        return None;
    };
    // inner assign must assign to a simple ident
    let AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) = &inner.left else {
        return None;
    };

    let inner_stmt = Stmt::Expr(ExprStmt {
        span,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: inner.span,
            op: inner.op,
            left: inner.left.clone(),
            right: inner.right.clone(),
        })),
    });

    let new_member = MemberExpr {
        span: member.span,
        obj: Box::new(Expr::Ident(ident.id.clone())),
        prop: member.prop.clone(),
    };
    let outer_stmt = Stmt::Expr(ExprStmt {
        span,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: outer.span,
            op: outer.op,
            left: AssignTarget::Simple(SimpleAssignTarget::Member(new_member)),
            right: outer.right.clone(),
        })),
    });

    Some(vec![inner_stmt, outer_stmt])
}

fn strip_paren(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => strip_paren(&paren.expr),
        _ => expr,
    }
}

// ---------------------------------------------------------------------------
// Variable declaration: split by declarator, extract sequence inits
// ---------------------------------------------------------------------------

fn split_var_decl(var: Box<VarDecl>, level: RewriteLevel) -> Vec<Stmt> {
    let span = var.span;
    let kind = var.kind;
    let ctxt = var.ctxt;
    let mut result = Vec::new();

    for decl in var.decls {
        if let Some(init) = decl.init {
            if level == RewriteLevel::Minimal && sequence_blocks_decl_name_inference(&init) {
                result.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
                    span,
                    ctxt,
                    kind,
                    declare: false,
                    decls: vec![VarDeclarator {
                        span: decl.span,
                        name: decl.name,
                        init: Some(init),
                        definite: decl.definite,
                    }],
                }))));
                continue;
            }
            let (prefix, last) = split_expr_seq(init);
            for expr in prefix {
                result.push(Stmt::Expr(ExprStmt { span, expr }));
            }
            result.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
                span,
                ctxt,
                kind,
                declare: false,
                decls: vec![VarDeclarator {
                    span: decl.span,
                    name: decl.name,
                    init: Some(last),
                    definite: decl.definite,
                }],
            }))));
        } else {
            result.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
                span,
                ctxt,
                kind,
                declare: false,
                decls: vec![decl],
            }))));
        }
    }

    result
}

// ---------------------------------------------------------------------------
// For loop: extract sequence from init expression
// ---------------------------------------------------------------------------

fn split_for_stmt(mut for_stmt: ForStmt, level: RewriteLevel) -> Vec<Stmt> {
    let mut prefix = Vec::new();

    if let Some(init) = for_stmt.init.take() {
        match init {
            VarDeclOrExpr::Expr(expr) => {
                let (pre, last) = split_expr_seq(expr);
                if pre.is_empty() {
                    // Not a sequence — restore unchanged
                    for_stmt.init = Some(VarDeclOrExpr::Expr(last));
                } else {
                    for p in pre {
                        prefix.push(Stmt::Expr(ExprStmt {
                            span: for_stmt.span,
                            expr: p,
                        }));
                    }
                    // Keep last as init only if it's an assignment expression
                    if is_assign_expr(&last) {
                        for_stmt.init = Some(VarDeclOrExpr::Expr(last));
                    } else {
                        prefix.push(Stmt::Expr(ExprStmt {
                            span: for_stmt.span,
                            expr: last,
                        }));
                        // for_stmt.init stays None
                    }
                }
            }
            VarDeclOrExpr::VarDecl(var) => {
                let (extracted, new_var) = extract_var_decl_prefix(var, for_stmt.span, level);
                prefix.extend(extracted);
                for_stmt.init = Some(VarDeclOrExpr::VarDecl(new_var));
            }
        }
    }

    if prefix.is_empty() {
        return vec![Stmt::For(for_stmt)];
    }

    prefix.push(Stmt::For(for_stmt));
    prefix
}

/// Extract sequence prefixes from each declarator's init, without splitting
/// the var decl into individual declarations (needed for for-loop scope).
fn extract_var_decl_prefix(
    var: Box<VarDecl>,
    span: swc_core::common::Span,
    level: RewriteLevel,
) -> (Vec<Stmt>, Box<VarDecl>) {
    let kind = var.kind;
    let ctxt = var.ctxt;
    let var_span = var.span;
    let mut prefix = Vec::new();
    let mut new_decls = Vec::new();

    for decl in var.decls {
        if let Some(init) = decl.init {
            if level == RewriteLevel::Minimal && sequence_blocks_decl_name_inference(&init) {
                new_decls.push(VarDeclarator {
                    span: decl.span,
                    name: decl.name,
                    init: Some(init),
                    definite: decl.definite,
                });
                continue;
            }
            let (pre, last) = split_expr_seq(init);
            for p in pre {
                prefix.push(Stmt::Expr(ExprStmt { span, expr: p }));
            }
            new_decls.push(VarDeclarator {
                span: decl.span,
                name: decl.name,
                init: Some(last),
                definite: decl.definite,
            });
        } else {
            new_decls.push(decl);
        }
    }

    let new_var = Box::new(VarDecl {
        span: var_span,
        ctxt,
        kind,
        declare: false,
        decls: new_decls,
    });

    (prefix, new_var)
}

fn sequence_blocks_decl_name_inference(expr: &Expr) -> bool {
    let expr = match expr {
        Expr::Paren(paren) => paren.expr.as_ref(),
        other => other,
    };
    let Expr::Seq(seq) = expr else {
        return false;
    };
    let Some(last) = seq.exprs.last() else {
        return false;
    };
    is_anonymous_function_or_class(last)
}

fn is_anonymous_function_or_class(expr: &Expr) -> bool {
    match expr {
        Expr::Fn(fn_expr) => fn_expr.ident.is_none(),
        Expr::Class(class_expr) => class_expr.ident.is_none(),
        Expr::Paren(paren) => is_anonymous_function_or_class(&paren.expr),
        _ => false,
    }
}

fn is_assign_expr(expr: &Box<Expr>) -> bool {
    matches!(**expr, Expr::Assign(_))
}

// ---------------------------------------------------------------------------
// For-in / For-of: extract sequence from the iterable expression
// ---------------------------------------------------------------------------

fn split_for_in_stmt(mut stmt: ForInStmt) -> Vec<Stmt> {
    if for_head_has_lexical_decl(&stmt.left) {
        return vec![Stmt::ForIn(stmt)];
    }
    let dummy = Box::new(Expr::Invalid(Invalid { span: DUMMY_SP }));
    let right = std::mem::replace(&mut stmt.right, dummy);
    let (pre, last) = split_expr_seq(right);
    stmt.right = last;
    if pre.is_empty() {
        return vec![Stmt::ForIn(stmt)];
    }
    let mut result: Vec<Stmt> = pre
        .into_iter()
        .map(|e| {
            Stmt::Expr(ExprStmt {
                span: stmt.span,
                expr: e,
            })
        })
        .collect();
    result.push(Stmt::ForIn(stmt));
    result
}

fn split_for_of_stmt(mut stmt: ForOfStmt) -> Vec<Stmt> {
    if for_head_has_lexical_decl(&stmt.left) {
        return vec![Stmt::ForOf(stmt)];
    }
    let dummy = Box::new(Expr::Invalid(Invalid { span: DUMMY_SP }));
    let right = std::mem::replace(&mut stmt.right, dummy);
    let (pre, last) = split_expr_seq(right);
    stmt.right = last;
    if pre.is_empty() {
        return vec![Stmt::ForOf(stmt)];
    }
    let mut result: Vec<Stmt> = pre
        .into_iter()
        .map(|e| {
            Stmt::Expr(ExprStmt {
                span: stmt.span,
                expr: e,
            })
        })
        .collect();
    result.push(Stmt::ForOf(stmt));
    result
}

fn for_head_has_lexical_decl(head: &ForHead) -> bool {
    matches!(
        head,
        ForHead::VarDecl(var) if matches!(var.kind, VarDeclKind::Let | VarDeclKind::Const)
    )
}

// ---------------------------------------------------------------------------
// Existing helpers
// ---------------------------------------------------------------------------

fn split_return(span: swc_core::common::Span, arg: Box<Expr>) -> Vec<Stmt> {
    let (prefix, last) = split_expr_seq(arg);
    if prefix.is_empty() {
        return vec![Stmt::Return(ReturnStmt {
            span,
            arg: Some(last),
        })];
    }
    let mut stmts = expr_stmts(span, prefix);
    stmts.push(Stmt::Return(ReturnStmt {
        span,
        arg: Some(last),
    }));
    stmts
}

fn split_throw(span: swc_core::common::Span, arg: Box<Expr>) -> Vec<Stmt> {
    let (prefix, last) = split_expr_seq(arg);
    if prefix.is_empty() {
        return vec![Stmt::Throw(ThrowStmt { span, arg: last })];
    }
    let mut stmts = expr_stmts(span, prefix);
    stmts.push(Stmt::Throw(ThrowStmt { span, arg: last }));
    stmts
}

fn split_if(mut if_stmt: IfStmt, level: RewriteLevel) -> Vec<Stmt> {
    if_stmt.cons = normalize_branch_stmt(*if_stmt.cons, level);
    if let Some(alt) = if_stmt.alt.take() {
        if_stmt.alt = Some(normalize_branch_stmt(*alt, level));
    }

    let (prefix, last_test) = split_expr_seq(if_stmt.test.clone());
    if prefix.is_empty() {
        return vec![Stmt::If(if_stmt)];
    }

    if_stmt.test = last_test;

    let mut stmts = expr_stmts(if_stmt.span, prefix);
    stmts.push(Stmt::If(if_stmt));
    stmts
}

fn split_switch(mut switch_stmt: SwitchStmt) -> Vec<Stmt> {
    let (prefix, last_discriminant) = split_expr_seq(switch_stmt.discriminant.clone());
    if prefix.is_empty() {
        return vec![Stmt::Switch(switch_stmt)];
    }

    switch_stmt.discriminant = last_discriminant;

    let mut stmts = expr_stmts(switch_stmt.span, prefix);
    stmts.push(Stmt::Switch(switch_stmt));
    stmts
}

fn normalize_branch_stmt(stmt: Stmt, level: RewriteLevel) -> Box<Stmt> {
    let mut split = split_stmt(stmt, level);
    if split.len() == 1 {
        Box::new(split.pop().expect("length checked"))
    } else {
        Box::new(Stmt::Block(BlockStmt {
            span: DUMMY_SP,
            ctxt: Default::default(),
            stmts: split,
        }))
    }
}

fn split_expr_seq(expr: Box<Expr>) -> (Vec<Box<Expr>>, Box<Expr>) {
    match *expr {
        Expr::Paren(paren) => split_expr_seq(paren.expr),
        Expr::Seq(SeqExpr { mut exprs, .. }) => {
            if exprs.len() <= 1 {
                let only = exprs
                    .pop()
                    .expect("sequence expressions should be non-empty");
                (Vec::new(), only)
            } else {
                let last = exprs.pop().expect("sequence length checked");
                (exprs, last)
            }
        }
        other => (Vec::new(), Box::new(other)),
    }
}

fn expr_stmts(span: swc_core::common::Span, exprs: Vec<Box<Expr>>) -> Vec<Stmt> {
    exprs
        .into_iter()
        .map(|expr| Stmt::Expr(ExprStmt { span, expr }))
        .collect()
}
