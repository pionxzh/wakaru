use crate::collections::{HashMap, HashSet};

use swc_core::ecma::ast::{Expr, Module, Pat, Stmt, VarDecl};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::analysis::binding_uses::{BindingUseIndex, UseKind};

use super::decl_utils::{binding_id, BindingId};
use super::rename_utils::collect_exported_binding_ids;

pub(crate) struct BindingFacts {
    /// Every declarator without an initializer, any kind. Dead-declaration
    /// removal uses this.
    pub(crate) uninitialized: HashSet<BindingId>,
    pub(crate) references: HashMap<BindingId, usize>,
}

pub(crate) fn collect_binding_facts(module: &Module) -> BindingFacts {
    let index = BindingUseIndex::collect(module);
    BindingFacts {
        uninitialized: index.uninitialized_bindings(),
        references: BindingUseIndex::collect_legacy_reference_counts(module),
    }
}

/// The proof that a compiler temp is confined to a matched pattern, so a
/// rewrite may drop its assignment (see "Generated Temporaries" in
/// `docs/rewrite-assumptions.md`).
///
/// A rule counts the uses its pattern consumes; this checks the rest:
/// - the module has no other use of the binding;
/// - its only declaration is an uninitialized declarator a pattern may
///   assign ([`BindingUseIndex::assignable_uninitialized_bindings`]). That
///   excludes parameters, which sloppy-mode `arguments` aliases, and a
///   `let` a pattern could write in its TDZ, whose ReferenceError dropping
///   the write would hide;
/// - the declaration is not exported (importers read the live binding) and
///   not ambient (`declare var` creates no binding, so the write reaches a
///   global).
///
/// Every rewrite that may drop a temp's write goes through
/// [`TempIsolation::accept_expr_rewrite`] (or the statement form) at the
/// rule's choke point. That enforces the rule above for every code path,
/// records which declarations the rewrite made dead, and keeps the counts in
/// step with the current module, so a temp consumed by several nested
/// rewrites is still proven. Pattern proofs stay as early exits and as shape
/// policy.
///
/// Dynamic scope is the caller's concern: an isolated compiler temp need
/// not bail on `with` or direct `eval`, but removing its declaration does
/// (`remove_consumed_uninitialized_decls` handles that).
#[derive(Default)]
pub(crate) struct TempIsolation {
    uses: BindingUseIndex,
    assignable: HashSet<BindingId>,
    /// Net uses per binding that recorded rewrites removed.
    removed_uses: HashMap<BindingId, isize>,
}

impl TempIsolation {
    pub(crate) fn collect(module: &Module) -> Self {
        let uses = BindingUseIndex::collect(module);
        let mut assignable = uses.assignable_uninitialized_bindings();
        if !assignable.is_empty() {
            for binding in collect_exported_binding_ids(module) {
                assignable.remove(&binding);
            }
            let mut ambient = AmbientVarCollector::default();
            module.visit_with(&mut ambient);
            for binding in ambient.bindings {
                assignable.remove(&binding);
            }
        }
        Self {
            uses,
            assignable,
            removed_uses: HashMap::default(),
        }
    }

    /// Whether `consumed_uses` accounts for every use of `binding`.
    pub(crate) fn is_isolated(&self, binding: &BindingId, consumed_uses: usize) -> bool {
        self.assignable.contains(binding)
            && self.uses.has_single_declaration(binding)
            && self.current_use_count(binding) == consumed_uses as isize
    }

    /// Check a rewrite of `before` into `after` at the rule's choke point.
    ///
    /// It is accepted only if every binding whose writes it drops is a temp
    /// isolated to `before` and not read by `after`; a pattern proof alone
    /// does not cover writes another code path drops, or reads a builder
    /// copies into the output. On acceptance, temps whose every use the
    /// rewrite removed are added to `consumed` (their declarations are dead)
    /// and the counts are updated for later rewrites. On rejection nothing
    /// changes.
    pub(crate) fn accept_expr_rewrite(
        &mut self,
        before: &Expr,
        after: &Expr,
        consumed: &mut HashSet<BindingId>,
    ) -> bool {
        self.accept_rewrite(
            &BindingUseIndex::collect_expr(before),
            &BindingUseIndex::collect_expr(after),
            consumed,
        )
    }

    /// [`accept_expr_rewrite`](Self::accept_expr_rewrite) for statements.
    pub(crate) fn accept_stmts_rewrite(
        &mut self,
        before: &[Stmt],
        after: &[Stmt],
        consumed: &mut HashSet<BindingId>,
    ) -> bool {
        self.accept_rewrite(
            &BindingUseIndex::collect_stmts(before),
            &BindingUseIndex::collect_stmts(after),
            consumed,
        )
    }

    fn accept_rewrite(
        &mut self,
        before: &BindingUseIndex,
        after: &BindingUseIndex,
        consumed: &mut HashSet<BindingId>,
    ) -> bool {
        let writes = |index: &BindingUseIndex, binding: &BindingId| {
            index
                .use_sites(binding)
                .iter()
                .filter(|site| matches!(site.kind, UseKind::Write | UseKind::ReadWrite))
                .count()
        };
        let before_bindings = before.referenced_bindings();
        let mut newly_consumed = Vec::new();
        for binding in &before_bindings {
            let isolated = self.is_isolated(binding, before.use_count(binding));
            if writes(after, binding) < writes(before, binding) {
                let read_after = after
                    .use_sites(binding)
                    .iter()
                    .any(|site| site.kind != UseKind::Write);
                if !isolated || read_after {
                    return false;
                }
            }
            if isolated && after.use_count(binding) == 0 {
                newly_consumed.push(binding.clone());
            }
        }
        consumed.extend(newly_consumed);
        self.record_rewrite(before, after);
        true
    }

    /// Account for a rewrite that replaced a node with `before`'s uses by one
    /// with `after`'s.
    fn record_rewrite(&mut self, before: &BindingUseIndex, after: &BindingUseIndex) {
        let mut bindings = before.referenced_bindings();
        bindings.extend(after.referenced_bindings());
        for binding in bindings {
            let delta = before.use_count(&binding) as isize - after.use_count(&binding) as isize;
            if delta != 0 {
                *self.removed_uses.entry(binding).or_default() += delta;
            }
        }
    }

    fn current_use_count(&self, binding: &BindingId) -> isize {
        self.uses.use_count(binding) as isize - self.removed_uses.get(binding).copied().unwrap_or(0)
    }

    /// Whether every use of each of `bindings` sits in `pattern`.
    pub(crate) fn are_isolated_to<'a>(
        &self,
        bindings: impl IntoIterator<Item = &'a BindingId>,
        pattern: &[&Expr],
    ) -> bool {
        let pattern_uses: Vec<BindingUseIndex> = pattern
            .iter()
            .map(|expr| BindingUseIndex::collect_expr(expr))
            .collect();
        bindings.into_iter().all(|binding| {
            let consumed = pattern_uses
                .iter()
                .map(|uses| uses.use_count(binding))
                .sum();
            self.is_isolated(binding, consumed)
        })
    }
}

/// The bindings `declare var` / `declare let` introduce.
#[derive(Default)]
struct AmbientVarCollector {
    bindings: Vec<BindingId>,
}

impl Visit for AmbientVarCollector {
    fn visit_var_decl(&mut self, var: &VarDecl) {
        if var.declare {
            for declarator in &var.decls {
                if let Pat::Ident(binding) = &declarator.name {
                    self.bindings.push(binding_id(&binding.id));
                }
            }
        }
        var.visit_children_with(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::common::{sync::Lrc, FileName, SourceMap, GLOBALS};
    use swc_core::ecma::ast::ExprStmt;
    use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax, TsSyntax};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};

    fn resolved(source: &str) -> Module {
        resolved_with(source, Syntax::Es(EsSyntax::default()))
    }

    fn resolved_with(source: &str, syntax: Syntax) -> Module {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            FileName::Custom("test.js".into()).into(),
            source.to_string(),
        );
        let lexer = Lexer::new(syntax, Default::default(), StringInput::from(&*fm), None);
        let mut module = Parser::new_from(lexer)
            .parse_module()
            .expect("source should parse");
        module.visit_mut_with(&mut resolver(Default::default(), Default::default(), false));
        module
    }

    /// Every expression statement, in source order.
    fn expr_stmts(module: &Module) -> Vec<Expr> {
        struct Collector(Vec<Expr>);
        impl Visit for Collector {
            fn visit_expr_stmt(&mut self, stmt: &ExprStmt) {
                self.0.push((*stmt.expr).clone());
            }
        }
        let mut collector = Collector(Vec::new());
        module.visit_with(&mut collector);
        collector.0
    }

    fn binding(module: &Module, name: &str) -> BindingId {
        let uses = BindingUseIndex::collect(module);
        uses.declared_bindings()
            .into_iter()
            .find(|(sym, _)| sym.as_ref() == name)
            .expect("binding should be declared")
    }

    /// Whether `t` is isolated to the first expression statement.
    fn t_isolated_to_first_stmt(source: &str) -> bool {
        GLOBALS.set(&Default::default(), || {
            let module = resolved(source);
            let isolation = TempIsolation::collect(&module);
            let stmts = expr_stmts(&module);
            isolation.are_isolated_to([&binding(&module, "t")], &[&stmts[0]])
        })
    }

    #[test]
    fn var_temp_used_only_in_pattern_is_isolated() {
        assert!(t_isolated_to_first_stmt("var t; (t = a()).b(t);"));
    }

    #[test]
    fn temp_read_outside_pattern_is_not_isolated() {
        assert!(!t_isolated_to_first_stmt("var t; (t = a()).b(t); use(t);"));
    }

    #[test]
    fn temp_read_by_a_closure_is_not_isolated() {
        assert!(!t_isolated_to_first_stmt(
            "var t; (t = a()).b(t); function peek() { return t; }"
        ));
    }

    #[test]
    fn parameter_is_not_isolated() {
        assert!(!t_isolated_to_first_stmt(
            "function f(t) { (t = a()).b(t); }"
        ));
    }

    #[test]
    fn let_declared_before_the_pattern_is_isolated() {
        assert!(t_isolated_to_first_stmt("let t; (t = a()).b(t);"));
    }

    #[test]
    fn let_written_in_its_tdz_is_not_isolated() {
        assert!(!t_isolated_to_first_stmt("(t = a()).b(t); let t;"));
    }

    #[test]
    fn initialized_declaration_is_not_isolated() {
        assert!(!t_isolated_to_first_stmt("var t = 0; (t = a()).b(t);"));
    }

    #[test]
    fn redeclared_temp_is_not_isolated() {
        assert!(!t_isolated_to_first_stmt("var t; var t; (t = a()).b(t);"));
    }

    #[test]
    fn exported_temp_is_not_isolated() {
        // Importers read the live binding.
        assert!(!t_isolated_to_first_stmt("export var t; (t = a()).b(t);"));
    }

    #[test]
    fn ambient_temp_is_not_isolated() {
        // `declare var` creates no binding, so the write reaches a global.
        GLOBALS.set(&Default::default(), || {
            let module = resolved_with(
                "declare var t; (t = a()).b(t);",
                Syntax::Typescript(TsSyntax::default()),
            );
            let isolation = TempIsolation::collect(&module);
            let stmts = expr_stmts(&module);
            assert!(!isolation.are_isolated_to([&binding(&module, "t")], &[&stmts[0]]));
        });
    }

    /// Run `accept_expr_rewrite` with the first expression statement as the
    /// input and `after` built from it; return the verdict and the consumed
    /// temps' names.
    fn accept(source: &str, after: impl Fn(&Expr) -> Expr) -> (bool, Vec<String>) {
        GLOBALS.set(&Default::default(), || {
            let module = resolved(source);
            let before = expr_stmts(&module).remove(0);
            let mut isolation = TempIsolation::collect(&module);
            let mut consumed = HashSet::default();
            let accepted = isolation.accept_expr_rewrite(&before, &after(&before), &mut consumed);
            let mut names: Vec<String> = consumed
                .into_iter()
                .map(|(sym, _)| sym.to_string())
                .collect();
            names.sort();
            (accepted, names)
        })
    }

    /// The call's receiver object without the `(t = ...)` wrapper, i.e. the
    /// rewrite `(t = a()).b(t)` → `a().b()`.
    fn drop_temp(before: &Expr) -> Expr {
        let Expr::Call(call) = before else {
            panic!("call expected")
        };
        let swc_core::ecma::ast::Callee::Expr(callee) = &call.callee else {
            panic!()
        };
        let Expr::Member(member) = callee.as_ref() else {
            panic!()
        };
        let Expr::Paren(paren) = member.obj.as_ref() else {
            panic!()
        };
        let Expr::Assign(assign) = paren.expr.as_ref() else {
            panic!()
        };
        let mut call = call.clone();
        let mut member = member.clone();
        member.obj = assign.right.clone();
        call.callee = swc_core::ecma::ast::Callee::Expr(Box::new(Expr::Member(member)));
        call.args.clear();
        Expr::Call(call)
    }

    /// Like `drop_temp`, but keeps the `t` argument: `a().b(t)`.
    fn drop_temp_keep_read(before: &Expr) -> Expr {
        let Expr::Call(original) = before else {
            panic!("call expected")
        };
        let Expr::Call(mut call) = drop_temp(before) else {
            unreachable!()
        };
        call.args = original.args.clone();
        Expr::Call(call)
    }

    #[test]
    fn accepts_dropping_an_isolated_temp_and_consumes_it() {
        assert_eq!(
            accept("var t; (t = a()).b(t);", drop_temp),
            (true, vec!["t".to_string()])
        );
    }

    #[test]
    fn rejects_dropping_a_temp_read_elsewhere() {
        assert_eq!(
            accept("var t; (t = a()).b(t); use(t);", drop_temp),
            (false, vec![])
        );
    }

    #[test]
    fn rejects_dropping_an_undeclared_temp() {
        assert_eq!(accept("(t = a()).b(t);", drop_temp), (false, vec![]));
    }

    #[test]
    fn rejects_an_output_that_reads_the_dropped_temp() {
        assert_eq!(
            accept("var t; (t = a()).b(t);", drop_temp_keep_read),
            (false, vec![])
        );
    }

    #[test]
    fn accepts_a_rewrite_that_drops_no_write() {
        assert_eq!(
            accept("(t = a()).b(t); use(t);", Expr::clone),
            (true, vec![])
        );
    }

    #[test]
    fn rejection_leaves_the_counts_unchanged() {
        GLOBALS.set(&Default::default(), || {
            let module = resolved("var t; (t = a()).b(t); use(t);");
            let before = expr_stmts(&module).remove(0);
            let t = binding(&module, "t");
            let mut isolation = TempIsolation::collect(&module);
            let mut consumed = HashSet::default();
            assert!(!isolation.accept_expr_rewrite(&before, &drop_temp(&before), &mut consumed));
            assert!(isolation.is_isolated(&t, 3));
        });
    }

    #[test]
    fn recorded_rewrites_update_the_use_count() {
        GLOBALS.set(&Default::default(), || {
            // The first statement stands for an earlier rewrite's input, the
            // second for its output.
            let module = resolved("var t; (t = a()).b(t).c(t); (t = a()).b(t); use(t);");
            let stmts = expr_stmts(&module);
            let t = binding(&module, "t");
            let mut isolation = TempIsolation::collect(&module);
            assert!(!isolation.is_isolated(&t, 2));
            isolation.record_rewrite(
                &BindingUseIndex::collect_expr(&stmts[0]),
                &BindingUseIndex::collect_expr(&stmts[1]),
            );
            // One use removed: 6 collected, 5 now.
            assert!(isolation.is_isolated(&t, 5));
            assert!(!isolation.is_isolated(&t, 6));
        });
    }

    #[test]
    fn exact_use_count_must_match() {
        GLOBALS.set(&Default::default(), || {
            let module = resolved("var t; (t = a()).b(t);");
            let isolation = TempIsolation::collect(&module);
            let t = binding(&module, "t");
            assert!(isolation.is_isolated(&t, 2));
            assert!(!isolation.is_isolated(&t, 1));
            assert!(!isolation.is_isolated(&t, 3));
        });
    }
}
