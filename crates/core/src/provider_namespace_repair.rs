//! Repair synthesized default imports whose provider has only an ESM
//! namespace surface.
//!
//! `UnEsm` must lower `var provider = require("./provider")` before
//! cross-module facts exist, so it initially chooses a default import. When
//! the recovered provider is later proven to expose named exports but no
//! default, the closest faithful ESM edge is a namespace import.
//!
//! This pass is deliberately conservative. It only touches imports synthesized
//! by `UnEsm`, requires a proven named/export-star provider with no default,
//! and accepts uses whose behavior is supported by an ESM namespace: static
//! member reads, `Object.keys(namespace)`, and a namespace used as an
//! `Object.assign` source. Mutation, binding escape, computed/meta access, and
//! `__esModule` observation leave the original import unchanged.

use std::collections::HashSet;

use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, CallExpr, Callee, Expr, Ident, ImportSpecifier,
    ImportStarAsSpecifier, MemberExpr, MemberProp, Module, ModuleDecl, ModuleItem,
    SimpleAssignTarget, UnaryExpr, UnaryOp, UpdateExpr,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::analysis::{binding_id, ident_matches_binding, BindingId};
use crate::facts::{ExportKind, ModuleFactsMap};
use crate::rules::expr_utils::is_unresolved_ident;

pub(crate) fn run_provider_namespace_repair(
    module: &mut Module,
    module_facts: &ModuleFactsMap,
    current_filename: Option<&str>,
    unresolved_mark: Mark,
) {
    let Some(current_filename) = current_filename else {
        return;
    };

    let mut candidates = HashSet::new();
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
        let Some(provider) = module_facts.get_from(Some(current_filename), source) else {
            continue;
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
        if has_default || !has_named_surface {
            continue;
        }

        for specifier in &import.specifiers {
            let ImportSpecifier::Default(default) = specifier else {
                continue;
            };
            let binding = binding_id(&default.local);
            let mut usage = NamespaceCompatibleUsage::new(&binding, unresolved_mark);
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

struct NamespaceCompatibleUsage<'a> {
    target: &'a BindingId,
    unresolved_mark: Mark,
    compatible: bool,
    has_meaningful_use: bool,
}

impl<'a> NamespaceCompatibleUsage<'a> {
    fn new(target: &'a BindingId, unresolved_mark: Mark) -> Self {
        Self {
            target,
            unresolved_mark,
            compatible: true,
            has_meaningful_use: false,
        }
    }

    fn is_target(&self, ident: &Ident) -> bool {
        ident_matches_binding(ident, self.target)
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
}

impl Visit for NamespaceCompatibleUsage<'_> {
    fn visit_import_decl(&mut self, _: &swc_core::ecma::ast::ImportDecl) {
        // The declaration is not a runtime use.
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
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
            if accepted_source {
                return;
            }
        }

        call.visit_children_with(self);
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
                MemberProp::Ident(property) if property.sym != "__esModule" => {
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
            // Any bare use not handled by the exact Object helpers above can
            // observe or mutate object identity/prototype/extensibility.
            self.compatible = false;
        }
    }
}
