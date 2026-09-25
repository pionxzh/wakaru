use crate::collections::{HashMap, HashSet};

use swc_core::ecma::ast::{Expr, Module, Pat, VarDecl};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::analysis::binding_uses::BindingUseIndex;

use super::decl_utils::{binding_id, BindingId};
use super::rename_utils::collect_exported_binding_ids;

pub(crate) struct BindingFacts {
    /// Every declarator without an initializer, any kind. Dead-declaration
    /// removal uses this.
    pub(crate) uninitialized: HashSet<BindingId>,
    /// Uninitialized declarators a pattern may assign without observable
    /// difference: hoisted `var _a;` (the compiler-temp shape) and `let _a;`
    /// that straight-line control flow definitely initializes before every
    /// use (what `VarDeclToLetConst` makes of the former before the cleanup
    /// passes) — see `BindingUseIndex::assignable_uninitialized_bindings`. A
    /// `let n;` declared after the pattern, or in a `switch` case another case
    /// can skip, is in its TDZ when the pattern assigns it; deleting the
    /// assignment would drop that ReferenceError.
    pub(crate) assignable_uninitialized: HashSet<BindingId>,
    pub(crate) references: HashMap<BindingId, usize>,
}

pub(crate) fn collect_binding_facts(module: &Module) -> BindingFacts {
    collect_binding_facts_and_temps(module).0
}

/// [`collect_binding_facts`] plus a [`TempIsolation`] built from the same
/// traversal.
pub(crate) fn collect_binding_facts_and_temps(module: &Module) -> (BindingFacts, TempIsolation) {
    let uses = BindingUseIndex::collect(module);
    let assignable = uses.assignable_uninitialized_bindings();
    let facts = BindingFacts {
        uninitialized: uses.uninitialized_bindings(),
        assignable_uninitialized: assignable.clone(),
        references: BindingUseIndex::collect_legacy_reference_counts(module),
    };
    let temps = TempIsolation::from_parts(module, uses, assignable);
    (facts, temps)
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
/// Counting uses across several sites of one binding is valid only when
/// each site reads nothing but its own write.
///
/// Dynamic scope is the caller's concern: an isolated compiler temp need
/// not bail on `with` or direct `eval`, but removing its declaration does
/// (`remove_consumed_uninitialized_decls` handles that).
#[derive(Default)]
pub(crate) struct TempIsolation {
    uses: BindingUseIndex,
    assignable: HashSet<BindingId>,
}

impl TempIsolation {
    pub(crate) fn collect(module: &Module) -> Self {
        let uses = BindingUseIndex::collect(module);
        let assignable = uses.assignable_uninitialized_bindings();
        Self::from_parts(module, uses, assignable)
    }

    fn from_parts(
        module: &Module,
        uses: BindingUseIndex,
        mut assignable: HashSet<BindingId>,
    ) -> Self {
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
        Self { uses, assignable }
    }

    /// Whether `consumed_uses` accounts for every use of `binding`.
    pub(crate) fn is_isolated(&self, binding: &BindingId, consumed_uses: usize) -> bool {
        self.assignable.contains(binding)
            && self.uses.has_single_declaration(binding)
            && self.uses.use_count(binding) == consumed_uses
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

    #[test]
    fn shared_traversal_matches_standalone_collect() {
        GLOBALS.set(&Default::default(), || {
            let module = resolved("var t; (t = a()).b(t);");
            let (_, shared) = collect_binding_facts_and_temps(&module);
            let t = binding(&module, "t");
            assert!(shared.is_isolated(&t, 2));
        });
    }
}
