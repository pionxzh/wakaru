use std::collections::HashSet;

use swc_core::ecma::ast::{
    Decl, ExportDecl, Expr, ForStmt, MemberExpr, ModuleDecl, ModuleItem, Pat, Stmt, VarDecl,
    VarDeclKind, VarDeclOrExpr, VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnVariableMerging;

impl VisitMut for UnVariableMerging {
    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);

        let old = std::mem::take(stmts);
        let mut new_stmts: Vec<Stmt> = Vec::with_capacity(old.len());

        for stmt in old {
            match stmt {
                // For-loop with a var init: maybe extract some declarators before the loop
                Stmt::For(for_stmt) => {
                    let extracted = extract_for_init_stmts(for_stmt);
                    new_stmts.extend(extracted);
                }
                // Plain var/let/const declaration with multiple declarators
                Stmt::Decl(Decl::Var(ref var_decl)) if var_decl.decls.len() > 1 => {
                    let split = split_var_decl(var_decl);
                    new_stmts.extend(
                        split
                            .into_iter()
                            .map(|v| Stmt::Decl(Decl::Var(Box::new(v)))),
                    );
                }
                other => new_stmts.push(other),
            }
        }

        *stmts = new_stmts;
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        let old = std::mem::take(items);
        let mut new_items: Vec<ModuleItem> = Vec::with_capacity(old.len());

        for item in old {
            match item {
                // export var a = 1, b = 2 → export var a = 1; export var b = 2;
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                    span,
                    decl: Decl::Var(ref var_decl),
                })) if var_decl.decls.len() > 1 => {
                    let split = split_var_decl(var_decl);
                    for v in split {
                        new_items.push(ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(
                            ExportDecl {
                                span,
                                decl: Decl::Var(Box::new(v)),
                            },
                        )));
                    }
                }
                // Plain statement with multiple var declarators
                ModuleItem::Stmt(Stmt::Decl(Decl::Var(ref var_decl)))
                    if var_decl.decls.len() > 1 =>
                {
                    let split = split_var_decl(var_decl);
                    for v in split {
                        new_items.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(v)))));
                    }
                }
                // For-loop inside module items
                ModuleItem::Stmt(Stmt::For(for_stmt)) => {
                    let extracted = extract_for_init_stmts(for_stmt);
                    for s in extracted {
                        new_items.push(ModuleItem::Stmt(s));
                    }
                }
                other => new_items.push(other),
            }
        }

        *items = new_items;
    }
}

/// Split a `VarDecl` with multiple declarators into multiple single-declarator `VarDecl`s.
fn split_var_decl(var_decl: &VarDecl) -> Vec<VarDecl> {
    var_decl
        .decls
        .iter()
        .map(|d| VarDecl {
            span: var_decl.span,
            ctxt: var_decl.ctxt,
            kind: var_decl.kind,
            declare: var_decl.declare,
            decls: vec![d.clone()],
        })
        .collect()
}

/// For `for (var a = 1, b = 2, c = 3; ...)`, extract declarators whose names do NOT appear
/// in the test or update expressions, and place them as separate `var` statements before
/// the for loop. Only applies to `var` kind (not `let` / `const`).
///
/// Returns a list of statements: [extracted vars..., modified for stmt].
fn extract_for_init_stmts(mut for_stmt: ForStmt) -> Vec<Stmt> {
    // Only act on `var` inits (any number of declarators)
    let should_process = matches!(
        &for_stmt.init,
        Some(VarDeclOrExpr::VarDecl(v))
            if v.kind == VarDeclKind::Var && !v.decls.is_empty()
    );

    if !should_process {
        return vec![Stmt::For(for_stmt)];
    }

    // Collect identifiers referenced in test and update (not body)
    let mut used_names: HashSet<String> = HashSet::new();
    if let Some(test) = &for_stmt.test {
        collect_ident_names_expr(test, &mut used_names);
    }
    if let Some(update) = &for_stmt.update {
        collect_ident_names_expr(update, &mut used_names);
    }

    // Take the VarDecl out of the for init
    let init_var = match std::mem::take(&mut for_stmt.init) {
        Some(VarDeclOrExpr::VarDecl(v)) => v,
        other => {
            for_stmt.init = other;
            return vec![Stmt::For(for_stmt)];
        }
    };

    let VarDecl {
        span,
        ctxt,
        kind,
        declare,
        decls,
    } = *init_var;

    // Phase 1: determine which declarator indices must stay in the for init.
    // Start with those whose bound names appear in test/update.
    let mut must_keep: HashSet<usize> = HashSet::new();
    for (i, decl) in decls.iter().enumerate() {
        let names = bound_names_pat(&decl.name);
        if names.iter().any(|n| used_names.contains(n)) {
            must_keep.insert(i);
        }
    }

    // Expand: any declarator whose init references a name bound by a must-keep declarator
    // must also stay. These cannot be safely extracted because var→let/const conversion
    // removes hoisting, and the referenced variable is only initialized at loop start.
    // Repeat until stable (handles transitive dependencies).
    loop {
        let keep_names: HashSet<String> = must_keep
            .iter()
            .flat_map(|&i| bound_names_pat(&decls[i].name))
            .collect();

        let mut changed = false;
        for (i, decl) in decls.iter().enumerate() {
            if must_keep.contains(&i) {
                continue;
            }
            let mut init_refs = HashSet::new();
            if let Some(init) = &decl.init {
                collect_ident_names_expr(init, &mut init_refs);
            }
            if init_refs.iter().any(|n| keep_names.contains(n)) {
                must_keep.insert(i);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Phase 2: partition declarators into keep/extract, preserving original order.
    let mut keep_in_for: Vec<VarDeclarator> = Vec::new();
    let mut extract_before: Vec<VarDeclarator> = Vec::new();
    for (i, decl) in decls.into_iter().enumerate() {
        if must_keep.contains(&i) {
            keep_in_for.push(decl);
        } else {
            extract_before.push(decl);
        }
    }

    let mut result: Vec<Stmt> = Vec::new();

    // Emit extracted declarators as individual var statements before the loop
    for d in extract_before {
        result.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
            span,
            ctxt,
            kind,
            declare,
            decls: vec![d],
        }))));
    }

    // Rebuild the for statement's init
    if keep_in_for.is_empty() {
        for_stmt.init = None;
    } else {
        for_stmt.init = Some(VarDeclOrExpr::VarDecl(Box::new(VarDecl {
            span,
            ctxt,
            kind,
            declare,
            decls: keep_in_for,
        })));
    }

    result.push(Stmt::For(for_stmt));
    result
}

/// Collect all identifier names referenced (not bound) in an expression.
fn collect_ident_names_expr(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Ident(i) => {
            out.insert(i.sym.to_string());
        }
        Expr::Member(MemberExpr { obj, prop, .. }) => {
            collect_ident_names_expr(obj, out);
            // Only computed props introduce new ident refs; static props are property names
            if let swc_core::ecma::ast::MemberProp::Computed(c) = prop {
                collect_ident_names_expr(&c.expr, out);
            }
        }
        Expr::Call(c) => {
            if let swc_core::ecma::ast::Callee::Expr(e) = &c.callee {
                collect_ident_names_expr(e, out);
            }
            for arg in &c.args {
                collect_ident_names_expr(&arg.expr, out);
            }
        }
        Expr::Bin(b) => {
            collect_ident_names_expr(&b.left, out);
            collect_ident_names_expr(&b.right, out);
        }
        Expr::Unary(u) => collect_ident_names_expr(&u.arg, out),
        Expr::Update(u) => collect_ident_names_expr(&u.arg, out),
        Expr::Assign(a) => {
            collect_ident_names_expr(&a.right, out);
        }
        Expr::Seq(s) => {
            for e in &s.exprs {
                collect_ident_names_expr(e, out);
            }
        }
        Expr::Cond(c) => {
            collect_ident_names_expr(&c.test, out);
            collect_ident_names_expr(&c.cons, out);
            collect_ident_names_expr(&c.alt, out);
        }
        Expr::New(n) => {
            collect_ident_names_expr(&n.callee, out);
            if let Some(args) = &n.args {
                for arg in args {
                    collect_ident_names_expr(&arg.expr, out);
                }
            }
        }
        Expr::Array(a) => {
            for elem in a.elems.iter().flatten() {
                collect_ident_names_expr(&elem.expr, out);
            }
        }
        Expr::Object(o) => {
            for prop in &o.props {
                if let swc_core::ecma::ast::PropOrSpread::Prop(p) = prop {
                    match p.as_ref() {
                        swc_core::ecma::ast::Prop::KeyValue(kv) => {
                            collect_ident_names_expr(&kv.value, out);
                        }
                        swc_core::ecma::ast::Prop::Shorthand(i) => {
                            out.insert(i.sym.to_string());
                        }
                        _ => {}
                    }
                }
            }
        }
        Expr::Paren(p) => collect_ident_names_expr(&p.expr, out),
        _ => {}
    }
}

/// Collect all identifier names bound in a pattern (the LHS of a declarator).
fn bound_names_pat(pat: &Pat) -> Vec<String> {
    let mut names = Vec::new();
    collect_bound_names(pat, &mut names);
    names
}

fn collect_bound_names(pat: &Pat, out: &mut Vec<String>) {
    match pat {
        Pat::Ident(i) => out.push(i.sym.to_string()),
        Pat::Array(a) => {
            for elem in a.elems.iter().flatten() {
                collect_bound_names(elem, out);
            }
        }
        Pat::Object(o) => {
            for prop in &o.props {
                match prop {
                    swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                        collect_bound_names(&kv.value, out);
                    }
                    swc_core::ecma::ast::ObjectPatProp::Assign(a) => {
                        out.push(a.key.sym.to_string());
                    }
                    swc_core::ecma::ast::ObjectPatProp::Rest(r) => {
                        collect_bound_names(&r.arg, out);
                    }
                }
            }
        }
        Pat::Rest(r) => collect_bound_names(&r.arg, out),
        Pat::Assign(a) => collect_bound_names(&a.left, out),
        _ => {}
    }
}
