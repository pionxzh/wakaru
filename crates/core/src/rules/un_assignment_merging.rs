use crate::collections::HashSet;
use crate::utils::paren::strip_parens;
use crate::utils::prototype_members::is_prototype_mutating_member_name;

use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, Expr, ExprStmt, Ident, Lit, MemberExpr, MemberProp, Module,
    ModuleItem, SimpleAssignTarget, Stmt, UnaryOp,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

/// Splits `a = b = value` into one assignment statement per target.
///
/// A chained assignment evaluates every target reference from the outermost
/// to the innermost, then evaluates the value, then writes from the innermost
/// to the outermost. The split keeps that write order (`b = value; a = value`)
/// so a throwing write or an own setter still observes the same prefix of
/// writes.
///
/// Splitting also moves each target's reference evaluation next to its write.
/// That is only invisible when evaluating the reference can neither throw nor
/// change between the writes, so the rule accepts exactly these targets:
///
/// - a plain identifier (resolving a name has no effect; a TDZ or `const`
///   violation throws at the write, whose order is preserved);
/// - a member rooted at the CommonJS wrapper bindings `module`, `exports`, or
///   `require`, which the wrapper always defines (`exports.foo = exports.bar
///   = 1`, `module.exports = exports = fn`).
/// - a chain consisting only of static `module.exports.name` stores, under
///   `commonjs_exports_data_properties`. These stores cannot replace the
///   `module.exports` slot. Mixed roots, root writes, prototype mutations, and
///   the `exports` key (which could replace the slot if module.exports aliases
///   module itself) are excluded. No receiver capture is introduced.
///
/// Keys must be identifiers, private names, or string/number literals. Any
/// other target keeps the chain: a local root may be in TDZ or be reassigned
/// by an inner setter, `this` throws before `super()` in a derived
/// constructor, a computed key may run code, a call receiver reorders calls.
/// There is no level gating.
///
/// The split also evaluates the value once per target instead of once. A
/// primitive literal or `void <number>` cannot change, but an identifier can
/// if an earlier write runs user
/// code that reassigns it (`o.a = o.b = value` with a `b` setter). An
/// identifier value is therefore accepted only when the writes are resolved
/// identifier targets, which run no code, or the CommonJS targets above. The
/// latter is an accepted assumption, not a guarantee: a module can install an
/// accessor on its own `exports` object, and such a setter reassigning the
/// value binding would make the second read differ. Compiler-emitted export
/// chains never do this, so `commonjs_exports_data_properties` covers it. An
/// undeclared global identifier target may be an accessor on the global
/// object, so it splits only with a literal value.
///
/// Other member chains remain intact. Minifiers only form a chain
/// when the inner target is an identifier (`a = v; b.c = a` becomes
/// `b.c = a = v`; swc's `merge_sequential_expr` requires `as_ident()` on the
/// merged target), which the identifier case above reverses. Member-only
/// chains such as `o.x = o.y = v`, flag tables `t[A] = t[B] = true`, and
/// TypeScript's synthesized `exports.A = exports.B = void 0` are how the
/// source was written or generated. CommonJS initialization chains are split
/// so UnEsm can classify every export in one pass, even when RemoveVoid must
/// retain `void 0` because of dynamic scope. The CommonJS exception rests on
/// the same fact TypeScript relies on when it
/// synthesizes that chain: the module owns its `exports` object.
///
/// A chain that writes both an `exports` property and a resolved local
/// (`exports.count = count = 0`) stays whole even when it could be split: it
/// is the evidence that the local stores the export, which UnEsm recovers as
/// one operation (`commonjs_export_mirror_coverage`).
///
/// A `standard`-only extension for chains whose targets share one root was
/// considered and not taken. TDZ would be covered (the innermost statement
/// throws first), but a setter reassigning the shared root has no proof and
/// would need a new named assumption, in exchange for expanding chains that
/// the source already contained.
pub struct UnAssignmentMerging {
    unresolved_ctxt: SyntaxContext,
}

impl UnAssignmentMerging {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self {
            unresolved_ctxt: SyntaxContext::empty().apply_mark(unresolved_mark),
        }
    }

    fn identifier_target_is_binding_write(&self, ident: &Ident) -> bool {
        ident.ctxt != self.unresolved_ctxt || is_commonjs_scope_binding(ident.sym.as_ref())
    }
}

impl VisitMut for UnAssignmentMerging {
    fn visit_mut_module(&mut self, module: &mut Module) {
        // Splitting re-reads the value and resolves each target after the
        // previous write. Inside a `with` body an accessor on the object can
        // observe both, so a module with a `with` statement keeps its chains;
        // it also stays CommonJS, so UnEsm needs no split export chain
        // (docs/rewrite-assumptions.md, dynamic-scope skip). A direct eval
        // cannot run between the writes or define an accessor on a binding,
        // so it does not block the split.
        if super::eval_utils::module_has_with_stmt(module) {
            return;
        }
        module.visit_mut_children_with(self);
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);

        let original = std::mem::take(items);
        let mut out = Vec::with_capacity(original.len());
        for item in original {
            match item {
                ModuleItem::Stmt(stmt) => {
                    let expanded = self.split_chained_assignment(stmt);
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
            out.extend(self.split_chained_assignment(stmt));
        }
        *stmts = out;
    }
}

impl UnAssignmentMerging {
    /// Returns true if the statement is a chained assignment with a simple final
    /// value, meaning it should be split.
    fn should_split(&self, stmt: &Stmt) -> bool {
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
        if !is_simple_value(cur)
            || !targets_can_be_split(a)
            || !targets_are_stable_references(a, self.unresolved_ctxt)
            || self.mirrors_local_into_export(a)
        {
            return false;
        }
        match cur {
            Expr::Ident(value)
                if value.ctxt == self.unresolved_ctxt && value.sym.as_ref() == "undefined" =>
            {
                true
            }
            Expr::Ident(_) => self.writes_cannot_change_value(a),
            _ => true,
        }
    }

    /// A chain that writes both an `exports` property and a resolved local
    /// (`exports.count = count = 0`) is how Babel, TypeScript, and sucrase
    /// keep an export in sync with the local that stores it. Split, the
    /// property write only repeats the value and no longer shows that it
    /// copies the local, so UnEsm recovers such a chain whole.
    fn mirrors_local_into_export(&self, assign: &AssignExpr) -> bool {
        let mut export = false;
        let mut local = false;
        let mut current = assign;
        loop {
            match &current.left {
                AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) => {
                    local |= ident.id.ctxt != self.unresolved_ctxt;
                }
                AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
                    export |= self.is_exports_object(&member.obj);
                }
                _ => {}
            }
            match current.right.as_ref() {
                Expr::Assign(next) if next.op == AssignOp::Assign => current = next,
                _ => return export && local,
            }
        }
    }

    /// `exports` or `module.exports`.
    fn is_exports_object(&self, expr: &Expr) -> bool {
        match strip_parens(expr) {
            Expr::Ident(ident) => ident.ctxt == self.unresolved_ctxt && ident.sym == "exports",
            Expr::Member(member) => {
                static_member_name(&member.prop) == Some("exports")
                    && matches!(strip_parens(&member.obj), Expr::Ident(module)
                        if module.ctxt == self.unresolved_ctxt && module.sym == "module")
            }
            _ => false,
        }
    }

    /// True when nothing between the repeated value reads can change the
    /// value: every write is a resolved binding store, or an export store on
    /// the module's own `exports` object (`commonjs_exports_data_properties`).
    fn writes_cannot_change_value(&self, assign: &AssignExpr) -> bool {
        let mut current = assign;
        loop {
            let ok = match &current.left {
                AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) => {
                    self.identifier_target_is_binding_write(&ident.id)
                }
                // Member targets already passed the CommonJS-root check;
                // their setters are excluded by assumption, not by proof.
                AssignTarget::Simple(SimpleAssignTarget::Member(_)) => true,
                _ => false,
            };
            if !ok {
                return false;
            }
            match current.right.as_ref() {
                Expr::Assign(next) if next.op == AssignOp::Assign => current = next,
                _ => return true,
            }
        }
    }

    /// Splits a chained assignment statement into individual assignment
    /// statements, if applicable. Otherwise returns the statement unchanged
    /// (wrapped in a Vec).
    ///
    /// Writes are emitted innermost first, matching the order in which the
    /// chained form commits them.
    fn split_chained_assignment(&self, stmt: Stmt) -> Vec<Stmt> {
        if !self.should_split(&stmt) {
            return vec![stmt];
        }
        split_chained_assignment_unchecked(stmt)
    }
}

/// Every target must be a reference whose evaluation cannot throw, run user
/// code, or observe the other writes. See the type-level docs for the list.
fn targets_are_stable_references(assign: &AssignExpr, unresolved_ctxt: SyntaxContext) -> bool {
    let nested_export_chain = is_stable_module_export_member(&assign.left, unresolved_ctxt);
    let mut current = assign;
    loop {
        let stable = if nested_export_chain {
            is_stable_module_export_member(&current.left, unresolved_ctxt)
        } else {
            target_is_stable_reference(&current.left, unresolved_ctxt)
        };
        if !stable {
            return false;
        }
        match current.right.as_ref() {
            Expr::Assign(next) if next.op == AssignOp::Assign => current = next,
            _ => return true,
        }
    }
}

/// This whole-chain alternative does not broaden the generic member-root
/// check. In particular, `module.exports.a = module.exports = value` must keep
/// the receiver captured by the source expression and cannot use this path.
fn is_stable_module_export_member(target: &AssignTarget, unresolved_ctxt: SyntaxContext) -> bool {
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = target else {
        return false;
    };
    if !is_static_member_key(&member.prop) {
        return false;
    }
    if let Some(name) = static_member_name(&member.prop) {
        if name == "exports" || is_prototype_mutating_member_name(name) {
            return false;
        }
    }
    let Expr::Member(exports) = strip_parens(&member.obj) else {
        return false;
    };
    if static_member_name(&exports.prop) != Some("exports") {
        return false;
    }
    matches!(strip_parens(&exports.obj), Expr::Ident(module)
        if module.ctxt == unresolved_ctxt && module.sym == "module")
}

/// Static keys match `member_has_stable_root`: identifiers plus string or
/// number literals. Only named keys can be the excluded `exports` or
/// prototype-mutating members.
fn is_static_member_key(prop: &MemberProp) -> bool {
    match prop {
        MemberProp::Ident(_) => true,
        MemberProp::Computed(key) => {
            matches!(
                strip_parens(&key.expr),
                Expr::Lit(Lit::Str(_) | Lit::Num(_))
            )
        }
        MemberProp::PrivateName(_) => false,
    }
}

fn static_member_name(prop: &MemberProp) -> Option<&str> {
    match prop {
        MemberProp::Ident(name) => Some(name.sym.as_ref()),
        MemberProp::Computed(key) => match strip_parens(&key.expr) {
            Expr::Lit(Lit::Str(name)) => name.value.as_str(),
            _ => None,
        },
        MemberProp::PrivateName(_) => None,
    }
}

fn target_is_stable_reference(target: &AssignTarget, unresolved_ctxt: SyntaxContext) -> bool {
    match target {
        AssignTarget::Simple(SimpleAssignTarget::Ident(_)) => true,
        AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
            member_has_stable_root(member, unresolved_ctxt)
        }
        _ => false,
    }
}

fn member_has_stable_root(member: &MemberExpr, unresolved_ctxt: SyntaxContext) -> bool {
    let key_is_static = match &member.prop {
        MemberProp::Ident(_) | MemberProp::PrivateName(_) => true,
        MemberProp::Computed(computed) => {
            matches!(computed.expr.as_ref(), Expr::Lit(Lit::Str(_) | Lit::Num(_)))
        }
    };
    if !key_is_static {
        return false;
    }
    match member.obj.as_ref() {
        Expr::Ident(root) => {
            root.ctxt == unresolved_ctxt && is_commonjs_scope_binding(root.sym.as_ref())
        }
        _ => false,
    }
}

/// The CommonJS wrapper always defines `module`, `exports`, and `require`, so
/// evaluating a member reference on them cannot throw a ReferenceError.
fn is_commonjs_scope_binding(name: &str) -> bool {
    matches!(name, "module" | "exports" | "require")
}

/// A "simple" value is an identifier, primitive literal, or pure `void <number>`.
/// Regex literals are excluded: each evaluation creates a new object,
/// so cloning would break identity and shared `lastIndex` state.
fn is_simple_value(expr: &Expr) -> bool {
    match expr {
        Expr::Ident(_) => true,
        Expr::Lit(lit) => !matches!(lit, swc_core::ecma::ast::Lit::Regex(_)),
        Expr::Unary(unary) if unary.op == UnaryOp::Void => {
            matches!(strip_parens(&unary.arg), Expr::Lit(Lit::Num(_)))
        }
        _ => false,
    }
}

use crate::analysis::BindingId as BindingKey;

fn targets_can_be_split(assign: &AssignExpr) -> bool {
    let mut assigned_bindings = HashSet::default();
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
        return HashSet::default();
    }

    let mut collector = IdentReferenceCollector {
        references: HashSet::default(),
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

fn split_chained_assignment_unchecked(stmt: Stmt) -> Vec<Stmt> {
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
                    .rev()
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
