//! Recover exact CommonJS default-object composition as one mutable ESM value.
//!
//! CSS-module toolchains commonly emit a module whose complete body is:
//!
//! ```text
//! module.exports = {};
//! Object.assign(module.exports, require("./provider.js") || {});
//! ```
//!
//! `UnEsm` can recover the initial assignment, but it must leave the later
//! CommonJS reads visible because the exported anonymous object has no local
//! binding. This pass runs at the cross-module barrier, after provider facts
//! prove that every copied value is the complete default object returned by
//! its provider. It introduces one stable local, imports those default values,
//! and keeps every `Object.assign` in source order.
//!
//! The plan is deliberately closed under proven dependencies. Direct object
//! providers seed a monotone fixed point; composition modules enter only after
//! all of their providers do. Cycles, authored ESM defaults, mixed export
//! surfaces, dynamic sources, and inexact consumer bodies therefore remain
//! honest CommonJS residuals.

use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Mark, Spanned, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    BinaryOp, BindingIdent, Callee, Decl, ExportDefaultExpr, Expr, ExprStmt, Ident, ImportDecl,
    ImportDefaultSpecifier, ImportSpecifier, Lit, MemberProp, Module, ModuleDecl, ModuleItem, Pat,
    Stmt, Str, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::facts::{ExportKind, ModuleFacts, ModuleFactsMap};
use crate::rules::expr_utils::is_unresolved_ident;
use crate::utils::paren::strip_parens;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CommonJsDefaultObjectCompositionPlan {
    sources_by_module: HashMap<String, Vec<Atom>>,
}

impl CommonJsDefaultObjectCompositionPlan {
    pub(crate) fn build(facts: &ModuleFactsMap) -> Self {
        let mut recoverable = facts
            .iter()
            .filter_map(|(filename, module)| {
                if has_exact_default_only_surface(module)
                    && module
                        .commonjs_default_object
                        .as_ref()
                        .is_some_and(|object| object.default_assignment_is_only_commonjs_use)
                {
                    Some(filename.to_string())
                } else {
                    None
                }
            })
            .collect::<HashSet<_>>();

        let mut pending = facts
            .iter()
            .filter_map(|(filename, module)| {
                let sources = module
                    .commonjs_default_object
                    .as_ref()?
                    .composition_sources
                    .clone();
                (has_exact_default_only_surface(module) && !sources.is_empty())
                    .then(|| (filename.to_string(), sources))
            })
            .collect::<Vec<_>>();
        pending.sort_by(|left, right| left.0.cmp(&right.0));

        let mut sources_by_module = HashMap::new();
        loop {
            let newly_recoverable = pending
                .iter()
                .filter(|(filename, _)| !recoverable.contains(filename))
                .filter(|(filename, sources)| {
                    sources.iter().all(|source| {
                        facts
                            .resolve_key_from(Some(filename), source.as_ref())
                            .is_some_and(|provider| recoverable.contains(&provider))
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            if newly_recoverable.is_empty() {
                break;
            }
            for (filename, sources) in newly_recoverable {
                recoverable.insert(filename.clone());
                sources_by_module.insert(filename, sources);
            }
        }

        Self { sources_by_module }
    }

    fn sources_for(&self, filename: &str) -> Option<&[Atom]> {
        let canonical = filename.strip_prefix("./").unwrap_or(filename);
        self.sources_by_module.get(canonical).map(Vec::as_slice)
    }
}

fn has_exact_default_only_surface(facts: &ModuleFacts) -> bool {
    !facts.has_export_all
        && facts.imports.is_empty()
        && facts.exports.len() == 1
        && facts.exports[0].kind == ExportKind::Default
        && facts.commonjs_default_object.is_some()
}

pub(crate) fn run_commonjs_default_object_composition(
    module: &mut Module,
    plan: &CommonJsDefaultObjectCompositionPlan,
    current_filename: Option<&str>,
    unresolved_mark: Mark,
) {
    let Some(current_filename) = current_filename else {
        return;
    };
    let Some(expected_sources) = plan.sources_for(current_filename) else {
        return;
    };
    let Some(composition) = match_normalized_composition(module, expected_sources, unresolved_mark)
    else {
        return;
    };

    let mut names = IdentifierNameCollector::default();
    module.visit_with(&mut names);
    let mut used_names = names.names;
    let target = fresh_ident("_defaultObject", &mut used_names);
    let mut source_locals = HashMap::<Atom, Ident>::new();
    let mut imports = Vec::new();
    for source in expected_sources {
        if source_locals.contains_key(source) {
            continue;
        }
        let local = fresh_ident("_source", &mut used_names);
        source_locals.insert(source.clone(), local.clone());
        imports.push(default_import(local, source));
    }

    let mut body = Vec::with_capacity(imports.len() + composition.copies.len() + 2);
    body.extend(imports);
    body.push(default_object_binding(
        target.clone(),
        composition.default_value,
    ));
    for (copy, source) in composition.copies.into_iter().zip(expected_sources) {
        let provider = source_locals
            .get(source)
            .expect("every proven composition source must have an import binding");
        let Some(copy) = rewrite_copy_expression(&copy, &target, provider, unresolved_mark) else {
            return;
        };
        body.push(ModuleItem::Stmt(Stmt::Expr(ExprStmt {
            span: copy.span(),
            expr: copy,
        })));
    }
    body.push(ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(
        ExportDefaultExpr {
            span: composition.export_span,
            expr: Box::new(Expr::Ident(target)),
        },
    )));
    module.body = body;
}

struct NormalizedComposition {
    export_span: swc_core::common::Span,
    default_value: Box<Expr>,
    copies: Vec<Box<Expr>>,
}

fn match_normalized_composition(
    module: &Module,
    expected_sources: &[Atom],
    unresolved_mark: Mark,
) -> Option<NormalizedComposition> {
    let mut items = module.body.iter().filter(|item| !is_use_strict_item(item));
    let ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(default)) = items.next()? else {
        return None;
    };
    if !matches!(strip_parens(&default.expr), Expr::Object(object) if object.props.is_empty()) {
        return None;
    }

    let mut copies = Vec::new();
    for item in items {
        let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
            return None;
        };
        collect_direct_sequence_expressions(&statement.expr, &mut copies);
    }
    if copies.len() != expected_sources.len()
        || copies
            .iter()
            .zip(expected_sources)
            .any(|(copy, expected)| copy_source(copy, unresolved_mark).as_ref() != Some(expected))
    {
        return None;
    }

    Some(NormalizedComposition {
        export_span: default.span,
        default_value: default.expr.clone(),
        copies,
    })
}

fn collect_direct_sequence_expressions(expr: &Expr, out: &mut Vec<Box<Expr>>) {
    match strip_parens(expr) {
        Expr::Seq(sequence) => {
            for expression in &sequence.exprs {
                collect_direct_sequence_expressions(expression, out);
            }
        }
        expression => out.push(Box::new(expression.clone())),
    }
}

fn is_use_strict_item(item: &ModuleItem) -> bool {
    matches!(item, ModuleItem::Stmt(Stmt::Expr(statement))
        if matches!(strip_parens(&statement.expr), Expr::Lit(Lit::Str(value))
            if value.value.as_str() == Some("use strict")))
}

fn copy_source(expr: &Expr, unresolved_mark: Mark) -> Option<Atom> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return None;
    };
    if !matches!(member.obj.as_ref(), Expr::Ident(object)
        if is_unresolved_ident(object, "Object", unresolved_mark))
        || !matches!(&member.prop, MemberProp::Ident(property) if property.sym == "assign")
        || call.args.len() != 2
        || call.args.iter().any(|argument| argument.spread.is_some())
        || !is_module_exports_expr(&call.args[0].expr, unresolved_mark)
    {
        return None;
    }
    let Expr::Bin(fallback) = strip_parens(&call.args[1].expr) else {
        return None;
    };
    if fallback.op != BinaryOp::LogicalOr
        || !matches!(strip_parens(&fallback.right), Expr::Object(object) if object.props.is_empty())
    {
        return None;
    }
    let Expr::Call(require) = strip_parens(&fallback.left) else {
        return None;
    };
    let Callee::Expr(callee) = &require.callee else {
        return None;
    };
    if !matches!(strip_parens(callee), Expr::Ident(ident)
        if is_unresolved_ident(ident, "require", unresolved_mark))
        || require.args.len() != 1
        || require.args[0].spread.is_some()
    {
        return None;
    }
    let Expr::Lit(Lit::Str(source)) = strip_parens(&require.args[0].expr) else {
        return None;
    };
    source.value.as_str().map(Atom::from)
}

fn is_module_exports_expr(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    matches!(member.obj.as_ref(), Expr::Ident(module)
        if is_unresolved_ident(module, "module", unresolved_mark))
        && matches!(&member.prop, MemberProp::Ident(property) if property.sym == "exports")
}

fn rewrite_copy_expression(
    expr: &Expr,
    target: &Ident,
    provider: &Ident,
    unresolved_mark: Mark,
) -> Option<Box<Expr>> {
    copy_source(expr, unresolved_mark)?;
    let Expr::Call(original) = strip_parens(expr) else {
        return None;
    };
    let mut call = original.clone();
    *call.args[0].expr = Expr::Ident(target.clone());
    let Expr::Bin(original_fallback) = strip_parens(&call.args[1].expr) else {
        return None;
    };
    let mut fallback = original_fallback.clone();
    fallback.left = Box::new(Expr::Ident(provider.clone()));
    *call.args[1].expr = Expr::Bin(fallback);
    Some(Box::new(Expr::Call(call)))
}

fn default_import(local: Ident, source: &Atom) -> ModuleItem {
    ModuleItem::ModuleDecl(ModuleDecl::Import(ImportDecl {
        span: DUMMY_SP,
        specifiers: vec![ImportSpecifier::Default(ImportDefaultSpecifier {
            span: DUMMY_SP,
            local,
        })],
        src: Box::new(Str {
            span: DUMMY_SP,
            value: source.as_ref().into(),
            raw: None,
        }),
        type_only: false,
        with: None,
        phase: Default::default(),
    }))
}

fn default_object_binding(local: Ident, init: Box<Expr>) -> ModuleItem {
    ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent::from(local)),
            init: Some(init),
            definite: false,
        }],
    }))))
}

fn fresh_ident(base: &str, used_names: &mut HashSet<Atom>) -> Ident {
    let base: Atom = base.into();
    if used_names.insert(base.clone()) {
        return Ident::new_no_ctxt(base, DUMMY_SP);
    }
    for suffix in 1usize.. {
        let candidate: Atom = format!("{base}_{suffix}").into();
        if used_names.insert(candidate.clone()) {
            return Ident::new_no_ctxt(candidate, DUMMY_SP);
        }
    }
    unreachable!("fresh identifier search must terminate")
}

#[derive(Default)]
struct IdentifierNameCollector {
    names: HashSet<Atom>,
}

impl Visit for IdentifierNameCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        self.names.insert(ident.sym.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{CommonJsDefaultObjectFact, ExportFact};

    fn base_facts() -> ModuleFacts {
        ModuleFacts {
            exports: vec![ExportFact {
                exported: "default".into(),
                local: None,
                kind: ExportKind::Default,
            }],
            commonjs_default_object: Some(CommonJsDefaultObjectFact {
                declared_properties: Vec::new(),
                default_assignment_is_only_commonjs_use: true,
                composition_sources: Vec::new(),
            }),
            ..Default::default()
        }
    }

    fn composition_facts(source: &str) -> ModuleFacts {
        let mut facts = base_facts();
        let default = facts.commonjs_default_object.as_mut().unwrap();
        default.default_assignment_is_only_commonjs_use = false;
        default.composition_sources = vec![source.into()];
        facts
    }

    #[test]
    fn fixed_point_is_independent_of_insertion_order_and_rejects_cycles() {
        let build = |reverse: bool| {
            let entries = [
                ("base.js", base_facts()),
                ("middle.js", composition_facts("./base.js")),
                ("entry.js", composition_facts("./middle.js")),
                ("left.js", composition_facts("./right.js")),
                ("right.js", composition_facts("./left.js")),
            ];
            let mut map = ModuleFactsMap::new();
            if reverse {
                for (filename, facts) in entries.into_iter().rev() {
                    map.insert(filename, facts);
                }
            } else {
                for (filename, facts) in entries {
                    map.insert(filename, facts);
                }
            }
            CommonJsDefaultObjectCompositionPlan::build(&map)
        };

        let forward = build(false);
        let reverse = build(true);
        assert_eq!(forward, reverse);
        assert_eq!(
            forward.sources_for("entry.js"),
            Some(&[Atom::from("./middle.js")][..])
        );
        assert!(forward.sources_for("left.js").is_none());
        assert!(forward.sources_for("right.js").is_none());
    }
}
