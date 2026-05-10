use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    ArrowExpr, AssignOp, AssignTarget, BindingIdent, BlockStmt, Callee, Expr, Function, Ident,
    MemberProp, ObjectPatProp, Pat, SimpleAssignTarget, Stmt, UpdateOp, VarDeclOrExpr,
    VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

/// Eliminates the Babel-compiled rest-args array-copy loop.
///
/// Babel compiles `function foo(...args) { … }` to an ES5 form:
/// ```js
/// function foo() {
///     for (var _len = arguments.length, _args = Array(_len), _key = 0; _key < _len; _key++)
///         _args[_key] = arguments[_key];
///     …
/// }
/// ```
///
/// After `ArgRest` converts `arguments` → `...args`, the copy is redundant — `_args` is
/// just a copy of the rest array:
/// ```js
/// function foo(...args) {
///     for (let _len = args.length, _args = Array(_len), _key = 0; _key < _len; _key++) {
///         _args[_key] = args[_key];
///     }
///     …
/// }
/// ```
///
/// This rule removes that loop and replaces every reference to `_args` with `args`.
///
/// # Nested-function name collision
///
/// Because `ArgRest` uses the same name (`args`, ctxt = empty) for every function it
/// transforms, two nested functions that both receive `...args` may shadow each other.
/// If the outer function's copy variable is referenced inside a nested function that
/// also binds the replacement name, replacing it there would silently change which
/// `args` the expression refers to.  In that situation the outer transformation is
/// skipped to preserve semantics.
pub struct UnRestArrayCopy;

impl VisitMut for UnRestArrayCopy {
    fn visit_mut_function(&mut self, func: &mut Function) {
        // Bottom-up: handle nested functions first
        func.visit_mut_children_with(self);

        let Some(rest) = get_rest_param(func) else {
            return;
        };
        let Some(body) = &mut func.body else { return };

        // There may be multiple copy loops (unlikely but possible)
        loop {
            let Some((loop_idx, copy)) = find_copy_loop(body, &rest) else {
                break;
            };

            // Safety: if the copy var is referenced inside a nested scope that
            // rebinds the rest name, replacing it there would pick up the wrong
            // `args` binding.  Skip the whole transformation in that case.
            if copy_escapes_into_rebinding_scope(body, &copy, &rest.0) {
                break;
            }

            body.stmts.remove(loop_idx);
            let mut replacer = IdentReplacer {
                from: copy,
                to: rest.clone(),
            };
            body.visit_mut_with(&mut replacer);
        }
    }
}

type BindingId = (Atom, SyntaxContext);

/// Return the `(sym, ctxt)` of the function's rest parameter, if any.
fn get_rest_param(func: &Function) -> Option<BindingId> {
    func.params.iter().rev().find_map(|p| {
        if let Pat::Rest(rest) = &p.pat {
            if let Pat::Ident(BindingIdent { id, .. }) = rest.arg.as_ref() {
                return Some((id.sym.clone(), id.ctxt));
            }
        }
        None
    })
}

/// Scan `body` for the Babel rest-args copy pattern whose source matches `rest`.
/// Returns `(statement_index, copy_binding_id)` on the first match.
fn find_copy_loop(body: &BlockStmt, rest: &BindingId) -> Option<(usize, BindingId)> {
    body.stmts
        .iter()
        .enumerate()
        .find_map(|(i, stmt)| match_copy_loop(stmt, rest).map(|copy| (i, copy)))
}

/// Match:
/// ```text
/// for (let len = REST.length, copy = Array(len), idx = 0; idx < len; idx++) {
///     copy[idx] = REST[idx];
/// }
/// ```
/// where `REST` has the given `BindingId`. Returns the `BindingId` of `copy`.
fn match_copy_loop(stmt: &Stmt, rest: &BindingId) -> Option<BindingId> {
    let Stmt::For(for_stmt) = stmt else {
        return None;
    };

    // Init: var/let/const declaration with exactly 3 declarators
    let Some(VarDeclOrExpr::VarDecl(init)) = &for_stmt.init else {
        return None;
    };
    if init.decls.len() != 3 {
        return None;
    }

    // Decl 0: len = rest.length
    let (len_sym, src) = extract_len_decl(&init.decls[0])?;
    if src != *rest {
        return None;
    }

    // Decl 1: copy = Array(len) or new Array(len)
    let copy = extract_array_copy_decl(&init.decls[1], &len_sym)?;

    // Decl 2: idx = 0
    let idx_sym = extract_zero_init_decl(&init.decls[2])?;

    // Test: idx < len
    if !matches_lt_test(for_stmt.test.as_deref(), &idx_sym, &len_sym) {
        return None;
    }

    // Update: idx++ or ++idx
    if !matches_increment(for_stmt.update.as_deref(), &idx_sym) {
        return None;
    }

    // Body: copy[idx] = rest[idx]  (bare or inside a block)
    if !matches_copy_body(&for_stmt.body, &copy.0, &idx_sym, &rest.0) {
        return None;
    }

    Some(copy)
}

// ── per-declarator extractors ────────────────────────────────────────────────

/// `len = src.length`  →  `(len_sym, src_binding_id)`
fn extract_len_decl(decl: &VarDeclarator) -> Option<(Atom, BindingId)> {
    let Pat::Ident(BindingIdent { id: len_id, .. }) = &decl.name else {
        return None;
    };
    let Expr::Member(member) = decl.init.as_deref()? else {
        return None;
    };
    let Expr::Ident(src_id) = member.obj.as_ref() else {
        return None;
    };
    if !matches!(&member.prop, MemberProp::Ident(p) if p.sym == "length") {
        return None;
    }
    Some((len_id.sym.clone(), (src_id.sym.clone(), src_id.ctxt)))
}

/// `copy = Array(len)` or `copy = new Array(len)`  →  `copy_binding_id`
fn extract_array_copy_decl(decl: &VarDeclarator, len_sym: &Atom) -> Option<BindingId> {
    let Pat::Ident(BindingIdent { id: copy_id, .. }) = &decl.name else {
        return None;
    };

    let is_array_ctor = |sym: &Atom| sym == "Array";
    let one_len_arg = |args: &[swc_core::ecma::ast::ExprOrSpread]| -> bool {
        args.len() == 1
            && args[0].spread.is_none()
            && matches!(args[0].expr.as_ref(), Expr::Ident(id) if &id.sym == len_sym)
    };

    match decl.init.as_deref()? {
        Expr::Call(call) => {
            let Callee::Expr(callee) = &call.callee else {
                return None;
            };
            let Expr::Ident(id) = callee.as_ref() else {
                return None;
            };
            if !is_array_ctor(&id.sym) || !one_len_arg(&call.args) {
                return None;
            }
        }
        Expr::New(new_expr) => {
            let Expr::Ident(id) = new_expr.callee.as_ref() else {
                return None;
            };
            if !is_array_ctor(&id.sym) {
                return None;
            }
            let args = new_expr.args.as_deref().unwrap_or(&[]);
            if !one_len_arg(args) {
                return None;
            }
        }
        _ => return None,
    }

    Some((copy_id.sym.clone(), copy_id.ctxt))
}

/// `idx = 0`  →  `idx_sym`
fn extract_zero_init_decl(decl: &VarDeclarator) -> Option<Atom> {
    let Pat::Ident(BindingIdent { id, .. }) = &decl.name else {
        return None;
    };
    match decl.init.as_deref()? {
        Expr::Lit(swc_core::ecma::ast::Lit::Num(n)) if n.value == 0.0 => {}
        _ => return None,
    }
    Some(id.sym.clone())
}

// ── condition / update / body matchers ──────────────────────────────────────

fn matches_lt_test(test: Option<&Expr>, idx_sym: &Atom, len_sym: &Atom) -> bool {
    let Some(Expr::Bin(bin)) = test else {
        return false;
    };
    bin.op == swc_core::ecma::ast::BinaryOp::Lt
        && matches!(bin.left.as_ref(), Expr::Ident(id) if &id.sym == idx_sym)
        && matches!(bin.right.as_ref(), Expr::Ident(id) if &id.sym == len_sym)
}

fn matches_increment(update: Option<&Expr>, idx_sym: &Atom) -> bool {
    let Some(Expr::Update(upd)) = update else {
        return false;
    };
    upd.op == UpdateOp::PlusPlus
        && matches!(upd.arg.as_ref(), Expr::Ident(id) if &id.sym == idx_sym)
}

/// Body is `copy[idx] = src[idx]` — either as a bare ExprStmt or inside a block.
fn matches_copy_body(body: &Stmt, copy_sym: &Atom, idx_sym: &Atom, src_sym: &Atom) -> bool {
    let expr = match body {
        Stmt::Expr(e) => e.expr.as_ref(),
        Stmt::Block(block) => {
            if block.stmts.len() != 1 {
                return false;
            }
            let Stmt::Expr(e) = &block.stmts[0] else {
                return false;
            };
            e.expr.as_ref()
        }
        _ => return false,
    };

    let Expr::Assign(assign) = expr else {
        return false;
    };
    if assign.op != AssignOp::Assign {
        return false;
    }

    // left: copy[idx]
    let AssignTarget::Simple(SimpleAssignTarget::Member(lm)) = &assign.left else {
        return false;
    };
    if !matches!(lm.obj.as_ref(), Expr::Ident(id) if &id.sym == copy_sym) {
        return false;
    }
    let MemberProp::Computed(lp) = &lm.prop else {
        return false;
    };
    if !matches!(lp.expr.as_ref(), Expr::Ident(id) if &id.sym == idx_sym) {
        return false;
    }

    // right: src[idx]
    let Expr::Member(rm) = assign.right.as_ref() else {
        return false;
    };
    if !matches!(rm.obj.as_ref(), Expr::Ident(id) if &id.sym == src_sym) {
        return false;
    }
    let MemberProp::Computed(rp) = &rm.prop else {
        return false;
    };
    matches!(rp.expr.as_ref(), Expr::Ident(id) if &id.sym == idx_sym)
}

// ── conflict detection ───────────────────────────────────────────────────────

/// Returns `true` if `copy` is referenced inside any nested function/arrow whose
/// param list rebinds `to_sym`.
///
/// In that situation replacing `copy` with `to_sym` would silently resolve to the
/// inner binding rather than the outer rest param, producing wrong semantics.
fn copy_escapes_into_rebinding_scope(body: &BlockStmt, copy: &BindingId, to_sym: &Atom) -> bool {
    let mut checker = EscapeChecker {
        copy: copy.clone(),
        to_sym: to_sym.clone(),
        in_rebinding_scope: false,
        found: false,
    };
    body.visit_with(&mut checker);
    checker.found
}

struct EscapeChecker {
    copy: BindingId,
    to_sym: Atom,
    in_rebinding_scope: bool,
    found: bool,
}

impl EscapeChecker {
    fn with_scope<F: FnOnce(&mut Self)>(&mut self, rebinds: bool, f: F) {
        let old = self.in_rebinding_scope;
        if rebinds {
            self.in_rebinding_scope = true;
        }
        f(self);
        self.in_rebinding_scope = old;
    }
}

impl Visit for EscapeChecker {
    fn visit_ident(&mut self, id: &Ident) {
        if self.in_rebinding_scope && id.sym == self.copy.0 && id.ctxt == self.copy.1 {
            self.found = true;
        }
    }

    fn visit_function(&mut self, func: &Function) {
        let rebinds = func
            .params
            .iter()
            .any(|p| pat_binds_sym(&p.pat, &self.to_sym));
        self.with_scope(rebinds, |s| func.visit_children_with(s));
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        let rebinds = arrow.params.iter().any(|p| pat_binds_sym(p, &self.to_sym));
        self.with_scope(rebinds, |s| arrow.visit_children_with(s));
    }
}

/// True if `pat` introduces a binding named `sym` (including rest, assign, nested).
fn pat_binds_sym(pat: &Pat, sym: &Atom) -> bool {
    match pat {
        Pat::Ident(bi) => &bi.id.sym == sym,
        Pat::Rest(r) => pat_binds_sym(&r.arg, sym),
        Pat::Assign(a) => pat_binds_sym(&a.left, sym),
        Pat::Array(arr) => arr.elems.iter().flatten().any(|e| pat_binds_sym(e, sym)),
        Pat::Object(obj) => obj.props.iter().any(|p| match p {
            ObjectPatProp::Assign(a) => &a.key.sym == sym,
            ObjectPatProp::KeyValue(kv) => pat_binds_sym(&kv.value, sym),
            ObjectPatProp::Rest(r) => pat_binds_sym(&r.arg, sym),
        }),
        Pat::Expr(_) | Pat::Invalid(_) => false,
    }
}

// ── ident replacer ───────────────────────────────────────────────────────────

/// Replaces every `Ident` matching `from` (sym + ctxt) with `to` (sym + ctxt).
/// Stops descending into nested functions/arrows that rebind `to.sym`, because
/// inside such scopes the replacement name resolves to the inner binding.
struct IdentReplacer {
    from: BindingId,
    to: BindingId,
}

impl VisitMut for IdentReplacer {
    fn visit_mut_ident(&mut self, id: &mut Ident) {
        if id.sym == self.from.0 && id.ctxt == self.from.1 {
            id.sym = self.to.0.clone();
            id.ctxt = self.to.1;
        }
    }

    fn visit_mut_function(&mut self, func: &mut Function) {
        if func
            .params
            .iter()
            .any(|p| pat_binds_sym(&p.pat, &self.to.0))
        {
            return; // to is rebound — inner references resolve to a different binding
        }
        func.visit_mut_children_with(self);
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        if arrow.params.iter().any(|p| pat_binds_sym(p, &self.to.0)) {
            return;
        }
        arrow.visit_mut_children_with(self);
    }
}
