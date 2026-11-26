use crate::collections::{HashMap, HashSet};
use swc_core::common::Mark;

use swc_core::ecma::ast::{
    ArrowExpr, BindingIdent, Class, DoWhileStmt, Expr, ForInStmt, ForOfStmt, ForStmt, Function,
    Module, Pat, Stmt, VarDecl, VarDeclKind, WhileStmt,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use crate::analysis::binding_uses::{BindingUseIndex, UseKind};

use super::dead_decls::remove_consumed_uninitialized_decls;
use super::decl_utils::{binding_id, BindingId};
use super::expr_utils::is_unresolved_undefined;

/// Remove redundant `= undefined` / `= void 0` from `let` and `var` declarations.
///
/// `let x = undefined` → `let x`
///
/// `const` is excluded because it requires an initializer.
pub struct UnUndefinedInit {
    unresolved_mark: Mark,
    binding_references: HashMap<BindingId, usize>,
    uses: BindingUseIndex,
    converted_unused_bindings: HashSet<BindingId>,
    /// The module contains a `with` statement or a direct eval.
    dynamic_scope: bool,
    /// The module can rebind `undefined` at runtime.
    undefined_blocked: bool,
    /// Number of loop bodies between the current node and its var scope.
    loop_body_depth: usize,
}

impl UnUndefinedInit {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self {
            unresolved_mark,
            binding_references: HashMap::default(),
            uses: BindingUseIndex::default(),
            converted_unused_bindings: HashSet::default(),
            dynamic_scope: false,
            undefined_blocked: false,
            loop_body_depth: 0,
        }
    }

    fn visit_mut_loop_body(&mut self, body: &mut Stmt) {
        self.loop_body_depth += 1;
        body.visit_mut_with(self);
        self.loop_body_depth -= 1;
    }

    /// Whether the `var` binding may hold a value other than `undefined` when
    /// this declarator runs, so its `= undefined` is a reset. Syntactic only:
    /// - another declaration of the binding (a redeclaring `var`, a
    ///   parameter, a hoisted function) may have given it a value;
    /// - a write earlier in the source can run first, since `var` hoists;
    /// - a later write inside a hoisted function declaration can run first,
    ///   because the function may be called before the declaration.
    ///
    /// A later write in a function expression, arrow, or class member cannot:
    /// that function is created after the declaration has run. The remaining
    /// orders a later write can run first in are a loop, which the loop-body
    /// check covers, and a `switch` whose `default` precedes a case test that
    /// writes the binding.
    fn may_hold_value(&self, binding: &BindingIdent) -> bool {
        let id = binding_id(&binding.id);
        let [declaration] = self.uses.declaration_sites(&id) else {
            return true;
        };
        let span = binding.id.span;
        self.uses.use_sites(&id).iter().any(|site| {
            matches!(site.kind, UseKind::Write | UseKind::ReadWrite)
                && (span.is_dummy()
                    || site.span.lo < span.lo
                    || (site.function_id != declaration.function_id
                        && self
                            .uses
                            .may_run_before_position(site.function_id, declaration.function_id)))
        })
    }

    fn visit_mut_var_scope<N: VisitMutWith<Self>>(&mut self, node: &mut N) {
        let depth = std::mem::take(&mut self.loop_body_depth);
        node.visit_mut_children_with(self);
        self.loop_body_depth = depth;
    }
}

impl VisitMut for UnUndefinedInit {
    fn visit_mut_module(&mut self, module: &mut Module) {
        self.dynamic_scope = super::eval_utils::has_dynamic_scope_construct(module);
        self.undefined_blocked = self.dynamic_scope
            && super::eval_utils::module_blocks_global_reference(module, "undefined");
        self.binding_references = BindingUseIndex::collect_legacy_reference_counts(module);
        self.uses = BindingUseIndex::collect(module);
        self.converted_unused_bindings.clear();

        module.visit_mut_children_with(self);
        self.uses = BindingUseIndex::default();
        remove_consumed_uninitialized_decls(module, &self.converted_unused_bindings);
    }

    fn visit_mut_var_decl(&mut self, decl: &mut VarDecl) {
        decl.visit_mut_children_with(self);

        if decl.kind == VarDeclKind::Const {
            return;
        }
        // A `var` initializer is an assignment: inside a `with` body it can
        // write the object, and a direct eval can assign the binding before
        // the declaration resets it. A `let` initializer only initializes its
        // own binding (docs/rewrite-assumptions.md, dynamic-scope skip).
        if decl.kind == VarDeclKind::Var && self.dynamic_scope {
            return;
        }
        // In a loop body, `var x = void 0` resets `x` on every iteration and a
        // bare `var x` does not. `VarDeclToLetConst` drops the initializer
        // when it turns the declaration into a `let`.
        if decl.kind == VarDeclKind::Var && self.loop_body_depth > 0 {
            return;
        }

        for declarator in &mut decl.decls {
            let Pat::Ident(binding) = &declarator.name else {
                continue;
            };
            if let Some(init) = &declarator.init {
                if is_unresolved_undefined(init, self.unresolved_mark)
                    && !(self.undefined_blocked && matches!(**init, Expr::Ident(_)))
                    && !(decl.kind == VarDeclKind::Var && self.may_hold_value(binding))
                {
                    let binding = binding_id(&binding.id);
                    declarator.init = None;
                    if self.binding_references.get(&binding).copied().unwrap_or(0) <= 1 {
                        self.converted_unused_bindings.insert(binding);
                    }
                }
            }
        }
    }

    fn visit_mut_for_stmt(&mut self, stmt: &mut ForStmt) {
        stmt.init.visit_mut_with(self);
        stmt.test.visit_mut_with(self);
        stmt.update.visit_mut_with(self);
        self.visit_mut_loop_body(&mut stmt.body);
    }

    fn visit_mut_for_in_stmt(&mut self, stmt: &mut ForInStmt) {
        stmt.left.visit_mut_with(self);
        stmt.right.visit_mut_with(self);
        self.visit_mut_loop_body(&mut stmt.body);
    }

    fn visit_mut_for_of_stmt(&mut self, stmt: &mut ForOfStmt) {
        stmt.left.visit_mut_with(self);
        stmt.right.visit_mut_with(self);
        self.visit_mut_loop_body(&mut stmt.body);
    }

    fn visit_mut_while_stmt(&mut self, stmt: &mut WhileStmt) {
        stmt.test.visit_mut_with(self);
        self.visit_mut_loop_body(&mut stmt.body);
    }

    fn visit_mut_do_while_stmt(&mut self, stmt: &mut DoWhileStmt) {
        self.visit_mut_loop_body(&mut stmt.body);
        stmt.test.visit_mut_with(self);
    }

    fn visit_mut_function(&mut self, func: &mut Function) {
        self.visit_mut_var_scope(func);
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        self.visit_mut_var_scope(arrow);
    }

    fn visit_mut_class(&mut self, class: &mut Class) {
        self.visit_mut_var_scope(class);
    }
}
