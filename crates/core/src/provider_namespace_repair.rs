//! Repair synthesized default imports whose provider has only an ESM
//! namespace surface.
//!
//! `UnEsm` must lower `var provider = require("./provider")` before
//! cross-module facts exist, so it initially chooses a default import. When
//! the recovered provider is later proven to expose named exports but no
//! default, the closest faithful ESM edge is a namespace import. The same
//! holds for a provider with a default export that never refers to `module`
//! outside a Cocos registration frame: requiring it returns its `exports` object, where `default` is one property
//! among the others, while a default from `module.exports = v` is the whole
//! required value.
//!
//! This pass is deliberately conservative. It only touches imports synthesized
//! by `UnEsm`, requires one of those proven providers
//! (or, without facts, the evidence in [`run_relative_namespace_repair`]),
//! and accepts uses whose behavior is supported by an ESM namespace: static
//! member reads, `Object.keys(namespace)`, and a namespace used as an
//! `Object.assign` source. A simple top-level alias may stop referring to the
//! namespace after an unconditional replacement assignment. With provider
//! facts, a binding passed on as a value is accepted too: the default import
//! cannot link against a provider without a default export. Mutation,
//! computed/meta access, `__esModule` observation, and (without facts) binding
//! escape leave the original import unchanged.

use crate::collections::HashSet;

use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, CallExpr, Callee, Expr, FnDecl, Ident, ImportSpecifier,
    ImportStarAsSpecifier, MemberExpr, MemberProp, Module, ModuleDecl, ModuleItem, Pat,
    SimpleAssignTarget, Stmt, UnaryExpr, UnaryOp, UpdateExpr, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::analysis::{binding_id, ident_matches_binding, BindingId};
use crate::facts::{ExportKind, ModuleFactsMap};
use crate::rules::expr_utils::is_unresolved_ident;
use crate::utils::paren::strip_parens;

/// Repair the synthesized default imports whose provider `module_facts` prove
/// to have named exports and no default, or a default export that is a
/// property of the required `exports` object. With `allow_value_escape`, a binding
/// passed on as a value is repaired too: its default import cannot link, so
/// the namespace is the only value it can still have. A caller with a better
/// fallback than the unlinkable import, such as restoring CommonJS, passes
/// `false`.
pub(crate) fn run_provider_namespace_repair(
    module: &mut Module,
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
    unresolved_mark: Mark,
    allow_value_escape: bool,
) {
    let Some(current_filename) = current_filename else {
        return;
    };

    let consumer = module_facts.get(current_filename);
    let whole_require_sources = consumer
        .map(|facts| facts.whole_require_sources.as_slice())
        .unwrap_or_default();
    let wildcard_require_sources = consumer
        .map(|facts| facts.wildcard_require_sources.as_slice())
        .unwrap_or_default();
    let namespace_provider = |source: &str| {
        let Some(provider) = module_facts.get_from(Some(current_filename), source) else {
            return false;
        };
        let has_default = provider
            .exports
            .iter()
            .any(|export| export.kind == ExportKind::Default);
        let has_named_surface = provider.has_export_all
            || provider
                .exports
                .iter()
                .any(|export| export.kind == ExportKind::Named);
        // A default export from `exports.default` is one property of the
        // required value; one from `module.exports = v` is the whole value.
        // An interop wrapper the consumer had unwrapped also makes the
        // binding mean the default export, except a wildcard interop of a
        // provider marked `__esModule`: it returns the required value as is.
        let whole_value = whole_require_sources
            .iter()
            .any(|whole| whole.as_ref() == source)
            || (provider.marks_es_module
                && wildcard_require_sources
                    .iter()
                    .any(|wrapped| wrapped.as_ref() == source));
        (has_named_surface || has_default)
            && (!has_default || (provider.require_returns_exports_object && whole_value))
    };
    let policy = UsagePolicy {
        allow_default_read: true,
        allow_value_escape,
    };
    repair_synthesized_default_imports(module, unresolved_mark, policy, |source, _| {
        namespace_provider(source)
    });
}

/// Without provider facts, repair the synthesized default imports of
/// `bindings`, relative requires that the caller has shown to be named
/// imports of a module compiled from ESM (`rules/relative_namespace_import.rs`).
/// A `.default` read keeps the default import.
pub(crate) fn run_relative_namespace_repair(
    module: &mut Module,
    unresolved_mark: Mark,
    bindings: &HashSet<BindingId>,
) {
    let policy = UsagePolicy {
        allow_default_read: false,
        allow_value_escape: false,
    };
    repair_synthesized_default_imports(module, unresolved_mark, policy, |_, binding| {
        bindings.contains(binding)
    });
}

/// Which uses beyond static member reads keep a binding a namespace candidate.
#[derive(Clone, Copy)]
struct UsagePolicy {
    allow_default_read: bool,
    allow_value_escape: bool,
}

fn repair_synthesized_default_imports(
    module: &mut Module,
    unresolved_mark: Mark,
    policy: UsagePolicy,
    namespace_candidate: impl Fn(&str, &BindingId) -> bool,
) {
    let mut candidates = HashSet::default();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        if !import.span.is_dummy() || import.type_only {
            continue;
        }
        let Some(source) = import.src.value.as_str() else {
            continue;
        };

        for specifier in &import.specifiers {
            let ImportSpecifier::Default(default) = specifier else {
                continue;
            };
            let binding = binding_id(&default.local);
            if !namespace_candidate(source, &binding) {
                continue;
            }
            let transparent_aliases = collect_transparent_aliases(module, &binding);
            let mut usage = NamespaceCompatibleUsage::new(
                transparent_aliases,
                binding.clone(),
                unresolved_mark,
                policy,
            );
            module.visit_with(&mut usage);
            if usage.compatible && usage.has_meaningful_use {
                candidates.insert(binding);
            }
        }
    }

    if candidates.is_empty() {
        return;
    }

    let mut rewritten = Vec::with_capacity(module.body.len() + candidates.len());
    for item in std::mem::take(&mut module.body) {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(mut import)) = item else {
            rewritten.push(item);
            continue;
        };

        let Some(default_index) = import.specifiers.iter().position(|specifier| {
            matches!(specifier, ImportSpecifier::Default(default)
                if candidates.contains(&binding_id(&default.local)))
        }) else {
            rewritten.push(ModuleItem::ModuleDecl(ModuleDecl::Import(import)));
            continue;
        };
        let ImportSpecifier::Default(default) = import.specifiers.remove(default_index) else {
            unreachable!("the selected import specifier must remain a default import")
        };
        let namespace = ImportSpecifier::Namespace(ImportStarAsSpecifier {
            span: DUMMY_SP,
            local: default.local,
        });

        if import.specifiers.is_empty() {
            import.specifiers.push(namespace);
            rewritten.push(ModuleItem::ModuleDecl(ModuleDecl::Import(import)));
        } else {
            // `import * as ns, { value }` is invalid syntax. Keep any sibling
            // named specifiers in their original declaration and add a second
            // declaration for the repaired namespace edge.
            let mut namespace_import = import.clone();
            namespace_import.span = DUMMY_SP;
            namespace_import.specifiers = vec![namespace];
            rewritten.push(ModuleItem::ModuleDecl(ModuleDecl::Import(namespace_import)));
            rewritten.push(ModuleItem::ModuleDecl(ModuleDecl::Import(import)));
        }
    }
    module.body = rewritten;
}

/// Follow simple local aliases of a synthesized import. A namespace
/// object and `let alias = namespace` have the same identity and live-binding
/// behavior; treating the initializer as an arbitrary bare escape would keep a
/// known-invalid default import. A later unconditional replacement of the
/// alias ends this lifetime without mutating the imported namespace.
fn collect_transparent_aliases(module: &Module, root: &BindingId) -> HashSet<BindingId> {
    let mut aliases = HashSet::from_iter([root.clone()]);
    loop {
        let mut changed = false;
        for item in &module.body {
            let ModuleItem::Stmt(swc_core::ecma::ast::Stmt::Decl(swc_core::ecma::ast::Decl::Var(
                var,
            ))) = item
            else {
                continue;
            };
            for declarator in &var.decls {
                let Pat::Ident(alias) = &declarator.name else {
                    continue;
                };
                let Some(Expr::Ident(source)) = declarator.init.as_deref() else {
                    continue;
                };
                let alias = binding_id(&alias.id);
                if aliases.contains(&alias)
                    || !aliases
                        .iter()
                        .any(|target| ident_matches_binding(source, target))
                {
                    continue;
                }
                changed |= aliases.insert(alias);
            }
        }
        if !changed {
            return aliases;
        }
    }
}

struct NamespaceCompatibleUsage {
    targets: HashSet<BindingId>,
    all_targets: HashSet<BindingId>,
    resettable_aliases: HashSet<BindingId>,
    unresolved_mark: Mark,
    policy: UsagePolicy,
    compatible: bool,
    has_meaningful_use: bool,
    #[cfg(test)]
    call_visits: usize,
}

impl NamespaceCompatibleUsage {
    fn new(
        targets: HashSet<BindingId>,
        root: BindingId,
        unresolved_mark: Mark,
        policy: UsagePolicy,
    ) -> Self {
        let all_targets = targets.clone();
        let resettable_aliases = targets
            .iter()
            .filter(|target| **target != root)
            .cloned()
            .collect();
        Self {
            targets,
            all_targets,
            resettable_aliases,
            unresolved_mark,
            policy,
            compatible: true,
            has_meaningful_use: false,
            #[cfg(test)]
            call_visits: 0,
        }
    }

    fn is_target(&self, ident: &Ident) -> bool {
        self.targets
            .iter()
            .any(|target| ident_matches_binding(ident, target))
    }

    fn target_member(&self, member: &MemberExpr) -> bool {
        matches!(member.obj.as_ref(), Expr::Ident(ident) if self.is_target(ident))
    }

    fn direct_target_arg(&self, arg: &swc_core::ecma::ast::ExprOrSpread) -> bool {
        arg.spread.is_none()
            && matches!(arg.expr.as_ref(), Expr::Ident(ident) if self.is_target(ident))
    }

    fn is_object_method(&self, call: &CallExpr, method: &str) -> bool {
        let Callee::Expr(callee) = &call.callee else {
            return false;
        };
        let Expr::Member(member) = callee.as_ref() else {
            return false;
        };
        matches!(member.obj.as_ref(), Expr::Ident(object)
            if is_unresolved_ident(object, "Object", self.unresolved_mark))
            && matches!(&member.prop, MemberProp::Ident(property) if property.sym == method)
    }

    fn assignment_targets_binding(&self, target: &AssignTarget) -> bool {
        match target {
            AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => self.is_target(&binding.id),
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) => self.target_member(member),
            _ => false,
        }
    }

    fn top_level_alias_reset<'a>(&self, item: &'a ModuleItem) -> Option<(BindingId, &'a Expr)> {
        let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
            return None;
        };
        let Expr::Assign(assign) = strip_parens(&statement.expr) else {
            return None;
        };
        if assign.op != AssignOp::Assign {
            return None;
        }
        let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &assign.left else {
            return None;
        };
        let binding = binding_id(&binding.id);
        (self.targets.contains(&binding) && self.resettable_aliases.contains(&binding))
            .then_some((binding, assign.right.as_ref()))
    }
}

impl Visit for NamespaceCompatibleUsage {
    fn visit_module(&mut self, module: &Module) {
        for item in &module.body {
            if let Some((binding, right)) = self.top_level_alias_reset(item) {
                // The right-hand side still observes the namespace value. The
                // simple top-level assignment ends the transparent alias
                // lifetime only after that evaluation completes.
                right.visit_with(self);
                self.targets.remove(&binding);
            } else {
                item.visit_with(self);
            }
        }
    }

    fn visit_import_decl(&mut self, _: &swc_core::ecma::ast::ImportDecl) {
        // The declaration is not a runtime use.
    }

    fn visit_fn_decl(&mut self, declaration: &FnDecl) {
        // Function declarations are hoisted and may run before an alias reset
        // even when their textual position is after it. Check every declared
        // function against the original namespace lifetime; using more targets
        // here can only make the repair fail closed.
        let active_targets = std::mem::replace(&mut self.targets, self.all_targets.clone());
        declaration.visit_children_with(self);
        self.targets = active_targets;
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        #[cfg(test)]
        {
            self.call_visits += 1;
        }
        if self.is_object_method(call, "keys")
            && call.args.len() == 1
            && self.direct_target_arg(&call.args[0])
        {
            self.has_meaningful_use = true;
            return;
        }

        if self.is_object_method(call, "assign") && call.args.len() >= 2 {
            let mut accepted_source = false;
            for (index, arg) in call.args.iter().enumerate() {
                if index > 0 && self.direct_target_arg(arg) {
                    accepted_source = true;
                    self.has_meaningful_use = true;
                } else {
                    arg.visit_with(self);
                }
            }
            call.type_args.visit_with(self);
            // The arguments are visited; visiting the children again would
            // repeat them once per nesting level, and a left-deep
            // `Object.assign(Object.assign(...), ...)` chain would cost 2^depth.
            if !accepted_source {
                call.callee.visit_with(self);
            }
            return;
        }

        call.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        let transparent_alias = matches!(
            (&declarator.name, declarator.init.as_deref()),
            (Pat::Ident(alias), Some(Expr::Ident(source)))
                if self.is_target(&alias.id) && self.is_target(source)
        );
        if !transparent_alias {
            declarator.visit_children_with(self);
        }
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if self.assignment_targets_binding(&assign.left) {
            self.compatible = false;
            assign.right.visit_with(self);
            return;
        }
        assign.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        let targets_binding = match update.arg.as_ref() {
            Expr::Ident(ident) => self.is_target(ident),
            Expr::Member(member) => self.target_member(member),
            _ => false,
        };
        if targets_binding {
            self.compatible = false;
            return;
        }
        update.visit_children_with(self);
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        if unary.op == UnaryOp::Delete
            && matches!(unary.arg.as_ref(), Expr::Member(member) if self.target_member(member))
        {
            self.compatible = false;
            return;
        }
        unary.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if self.target_member(member) {
            match &member.prop {
                MemberProp::Ident(property)
                    if property.sym != "__esModule"
                        && (self.policy.allow_default_read || property.sym != "default") =>
                {
                    self.has_meaningful_use = true;
                }
                _ => self.compatible = false,
            }
            return;
        }
        member.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if self.is_target(ident) {
            if self.policy.allow_value_escape {
                // The escape may observe or mutate identity or extensibility,
                // which an ESM namespace fixes. The default import it
                // replaces fails before any code runs, or binds a value that
                // is not the required `exports` object.
                self.has_meaningful_use = true;
            } else {
                // Any bare use not handled by the exact Object helpers above can
                // observe or mutate object identity/prototype/extensibility.
                self.compatible = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, FileName, Globals, SourceMap, GLOBALS};
    use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
    use swc_core::ecma::transforms::base::resolver;
    use swc_core::ecma::visit::VisitMutWith;

    use super::*;
    use crate::facts::{ExportFact, ModuleFacts};

    fn parse_consumer(cm: &Lrc<SourceMap>, source: &str) -> (Module, Mark) {
        let file = cm.new_source_file(
            FileName::Custom("consumer.js".into()).into(),
            source.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax::default()),
            Default::default(),
            StringInput::from(&*file),
            None,
        );
        let mut module = Parser::new_from(lexer)
            .parse_module()
            .expect("consumer should parse");
        let unresolved_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));
        (module, unresolved_mark)
    }

    #[test]
    fn nested_object_assign_is_visited_once_per_call() {
        GLOBALS.set(&Globals::new(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let depth = 20;
            let mut chain = "{}".to_string();
            for level in 0..depth {
                chain = format!("Object.assign({chain}, {{ k{level}: 1 }})");
            }
            let source = format!(
                "import imported from \"./provider.js\";\nconsume(imported.alpha, {chain});\n"
            );
            let (module, unresolved_mark) = parse_consumer(&cm, &source);
            let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = &module.body[0] else {
                panic!("expected leading import")
            };
            let ImportSpecifier::Default(default) = &import.specifiers[0] else {
                panic!("expected default import")
            };
            let binding = binding_id(&default.local);
            let mut usage = NamespaceCompatibleUsage::new(
                HashSet::from_iter([binding.clone()]),
                binding,
                unresolved_mark,
                UsagePolicy {
                    allow_default_read: true,
                    allow_value_escape: true,
                },
            );

            module.visit_with(&mut usage);

            assert!(usage.compatible && usage.has_meaningful_use);
            assert_eq!(
                usage.call_visits,
                depth + 1,
                "each call is visited once: the chain plus `consume`"
            );
        });
    }

    #[test]
    fn hoisted_function_after_alias_reset_is_checked_against_the_original_lifetime() {
        GLOBALS.set(&Globals::new(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let source = r#"
import imported from "./provider.js";
let provider = imported;
const before = provider.alpha;
mutateProvider();
provider = { alpha: 2 };
function mutateProvider() {
    provider.alpha = 3;
}
consume(before, provider.alpha);
"#;
            let (mut module, unresolved_mark) = parse_consumer(&cm, source);
            let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = &mut module.body[0] else {
                panic!("expected leading import")
            };
            import.span = DUMMY_SP;

            let mut facts = ModuleFactsMap::new();
            facts.insert(
                "provider.js",
                ModuleFacts {
                    exports: vec![ExportFact {
                        exported: "alpha".into(),
                        local: Some("alpha".into()),
                        kind: ExportKind::Named,
                    }],
                    ..Default::default()
                },
            );

            run_provider_namespace_repair(
                &mut module,
                &facts,
                Some("consumer.js"),
                unresolved_mark,
                true,
            );

            let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = &module.body[0] else {
                panic!("expected leading import")
            };
            assert!(
                matches!(import.specifiers.first(), Some(ImportSpecifier::Default(_))),
                "the hoisted mutation can run before the alias reset, so repair must fail closed"
            );
        });
    }
}
